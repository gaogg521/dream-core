//! Retrieval text out of a PDF, for the knowledge-base import.
//!
//! Only the text layer is read. A scanned PDF has none, and says so instead of
//! indexing nothing under a "ready" label. The parser is third-party code fed
//! with files from a share the administrator pointed us at, so it runs inside
//! `catch_unwind` and its output is bounded by the caller.

use crate::DevopsError;

/// Fewer visible characters than this is a page of images, not a document.
const MIN_TEXT_CHARS: usize = 20;

/// Pages read per document; more than this is a book, not a note.
const MAX_PAGES: usize = 3000;

pub(crate) fn extract(bytes: &[u8]) -> Result<String, DevopsError> {
    if !bytes.starts_with(b"%PDF") {
        return Err(DevopsError::BadRequest("not a PDF file".into()));
    }
    let unreadable = || DevopsError::BadRequest("the PDF could not be read".into());
    let mut doc = pdf_extract::Document::load_mem(bytes).map_err(|_| unreadable())?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Err(DevopsError::BadRequest("the PDF is password-protected".into()));
    }
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    if pages.len() > MAX_PAGES {
        return Err(DevopsError::BadRequest(format!(
            "the PDF has more than {MAX_PAGES} pages; split it"
        )));
    }

    // One page at a time, each inside its own `catch_unwind`: the parser panics
    // on some font encodings, and one such page must not cost the whole document.
    let mut raw = String::new();
    let mut readable = 0usize;
    for page in &pages {
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut text = String::new();
            let result = {
                let mut output = pdf_extract::PlainTextOutput::new(&mut text);
                pdf_extract::output_doc_page(&doc, &mut output, *page)
            };
            result.map(|_| text)
        }));
        if let Ok(Ok(text)) = attempt {
            readable += 1;
            raw.push_str(&text);
            raw.push('\n');
        }
    }
    if readable == 0 && !pages.is_empty() {
        return Err(DevopsError::BadRequest(
            "the PDF uses a font encoding the reader cannot parse (seen with some Chinese fonts); export it again or import a Word/Markdown copy".into(),
        ));
    }
    let text = tidy(&raw);
    if text.chars().filter(|c| !c.is_whitespace()).count() < MIN_TEXT_CHARS {
        return Err(DevopsError::BadRequest(
            "the PDF has no text layer (a scan or images only); it cannot be searched".into(),
        ));
    }
    Ok(text)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F)
}

/// Collapse the page-layout whitespace PDF text comes with, keeping paragraph
/// breaks: runs of spaces become one, three or more newlines become two.
fn tidy(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut newlines = 0;
    let mut space = false;
    for ch in raw.replace('\r', "").chars() {
        match ch {
            '\n' => {
                newlines += 1;
                space = false;
            }
            c if c.is_whitespace() || c == '\u{0}' => space = true,
            c => {
                if newlines > 0 {
                    out.push_str(if newlines > 1 { "\n\n" } else { "\n" });
                } else if space {
                    // PDF text often puts a space between every Chinese
                    // character; that would break word matching, so drop it
                    // between two CJK characters.
                    if !(out.chars().next_back().is_some_and(is_cjk) && is_cjk(c)) {
                        out.push(' ');
                    }
                }
                newlines = 0;
                space = false;
                out.push(c);
            }
        }
    }
    out
}

/// A one-page PDF with a Helvetica text line, written by hand so a test owns its fixture.
#[cfg(test)]
pub(crate) fn tiny_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 18 Tf 72 700 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_owned(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_layer_of_a_pdf_is_read() {
        let text = extract(&tiny_pdf("Quarterly handbook passphrase zebra-42 for retrieval")).unwrap();
        assert!(text.contains("passphrase zebra-42"), "{text:?}");
    }

    #[test]
    fn a_pdf_without_text_says_so() {
        let error = extract(&tiny_pdf("")).unwrap_err().to_string();
        assert!(error.contains("no text layer"), "{error}");
    }

    #[test]
    fn garbage_is_refused_without_taking_the_process_down() {
        assert!(
            extract(b"PK\x03\x04 not a pdf")
                .unwrap_err()
                .to_string()
                .contains("not a PDF")
        );
        let truncated = &tiny_pdf("Some words that will be cut off mid file")[..120];
        assert!(extract(truncated).is_err());
        assert!(extract(b"%PDF-1.7\n1 0 obj\n<<").is_err());
    }

    #[test]
    fn spaces_between_chinese_characters_go_but_words_keep_theirs() {
        assert_eq!(tidy("经 验 交 流 会 for Work  2026"), "经验交流会 for Work 2026");
    }

    #[test]
    fn layout_whitespace_is_collapsed_but_paragraphs_survive() {
        assert_eq!(tidy("a   b\r\n\r\n\r\n\r\nc \n d"), "a b\n\nc\nd");
    }

    /// Real documents, when a directory of them is supplied. Not run by default.
    #[test]
    #[ignore]
    fn real_pdfs_from_a_directory() {
        let dir = std::env::var("DREAM_TEST_PDF_DIR").expect("set DREAM_TEST_PDF_DIR");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let started = std::time::Instant::now();
            match extract(&bytes) {
                Ok(text) => {
                    let head: String = text.chars().filter(|c| !c.is_control()).take(60).collect();
                    println!(
                        "OK   {} | {} KiB -> {} chars in {} ms | {head}",
                        path.file_name().unwrap().to_string_lossy(),
                        bytes.len() / 1024,
                        text.chars().count(),
                        started.elapsed().as_millis()
                    );
                }
                Err(error) => println!(
                    "ERR  {} | {} KiB | {error}",
                    path.file_name().unwrap().to_string_lossy(),
                    bytes.len() / 1024
                ),
            }
        }
    }
}
