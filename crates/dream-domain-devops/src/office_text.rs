//! Bounded retrieval text extraction after an intranet editor saves an original.
use crate::DevopsError;
use quick_xml::events::Event;
use std::io::{BufReader, Cursor, Read, Seek};

const TEXT_LIMIT: usize = 16 * 1024 * 1024;
/// One slide / document part, read into memory as a string.
const ZIP_LIMIT: u64 = 128 * 1024 * 1024;
/// What an archive may declare in total. Media and styles are never inflated, so this only has to refuse an
/// archive that claims an absurd size.
const ARCHIVE_LIMIT: u64 = 1024 * 1024 * 1024;
/// How much of one worksheet is read. Sheets are streamed, never held, so this is a time bound: Excel files
/// whose used range was dragged to the bottom of the sheet carry hundreds of megabytes of empty styled cells.
const SHEET_STREAM_LIMIT: u64 = 1024 * 1024 * 1024;

fn invalid() -> DevopsError {
    DevopsError::BadRequest("saved Office file could not be indexed".into())
}

fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| {
            let raw = String::from_utf8_lossy(&attribute.value).into_owned();
            quick_xml::escape::unescape(&raw)
                .map(|text| text.into_owned())
                .unwrap_or(raw)
        })
}

/// Text of an XML text or entity event, when it is inside an element whose text we want.
fn push_text_event(event: &Event<'_>, out: &mut String) -> Result<(), DevopsError> {
    match event {
        Event::Text(value) => out.push_str(&value.decode().map_err(|_| invalid())?),
        Event::GeneralRef(value) => {
            let entity = format!("&{};", value.decode().map_err(|_| invalid())?);
            out.push_str(&quick_xml::escape::unescape(&entity).map_err(|_| invalid())?);
        }
        _ => {}
    }
    Ok(())
}

fn xml_reader<'a, R: Read + Seek>(
    archive: &'a mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<quick_xml::Reader<BufReader<std::io::Take<zip::read::ZipFile<'a, R>>>>, DevopsError> {
    let entry = archive.by_name(name).map_err(|_| invalid())?;
    Ok(quick_xml::Reader::from_reader(BufReader::new(entry.take(limit))))
}

/// The shared string table: `<si>` items, each the text of its `<t>` runs (phonetic guides excluded).
fn shared_strings<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[String],
) -> Result<Vec<String>, DevopsError> {
    if !names.iter().any(|name| name == "xl/sharedStrings.xml") {
        return Ok(Vec::new());
    }
    let mut reader = xml_reader(archive, "xl/sharedStrings.xml", ZIP_LIMIT)?;
    let (mut strings, mut buf) = (Vec::new(), Vec::new());
    let (mut current, mut in_t, mut skip_depth, mut total) = (String::new(), false, 0u32, 0usize);
    loop {
        let event = reader.read_event_into(&mut buf).map_err(|_| invalid())?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"rPh" => skip_depth += 1,
            Event::End(e) if e.local_name().as_ref() == b"rPh" => skip_depth = skip_depth.saturating_sub(1),
            Event::Start(e) if e.local_name().as_ref() == b"si" => current.clear(),
            Event::Start(e) if e.local_name().as_ref() == b"t" => in_t = true,
            Event::End(e) if e.local_name().as_ref() == b"t" => in_t = false,
            Event::End(e) if e.local_name().as_ref() == b"si" => {
                total += current.len();
                if total > TEXT_LIMIT {
                    return Err(invalid());
                }
                strings.push(std::mem::take(&mut current));
            }
            Event::Eof => break,
            other if in_t && skip_depth == 0 => push_text_event(other, &mut current)?,
            _ => {}
        }
        buf.clear();
    }
    Ok(strings)
}

