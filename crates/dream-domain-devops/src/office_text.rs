//! Bounded retrieval text extraction after an intranet editor saves an original.
use crate::DevopsError;
use calamine::Reader;
use quick_xml::events::Event;
use std::io::{Cursor, Read};

const TEXT_LIMIT: usize = 16 * 1024 * 1024;
const ZIP_LIMIT: u64 = 128 * 1024 * 1024;

fn check_sheet_bounds(xml: &str) -> Result<(), DevopsError> {
    let invalid = || DevopsError::BadRequest("spreadsheet range exceeds retrieval extraction limits".into());
    let mut reader = quick_xml::Reader::from_str(xml);
    let (mut max_row, mut max_column) = (0u64, 0u64);
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(element) | Event::Empty(element)
                if [b"c".as_slice(), b"dimension".as_slice()].contains(&element.local_name().as_ref()) =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| invalid())?;
                    if ![b"r".as_slice(), b"ref".as_slice()].contains(&attribute.key.local_name().as_ref()) {
                        continue;
                    }
                    for address in std::str::from_utf8(&attribute.value).map_err(|_| invalid())?.split(':') {
                        let mut column = 0u64;
                        let mut row = String::new();
                        for value in address.bytes().filter(|value| *value != b'$') {
                            if value.is_ascii_alphabetic() {
                                column = column
                                    .saturating_mul(26)
                                    .saturating_add((value.to_ascii_uppercase() - b'A' + 1) as u64);
                            } else if value.is_ascii_digit() {
                                row.push(value as char);
                            } else {
                                return Err(invalid());
                            }
                        }
                        max_column = max_column.max(column);
                        max_row = max_row.max(row.parse::<u64>().map_err(|_| invalid())?);
                        if max_column.saturating_mul(max_row) > 1_000_000 {
                            return Err(invalid());
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn extract(bytes: &[u8], extension: &str) -> Result<String, DevopsError> {
    let invalid = || DevopsError::BadRequest("saved Office file could not be indexed".into());
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        expanded = expanded.saturating_add(archive.by_index(index).map_err(|_| invalid())?.size());
        if expanded > ZIP_LIMIT {
            return Err(invalid());
        }
    }
    let entry_names: Vec<String> = archive
        .file_names()
        .map(|name| name.map(|name| name.into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|_| invalid())?;
    let mut text = String::new();
    if extension == "xlsx" {
        // Sparse cells can describe an enormous rectangular allocation even
        // when the compressed archive and XML are small.
        let sheets: Vec<String> = entry_names
            .iter()
            .filter(|name| name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
            .cloned()
            .collect();
        for name in sheets {
            let mut xml = String::new();
            archive
                .by_name(&name)
                .map_err(|_| invalid())?
                .take(ZIP_LIMIT + 1)
                .read_to_string(&mut xml)
                .map_err(|_| invalid())?;
            check_sheet_bounds(&xml)?;
        }
        let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|_| invalid())?;
        for name in workbook.sheet_names() {
            text.push_str(&name);
            text.push('\n');
            let range = workbook.worksheet_range(&name).map_err(|_| invalid())?;
            for row in range.rows() {
                for cell in row {
                    text.push_str(&cell.to_string());
                    text.push('\t');
                    if text.len() > TEXT_LIMIT {
                        return Err(invalid());
                    }
                }
                text.push('\n');
            }
        }
    } else {
        let mut names: Vec<String> = entry_names
            .iter()
            .filter(|name| {
                (extension == "docx" && name.as_str() == "word/document.xml")
                    || (extension == "pptx" && name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            })
            .cloned()
            .collect();
        names.sort_by_key(|name| {
            name.strip_prefix("ppt/slides/slide")
                .and_then(|name| name.strip_suffix(".xml"))
                .and_then(|index| index.parse::<u32>().ok())
                .unwrap_or(0)
        });
        if names.is_empty() {
            return Err(invalid());
        }
        for name in names {
            let mut xml = String::new();
            archive
                .by_name(&name)
                .map_err(|_| invalid())?
                .take(ZIP_LIMIT + 1)
                .read_to_string(&mut xml)
                .map_err(|_| invalid())?;
            let mut reader = quick_xml::Reader::from_str(&xml);
            let mut in_text = false;
            loop {
                match reader.read_event().map_err(|_| invalid())? {
                    Event::Start(element) if element.local_name().as_ref() == b"t" => in_text = true,
                    Event::End(element) => {
                        if element.local_name().as_ref() == b"t" {
                            in_text = false;
                        }
                        if element.local_name().as_ref() == b"p" {
                            text.push('\n');
                        }
                    }
                    Event::Text(value) if in_text => text.push_str(&value.decode().map_err(|_| invalid())?),
                    Event::GeneralRef(value) if in_text => {
                        let entity = format!("&{};", value.decode().map_err(|_| invalid())?);
                        text.push_str(&quick_xml::escape::unescape(&entity).map_err(|_| invalid())?);
                    }
                    Event::Eof => break,
                    _ => {}
                }
                if text.len() > TEXT_LIMIT {
                    return Err(invalid());
                }
            }
            text.push('\n');
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_sparse_sheet_ranges_before_allocating_the_workbook() {
        assert!(
            check_sheet_bounds(
                "<worksheet><dimension ref='A1:D20'/><sheetData><row><c r='D20'/></row></sheetData></worksheet>"
            )
            .is_ok()
        );
        assert!(check_sheet_bounds("<worksheet><dimension ref='A1:XFD1048576'/></worksheet>").is_err());
        assert!(
            check_sheet_bounds("<worksheet><sheetData><row><c r='XFD1048576'/></row></sheetData></worksheet>").is_err()
        );
    }
}
