use super::TaskService;
use pab_document::{
    Document, OcrMode,
    query::{document_cache_size, overview, read_document, search_document},
};
use pab_protocol::{
    DocumentAction, DocumentReply, DocumentRequest, ExecutionSelection, OperatorRef, RequestId,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{Mutex, Semaphore, watch},
};

const DOCUMENT_LIFETIME: Duration = Duration::from_secs(3600);
const PARSE_DEADLINE: Duration = Duration::from_secs(240);
const MAX_DOCUMENTS: usize = 8;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct DocumentCache {
    entries: Arc<Mutex<HashMap<RequestId, Entry>>>,
    slot: Arc<Semaphore>,
    reads: Arc<Semaphore>,
}

impl Default for DocumentCache {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            slot: Arc::new(Semaphore::new(1)),
            reads: Arc::new(Semaphore::new(4)),
        }
    }
}

struct Entry {
    owner: OperatorRef,
    path: String,
    execution: ExecutionSelection,
    fingerprint: Vec<u8>,
    reply: DocumentReply,
    document: Option<Arc<Document>>,
    cancel: Option<watch::Sender<bool>>,
    created: Instant,
    previews: Vec<PathBuf>,
    cache_bytes: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WorkerReply {
    Success { document: Document },
    Failure { error: String },
}

impl TaskService {
    pub(super) async fn document_request(
        &self,
        actor: OperatorRef,
        request: DocumentRequest,
    ) -> DocumentReply {
        let mut reply = DocumentReply::pending(&request);
        if let Err(message) = request.validate() {
            return failed(reply, message);
        }
        match request.operation {
            DocumentAction::Open {
                path,
                execution,
                ocr_mode,
            } => {
                if !Path::new(&path).is_absolute() {
                    return failed(reply, "document path must be absolute");
                }
                let user = match self.prepare_user(actor, execution).await {
                    Ok(value) => value,
                    Err(error) => return failed(reply, &error.to_string()),
                };
                let fingerprint =
                    serde_json::to_vec(&(path.clone(), execution, ocr_mode)).unwrap_or_default();
                let mut entries = self.documents.entries.lock().await;
                entries.retain(|_, value| {
                    if value.created.elapsed() < DOCUMENT_LIFETIME {
                        true
                    } else {
                        cleanup_previews(&value.previews);
                        false
                    }
                });
                if let Some(existing) = entries.get(&request.request_id) {
                    return if existing.owner == actor && existing.fingerprint == fingerprint {
                        existing.reply.clone()
                    } else {
                        failed(reply, "request_conflict")
                    };
                }
                if entries.len() >= MAX_DOCUMENTS {
                    return failed(reply, "document_cache_full: close an earlier document");
                }
                let Ok(permit) = self.documents.slot.clone().try_acquire_owned() else {
                    return failed(reply, "document_worker_busy");
                };
                let (cancel, mut cancelled) = watch::channel(false);
                entries.insert(
                    request.request_id,
                    Entry {
                        owner: actor,
                        path: path.clone(),
                        execution,
                        fingerprint,
                        reply: reply.clone(),
                        document: None,
                        cancel: Some(cancel),
                        created: Instant::now(),
                        previews: Vec::new(),
                        cache_bytes: 0,
                    },
                );
                drop(entries);
                let entries = self.documents.entries.clone();
                let expiry_entries = entries.clone();
                let id = request.request_id;
                tokio::spawn(async move {
                    tokio::time::sleep(DOCUMENT_LIFETIME).await;
                    let mut entries = expiry_entries.lock().await;
                    if entries
                        .get(&id)
                        .is_some_and(|entry| entry.created.elapsed() >= DOCUMENT_LIFETIME)
                    {
                        if let Some(entry) = entries.remove(&id) {
                            cleanup_previews(&entry.previews);
                        }
                    }
                });
                tokio::spawn(async move {
                    let _permit = permit;
                    let mode = match ocr_mode {
                        pab_protocol::DocumentOcrMode::Auto => OcrMode::Auto,
                        pab_protocol::DocumentOcrMode::Off => OcrMode::Off,
                        pab_protocol::DocumentOcrMode::On => OcrMode::On,
                    };
                    let result = if let Some((prepared, _identity)) = user {
                        run_user_worker(&path, mode, id, prepared, cancelled).await
                    } else {
                        tokio::select! {
                            value = tokio::time::timeout(PARSE_DEADLINE, run_worker(&path, mode)) =>
                                value.unwrap_or_else(|_| Err("worker_timeout".into())),
                            _ = cancelled.changed() => Err("cancelled".into()),
                        }
                    };
                    let result = result.and_then(|document| {
                        let bytes = document_cache_size(&document);
                        if bytes > MAX_CACHE_BYTES {
                            Err("document_cache_size_limit".into())
                        } else {
                            Ok((document, bytes))
                        }
                    });
                    let mut entries = entries.lock().await;
                    let other_bytes: usize = entries.values().map(|entry| entry.cache_bytes).sum();
                    if let Some(entry) = entries.get_mut(&id) {
                        if entry.owner != actor {
                            return;
                        }
                        entry.cancel = None;
                        match result {
                            Ok((document, bytes)) if other_bytes + bytes <= MAX_CACHE_BYTES => {
                                entry.reply.state = "completed".into();
                                entry.reply.document_ref = Some(id);
                                entry.reply.data = Some(overview(&document));
                                entry.document = Some(Arc::new(document));
                                entry.cache_bytes = bytes;
                            }
                            Ok(_) => {
                                entry.reply.state = "failed".into();
                                entry.reply.error = Some("document_cache_full".into());
                            }
                            Err(error) => {
                                entry.reply.state = if error == "cancelled" {
                                    "cancelled"
                                } else {
                                    "failed"
                                }
                                .into();
                                entry.reply.error = Some(error.chars().take(512).collect());
                            }
                        }
                    }
                });
                reply
            }
            DocumentAction::Read {
                document_ref,
                selector,
                cursor,
            } => {
                let Ok(permit) = self.documents.reads.clone().try_acquire_owned() else {
                    return failed(reply, "document_read_busy");
                };
                let document = match self.lookup(actor, document_ref).await {
                    Ok(value) => value,
                    Err(message) => return failed(reply, &message),
                };
                let fallback = reply.clone();
                match tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    read_document(reply, document_ref, &document, &selector, cursor.as_deref())
                })
                .await
                {
                    Ok(value) => value,
                    Err(_) => failed(fallback, "document_read_worker_failed"),
                }
            }
            DocumentAction::Search {
                document_ref,
                query,
                case_sensitive,
                limit,
                cursor,
            } => {
                let Ok(permit) = self.documents.reads.clone().try_acquire_owned() else {
                    return failed(reply, "document_search_busy");
                };
                let document = match self.lookup(actor, document_ref).await {
                    Ok(value) => value,
                    Err(message) => return failed(reply, &message),
                };
                let fallback = reply.clone();
                match tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    search_document(
                        reply,
                        document_ref,
                        &document,
                        &query,
                        case_sensitive,
                        limit,
                        cursor.as_deref(),
                    )
                })
                .await
                {
                    Ok(value) => value,
                    Err(_) => failed(fallback, "document_search_worker_failed"),
                }
            }
            DocumentAction::RenderPage {
                document_ref,
                page_number,
                max_dimension,
            } => {
                let Ok(_permit) = self.documents.slot.clone().try_acquire_owned() else {
                    return failed(reply, "document_worker_busy");
                };
                let (path, execution, hash, page_count) = {
                    let entries = self.documents.entries.lock().await;
                    let Some(entry) = entries.get(&document_ref).filter(|entry| {
                        entry.owner == actor && entry.created.elapsed() < DOCUMENT_LIFETIME
                    }) else {
                        return failed(reply, "document_expired_or_not_found");
                    };
                    let Some(document) = &entry.document else {
                        return failed(reply, "document_not_ready");
                    };
                    (
                        entry.path.clone(),
                        entry.execution,
                        document.sha256.clone(),
                        document.pages.len(),
                    )
                };
                if page_number as usize > page_count {
                    return failed(reply, "page_not_found");
                }
                let user = match self.prepare_user(actor, execution).await {
                    Ok(value) => value,
                    Err(error) => return failed(reply, &error.to_string()),
                };
                let output = if let Some((prepared, _)) = user.as_ref() {
                    let home = prepared.identity().home.clone();
                    home.join(format!(
                        ".pixels-document-preview-{}.png",
                        request.request_id
                    ))
                } else {
                    std::env::temp_dir().join(format!(
                        "pixels-document-preview-{}.png",
                        request.request_id
                    ))
                };
                let result = if let Some((prepared, _)) = user {
                    run_user_preview(
                        path,
                        hash,
                        page_number,
                        max_dimension,
                        output.clone(),
                        prepared,
                    )
                    .await
                } else {
                    run_preview(path, hash, page_number, max_dimension, output.clone()).await
                };
                let (size, sha256) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = std::fs::remove_file(&output);
                        return failed(reply, &error);
                    }
                };
                let mut entries = self.documents.entries.lock().await;
                let Some(entry) = entries
                    .get_mut(&document_ref)
                    .filter(|entry| entry.owner == actor)
                else {
                    let _ = std::fs::remove_file(&output);
                    return failed(reply, "document_expired_or_not_found");
                };
                if entry.previews.len() >= 4 {
                    let old = entry.previews.remove(0);
                    let _ = std::fs::remove_file(old);
                }
                entry.previews.push(output.clone());
                reply.state = "completed".into();
                reply.document_ref = Some(document_ref);
                reply.data = Some(json!({"path":output,"page_number":page_number,
                    "mime_type":"image/png","size":size,"sha256":sha256}));
                reply
            }
            DocumentAction::Close { document_ref } => {
                let mut entries = self.documents.entries.lock().await;
                match entries.get(&document_ref) {
                    Some(entry) if entry.owner == actor => {
                        if let Some(cancel) = &entry.cancel {
                            let _ = cancel.send(true);
                        }
                        if let Some(entry) = entries.remove(&document_ref) {
                            cleanup_previews(&entry.previews);
                        }
                        reply.state = "completed".into();
                        reply
                    }
                    None => {
                        reply.state = "completed".into();
                        reply
                    }
                    _ => failed(reply, "document_expired_or_not_found"),
                }
            }
        }
    }

    pub(super) async fn get_document(
        &self,
        actor: OperatorRef,
        request_id: RequestId,
    ) -> DocumentReply {
        let entries = self.documents.entries.lock().await;
        match entries.get(&request_id) {
            Some(entry) if entry.owner == actor && entry.created.elapsed() < DOCUMENT_LIFETIME => {
                entry.reply.clone()
            }
            _ => missing(request_id),
        }
    }

    pub(super) async fn cancel_document(
        &self,
        actor: OperatorRef,
        request_id: RequestId,
    ) -> DocumentReply {
        let mut entries = self.documents.entries.lock().await;
        match entries.get_mut(&request_id) {
            Some(entry) if entry.owner == actor => {
                if let Some(cancel) = &entry.cancel {
                    let _ = cancel.send(true);
                    entry.reply.state = "cancel_requested".into();
                }
                entry.reply.clone()
            }
            _ => missing(request_id),
        }
    }

    async fn lookup(&self, actor: OperatorRef, id: RequestId) -> Result<Arc<Document>, String> {
        let entries = self.documents.entries.lock().await;
        match entries.get(&id) {
            Some(entry) if entry.owner == actor && entry.created.elapsed() < DOCUMENT_LIFETIME => {
                entry
                    .document
                    .clone()
                    .ok_or_else(|| format!("document_{}", entry.reply.state))
            }
            _ => Err("document_expired_or_not_found".into()),
        }
    }
}

