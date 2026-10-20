//! PPTX location adapter. AnyDoc supplies the canonical conversion and table
//! model; this small reader restores presentation-order slide and note bounds.
use super::{DocumentError, MAX_PAGES, Page};
use quick_xml::{
    Reader,
    escape::unescape,
    events::{BytesStart, Event},
};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
};
use zip::ZipArchive;

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn attribute(element: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref() == name)
        .and_then(|attr| {
            std::str::from_utf8(attr.value.as_ref())
                .ok()
                .and_then(|value| unescape(value).ok().map(|value| value.into_owned()))
        })
}

fn member(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<Vec<u8>>, DocumentError> {
    let Ok(file) = archive.by_name(name) else {
        return Ok(None);
    };
    if file.size() > 8 * 1024 * 1024 {
        return Err(DocumentError::ResourceLimit(
            "presentation XML part exceeds 8 MiB",
        ));
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| DocumentError::Extraction(e.to_string()))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(DocumentError::ResourceLimit(
            "presentation XML part exceeds 8 MiB",
        ));
    }
    Ok(Some(bytes))
}

fn reader(bytes: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    reader
}

fn relationships(bytes: &[u8], kind: &str) -> Result<HashMap<String, String>, DocumentError> {
    let mut xml = reader(bytes);
    let mut result = HashMap::new();
    loop {
        match xml.read_event() {
            Ok(Event::Start(e) | Event::Empty(e))
                if local(e.name().as_ref()) == b"Relationship" =>
            {
                if attribute(&e, b"Type").is_some_and(|value| value.ends_with(kind)) {
                    if let (Some(id), Some(target)) =
                        (attribute(&e, b"Id"), attribute(&e, b"Target"))
                    {
                        result.insert(id, target);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(DocumentError::Extraction(error.to_string())),
            _ => {}
        }
    }
    Ok(result)
}

fn ordered_ids(bytes: &[u8]) -> Result<Vec<String>, DocumentError> {
    let mut xml = reader(bytes);
    let mut ids = Vec::new();
    loop {
        match xml.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == b"sldId" => {
                if let Some(id) = attribute(&e, b"r:id") {
                    ids.push(id);
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(DocumentError::Extraction(error.to_string())),
            _ => {}
        }
    }
    if ids.is_empty() {
        return Err(DocumentError::Extraction(
            "presentation has no slide order".into(),
        ));
    }
    if ids.len() > MAX_PAGES {
        return Err(DocumentError::ResourceLimit("too many slides"));
    }
    Ok(ids)
}

fn target_name(target: &str, prefix: &str) -> Result<String, DocumentError> {
    let filename = target.rsplit('/').next().unwrap_or("");
    let Some(index) = filename
        .strip_prefix(prefix)
        .and_then(|v| v.strip_suffix(".xml"))
    else {
        return Err(DocumentError::Extraction(
            "invalid presentation relationship target".into(),
        ));
    };
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DocumentError::Extraction(
            "invalid presentation relationship target".into(),
        ));
    }
    Ok(filename.to_owned())
}

fn body_text(bytes: &[u8], notes: bool) -> Result<String, DocumentError> {
    let mut xml = reader(bytes);
    let mut result = String::new();
    let mut in_text = false;
    let mut skip_shape = false;
    loop {
        match xml.read_event() {
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"sp" if notes => skip_shape = false,
                b"ph" if notes => {
                    skip_shape = attribute(&e, b"type").is_some_and(|value| {
                        ["sldImg", "sldNum", "hdr", "ftr", "dt"].contains(&value.as_str())
                    });
                }
                b"t" => in_text = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if notes && local(e.name().as_ref()) == b"ph" => {
                skip_shape = attribute(&e, b"type").is_some_and(|value| {
                    ["sldImg", "sldNum", "hdr", "ftr", "dt"].contains(&value.as_str())
                });
            }
            Ok(Event::Text(e)) if in_text && !skip_shape => {
                let decoded = e
                    .decode()
                    .map_err(|error| DocumentError::Extraction(error.to_string()))?;
                let text = unescape(&decoded)
                    .map_err(|error| DocumentError::Extraction(error.to_string()))?;
                if !result.ends_with(['\n', ' ']) && !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(&text);
            }
            Ok(Event::End(e)) => match local(e.name().as_ref()) {
                b"t" => in_text = false,
                b"p" if !result.ends_with('\n') && !result.is_empty() => result.push('\n'),
                b"sp" if notes => skip_shape = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => return Err(DocumentError::Extraction(error.to_string())),
            _ => {}
        }
    }
    Ok(result.trim().to_owned())
}

pub(super) fn slides(bytes: &[u8]) -> Result<Vec<Page>, DocumentError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| DocumentError::Extraction(error.to_string()))?;
    let presentation = member(&mut archive, "ppt/presentation.xml")?
        .ok_or_else(|| DocumentError::Extraction("missing presentation.xml".into()))?;
    let rels = member(&mut archive, "ppt/_rels/presentation.xml.rels")?
        .ok_or_else(|| DocumentError::Extraction("missing presentation relationships".into()))?;
    let ids = ordered_ids(&presentation)?;
    let slide_targets = relationships(&rels, "/slide")?;
    let mut pages = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        let filename = target_name(
            slide_targets
                .get(id)
                .ok_or_else(|| DocumentError::Extraction("missing slide relationship".into()))?,
            "slide",
        )?;
        let slide = member(&mut archive, &format!("ppt/slides/{filename}"))?
            .ok_or_else(|| DocumentError::Extraction("missing slide XML".into()))?;
        let text = body_text(&slide, false)?;
        let slide_rels = member(&mut archive, &format!("ppt/slides/_rels/{filename}.rels"))?;
        let notes = if let Some(slide_rels) = slide_rels {
            let notes_target = relationships(&slide_rels, "/notesSlide")?
                .into_values()
                .next();
            if let Some(target) = notes_target {
                let filename = target_name(&target, "notesSlide")?;
                member(&mut archive, &format!("ppt/notesSlides/{filename}"))?
                    .map(|bytes| body_text(&bytes, true))
                    .transpose()?
            } else {
                None
            }
        } else {
            None
        };
        pages.push(Page {
            number: (index + 1) as u32,
            sheet_name: None,
            text,
            speaker_notes: notes.filter(|text| !text.is_empty()),
            ocr_confidence: None,
        });
    }
    Ok(pages)
}
