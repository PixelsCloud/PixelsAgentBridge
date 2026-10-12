//! Interactive-user Playwright MCP process with a Pixels-owned browser profile.
use pab_protocol::{
    AppSnapshot, BrowserAction, BrowserRequest, BrowserResult, ExecutionIdentity, OperatorRef,
    RequestId, SystemQueryData, SystemQueryReply,
};
use serde_json::{Value, json};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const CALL_TIMEOUT: Duration = Duration::from_secs(40);
const MAX_REPLY: usize = 2 * 1024 * 1024;
const MAX_SESSION_AGE: Duration = Duration::from_secs(60 * 60);
const IDLE_SESSION_AGE: Duration = Duration::from_secs(10 * 60);
const HANDOFF_AGE: Duration = Duration::from_secs(60 * 60);

pub struct BrowserManager {
    active: Option<BrowserSession>,
    terminal: Option<BrowserTerminal>,
}

struct BrowserTerminal {
    id: RequestId,
    owner: OperatorRef,
    pixels_user: pab_protocol::UserId,
    status: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserKind {
    Chrome,
    Edge,
}

impl BrowserKind {
    fn name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
        }
    }

    fn channel(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "msedge",
        }
    }
}

struct BrowserSession {
    id: RequestId,
    owner: OperatorRef,
    pixels_user: pab_protocol::UserId,
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    paused: bool,
    handoff_started_at: Option<Instant>,
    connected: bool,
    browser_kind: BrowserKind,
    _profile_lock: File,
    output_dir: PathBuf,
    downloads: BTreeSet<PathBuf>,
    created_at: Instant,
    last_active: Instant,
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self { active: None, terminal: None }
    }
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        let Some(mut session) = self.active.take() else { return; };
        session.clear_handoff();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { session.shutdown().await; });
        } else {
            let _ = session.child.start_kill();
        }
    }
}

impl BrowserManager {
    pub async fn expire_idle(&mut self) {
        let terminal = self.active.as_ref().and_then(|session| {
            if session.paused && session.handoff_cancelled() {
                Some("cancelled")
            } else if session.handoff_started_at.is_some_and(|at| at.elapsed() >= HANDOFF_AGE) {
                Some("timed_out")
            } else if !session.paused && (session.created_at.elapsed() >= MAX_SESSION_AGE
                || session.last_active.elapsed() >= IDLE_SESSION_AGE) {
                Some("expired")
            } else {
                None
            }
        });
        if let Some(status) = terminal {
            if let Some(mut session) = self.active.take() {
                self.terminal = Some(BrowserTerminal { id: session.id, owner: session.owner,
                    pixels_user: session.pixels_user, status });
                session.shutdown().await;
            }
        }
    }

