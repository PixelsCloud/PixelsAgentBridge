//! Shared document projections and bounded text search, independent of worker/runtime.
use crate::Document;
use pab_protocol::{DocumentReply, DocumentSelector, RequestId};
use serde_json::{Value, json};
const MAX_REPLY_TEXT: usize = 14 * 1024;

pub fn read_document(
    mut reply: DocumentReply,
    document_ref: RequestId,
    document: &Document,
    selector: &DocumentSelector,
    cursor: Option<&str>,
) -> DocumentReply {
    let (body, location) = match select(&document, &selector) {
        Ok(value) => value,
        Err(message) => return failed(reply, message),
    };
    let offset = match parse_cursor(cursor, &body) {
        Ok(offset) => offset,
        Err(message) => return failed(reply, message),
    };
    let mut end = (offset + MAX_REPLY_TEXT).min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    reply.state = "completed".into();
    reply.document_ref = Some(document_ref);
    loop {
        reply.data = Some(json!({"text": &body[offset..end], "location": location,
                        "next_cursor": if end < body.len() { Some(end.to_string()) } else { None },
                        "truncated": end < body.len()}));
        if serde_json::to_vec(&reply).is_ok_and(|b| b.len() <= 30 * 1024) {
            break;
        }
        end = offset + (end - offset) / 2;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
    }
    reply
}

pub fn search_document(
    mut reply: DocumentReply,
    document_ref: RequestId,
    document: &Document,
    query: &str,
    case_sensitive: bool,
    limit: u16,
    cursor: Option<&str>,
) -> DocumentReply {
    let offset = match parse_cursor(cursor, &document.text) {
        Ok(offset) => offset,
        Err(message) => return failed(reply, message),
    };
    let matcher = match regex::RegexBuilder::new(&regex::escape(&query))
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(value) => value,
        Err(_) => return failed(reply, "invalid_search_query"),
    };
    let mut matches = Vec::new();
    let mut position = offset;
    let mut budget_limited = false;
    while let Some(found) = matcher.find_at(&document.text, position) {
        position = found.start();
        let mut start = position.saturating_sub(64);
        let mut end = (found.end() + 64).min(document.text.len());
        while !document.text.is_char_boundary(start) {
            start += 1;
        }
        while !document.text.is_char_boundary(end) {
            end -= 1;
        }
        matches.push(
            json!({"offset": position, "preview": &document.text[start..end],
                        "location": location_at_offset(document, position)}),
        );
        if serde_json::to_vec(&matches).is_ok_and(|b| b.len() > 28 * 1024) {
            matches.pop();
            budget_limited = true;
            break;
        }
        position = found.end();
        if matches.len() == limit as usize || position >= document.text.len() {
            break;
        }
    }
    reply.state = "completed".into();
    reply.document_ref = Some(document_ref);
    let has_more = budget_limited
        || (matches.len() == limit as usize
            && position < document.text.len()
            && matcher.find_at(&document.text, position).is_some());
    reply.data = Some(json!({"matches": matches,
                    "next_cursor": if has_more { Some(position.to_string()) } else { None }}));
    reply
}

fn page_at_offset(document: &Document, offset: usize) -> Option<u32> {
    // Image OCR has no generated page heading in document.text.
    if document.format.starts_with("image/") && offset < document.text.len() {
        return document.pages.first().map(|page| page.number);
    }
    if document.pages.len() == 1
        && !document.format.eq("application/pdf")
        && !document
            .format
            .eq("application/vnd.openxmlformats-officedocument.presentationml.presentation")
    {
        return Some(1);
    }
    let mut begin = 0usize;
    let presentation = document.format
        == "application/vnd.openxmlformats-officedocument.presentationml.presentation";
    for page in &document.pages {
        let header = format!(
            "## {} {}\n\n",
            if presentation { "Slide" } else { "Page" },
            page.number
        );
        let notes_len = if presentation {
            page.speaker_notes
                .as_ref()
                .map_or(0, |notes| "\n\n> Speaker notes: ".len() + notes.len())
        } else {
            0
        };
        let end = begin + header.len() + page.text.len() + notes_len;
        if (begin + header.len()..end).contains(&offset) {
            return Some(page.number);
        }
        begin = end + 2;
    }
    None
}

