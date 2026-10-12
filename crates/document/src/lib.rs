//! Bounded document extraction. Call this only inside a disposable worker process:
//! an async extractor can still allocate or block inside native codecs.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::{Cursor, Read}, path::Path};
use std::collections::BTreeMap;
use thiserror::Error;
use anydoc::{ConvertError, Format, model::{Block, CellSlot, inlines_to_plain_text}};
use calamine::{Data, Reader, open_workbook_auto_from_rs};
use pdfium_render::prelude::*;
use xberg_tesseract::{TesseractAPI, TessPageSegMode};
mod presentation;

pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAGES: usize = 500;
pub const MAX_CELLS: usize = 250_000;

#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("unsupported_format: {0}")]
    UnsupportedFormat(String),
    #[error("resource_limit: {0}")]
    ResourceLimit(&'static str),
    #[error("file_changed_during_snapshot")]
    FileChanged,
    #[error("permission_denied: {0}")]
    PermissionDenied(String),
    #[error("worker_failed: {0}")]
    Extraction(String),
    #[error("password_required")]
    PasswordRequired,
    #[error("no_extractable_text")]
    NoText,
    #[error("ocr_unavailable: bundled English and Chinese models are missing")]
    OcrUnavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrMode { Auto, Off, On }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub number: u32,
    pub sheet_name: Option<String>,
    pub text: String,
    pub speaker_notes: Option<String>,
    pub ocr_confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub page_number: u32,
    pub cells: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpreadsheetCell {
    /// Absolute, one-based coordinates. Empty cells are represented by gaps.
    pub row: u32,
    pub column: u32,
    pub address: String,
    pub value: String,
    pub value_type: String,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    pub cells: Vec<SpreadsheetCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub byte_size: u64,
    pub sha256: String,
    pub text: String,
    pub pages: Vec<Page>,
    pub tables: Vec<Table>,
    pub sheets: Vec<Sheet>,
    pub warnings: Vec<String>,
}

fn address(row: u32, column: u32) -> String {
    let mut col = column + 1;
    let mut letters = String::new();
    while col > 0 {
        col -= 1;
        letters.insert(0, (b'A' + (col % 26) as u8) as char);
        col /= 26;
    }
    format!("{letters}{}", row + 1)
}

fn spreadsheet(bytes: &[u8]) -> Result<Vec<Sheet>, DocumentError> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let names = workbook.sheet_names().to_vec();
    if names.len() > MAX_PAGES { return Err(DocumentError::ResourceLimit("too many worksheets")); }
    let mut sheets = Vec::with_capacity(names.len());
    let mut total_cells = 0usize;
    for name in names {
        let range = workbook.worksheet_range(&name)
            .map_err(|e| DocumentError::Extraction(e.to_string()))?;
        let formulas = workbook.worksheet_formula(&name)
            .map_err(|e| DocumentError::Extraction(e.to_string()))?;
        let (start_row, start_column) = range.start().unwrap_or((0, 0));
        let mut cells = BTreeMap::new();
        for (relative_row, relative_column, data) in range.used_cells() {
            let row = start_row + relative_row as u32;
            let column = start_column + relative_column as u32;
            let (value_type, value) = match data {
                Data::Int(v) => ("integer", v.to_string()),
                Data::Float(v) => ("number", v.to_string()),
                Data::String(v) => ("text", v.clone()),
                Data::Bool(v) => ("boolean", v.to_string()),
                Data::DateTime(v) => ("date", v.to_string()),
                Data::DateTimeIso(v) => ("date", v.clone()),
                Data::DurationIso(v) => ("duration", v.clone()),
                Data::Error(v) => ("error", format!("{v:?}")),
                Data::Empty => continue,
            };
            let formula = formulas.get_value((row, column)).filter(|v| !v.is_empty()).cloned();
            cells.insert((row, column), SpreadsheetCell { row: row + 1, column: column + 1, address: address(row, column), value, value_type: value_type.into(), formula });
        }
        let (formula_start_row, formula_start_column) = formulas.start().unwrap_or((0, 0));
        for (relative_row, relative_column, formula) in formulas.used_cells() {
            let row = formula_start_row + relative_row as u32;
            let column = formula_start_column + relative_column as u32;
            cells.entry((row, column)).and_modify(|cell| cell.formula = Some(formula.clone()))
                .or_insert_with(|| SpreadsheetCell { row: row + 1, column: column + 1,
                    address: address(row, column), value: String::new(), value_type: "unavailable".into(),
                    formula: Some(formula.clone()) });
        }
        total_cells += cells.len();
        if total_cells > MAX_CELLS { return Err(DocumentError::ResourceLimit("too many nonempty cells")); }
        sheets.push(Sheet { name, cells: cells.into_values().collect() });
    }
    Ok(sheets)
}