    pub async fn query(
        &mut self,
        id: RequestId,
        request: &BrowserRequest,
        identity: &ExecutionIdentity,
        active: impl Fn() -> bool,
    ) -> SystemQueryReply {
        self.expire_idle().await;
        let mut reply = SystemQueryReply::pending_kind(id, request.action.name());
        let mut attempted = false;
        let result = async {
            request.validate().map_err(str::to_owned)?;
            pab_desktop_control::apps::verify_interactive_identity(identity, &active)?;
            let owner = request.owner.ok_or("browser owner missing from Executor")?;
            let pixels_user=request.pixels_user.ok_or("pixels_login_required: sign in before using browser tools")?;
            let result = if request.action == BrowserAction::SetupInfo {
                let runtime_ready = runtime_paths().is_ok();
                let pixels_browser = pixels_browser_executable();
                BrowserResult {
                    session_ref: None,
                    status: if !runtime_ready {
                        "browser_runtime_missing"
                    } else if pixels_browser.is_none() {
                        "browser_missing"
                    } else {
                        "ready"
                    }.into(),
                    data: json!({
                        "pixels_browser_available": runtime_ready && pixels_browser.is_some(),
                        "pixels_browser_channel": pixels_browser.as_ref().map(|(kind, _)| kind.channel()),
                        "profile_persistent": true,
                        "next": if runtime_ready && pixels_browser.is_some() {"Call pab_browser_attach to open the Pixels browser."} else {"Install the browser runtime and Chrome or Edge on the target device."}
                    }),
                    execution_identity: identity.clone(),
                }
            } else if request.action == BrowserAction::Attach {
                if self.active.is_some() { return Err("browser_busy: close the existing browser session first".into()); }
                let session = BrowserSession::start(owner,pixels_user).await?;
                let reference = session.id;
                let browser_kind = session.browser_kind;
                self.active = Some(session);
                self.terminal = None;
                BrowserResult { session_ref: Some(reference), status: "connected".into(),
                    data: json!({"mode":"pixels","browser":browser_kind.name(),"profile_persistent":true,"next":"Pixels opened its own browser window. Continue with the browser tools."}),
                    execution_identity: identity.clone() }
            } else {
                let Some(session) = self.active.as_mut() else {
                    if let Some(terminal) = self.terminal.as_ref()
                        && terminal.id == request.session_ref.ok_or("session_ref required")?
                        && terminal.owner == owner && terminal.pixels_user == pixels_user {
                        if request.action == BrowserAction::GetSession {
                            return Ok(BrowserResult { session_ref:Some(terminal.id), status:terminal.status.into(),
                                data:json!({"paused":false,"tabs":null}), execution_identity:identity.clone() });
                        }
                        if request.action == BrowserAction::Resume {
                            return Err(format!("browser_handoff_{}: start a new browser session", terminal.status));
                        }
                    }
                    return Err("browser_session_missing: attach first".into());
                };
                if session.id != request.session_ref.ok_or("session_ref required")? || session.owner != owner || session.pixels_user != pixels_user {
                    return Err("browser_session_mismatch: no access to this session".into());
                }
                session.last_active=Instant::now();
                if request.action == BrowserAction::Close {
                    let reference = session.id;
                    session.shutdown().await;
                    self.active = None;
                    BrowserResult { session_ref: Some(reference), status:"closed".into(), data:json!({}),
                        execution_identity:identity.clone() }
                } else if request.action == BrowserAction::RequestHandoff {
                    if session.paused { return Err("browser_handoff_already_active".into()); }
                    let marker_path = session.write_handoff(request.arguments["reason"].as_str().ok_or("handoff reason required")?)?;
                    session.paused = true;
                    session.handoff_started_at=Some(Instant::now());
                    BrowserResult { session_ref:Some(session.id),status:"awaiting_user".into(),
                        data:json!({"reason":request.arguments["reason"],"marker_path":marker_path,"expires_at_unix_ms":now()+HANDOFF_AGE.as_millis() as i64,
                            "instruction":"Complete sign-in in the same browser tab on the target device, then confirm in Pixels Desktop. Browser reads and writes are paused until resume."}),
                        execution_identity:identity.clone() }
                } else if request.action == BrowserAction::GetSession {
                    let observed=if session.paused { None } else { Some(session.tool("browser_tabs",json!({"action":"list"})).await) };
                    if let Some(observed)=&observed {
                        session.connected=observed.is_ok();
                    }
                    BrowserResult { session_ref:Some(session.id),status:if session.paused && session.handoff_cancelled() {"cancelled"} else if session.paused && session.handoff_ready() {"ready_to_resume"} else if session.paused {"awaiting_user"} else if session.connected {"connected"} else {"connection_lost"}.into(),
                        data:json!({"mode":"pixels","browser":session.browser_kind.name(),"profile_persistent":true,"paused":session.paused,"tabs":observed.and_then(Result::ok)}),
                        execution_identity:identity.clone() }
                } else {
                    if session.paused && request.action != BrowserAction::Resume {
                        return Err("auth_required: browser session is paused for user sign-in".into());
                    }
                    if request.action == BrowserAction::Resume && session.handoff_cancelled() {
                        return Err("browser_handoff_cancelled: close this session or start another task".into());
                    }
                    if request.action == BrowserAction::Resume && (!session.paused || !session.handoff_ready()) {
                        return Err("auth_required: wait for the target user to confirm sign-in in Pixels Desktop".into());
                    }
                    attempted = request.action.is_mutation();
                    let data = session.dispatch(request).await?;
                    if request.action == BrowserAction::Resume {
                        session.paused = false;
                        session.handoff_started_at=None;
                        session.created_at=Instant::now();
                        session.clear_handoff();
                    }
                    BrowserResult { session_ref:Some(session.id),status:"completed".into(),data,
                        execution_identity:identity.clone() }
                }
            };
            pab_desktop_control::apps::verify_interactive_identity(identity, &active)?;
            Ok::<_,String>(result)
        }.await;
        match result {
            Ok(result) => {
                reply.returned_count = 1;
                reply.data = Some(SystemQueryData::Applications {
                    snapshot: AppSnapshot::Browser { result },
                });
                reply.state = "completed".into();
            }
            Err(error) => {
                reply.state = if attempted { "unconfirmed" } else { "failed" }.into();
                reply.error = Some(error);
            }
        }
        reply.sampled_at_unix_ms = Some(now());
        reply
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn packaged_runtime() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let support = exe
        .parent()
        .ok_or("desktop executable has no parent")?
        .join("browser-runtime");
    #[cfg(target_os = "macos")]
    let support = PathBuf::from("/Library/Application Support/PixelsAgentBridge/browser-runtime");
    #[cfg(not(any(windows, target_os = "macos")))]
    let support = exe
        .parent()
        .ok_or("desktop executable has no parent")?
        .join("browser-runtime");
    Ok(support)
}

fn runtime_paths() -> Result<(PathBuf, PathBuf), String> {
    let support = packaged_runtime()?;
    let node = support.join(if cfg!(windows) { "node.exe" } else { "node" });
    let cli = support.join("node_modules/@playwright/mcp/cli.js");
    if !node.is_file() || !cli.is_file() {
        return Err("browser_runtime_missing: install the Pixels browser runtime".into());
    }
    Ok((node, cli))
}

pub(crate) fn chrome_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                candidates.push(PathBuf::from(root).join("Google/Chrome/Application/chrome.exe"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        candidates.extend(
            [
                "/usr/bin/google-chrome",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
            ]
            .map(PathBuf::from),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

pub(crate) fn edge_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    for variable in ["PROGRAMFILES(X86)", "PROGRAMFILES", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(root).join("Microsoft/Edge/Application/msedge.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            );
        }
    }
    #[cfg(target_os = "linux")]
    candidates.push(PathBuf::from("/opt/microsoft/msedge/msedge"));
    candidates.into_iter().find(|path| path.is_file())
}

fn pixels_browser_executable() -> Option<(BrowserKind, PathBuf)> {
    prefer_browser(chrome_executable(), edge_executable())
}

fn prefer_browser(
    chrome: Option<PathBuf>,
    edge: Option<PathBuf>,
) -> Option<(BrowserKind, PathBuf)> {
    chrome
        .map(|path| (BrowserKind::Chrome, path))
        .or_else(|| edge.map(|path| (BrowserKind::Edge, path)))
}

fn pixels_profile_dir(
    user_data_root: &Path,
    pixels_user: pab_protocol::UserId,
    browser: BrowserKind,
) -> PathBuf {
    user_data_root
        .join("browser-profiles")
        .join(pixels_user.to_string())
        .join(browser.name())
}

fn profile_lock_path(root: &Path, user: pab_protocol::UserId, browser: BrowserKind) -> PathBuf {
    root.join("browser-profiles")
        .join(user.to_string())
        .join(format!("{}.lock", browser.name()))
}

fn lock_profile(root: &Path, user: pab_protocol::UserId, browser: BrowserKind) -> Result<File, String> {
    let path = profile_lock_path(root, user, browser);
    pab_agent_core::ensure_data_parent(&path).map_err(|e| e.to_string())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|e| e.to_string())?;
    pab_agent_core::restrict_private_file(&path).map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_| "browser_profile_in_use: close the Pixels browser session first".to_owned())?;
    Ok(file)
}

pub(crate) fn saved_profiles(root: &Path, user: pab_protocol::UserId) -> [bool; 2] {
    [BrowserKind::Chrome, BrowserKind::Edge]
        .map(|browser| pixels_profile_dir(root, user, browser).is_dir())
}

pub(crate) fn clear_profile(root: &Path, user: pab_protocol::UserId, name: &str) -> Result<(), String> {
    let browser = match name {
        "chrome" => BrowserKind::Chrome,
        "edge" => BrowserKind::Edge,
        _ => return Err("invalid browser profile".into()),
    };
    let _lock = lock_profile(root, user, browser)?;
    let path = pixels_profile_dir(root, user, browser);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("browser profile is not an ordinary directory".into());
    }
    let canonical_parent = path.parent().ok_or("invalid browser profile")?
        .canonicalize().map_err(|e| e.to_string())?;
    let canonical_profile = path.canonicalize().map_err(|e| e.to_string())?;
    if canonical_profile != canonical_parent.join(browser.name()) {
        return Err("browser profile path resolves outside its account directory".into());
    }
    fs::remove_dir_all(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod browser_setup_tests {
    use super::*;

    #[test]
    fn pixels_prefers_chrome_and_falls_back_to_edge() {
        let chrome = PathBuf::from("chrome");
        let edge = PathBuf::from("edge");
        assert_eq!(
            prefer_browser(Some(chrome.clone()), Some(edge.clone())),
            Some((BrowserKind::Chrome, chrome))
        );
        assert_eq!(
            prefer_browser(None, Some(edge.clone())),
            Some((BrowserKind::Edge, edge))
        );
        assert_eq!(prefer_browser(None, None), None);
    }

    #[test]
    fn pixels_profiles_are_stable_and_account_browser_separated() {
        let root = PathBuf::from("pixels-data");
        let first = pab_protocol::UserId::from_u128(1);
        let second = pab_protocol::UserId::from_u128(2);
        let path = pixels_profile_dir(&root, first, BrowserKind::Chrome);
        assert_eq!(path, pixels_profile_dir(&root, first, BrowserKind::Chrome));
        assert_ne!(path, pixels_profile_dir(&root, second, BrowserKind::Chrome));
        assert_ne!(path, pixels_profile_dir(&root, first, BrowserKind::Edge));
        assert!(path.starts_with(&root));
    }

    #[test]
    fn profile_clear_is_account_scoped_and_refuses_active_profile() {
        let temp = tempfile::tempdir().unwrap();
        let first = pab_protocol::UserId::from_u128(1);
        let second = pab_protocol::UserId::from_u128(2);
        let first_path = pixels_profile_dir(temp.path(), first, BrowserKind::Chrome);
        let second_path = pixels_profile_dir(temp.path(), second, BrowserKind::Chrome);
        pab_agent_core::ensure_data_dir(&first_path).unwrap();
        pab_agent_core::ensure_data_dir(&second_path).unwrap();
        fs::write(first_path.join("marker"), b"first").unwrap();
        fs::write(second_path.join("marker"), b"second").unwrap();
        let lock = lock_profile(temp.path(), first, BrowserKind::Chrome).unwrap();
        assert!(clear_profile(temp.path(), first, "chrome").unwrap_err().contains("in_use"));
        drop(lock);
        clear_profile(temp.path(), first, "chrome").unwrap();
        assert!(!first_path.exists());
        assert!(second_path.join("marker").exists());
        assert!(clear_profile(temp.path(), first, "../../other").is_err());
    }

}

impl BrowserSession {
    async fn shutdown(&mut self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.tool("browser_close", json!({})),
        )
        .await;
        let _ = self.child.kill().await;
        self.clear_handoff();
    }

    fn write_handoff(&self, reason: &str) -> Result<PathBuf, String> {
        let path = handoff_path()?;
        let parent = path.parent().ok_or("invalid handoff path")?;
        pab_agent_core::ensure_data_dir(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let marker = json!({"session_ref":self.id,"helper_pid":std::process::id(),"reason":reason,"expires_at_unix_ms":now()+HANDOFF_AGE.as_millis() as i64,"ready":false,"cancelled":false});
        serde_json::to_writer(file.as_file_mut(), &marker).map_err(|e| e.to_string())?;
        file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
        pab_agent_core::restrict_private_file(file.path()).map_err(|e| e.to_string())?;
        file.persist(&path).map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn handoff_ready(&self) -> bool {
        let Ok(path) = handoff_path() else {
            return false;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        if bytes.len() > 4096 {
            return false;
        }
        let Ok(marker) = serde_json::from_slice::<Value>(&bytes) else {
            return false;
        };
        marker["session_ref"] == json!(self.id)
            && marker["ready"] == true
            && marker["cancelled"] != true
            && marker["expires_at_unix_ms"]
                .as_i64()
                .is_some_and(|at| at > now())
    }

    fn handoff_cancelled(&self) -> bool {
        let Ok(path) = handoff_path() else { return false; };
        let Ok(bytes) = std::fs::read(path) else { return false; };
        if bytes.len() > 4096 { return false; }
        let Ok(marker) = serde_json::from_slice::<Value>(&bytes) else { return false; };
        marker["session_ref"] == json!(self.id) && marker["cancelled"] == true
    }

    fn clear_handoff(&self) {
        let Ok(path) = handoff_path() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        if bytes.len() > 4096 {
            return;
        }
        let Ok(marker) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        if marker["session_ref"] == json!(self.id) {
            let _ = std::fs::remove_file(path);
        }
    }

    async fn start(owner: OperatorRef, pixels_user: pab_protocol::UserId) -> Result<Self, String> {
        let (node, cli) = runtime_paths()?;
        let (browser_kind, executable) = pixels_browser_executable().ok_or(
            "browser_missing: the Pixels browser needs an installed Chrome or Edge executable",
        )?;
        let profile_dir = pixels_profile_dir(
            pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
                .map_err(|e| e.to_string())?
                .root(),
            pixels_user,
            browser_kind,
        );
        let profile_lock = lock_profile(
            pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
                .map_err(|e| e.to_string())?
                .root(),
            pixels_user,
            browser_kind,
        )?;
        pab_agent_core::ensure_data_dir(&profile_dir).map_err(|e| e.to_string())?;
        let output_dir = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
            .map_err(|e| e.to_string())?
            .root()
            .join("browser-artifacts")
            .join(RequestId::new().to_string());
        pab_agent_core::ensure_data_dir(&output_dir).map_err(|e| e.to_string())?;
        let launch_config = output_dir.join("browser-launch.json");
        fs::write(&launch_config, serde_json::to_vec(&json!({
            "browser": {
                "launchOptions": { "args": ["--start-maximized"] },
                "contextOptions": { "viewport": null }
            }
        })).map_err(|e| e.to_string())?).map_err(|e| format!("browser launch config failed: {e}"))?;
        let mut command = Command::new(node);
        command
            .arg(cli)
            .arg("--config")
            .arg(&launch_config)
            .arg("--no-webmcp")
            .arg("--allow-unrestricted-file-access")
            .arg("--image-responses=omit")
            .arg("--caps")
            .arg("vision,pdf")
            .arg("--snapshot-boxes")
            .arg("--timeout-navigation")
            .arg("20000")
            .arg("--timeout-action")
            .arg("15000")
            .arg("--output-dir")
            .arg(&output_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
            .arg("--browser")
            .arg(browser_kind.channel())
            .arg("--user-data-dir")
            .arg(&profile_dir)
            .arg("--executable-path")
            .arg(&executable);
        #[cfg(windows)]
        command
            .as_std_mut()
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| format!("browser runtime start failed: {e}"))?;
        let input = child.stdin.take().ok_or("browser runtime has no stdin")?;
        let output = BufReader::new(child.stdout.take().ok_or("browser runtime has no stdout")?);
        let mut session = Self {
            id: RequestId::new(),
            owner,
            pixels_user,
            child,
            input,
            output,
            next_id: 1,
            paused: false,
            handoff_started_at: None,
            output_dir,
            downloads: BTreeSet::new(),
            connected: false,
            browser_kind,
            _profile_lock: profile_lock,
            created_at: Instant::now(),
            last_active: Instant::now(),
        };
        session.rpc("initialize",json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"Pixels Agent Bridge","version":env!("CARGO_PKG_VERSION")}})).await?;
        session
            .write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        session
            .tool("browser_tabs", json!({"action":"list"}))
            .await?;
        session.connected = true;
        Ok(session)
    }

    async fn write(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|e| e.to_string())?;
        self.input.flush().await.map_err(|e| e.to_string())
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(CALL_TIMEOUT, async {
            loop {
                let mut line = Vec::new();
                loop {
                    let chunk = self.output.fill_buf().await.map_err(|e| e.to_string())?;
                    if chunk.is_empty() {
                        return Err("browser runtime exited".into());
                    }
                    let end = chunk
                        .iter()
                        .position(|b| *b == b'\n')
                        .map_or(chunk.len(), |i| i + 1);
                    if line.len() + end > MAX_REPLY {
                        return Err("browser reply exceeds limit".into());
                    }
                    let done = chunk[end - 1] == b'\n';
                    line.extend_from_slice(&chunk[..end]);
                    self.output.consume(end);
                    if done {
                        break;
                    }
                }
                let value: Value =
                    serde_json::from_slice(&line).map_err(|_| "invalid browser runtime reply")?;
                if value["id"].as_u64() != Some(id) {
                    continue;
                }
                if !value["error"].is_null() {
                    return Err(format!(
                        "browser runtime: {}",
                        value["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                    ));
                }
                return Ok(value["result"].clone());
            }
        })
        .await
        .map_err(|_| "browser runtime timed out; preserve request_id and inspect session")?
    }

    async fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let response = self
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await?;
        if response["isError"] == true {
            let message = response["content"]
                .as_array()
                .and_then(|a| a.iter().find_map(|v| v["text"].as_str()))
                .unwrap_or("browser tool failed");
            return Err(message.chars().take(1000).collect());
        }
        let mut bytes = 0usize;
        let mut content = Vec::new();
        let mut truncated = false;
        for item in response["content"].as_array().into_iter().flatten() {
            if item["type"] != "text" {
                continue;
            }
            if let Some(value) = item["text"].as_str() {
                self.record_downloads(value);
                if bytes >= 20_000 {
                    truncated = true;
                    break;
                }
                let mut end = value.len().min(20_000 - bytes);
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                content.push(json!({"type":"text","text":&value[..end]}));
                bytes += end;
                truncated |= end < value.len();
            }
        }
        Ok(json!({"content":content,"truncated":truncated}))
    }

    async fn dispatch(&mut self, request: &BrowserRequest) -> Result<Value, String> {
        use BrowserAction::*;
        let a = &request.arguments;
        let call = match request.action {
            Resume | Snapshot => ("browser_snapshot", json!({})),
            Find => ("browser_find", a.clone()),
            Navigate => {
                if let Some(url) = a["url"].as_str() {
                    ("browser_navigate", json!({"url":http_url(url)?}))
                } else {
                    match a["direction"].as_str() {
                        Some("back") => ("browser_navigate_back", json!({})),
                        Some("forward") | Some("reload") => {
                            let function = if a["direction"] == "forward" {
                                "() => { history.forward(); return 'forward requested'; }"
                            } else {
                                "() => { location.reload(); return 'reload requested'; }"
                            };
                            self.tool("browser_evaluate", json!({"function":function}))
                                .await?;
                            self.tool("browser_wait_for", json!({"time":0.5})).await?;
                            return self.tool("browser_snapshot", json!({})).await;
                        }
                        _ => return Err("unsupported navigation direction".into()),
                    }
                }
            }
            Open => (
                "browser_tabs",
                json!({"action":"new","url":http_url(a["url"].as_str().ok_or("url required")?)?}),
            ),
            Tabs => {
                let mut args = a.clone();
                if a["action"] == "new" {
                    args["url"] = json!(http_url(a["url"].as_str().ok_or("url required")?)?);
                }
                ("browser_tabs", args)
            }
            Screenshot => {
                let path = self
                    .output_dir
                    .join(format!("screenshot-{}.jpeg", RequestId::new()));
                (
                    "browser_take_screenshot",
                    json!({"type":"jpeg","scale":"css","filename":path}),
                )
            }
            Wait => (
                "browser_wait_for",
                json!({"text":a["text"],"textGone":a["text_gone"],"time":a["time"]}),
            ),
            Click => (
                "browser_click",
                json!({"target":a["target"],"element":a["element"],"doubleClick":a["double_click"],"button":a["button"]}),
            ),
            Fill => (
                "browser_type",
                json!({"target":a["target"],"element":a["element"],"text":a["text"],"slowly":a["slowly"],"submit":a["submit"]}),
            ),
            FillForm => ("browser_fill_form", a.clone()),
            PressKey => ("browser_press_key", a.clone()),
            SelectOption => ("browser_select_option", a.clone()),
            Hover => ("browser_hover", a.clone()),
            Drag => (
                "browser_drag",
                json!({"startTarget":a["start_target"],"endTarget":a["end_target"]}),
            ),
            HandleDialog => (
                "browser_handle_dialog",
                json!({"accept":a["accept"],"promptText":a["prompt_text"]}),
            ),
            FileUpload => ("browser_file_upload", a.clone()),
            Scroll => {
                let key = if a["direction"] == "up" {
                    "PageUp"
                } else {
                    "PageDown"
                };
                let pages = a["pages"].as_u64().unwrap_or(1).min(10);
                let mut result = Value::Null;
                for _ in 0..pages {
                    result = self.tool("browser_press_key", json!({"key":key})).await?;
                }
                return Ok(result);
            }
            SetChecked => {
                let desired = a["checked"].as_bool().ok_or("checked required")?;
                let function = format!(
                    "(element) => {{ if (!(element instanceof HTMLInputElement) || !['checkbox','radio'].includes(element.type)) throw Error('target is not a checkbox or radio'); if (element.checked !== {desired}) element.click(); return {{checked:element.checked}}; }}"
                );
                (
                    "browser_evaluate",
                    json!({"target":a["target"],"element":"checkbox or radio","function":function}),
                )
            }
            Extract => {
                let kind = a["kind"].as_str().ok_or("kind required")?;
                let limit = a["limit"].as_u64().unwrap_or(100).min(100);
                let expression=match kind {
                    "text"=>"element.innerText?.slice(0,16000)".to_owned(),
                    "link"=>"element.href || null".to_owned(),
                    "value"=>"element.matches('input[type=password],input[autocomplete=one-time-code],input[type=hidden]') ? '[redacted]' : (element.value ?? null)".to_owned(),
                    "attributes"=>format!("Object.fromEntries([...element.attributes].filter(a=>!['value','data-token','data-secret'].includes(a.name)).slice(0,{limit}).map(a=>[a.name,a.value.slice(0,1000)]))"),
                    "table"=>format!("[...element.querySelectorAll('tr')].slice(0,{limit}).map(r=>[...r.children].slice(0,30).map(c=>c.innerText.slice(0,500)))"),
                    _=>return Err("invalid extract kind".into()),
                };
                (
                    "browser_evaluate",
                    json!({"target":a["target"],"element":"selected element","function":format!("(element) => ({expression})")}),
                )
            }
            Downloads => return self.downloads(a["filename"].as_str()),
            Pointer => {
                let tool = match a["action"].as_str() {
                    Some("click") => "browser_mouse_click_xy",
                    Some("move") => "browser_mouse_move_xy",
                    Some("wheel") => "browser_mouse_wheel",
                    _ => return Err("invalid pointer action".into()),
                };
                let args = if a["action"] == "wheel" {
                    json!({"deltaX":a["delta_x"].as_i64().unwrap_or(0),"deltaY":a["delta_y"].as_i64().unwrap_or(0)})
                } else {
                    json!({"x":a["x"],"y":a["y"],"button":a["button"]})
                };
                (tool, args)
            }
            Drop => ("browser_drop", a.clone()),
            ConsoleMessages => ("browser_console_messages", a.clone()),
            NetworkRequests => (
                "browser_network_requests",
                json!({"static":a["static"],"filter":a["filter"].as_str().map(|v|v.replace('.',"\\."))}),
            ),
            PdfSave => ("browser_pdf_save", json!({})),
            SetupInfo | Attach | GetSession | Close | RequestHandoff => {
                return Err("internal browser action".into());
            }
        };
        let mut args = call.1;
        if let Some(map) = args.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        self.tool(call.0, args).await
    }
}

pub(crate) fn handoff_path() -> Result<PathBuf, String> {
    Ok(
        pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
            .map_err(|e| e.to_string())?
            .root()
            .join("browser-handoff.json"),
    )
}

impl BrowserSession {
    fn record_downloads(&mut self, text: &str) {
        let Ok(root) = std::fs::canonicalize(&self.output_dir) else {
            return;
        };
        for line in text
            .lines()
            .filter(|line| line.starts_with("- Downloaded file "))
        {
            let Some((_, path)) = line.rsplit_once(" to \"") else {
                continue;
            };
            let Some(path) = path.strip_suffix('"') else {
                continue;
            };
            let Ok(path) = std::fs::canonicalize(path) else {
                continue;
            };
            if path.starts_with(&root) && path.is_file() {
                self.downloads.insert(path);
            }
        }
    }

    fn downloads(&self, exact_filename: Option<&str>) -> Result<Value, String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut files = Vec::new();
        let mut scanned = 0usize;
        for path in &self.downloads {
            if exact_filename
                .is_some_and(|name| path.file_name().and_then(|file| file.to_str()) != Some(name))
            {
                continue;
            }
            scanned += 1;
            if scanned > 100 {
                break;
            }
            let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
                continue;
            }
            let mut reader = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            files.push(json!({"path":path,"bytes":metadata.len(),"sha256":format!("{:x}",digest.finalize()),"source":"playwright_reported"}));
        }
        Ok(json!({
            "downloads":files,
            "truncated":scanned>100,
            "tracking":"playwright_reported",
            "note":"Only files reported by the Pixels browser runtime for this session are listed. Use an exact filename to filter these files."
        }))
    }
}

fn http_url(input: &str) -> Result<&str, String> {
    if (input.starts_with("https://") || input.starts_with("http://"))
        && !input.chars().any(char::is_control)
    {
        Ok(input)
    } else {
        Err("only http(s) URLs are allowed".into())
    }
}