/// Sheet names with the archive path of each, in workbook order.
fn sheet_parts<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, names: &[String]) -> Vec<(String, String)> {
    let mut targets = std::collections::HashMap::new();
    if names.iter().any(|name| name == "xl/_rels/workbook.xml.rels")
        && let Ok(mut reader) = xml_reader(archive, "xl/_rels/workbook.xml.rels", ZIP_LIMIT)
    {
        let mut buf = Vec::new();
        while let Ok(event) = reader.read_event_into(&mut buf) {
            match &event {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                    if let (Some(id), Some(target)) = (attribute(e, b"Id"), attribute(e, b"Target")) {
                        let path = match target.strip_prefix('/') {
                            Some(absolute) => absolute.to_owned(),
                            None => format!("xl/{target}"),
                        };
                        targets.insert(id, path);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let mut sheets = Vec::new();
    if names.iter().any(|name| name == "xl/workbook.xml")
        && let Ok(mut reader) = xml_reader(archive, "xl/workbook.xml", ZIP_LIMIT)
    {
        let mut buf = Vec::new();
        while let Ok(event) = reader.read_event_into(&mut buf) {
            match &event {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"sheet" => {
                    if let (Some(name), Some(id)) = (attribute(e, b"name"), attribute(e, b"id"))
                        && let Some(path) = targets.get(&id)
                    {
                        sheets.push((name, path.clone()));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    if sheets.is_empty() {
        // No readable workbook map: fall back to the sheet files in numeric order.
        let mut files: Vec<(u32, String)> = names
            .iter()
            .filter_map(|name| {
                let index = name
                    .strip_prefix("xl/worksheets/sheet")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((index, name.clone()))
            })
            .collect();
        files.sort();
        sheets = files
            .into_iter()
            .map(|(index, name)| (format!("Sheet{index}"), name))
            .collect();
    }
    sheets
}

/// Spreadsheet cells as tab-separated text, one line per row that has any value.
///
/// Worksheets are streamed and only cells that hold a value are kept. A dense `range` (what the workbook library
/// builds) allocates one slot for every cell between A1 and the last used one, which for a sheet whose used range
/// reaches row 1,000,000 is gigabytes for a few hundred real values; this reads the same file in constant memory.
fn extract_xlsx<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[String],
    text: &mut String,
) -> Result<(), DevopsError> {
    let strings = shared_strings(archive, names)?;
    for (sheet_name, part) in sheet_parts(archive, names) {
        if !names.contains(&part) {
            continue;
        }
        text.push_str(&sheet_name);
        text.push('\n');
        let mut reader = xml_reader(archive, &part, SHEET_STREAM_LIMIT)?;
        let mut buf = Vec::new();
        let (mut kind, mut value) = (String::new(), String::new());
        let (mut in_cell, mut in_value, mut in_inline, mut row_has_value) = (false, false, false, false);
        loop {
            let event = reader.read_event_into(&mut buf).map_err(|_| invalid())?;
            match &event {
                Event::Start(e) if e.local_name().as_ref() == b"c" => {
                    in_cell = true;
                    kind = attribute(e, b"t").unwrap_or_default();
                    value.clear();
                }
                Event::Start(e) if in_cell && e.local_name().as_ref() == b"v" => in_value = true,
                Event::Start(e) if in_cell && e.local_name().as_ref() == b"is" => in_inline = true,
                Event::End(e) if e.local_name().as_ref() == b"v" => in_value = false,
                Event::End(e) if e.local_name().as_ref() == b"is" => in_inline = false,
                Event::End(e) if e.local_name().as_ref() == b"c" => {
                    in_cell = false;
                    let shown = match kind.as_str() {
                        "s" => value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| strings.get(index))
                            .cloned(),
                        "b" => Some(if value.trim() == "1" { "TRUE" } else { "FALSE" }.to_owned()),
                        _ => Some(value.trim().to_owned()),
                    };
                    if let Some(shown) = shown.filter(|shown| !shown.is_empty()) {
                        text.push_str(&shown);
                        text.push('\t');
                        row_has_value = true;
                        if text.len() > TEXT_LIMIT {
                            return Err(invalid());
                        }
                    }
                }
                Event::End(e) if e.local_name().as_ref() == b"row" => {
                    if row_has_value {
                        text.push('\n');
                    }
                    row_has_value = false;
                }
                Event::Eof => break,
                other if in_cell && (in_value || in_inline) => push_text_event(other, &mut value)?,
                _ => {}
            }
            buf.clear();
        }
    }
    Ok(())
}

pub(crate) fn extract(bytes: &[u8], extension: &str) -> Result<String, DevopsError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        expanded = expanded.saturating_add(archive.by_index(index).map_err(|_| invalid())?.size());
        if expanded > ARCHIVE_LIMIT {
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
        extract_xlsx(&mut archive, &entry_names, &mut text)?;
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
    use std::io::Write;

    fn xlsx(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for (name, body) in parts {
                zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    const WORKBOOK: &str = r#"<workbook xmlns:r="r"><sheets><sheet name="采购" sheetId="1" r:id="rId1"/><sheet name="Second &amp; last" sheetId="2" r:id="rId2"/></sheets></workbook>"#;
    const RELS: &str = r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/></Relationships>"#;

    /// A sheet that claims a used range down to the last row of the file but holds a handful of values, which is
    /// what a 200 MB, million-row export with a hundred real rows looks like.
    #[test]
    fn a_sheet_whose_used_range_is_huge_but_sparse_is_read_by_its_values_only() {
        let bytes = xlsx(&[
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", RELS),
            (
                "xl/sharedStrings.xml",
                "<sst><si><t>部门</t></si><si><r><t>办公</t></r><r><t>费</t></r><rPh><t>ignored</t></rPh></si><si><t>A &amp; B</t></si></sst>",
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><dimension ref="A1:R1039651"/><sheetData>
                <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1"/></row>
                <row r="2"><c r="A2" s="4"/><c r="B2" s="4"/></row>
                <row r="3"><c r="A3"><v>15120</v></c><c r="B3" t="inlineStr"><is><t>inline text</t></is></c><c r="C3" t="b"><v>1</v></c><c r="D3" t="s"><v>2</v></c></row>
                <row r="1039651"><c r="R1039651" t="str"><v>last row</v></c></row>
                </sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="e"><v>#N/A</v></c></row></sheetData></worksheet>"#,
            ),
        ]);

        let text = extract(&bytes, "xlsx").unwrap();

        assert_eq!(
            text,
            "采购\n部门\t办公费\t\n15120\tinline text\tTRUE\tA & B\t\nlast row\t\nSecond & last\n#N/A\t\n"
        );
    }

    #[test]
    fn a_workbook_without_a_sheet_map_is_still_read_in_file_order() {
        let bytes = xlsx(&[(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet><sheetData><row><c r="A1"><v>1</v></c><c r="B1"><v>2</v></c></row></sheetData></worksheet>"#,
        )]);
        assert_eq!(extract(&bytes, "xlsx").unwrap(), "Sheet1\n1\t2\t\n");
    }

    #[test]
    fn too_much_text_is_refused_not_truncated() {
        let cell = format!("<c t=\"inlineStr\"><is><t>{}</t></is></c>", "x".repeat(1 << 20));
        let rows = cell.repeat(18);
        let bytes = xlsx(&[(
            "xl/worksheets/sheet1.xml",
            &format!("<worksheet><sheetData><row>{rows}</row></sheetData></worksheet>"),
        )]);
        assert!(extract(&bytes, "xlsx").is_err());
    }

    /// Real files, when their paths are supplied (`;`-separated). Not run by default.
    #[test]
    #[ignore]
    fn real_office_files() {
        for path in std::env::var("DREAM_TEST_OFFICE_FILES")
            .expect("set DREAM_TEST_OFFICE_FILES")
            .split(';')
        {
            let bytes = std::fs::read(path).unwrap();
            let extension = path.rsplit('.').next().unwrap();
            let started = std::time::Instant::now();
            match extract(&bytes, extension) {
                Ok(text) => println!(
                    "OK  {} | {} MiB -> {} chars in {} ms | {}",
                    path.rsplit(['/', '\\']).next().unwrap(),
                    bytes.len() >> 20,
                    text.chars().count(),
                    started.elapsed().as_millis(),
                    text.chars().take(50).collect::<String>().replace('\n', " ")
                ),
                Err(error) => println!("ERR {path}: {error}"),
            }
        }
    }

    #[test]
    fn something_that_is_not_an_archive_is_refused() {
        assert!(extract(b"not a zip", "xlsx").is_err());
        assert!(extract(b"not a zip", "pptx").is_err());
    }
}