fn mime(path: &Path) -> Result<&'static str, DocumentError> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Ok("application/pdf"),
        "docx" => Ok("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xlsx" => Ok("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "pptx" => Ok("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "txt" | "md" => Ok("text/plain"),
        "html" | "htm" => Ok("text/html"),
        "csv" => Ok("text/csv"),
        "doc" => Ok("application/msword"),
        "ppt" => Ok("application/vnd.ms-powerpoint"),
        "xls" => Ok("application/vnd.ms-excel"),
        "odt" => Ok("application/vnd.oasis.opendocument.text"),
        "ods" => Ok("application/vnd.oasis.opendocument.spreadsheet"),
        "odp" => Ok("application/vnd.oasis.opendocument.presentation"),
        "rtf" => Ok("application/rtf"),
        "epub" => Ok("application/epub+zip"),
        _ => Err(DocumentError::UnsupportedFormat(ext)),
    }
}

fn check_zip_budget(bytes: &[u8]) -> Result<(), DocumentError> {
    if !bytes.starts_with(b"PK\x03\x04") { return Ok(()); }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| DocumentError::Extraction(error.to_string()))?;
    if archive.len() > 10_000 { return Err(DocumentError::ResourceLimit("archive has too many entries")); }
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let part = archive.by_index(index).map_err(|error| DocumentError::Extraction(error.to_string()))?;
        expanded = expanded.saturating_add(part.size());
        if part.size() > 64 * 1024 * 1024 || expanded > 128 * 1024 * 1024 {
            return Err(DocumentError::ResourceLimit("archive expansion exceeds 128 MiB"));
        }
    }
    Ok(())
}

fn convert_error(error: ConvertError) -> DocumentError {
    match error {
        ConvertError::Unsupported(message) => DocumentError::UnsupportedFormat(message),
        ConvertError::Encrypted => DocumentError::PasswordRequired,
        ConvertError::ResourceLimit { .. } => DocumentError::ResourceLimit("anydoc parser limit"),
        ConvertError::NeedsOcr { .. } => DocumentError::OcrUnavailable,
        other => DocumentError::Extraction(other.to_string()),
    }
}

fn block_text(block: &Block) -> String {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => inlines_to_plain_text(content),
        Block::CodeBlock { text, .. } | Block::Math(text) => text.clone(),
        Block::BlockQuote(blocks) => blocks.iter().map(block_text).collect::<Vec<_>>().join("\n"),
        Block::Table(table) => table.grid.iter().map(|row| row.iter().map(|slot| match slot {
            CellSlot::Origin(cell) => cell.blocks.iter().map(block_text).collect::<Vec<_>>().join("\n"),
            CellSlot::Covered { .. } => String::new(),
        }).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"),
        Block::List(_) | Block::Rule => String::new(),
    }
}

fn collect_tables(blocks: &[Block], tables: &mut Vec<Table>) {
    for block in blocks {
        match block {
            Block::Table(table) => tables.push(Table {
                page_number: 0,
                cells: table.grid.iter().map(|row| row.iter().map(|slot| match slot {
                    CellSlot::Origin(cell) => cell.blocks.iter().map(block_text).collect::<Vec<_>>().join("\n"),
                    CellSlot::Covered { .. } => String::new(),
                }).collect()).collect(),
            }),
            Block::BlockQuote(nested) => collect_tables(nested, tables),
            _ => {}
        }
    }
}