fn failed(mut reply: DocumentReply, message: &str) -> DocumentReply {
    reply.state = "failed".into();
    reply.error = Some(message.to_owned());
    reply
}

fn missing(request_id: RequestId) -> DocumentReply {
    DocumentReply {
        request_id,
        kind: "document_open".into(),
        state: "failed".into(),
        document_ref: None,
        data: None,
        error: Some("document_expired_or_not_found".into()),
    }
}

async fn run_worker(path: &str, mode: OcrMode) -> Result<Document, String> {
    let worker = worker_path().map_err(|e| e.to_string())?;
    let mut command = Command::new(worker);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("worker_start_failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(
                &serde_json::to_vec(&json!({"path": path, "ocr_mode": mode}))
                    .map_err(|e| e.to_string())?,
            )
            .await
            .map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.len() > pab_document::MAX_RESULT_BYTES + 1024 {
        return Err("worker_failed_or_output_limit".into());
    }
    tokio::task::spawn_blocking(move || decode_worker_reply(&output.stdout))
        .await
        .map_err(|error| format!("worker_decode_failed: {error}"))?
}

async fn run_user_worker(
    path: &str,
    mode: OcrMode,
    id: RequestId,
    prepared: pab_os_control::execution::PreparedUser,
    cancelled: watch::Receiver<bool>,
) -> Result<Document, String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let worker = worker_path().map_err(|e| e.to_string())?;
        let home = prepared.identity().home.clone();
        let output_path = home.join(format!(".pixels-document-{id}.json"));
        let mode = match mode {
            OcrMode::Auto => "auto",
            OcrMode::Off => "off",
            OcrMode::On => "on",
        };
        let args = [
            OsString::from("--output"),
            OsString::from(path),
            OsString::from(mode),
            output_path.as_os_str().to_owned(),
        ];
        let mut process = prepared
            .spawn(&worker, &args, &home)
            .map_err(|e| format!("user_worker_start_failed: {e}"))?;
        let deadline = Instant::now() + PARSE_DEADLINE;
        let result = loop {
            if *cancelled.borrow() {
                break Err("cancelled".into());
            }
            if Instant::now() >= deadline {
                break Err("worker_timeout".into());
            }
            match process.try_wait() {
                Ok(Some(0)) => {
                    let mut bytes = Vec::new();
                    let read = std::fs::File::open(&output_path).and_then(|file| {
                        file.take((pab_document::MAX_RESULT_BYTES + 1025) as u64)
                            .read_to_end(&mut bytes)
                    });
                    break match read {
                        Ok(_) if bytes.len() <= pab_document::MAX_RESULT_BYTES + 1024 => {
                            decode_worker_reply(&bytes)
                        }
                        Ok(_) => Err("worker_output_limit".into()),
                        Err(error) => Err(format!("worker_output_unavailable: {error}")),
                    };
                }
                Ok(Some(_)) => break Err("user_worker_failed".into()),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => break Err(format!("user_worker_wait_failed: {error}")),
            }
        };
        drop(process); // kill descendants before removing the output file
        let _ = std::fs::remove_file(&output_path);
        result
    })
    .await
    .map_err(|e| format!("worker_thread_failed: {e}"))?
}

