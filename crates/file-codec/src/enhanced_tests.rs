use super::*;
use pab_protocol::*;
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn streaming_decoding_keeps_partial_characters_and_rejects_invalid_interior_bytes() {
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let original = "甲😀乙";
        let bytes = encode(original, encoding, false).unwrap();
        let mut actual = String::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let end = (offset + 4).min(bytes.len());
            let (text, consumed, _) = log::decode(&bytes[offset..end], encoding).unwrap();
            assert!(consumed > 0);
            actual.push_str(&text);
            offset += consumed;
        }
        assert_eq!(actual, original);
        let (_, consumed, incomplete) = log::decode(&bytes[..bytes.len() - 1], encoding).unwrap();
        assert!(incomplete);
        assert!(consumed < bytes.len() - 1);
    }
    assert_eq!(
        log::decode(&[b'a', 0xff, b'b'], TextEncoding::Utf8)
            .unwrap_err()
            .0
            .code,
        "invalid_encoding"
    );
    assert_eq!(log::tail_skip(&[0x80, 0x81, b'a'], TextEncoding::Utf8), 2);
}

#[test]
fn patch_preview_and_applied_bytes_share_the_same_encoding_and_match_checks() {
    let original = encode("甲\r\nold😀\r\n", TextEncoding::Utf16Le, true).unwrap();
    let edits = vec![TextEdit {
        find: "old".into(),
        replace: "新".into(),
        expected_matches: 1,
    }];
    let plan = plan_patch(&original, None, &edits).unwrap();
    assert_eq!(plan.encoding, TextEncoding::Utf16Le);
    assert_eq!(decode(&plan.bytes, None).unwrap().text, "甲\r\n新😀\r\n");
    assert!(plan.preview.changed);
    assert_eq!(plan.preview.result_size, plan.bytes.len() as u64);
    let wrong = vec![TextEdit {
        expected_matches: 2,
        ..edits[0].clone()
    }];
    assert!(plan_patch(&original, None, &wrong).is_err());
    assert_eq!(decode(&original, None).unwrap().text, "甲\r\nold😀\r\n");
}

struct Source(BTreeMap<String, Vec<u8>>);
impl search::SearchSource for Source {
    async fn load(&self, path: &str, _: usize) -> Result<Vec<u8>, FileError> {
        Ok(self.0[path].clone())
    }
}
fn entries() -> Vec<search::Entry> {
    vec![search::Entry {
        path: "/root/a.txt".into(),
        name: "a.txt".into(),
        relative: "a.txt".into(),
        size: 64,
        directory: false,
        regular: true,
        fingerprint: "stable-metadata".into(),
    }]
}
fn operation() -> FileSystemAction {
    FileSystemAction::Search {
        mode: FileSearchMode::Content,
        query: "错误[12]".into(),
        glob: "**".into(),
        case_sensitive: true,
        max_results: 1,
        max_depth: 4,
        max_file_bytes: 1024,
        options: FileSearchOptions {
            regex: true,
            context_lines: 1,
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn shared_search_pages_exact_lines_and_refuses_changed_query_tree_or_resume_file() {
    let mut source = Source(BTreeMap::from([(
        "/root/a.txt".into(),
        "开头\n错误1\n中间\n错误2\n结尾".as_bytes().to_vec(),
    )]));
    let mut op = operation();
    let patterns = search::Patterns::new(&op).unwrap();
    let first = search::search(
        &source,
        "/root",
        &op,
        FileSearchSummary::default(),
        entries(),
        tokio::time::Instant::now() + Duration::from_secs(5),
        &patterns,
    )
    .await
    .unwrap();
    assert_eq!(first.matches[0].line, Some(2));
    assert_eq!(first.matches[0].context.len(), 3);
    let cursor = first.next_cursor.unwrap();
    if let FileSystemAction::Search { options, .. } = &mut op {
        options.cursor = Some(cursor.clone());
    }
    let second = search::search(
        &source,
        "/root",
        &op,
        FileSearchSummary::default(),
        entries(),
        tokio::time::Instant::now() + Duration::from_secs(5),
        &patterns,
    )
    .await
    .unwrap();
    assert_eq!(second.matches[0].line, Some(4));
    source.0.insert(
        "/root/a.txt".into(),
        "改动\n错误1\n中间\n错误2\n结尾".as_bytes().to_vec(),
    );
    let error = search::search(
        &source,
        "/root",
        &op,
        FileSearchSummary::default(),
        entries(),
        tokio::time::Instant::now() + Duration::from_secs(5),
        &patterns,
    )
    .await
    .unwrap_err();
    assert_eq!(error.0.code, "search_changed");
    if let FileSystemAction::Search { query, .. } = &mut op {
        query.push('x');
    }
    let error = search::search(
        &source,
        "/root",
        &op,
        FileSearchSummary::default(),
        entries(),
        tokio::time::Instant::now() + Duration::from_secs(5),
        &patterns,
    )
    .await
    .unwrap_err();
    assert_eq!(error.0.code, "invalid_cursor");
}