fn ocr_image(bytes: &[u8], data_dir: &Path) -> Result<(String, f32), DocumentError> {
    let image = image::load_from_memory(bytes).map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 20_000_000 {
        return Err(DocumentError::ResourceLimit("image exceeds 20 million pixels"));
    }
    let rgb = image.into_rgb8();
    let api = TesseractAPI::new().map_err(|e| DocumentError::Extraction(e.to_string()))?;
    api.init(data_dir, "chi_sim+eng").map_err(|e| DocumentError::Extraction(e.to_string()))?;
    api.set_page_seg_mode(TessPageSegMode::PSM_AUTO).map_err(|e| DocumentError::Extraction(e.to_string()))?;
    api.set_image(rgb.as_raw(), width as i32, height as i32, 3, width as i32 * 3)
        .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let text = api.get_utf8_text().map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let confidence = api.mean_text_conf().map_err(|e| DocumentError::Extraction(e.to_string()))? as f32;
    Ok((text, confidence))
}

fn render_pdf_page(bytes: &[u8], page_number: u32, max_dimension: u16) -> Result<Vec<u8>, DocumentError> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| DocumentError::Extraction("cannot locate PDF renderer".into()))?;
    let runtime = exe_dir.join("document-runtime");
    let library = Pdfium::pdfium_platform_library_name_at_path(if runtime.is_dir() { &runtime } else { &exe_dir });
    let bindings = Pdfium::bind_to_library(library)
        .map_err(|e| DocumentError::Extraction(format!("pdfium_unavailable: {e}")))?;
    let pdfium = Pdfium::new(bindings);
    let pdf = pdfium.load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| DocumentError::Extraction(format!("PDF render failed: {e}")))?;
    let page_index = u16::try_from(page_number.saturating_sub(1))
        .map_err(|_| DocumentError::ResourceLimit("PDF page index"))?;
    let page = pdf.pages().get(page_index).map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(max_dimension as i32).set_maximum_height(max_dimension as i32))
        .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    let mut output = Cursor::new(Vec::new());
    bitmap.as_image().write_to(&mut output, image::ImageFormat::Png)
        .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    Ok(output.into_inner())
}

/// Reads one immutable input snapshot under the file budget. The caller must
/// execute this inside a worker with its own process memory and time limits.
pub async fn extract_path(path: &Path, ocr: OcrMode) -> Result<Document, DocumentError> {
    let data_dir = std::env::current_exe().ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("document-runtime").join("tessdata")));
    extract_path_with_data(path, ocr, data_dir.as_deref()).await
}

/// Renders a page of the same input snapshot used by `extract_path`.
/// This must run in the disposable worker, never in the Executor event loop.
pub fn render_page(path: &Path, expected_sha256: &str, page_number: u32,
    max_dimension: u16) -> Result<Vec<u8>, DocumentError> {
    if page_number == 0 || !(256..=2048).contains(&max_dimension) {
        return Err(DocumentError::ResourceLimit("invalid preview dimensions"));
    }
    let mut file = File::open(path).map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    let size = file.metadata().map_err(|e| DocumentError::PermissionDenied(e.to_string()))?.len();
    if size > MAX_INPUT_BYTES { return Err(DocumentError::ResourceLimit("input exceeds 64 MiB")); }
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file).take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    if bytes.len() as u64 != size || hex::encode(Sha256::digest(&bytes)) != expected_sha256 {
        return Err(DocumentError::FileChanged);
    }
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
    let png = if extension.eq_ignore_ascii_case("pdf") {
        render_pdf_page(&bytes, page_number, max_dimension)?
    } else if ["png", "jpg", "jpeg"].iter().any(|v| extension.eq_ignore_ascii_case(v)) && page_number == 1 {
        let image = image::load_from_memory(&bytes).map_err(|e| DocumentError::Extraction(e.to_string()))?;
        if u64::from(image.width()) * u64::from(image.height()) > 20_000_000 {
            return Err(DocumentError::ResourceLimit("image exceeds 20 million pixels"));
        }
        let mut output = Cursor::new(Vec::new());
        image.thumbnail(u32::from(max_dimension), u32::from(max_dimension))
            .write_to(&mut output, image::ImageFormat::Png)
            .map_err(|e| DocumentError::Extraction(e.to_string()))?;
        output.into_inner()
    } else {
        return Err(DocumentError::UnsupportedFormat("preview supports PDF and images".into()));
    };
    if png.len() > 8 * 1024 * 1024 { return Err(DocumentError::ResourceLimit("preview exceeds 8 MiB")); }
    Ok(png)
}