fn decode_worker_reply(bytes: &[u8]) -> Result<Document, String> {
    match serde_json::from_slice::<WorkerReply>(bytes)
        .map_err(|e| format!("worker_reply_invalid: {e}"))?
    {
        WorkerReply::Success { document } => Ok(document),
        WorkerReply::Failure { error } => Err(error),
    }
}

fn worker_path() -> std::io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    Ok(executable.with_file_name(format!(
        "pab-document-worker{}",
        std::env::consts::EXE_SUFFIX
    )))
}

fn cleanup_previews(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn preview_args(
    path: String,
    hash: String,
    page: u32,
    dimension: u16,
    output: &Path,
) -> [OsString; 6] {
    [
        OsString::from("--render-output"),
        OsString::from(path),
        OsString::from(hash),
        OsString::from(page.to_string()),
        OsString::from(dimension.to_string()),
        output.as_os_str().to_owned(),
    ]
}

fn verify_preview(output: &Path) -> Result<(u64, String), String> {
    let file = std::fs::File::open(output).map_err(|e| format!("preview_unavailable: {e}"))?;
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("preview_invalid_or_too_large".into());
    }
    Ok((bytes.len() as u64, hex::encode(Sha256::digest(&bytes))))
}

async fn run_preview(
    path: String,
    hash: String,
    page: u32,
    dimension: u16,
    output: PathBuf,
) -> Result<(u64, String), String> {
    let mut command = Command::new(worker_path().map_err(|e| e.to_string())?);
    command
        .args(preview_args(path, hash, page, dimension, &output))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("preview_worker_start_failed: {e}"))?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .map_err(|_| "preview_timeout".to_owned())?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("preview_worker_failed_or_file_changed".into());
    }
    tokio::task::spawn_blocking(move || verify_preview(&output))
        .await
        .map_err(|e| e.to_string())?
}