fn location_at_offset(document: &Document, offset: usize) -> Value {
    if let Some(number) = page_at_offset(document, offset) {
        return json!({"kind": if document.format.contains("presentationml") { "slide" } else { "page" },
            "number":number});
    }
    let mut begin = 0usize;
    for sheet in &document.sheets {
        let header = format!("## Sheet {}\n\n", sheet.name);
        let header_end = begin + header.len();
        if (begin..header_end).contains(&offset) {
            return json!({"kind":"sheet","name":sheet.name});
        }
        begin = header_end;
        for (index, cell) in sheet.cells.iter().enumerate() {
            if index > 0 {
                begin += 1;
            }
            let line_len = cell.address.len()
                + cell.value_type.len()
                + cell.value.len()
                + cell.formula.as_ref().map_or(0, String::len)
                + 3;
            if (begin..begin + line_len).contains(&offset) {
                return json!({"kind":"cell","sheet":sheet.name,"address":cell.address});
            }
            begin += line_len;
        }
        begin += 2;
    }
    json!({"kind":"unknown"})
}

fn failed(mut reply: DocumentReply, message: &str) -> DocumentReply {
    reply.state = "failed".into();
    reply.error = Some(message.to_owned());
    reply
}

pub fn overview(document: &Document) -> Value {
    json!({"format": document.format, "byte_size": document.byte_size,
        "sha256": document.sha256, "pages": if document.pages.is_empty() { None } else { Some(document.pages.len()) },
        "tables": document.tables.len(),
        "sheets": document.sheets.iter().map(|sheet| sheet.name.clone()).collect::<Vec<_>>(),
        "ocr_pages": document.pages.iter().filter_map(|page| page.ocr_confidence.map(|confidence| json!({"number": page.number, "confidence": confidence}))).collect::<Vec<_>>(),
        "warnings": document.warnings})
}

pub fn document_cache_size(document: &Document) -> usize {
    let pages = document
        .pages
        .iter()
        .map(|page| page.text.len())
        .sum::<usize>();
    let tables = document
        .tables
        .iter()
        .flat_map(|table| &table.cells)
        .flat_map(|row| row.iter())
        .map(String::len)
        .sum::<usize>();
    let cells = document
        .sheets
        .iter()
        .flat_map(|sheet| &sheet.cells)
        .map(|cell| cell.value.len() + cell.formula.as_ref().map_or(0, String::len) + 128)
        .sum::<usize>();
    document
        .text
        .len()
        .saturating_add(pages)
        .saturating_add(tables)
        .saturating_add(cells)
}

fn select(
    document: &Document,
    selector: &DocumentSelector,
) -> Result<(String, Value), &'static str> {
    match selector {
        DocumentSelector::Overview => Ok((serde_json::to_string(&overview(document)).unwrap_or_default(), json!({"kind":"overview"}))),
        DocumentSelector::Page { number } => document.pages.iter().find(|page| page.number == *number)
            .map(|page| {
                let text = page.speaker_notes.as_ref().map_or_else(|| page.text.clone(),
                    |notes| format!("{}\n\n> Speaker notes: {notes}", page.text));
                (text, json!({"kind":if document.format.contains("presentationml") {"slide"} else {"page"},
                    "number":number,"ocr_confidence":page.ocr_confidence}))
            })
            .ok_or("page_not_found"),
        DocumentSelector::Sheet { name } => document.sheets.iter().find(|sheet| sheet.name == *name)
            .map(|sheet| (sheet.cells.iter().map(|cell| format!("{}\t{}\t{}\t{}", cell.address, cell.value_type,
                cell.value, cell.formula.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n"),
                json!({"kind":"sheet","name":name}))).ok_or("sheet_not_found"),
        DocumentSelector::Table { number } => document.tables.get((*number as usize).saturating_sub(1))
            .map(|table| (table.cells.iter().map(|row| row.join("\t")).collect::<Vec<_>>().join("\n"),
                json!({"kind":"table","number":number,"page":table.page_number}))).ok_or("table_not_found"),
        DocumentSelector::Chunk { number } if *number == 1 => Ok((document.text.clone(), json!({"kind":"document"}))),
        DocumentSelector::Chunk { .. } => Err("chunk_not_found"),
    }
}

fn parse_cursor(cursor: Option<&str>, body: &str) -> Result<usize, &'static str> {
    let offset = cursor
        .map(str::parse)
        .transpose()
        .map_err(|_| "invalid_cursor")?
        .unwrap_or(0);
    if offset > body.len() || !body.is_char_boundary(offset) {
        Err("invalid_cursor")
    } else {
        Ok(offset)
    }
}