pub async fn extract_path_with_data(path: &Path, ocr: OcrMode, data_dir: Option<&Path>) -> Result<Document, DocumentError> {
    let file_mime = mime(path)?;
    let mut file = File::open(path).map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    let before = file.metadata().map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    if before.len() > MAX_INPUT_BYTES { return Err(DocumentError::ResourceLimit("input exceeds 64 MiB")); }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&mut file).take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES { return Err(DocumentError::ResourceLimit("input exceeds 64 MiB")); }
    let after = file.metadata().map_err(|e| DocumentError::PermissionDenied(e.to_string()))?;
    if after.len() != before.len() || after.modified().ok() != before.modified().ok() || bytes.len() as u64 != before.len() {
        return Err(DocumentError::FileChanged);
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    check_zip_budget(&bytes)?;
    let assets_available = data_dir.is_some_and(|dir| {
        dir.join("eng.traineddata").is_file() && dir.join("chi_sim.traineddata").is_file()
    });
    let mut document = Document {
        format: file_mime.to_owned(), byte_size: before.len(), sha256,
        text: String::new(), pages: Vec::new(), tables: Vec::new(), sheets: Vec::new(), warnings: Vec::new(),
    };
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
    if ["png", "jpg", "jpeg"].iter().any(|value| extension.eq_ignore_ascii_case(value)) {
        if matches!(ocr, OcrMode::Off) { return Err(DocumentError::NoText); }
        let Some(dir) = data_dir.filter(|_| assets_available) else { return Err(DocumentError::OcrUnavailable); };
        let (text, confidence) = ocr_image(&bytes, dir)?;
        document.pages.push(Page { number: 1, sheet_name: None, text: text.clone(), speaker_notes: None,
            ocr_confidence: Some(confidence) });
        document.text = text;
        if confidence < 60.0 { document.warnings.push("page 1 OCR confidence is low; verify the source image".into()); }
    } else if extension.eq_ignore_ascii_case("pdf") {
        match lopdf::Document::load_mem(&bytes) {
            Ok(pdf) if pdf.trailer.get(b"Encrypt").is_ok() => return Err(DocumentError::PasswordRequired),
            Err(lopdf::Error::InvalidPassword | lopdf::Error::AlreadyEncrypted) =>
                return Err(DocumentError::PasswordRequired),
            _ => {}
        }
        // AnyDoc is the primary conversion entry point. Its NeedsOcr result is
        // supplemented by pdf-inspector's page locations and local OCR.
        let anydoc_result = anydoc::to_markdown_bytes(&bytes, Some(Format::Pdf));
        if matches!(&anydoc_result, Err(ConvertError::Encrypted)) {
            return Err(DocumentError::PasswordRequired);
        }
        let extracted = pdf_inspector::extract_pages_markdown_mem(&bytes, None)
            .map_err(|e| match e {
                pdf_inspector::PdfError::Encrypted => DocumentError::PasswordRequired,
                other => DocumentError::Extraction(other.to_string()),
            })?;
        if extracted.pages.is_empty() {
            return Err(anydoc_result.err().map(convert_error).unwrap_or(DocumentError::NoText));
        }
        if extracted.pages.len() > MAX_PAGES { return Err(DocumentError::ResourceLimit("too many PDF pages")); }
        for page in extracted.pages {
            let needs_ocr = page.needs_ocr || page.markdown.trim().is_empty();
            let do_ocr = matches!(ocr, OcrMode::On) || (needs_ocr && matches!(ocr, OcrMode::Auto));
            let (text, confidence) = if do_ocr {
                let Some(dir) = data_dir.filter(|_| assets_available) else { return Err(DocumentError::OcrUnavailable); };
                let rendered = render_pdf_page(&bytes, page.page + 1, 1800)?;
                let (text, confidence) = ocr_image(&rendered, dir)?;
                (text, Some(confidence))
            } else if needs_ocr {
                return Err(DocumentError::NoText);
            } else {
                (page.markdown, None)
            };
            if text.trim().is_empty() {
                document.warnings.push(format!("page {} has no recognized text", page.page + 1));
            }
            if confidence.is_some_and(|value| value < 60.0) {
                document.warnings.push(format!("page {} OCR confidence is low; verify the page image", page.page + 1));
            }
            document.pages.push(Page { number: page.page + 1, sheet_name: None, text, speaker_notes: None,
                ocr_confidence: confidence });
        }
        if matches!(ocr, OcrMode::Off) { anydoc_result.map_err(convert_error)?; }
        document.text = document.pages.iter().map(|p| format!("## Page {}\n\n{}", p.number, p.text)).collect::<Vec<_>>().join("\n\n");
    } else if extension.eq_ignore_ascii_case("txt") || extension.eq_ignore_ascii_case("md") {
        document.text = String::from_utf8(bytes).map_err(|e| DocumentError::Extraction(e.to_string()))?;
    } else if extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm") {
        document.text = html2text::from_read(&bytes[..], 120)
            .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    } else {
        let format = Format::from_bytes(&bytes).or_else(|| Format::from_extension(extension))
            .ok_or_else(|| DocumentError::UnsupportedFormat(extension.into()))?;
        document.text = anydoc::to_markdown_bytes(&bytes, Some(format)).map_err(convert_error)?;
        if format != Format::Pdf {
            let model = anydoc::to_document(&bytes, Some(format)).map_err(convert_error)?;
            collect_tables(&model.blocks, &mut document.tables);
        }
        if matches!(format, Format::Excel) {
            document.sheets = spreadsheet(&bytes)?;
            document.text = document.sheets.iter().map(|sheet| {
                let lines = sheet.cells.iter().map(|cell| format!("{}\t{}\t{}\t{}", cell.address,
                    cell.value_type, cell.value, cell.formula.as_deref().unwrap_or("")))
                    .collect::<Vec<_>>().join("\n");
                format!("## Sheet {}\n\n{lines}", sheet.name)
            }).collect::<Vec<_>>().join("\n\n");
        } else if matches!(format, Format::Pptx) {
            document.pages = presentation::slides(&bytes)?;
            document.text = document.pages.iter().map(|p| format!("## Slide {}\n\n{}{}", p.number, p.text,
                p.speaker_notes.as_ref().map(|notes| format!("\n\n> Speaker notes: {notes}")).unwrap_or_default()))
                .collect::<Vec<_>>().join("\n\n");
        }
    }
    if document.text.trim().is_empty() && document.pages.iter().all(|p| p.text.trim().is_empty())
        && document.sheets.iter().all(|sheet| sheet.cells.is_empty()) {
        return Err(DocumentError::NoText);
    }
    let encoded = serde_json::to_vec(&document).map_err(|e| DocumentError::Extraction(e.to_string()))?;
    if encoded.len() > MAX_RESULT_BYTES { return Err(DocumentError::ResourceLimit("extracted result exceeds 64 MiB")); }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../acceptance/fixtures/document").join(name)
    }
    fn tessdata() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tessdata")
    }

    #[test]
    fn excel_addresses_are_absolute_and_one_based() {
        assert_eq!(address(0, 0), "A1");
        assert_eq!(address(11, 25), "Z12");
        assert_eq!(address(1, 26), "AA2");
        assert_eq!(address(0, 702), "AAA1");
    }

    #[tokio::test]
    async fn text_document_has_a_stable_snapshot_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.txt");
        std::fs::write(&path, "Pixels document test: 中文和 English.").unwrap();
        let document = extract_path_with_data(&path, OcrMode::Off, None).await.unwrap();
        assert!(document.text.contains("Pixels document test"));
        assert_eq!(document.sha256.len(), 64);
        assert_eq!(document.byte_size, std::fs::metadata(path).unwrap().len());
    }

    #[tokio::test]
    async fn presentation_preserves_slide_order_and_speaker_notes() {
        let document = extract_path_with_data(&fixture("slides-notes.pptx"), OcrMode::Off, None).await.unwrap();
        assert_eq!(document.pages.len(), 2);
        assert!(document.pages[0].text.contains("风险汇报"));
        assert!(document.pages[0].speaker_notes.as_deref().unwrap_or("").contains("复核第七条"));
        assert!(document.pages[1].text.contains("Item B"));
        assert!(document.text.contains("Slide 2"));
    }

    #[tokio::test]
    async fn protected_and_broken_pdf_fail_with_distinct_errors() {
        let protected = extract_path_with_data(&fixture("encrypted.pdf"), OcrMode::Auto, None).await;
        assert!(matches!(protected, Err(DocumentError::PasswordRequired)), "{protected:?}");
        assert!(matches!(extract_path_with_data(&fixture("malformed.pdf"), OcrMode::Auto, None).await,
            Err(DocumentError::Extraction(_))));
    }

    #[tokio::test]
    async fn html_is_rendered_as_text_without_executing_page_code() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.html");
        std::fs::write(&path, "<h1>风险报告</h1><p>Payment due in 30 days.</p><script>alert('ignore')</script>").unwrap();
        let document = extract_path_with_data(&path, OcrMode::Off, None).await.unwrap();
        assert!(document.text.contains("风险报告"));
        assert!(document.text.contains("Payment due"));
        assert!(!document.text.contains("alert"));
    }

    #[tokio::test]
    async fn size_is_rejected_before_reading_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.pdf");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_INPUT_BYTES + 1).unwrap();
        drop(file);
        assert!(matches!(extract_path_with_data(&path, OcrMode::Off, None).await, Err(DocumentError::ResourceLimit(_))));
    }

    #[tokio::test]
    async fn compressed_office_bomb_is_rejected_before_decompression() {
        assert!(matches!(extract_path_with_data(&fixture("oversized-expanded.docx"), OcrMode::Off, None).await,
            Err(DocumentError::ResourceLimit(_))));
    }

    #[tokio::test]
    async fn pdf_retains_real_page_location() {
        let doc = extract_path_with_data(&fixture("native-table.pdf"), OcrMode::Off, None).await.unwrap();
        assert!(doc.pages.iter().any(|page| page.number == 1 && page.text.contains("Risk Clause 7")), "{doc:?}");
    }

    #[tokio::test]
    async fn workbook_keeps_sheet_cell_types_and_formula() {
        let doc = extract_path_with_data(&fixture("sheets-formula.xlsx"), OcrMode::Off, None).await.unwrap();
        assert_eq!(doc.sheets.len(), 2);
        let revenue = &doc.sheets[0];
        assert_eq!(revenue.name, "Revenue");
        assert!(revenue.cells.iter().any(|cell| cell.address == "B2" && ["integer", "number"].contains(&cell.value_type.as_str()) && cell.value == "42"));
        assert!(revenue.cells.iter().any(|cell| cell.address == "C2" && cell.formula.as_deref() == Some("B2*2")));
        assert!(doc.sheets[1].cells.iter().any(|cell| cell.address == "B3" && cell.value == "逾期"));
    }

    #[tokio::test]
    async fn word_table_preserves_order() {
        let doc = extract_path_with_data(&fixture("heading-table.docx"), OcrMode::Off, None).await.unwrap();
        assert!(doc.text.contains("付款期限为三十天"));
        assert!(doc.tables.iter().any(|table| table.cells.iter().any(|row| row.iter().any(|cell| cell == "金额"))));
    }

    #[tokio::test]
    async fn offline_chinese_english_image_ocr() {
        let doc = extract_path_with_data(&fixture("scan-zh-en.png"), OcrMode::On, Some(&tessdata())).await.unwrap();
        let chinese = doc.text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert!(chinese.contains("中文") && chinese.contains("合同风险条"), "{}", doc.text);
        assert!(doc.text.contains("Invoice"), "{}", doc.text);
    }

    #[tokio::test]
    async fn scanned_pdf_has_ocr_text_on_page_one() {
        let doc = extract_path_with_data(&fixture("scan-zh-en.pdf"), OcrMode::Auto, Some(&tessdata())).await.unwrap();
        assert!(doc.pages.iter().any(|page| page.number == 1 && page.text.contains("Invoice")), "{doc:?}");
    }

    #[tokio::test]
    async fn mixed_pdf_keeps_native_and_ocr_page_boundaries() {
        let doc = extract_path_with_data(&fixture("mixed.pdf"), OcrMode::Auto, Some(&tessdata())).await.unwrap();
        assert!(doc.pages.iter().any(|page| page.number == 1 && page.text.contains("Risk Clause")), "{doc:?}");
        assert!(doc.pages.iter().any(|page| page.number == 2 && page.text.contains("Invoice")), "{doc:?}");
    }
}