async fn run_user_preview(
    path: String,
    hash: String,
    page: u32,
    dimension: u16,
    output: PathBuf,
    prepared: pab_os_control::execution::PreparedUser,
) -> Result<(u64, String), String> {
    tokio::task::spawn_blocking(move || {
        let worker = worker_path().map_err(|e| e.to_string())?;
        let home = prepared.identity().home.clone();
        let args = preview_args(path, hash, page, dimension, &output);
        let mut process = prepared
            .spawn(&worker, &args, &home)
            .map_err(|e| format!("preview_user_worker_start_failed: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let result = loop {
            if Instant::now() >= deadline {
                break Err("preview_timeout".into());
            }
            match process.try_wait() {
                Ok(Some(0)) => break verify_preview(&output),
                Ok(Some(_)) => break Err("preview_worker_failed_or_file_changed".into()),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => break Err(format!("preview_worker_wait_failed: {error}")),
            }
        };
        drop(process);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_document::{Page, Sheet, SpreadsheetCell};
    use pab_protocol::DocumentSelector;

    fn reply() -> DocumentReply {
        DocumentReply {
            request_id: RequestId::new(),
            kind: "document_search".into(),
            state: "running".into(),
            document_ref: None,
            data: None,
            error: None,
        }
    }

    #[test]
    fn spreadsheet_search_cites_cell_and_stops_at_last_match() {
        let document = Document {
            format: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            byte_size: 100,
            sha256: "0".repeat(64),
            text: "## Sheet Revenue\n\nA2\ttext\tWidget\t".into(),
            pages: vec![],
            tables: vec![],
            warnings: vec![],
            sheets: vec![Sheet {
                name: "Revenue".into(),
                cells: vec![SpreadsheetCell {
                    row: 2,
                    column: 1,
                    address: "A2".into(),
                    value: "Widget".into(),
                    value_type: "text".into(),
                    formula: None,
                }],
            }],
        };
        let result = search_document(
            reply(),
            RequestId::new(),
            &document,
            "Widget",
            false,
            1,
            None,
        );
        assert_eq!(
            result.data.as_ref().unwrap()["matches"][0]["location"],
            json!({"kind":"cell","sheet":"Revenue","address":"A2"})
        );
        assert!(result.data.as_ref().unwrap()["next_cursor"].is_null());
    }

    #[test]
    fn pdf_search_retains_page_location() {
        let document = Document {
            format: "application/pdf".into(),
            byte_size: 100,
            sha256: "0".repeat(64),
            text: "## Page 1\n\nFirst\n\n## Page 2\n\nRisk Clause".into(),
            pages: vec![
                Page {
                    number: 1,
                    sheet_name: None,
                    text: "First".into(),
                    speaker_notes: None,
                    ocr_confidence: None,
                },
                Page {
                    number: 2,
                    sheet_name: None,
                    text: "Risk Clause".into(),
                    speaker_notes: None,
                    ocr_confidence: None,
                },
            ],
            tables: vec![],
            sheets: vec![],
            warnings: vec![],
        };
        let result = search_document(
            reply(),
            RequestId::new(),
            &document,
            "Risk",
            false,
            20,
            None,
        );
        assert_eq!(
            result.data.as_ref().unwrap()["matches"][0]["location"],
            json!({"kind":"page","number":2})
        );
    }

    #[test]
    fn slide_read_includes_speaker_notes_and_slide_location() {
        let document = Document {
            format: "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                .into(),
            byte_size: 100,
            sha256: "0".repeat(64),
            text: "## Slide 1\n\nTitle\n\n> Speaker notes: 讲者备注".into(),
            pages: vec![Page {
                number: 1,
                sheet_name: None,
                text: "Title".into(),
                speaker_notes: Some("讲者备注".into()),
                ocr_confidence: None,
            }],
            tables: vec![],
            sheets: vec![],
            warnings: vec![],
        };
        let result = read_document(
            reply(),
            RequestId::new(),
            &document,
            &DocumentSelector::Page { number: 1 },
            None,
        );
        assert!(
            result.data.as_ref().unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("讲者备注")
        );
        assert_eq!(result.data.as_ref().unwrap()["location"]["kind"], "slide");
        assert_eq!(result.data.as_ref().unwrap()["location"]["number"], 1);
    }
}
