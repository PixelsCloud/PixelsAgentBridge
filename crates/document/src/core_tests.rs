use super::*;
use pab_protocol::{DocumentReply, DocumentSelector, RequestId};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../acceptance/fixtures/document")
        .join(name)
}
fn reply() -> DocumentReply {
    DocumentReply {
        request_id: RequestId::new(),
        kind: "document_read".into(),
        state: "running".into(),
        document_ref: None,
        data: None,
        error: None,
    }
}

#[tokio::test]
async fn portable_parsers_keep_table_cells_page_locations_and_protected_errors() {
    let docx = extract_path_with_data(&fixture("heading-table.docx"), OcrMode::Off, None)
        .await
        .unwrap();
    assert!(!docx.tables.is_empty());
    let xlsx = extract_path_with_data(&fixture("sheets-formula.xlsx"), OcrMode::Off, None)
        .await
        .unwrap();
    assert!(
        xlsx.sheets
            .iter()
            .flat_map(|s| &s.cells)
            .any(|c| c.formula.is_some())
    );
    let pdf = extract_path_with_data(&fixture("native-table.pdf"), OcrMode::Off, None)
        .await
        .unwrap();
    assert!(!pdf.pages.is_empty());
    let chinese = extract_path_with_data(&fixture("native-zh-multipage.pdf"), OcrMode::Off, None)
        .await
        .unwrap();
    assert_eq!(chinese.pages.len(), 2);
    assert!(chinese.pages[1].text.contains("中文风险条款"));
    assert!(matches!(
        extract_path_with_data(&fixture("cid-without-unicode.pdf"), OcrMode::Auto, None).await,
        Err(DocumentError::OcrUnavailable)
    ));
    assert!(matches!(
        extract_path_with_data(&fixture("encrypted.pdf"), OcrMode::Auto, None).await,
        Err(DocumentError::PasswordRequired)
    ));
    assert!(
        extract_path_with_data(&fixture("malformed.pdf"), OcrMode::Off, None)
            .await
            .is_err()
    );
    assert!(matches!(
        extract_path_with_data(&fixture("oversized-expanded.docx"), OcrMode::Off, None).await,
        Err(DocumentError::ResourceLimit(_))
    ));
    assert!(matches!(
        extract_path_with_data(&fixture("scan-zh-en.pdf"), OcrMode::Auto, None).await,
        Err(DocumentError::OcrUnavailable)
    ));
}

#[tokio::test]
async fn escaped_unicode_text_pages_fit_wire_budget_and_reconstruct_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    let text = "\u{1}中文😀 risk RISK\n".repeat(6000);
    std::fs::write(&input, &text).unwrap();
    let document = extract_snapshot(&input, "txt", OcrMode::Off).await.unwrap();
    let id = RequestId::new();
    let mut cursor = None;
    let mut combined = String::new();
    loop {
        let page = query::read_document(
            reply(),
            id,
            &document,
            &DocumentSelector::Chunk { number: 1 },
            cursor.as_deref(),
        );
        assert_eq!(page.state, "completed");
        assert!(serde_json::to_vec(&page).unwrap().len() <= 30 * 1024);
        let data = page.data.unwrap();
        combined.push_str(data["text"].as_str().unwrap());
        cursor = data["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(combined, text);
    let found = query::search_document(reply(), id, &document, "risk", false, 1, None);
    assert_eq!(
        found.data.as_ref().unwrap()["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(found.data.unwrap()["next_cursor"].is_string());
    let bad = query::read_document(
        reply(),
        id,
        &document,
        &DocumentSelector::Chunk { number: 1 },
        Some("2"),
    );
    assert_eq!(bad.error.as_deref(), Some("invalid_cursor"));
}

struct FakeOcr {
    pages: Vec<u32>,
    confidence: f32,
    text: String,
}
impl PageOcr for FakeOcr {
    fn recognize(&mut self, page: u32) -> Result<(String, f32), DocumentError> {
        self.pages.push(page);
        Ok((self.text.clone(), self.confidence))
    }
}

#[tokio::test]
async fn platform_ocr_preserves_native_pages_and_mode_semantics() {
    let mut ocr = FakeOcr {
        pages: Vec::new(),
        confidence: 93.0,
        text: "中文扫描 Invoice 12345".into(),
    };
    let mut extract = async |name: &str, mode| {
        extract_file_with_ocr(
            File::open(fixture(name)).unwrap(),
            name.rsplit('.').next().unwrap(),
            mode,
            &mut ocr,
        )
        .await
    };
    let mixed = extract("mixed.pdf", OcrMode::Auto).await.unwrap();
    assert!(mixed.pages[0].text.contains("Risk Clause"));
    assert_eq!(mixed.pages[0].ocr_confidence, None);
    assert_eq!(mixed.pages[1].number, 2);
    assert_eq!(mixed.pages[1].ocr_confidence, Some(93.0));
    assert!(mixed.pages[1].text.contains("中文扫描"));
    assert!(extract("scan-zh-en.pdf", OcrMode::Off).await.is_err());
    assert!(extract("scan-zh-en.png", OcrMode::Off).await.is_err());
    assert!(matches!(
        extract("encrypted.pdf", OcrMode::Auto).await,
        Err(DocumentError::PasswordRequired)
    ));
    let image = extract("scan-zh-en.png", OcrMode::Auto).await.unwrap();
    assert_eq!(image.pages[0].number, 1);
    let found = query::search_document(reply(), RequestId::new(), &image, "中文", false, 20, None);
    assert_eq!(
        found.data.unwrap()["matches"][0]["location"],
        serde_json::json!({"kind":"page", "number":1})
    );
    let forced = extract("native-table.pdf", OcrMode::On).await.unwrap();
    assert!(forced.pages[0].ocr_confidence.is_some());
    assert_eq!(ocr.pages, [2, 1, 1]);
}

#[tokio::test]
async fn extended_portable_formats_keep_unicode_and_ods_coordinates() {
    for name in [
        "extended.odt",
        "extended.odp",
        "extended.rtf",
        "extended.epub",
        "heading-table.doc",
        "slides-notes.ppt",
    ] {
        let document = extract_path_with_data(&fixture(name), OcrMode::Off, None)
            .await
            .unwrap();
        assert!(document.text.contains("风险"), "{name}: {}", document.text);
    }
    let document = extract_path_with_data(&fixture("extended.ods"), OcrMode::Off, None)
        .await
        .unwrap();
    let sheet = document.sheets.iter().find(|s| s.name == "风险").unwrap();
    assert!(
        sheet
            .cells
            .iter()
            .any(|cell| cell.address == "B1" && cell.value == "42")
    );
    assert!(
        sheet
            .cells
            .iter()
            .any(|cell| cell.address == "C1" && cell.value == "84" && cell.formula.is_some())
    );
    let legacy = extract_path_with_data(&fixture("sheets-formula.xls"), OcrMode::Off, None)
        .await
        .unwrap();
    assert!(legacy.sheets.iter().any(|sheet| {
        sheet.name == "风险"
            && sheet
                .cells
                .iter()
                .any(|cell| cell.address == "B3" && cell.value == "逾期")
    }));
}

#[test]
fn ods_normalization_preserves_paragraph_whitespace_and_refuses_dtd() {
    use std::io::Write;
    let original = std::fs::read(fixture("extended.ods")).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&original)).unwrap();
    let mut content = String::new();
    archive
        .by_name("content.xml")
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    let make = |content: &str| {
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in [
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet".as_slice(),
            ),
            ("META-INF/manifest.xml", b"<manifest/>".as_slice()),
            ("content.xml", content.as_bytes()),
        ] {
            output
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            output.write_all(bytes).unwrap();
        }
        output.finish().unwrap().into_inner()
    };
    let spaced = content.replace("项目", "  项目  ");
    let normalized = ods_cells_input(&make(&spaced)).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(normalized)).unwrap();
    let mut content = String::new();
    archive
        .by_name("content.xml")
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert!(content.contains("<text:p>  项目  </text:p>"));
    assert!(!content.contains("</table:table-cell>\n<table:table-cell"));
    assert!(ods_cells_input(&make("<!DOCTYPE x [<!ENTITY y 'data'>]><x/>")).is_err());
}

#[tokio::test]
async fn platform_ocr_rejects_invalid_confidence_and_unbounded_page_text() {
    let mut ocr = FakeOcr {
        pages: Vec::new(),
        confidence: f32::NAN,
        text: "text".into(),
    };
    assert!(
        extract_file_with_ocr(
            File::open(fixture("scan-zh-en.png")).unwrap(),
            "png",
            OcrMode::Auto,
            &mut ocr
        )
        .await
        .is_err()
    );
    ocr.confidence = 50.0;
    ocr.text = "x".repeat(1024 * 1024 + 1);
    assert!(matches!(
        extract_file_with_ocr(
            File::open(fixture("scan-zh-en.png")).unwrap(),
            "png",
            OcrMode::Auto,
            &mut ocr
        )
        .await,
        Err(DocumentError::ResourceLimit(_))
    ));
    ocr.text = "review".into();
    let low = extract_file_with_ocr(
        File::open(fixture("scan-zh-en.png")).unwrap(),
        "png",
        OcrMode::Auto,
        &mut ocr,
    )
    .await
    .unwrap();
    assert!(low.warnings.iter().any(|v| v.contains("confidence is low")));
}
