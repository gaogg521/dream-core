//! Where the agent-facing CLIs (`config`, `diagnose`, `team`) read their JSON payload from.
//!
//! The documented path is stdin. On Windows that path is lossy in ways the CLI cannot see:
//!
//! - Windows PowerShell 5.1 encodes everything piped to a native program with
//!   `$OutputEncoding`, which defaults to **ASCII**. Every non-ASCII character becomes a
//!   literal `?` *before* the bytes reach us, so a Chinese task name arrives as `????` and
//!   is stored that way, with nothing to tell us it happened.
//! - Setting that encoding to UTF-8 fixes the text but prepends a BOM, which a plain
//!   `read_to_string` hands to the JSON parser as an unexpected character.
//! - Redirected/`Set-Content` output may be UTF-16.
//!
//! So this module (1) decodes the bytes properly, whatever BOM they carry, (2) offers
//! `--input-file <path>` as a channel that bypasses the pipe entirely, and (3) refuses a
//! stdin payload whose strings look like they were hollowed out into question marks, with
//! an error that says how to send the text safely.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

static INPUT_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Remember the `--input-file` the process was started with. Called once from `main`.
pub(crate) fn set_input_file(path: Option<PathBuf>) {
    if let Some(path) = path {
        let _ = INPUT_FILE.set(path);
    }
}

/// Where the payload text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadSource {
    File,
    Stdin,
}

/// Decode payload bytes: UTF-8 (with or without BOM) or UTF-16 with a BOM.
pub(crate) fn decode_payload_bytes(bytes: &[u8]) -> Result<String, io::Error> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    // Windows PowerShell 5.1 emits one BOM for `$OutputEncoding = UTF8Encoding($false)` and two for
    // `[Text.Encoding]::UTF8` (measured), so strip every leading one rather than just the first.
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let mut rest = bytes;
        while let Some(next) = rest.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            rest = next;
        }
        return String::from_utf8(rest.to_vec()).map_err(|_| invalid("payload is not valid UTF-8"));
    }
    let utf16 = |rest: &[u8], le: bool| -> Result<String, io::Error> {
        if !rest.len().is_multiple_of(2) {
            return Err(invalid("payload is not valid UTF-16"));
        }
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| {
                if le {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| invalid("payload is not valid UTF-16"))
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("payload is not valid UTF-8"))
}

/// Read the payload text from `--input-file` when given, otherwise from stdin.
pub(crate) fn read_payload_text() -> Result<(String, PayloadSource), io::Error> {
    if let Some(path) = INPUT_FILE.get() {
        return read_file(path).map(|text| (text, PayloadSource::File));
    }
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    decode_payload_bytes(&bytes).map(|text| (text, PayloadSource::Stdin))
}

fn read_file(path: &Path) -> Result<String, io::Error> {
    let bytes = std::fs::read(path)?;
    decode_payload_bytes(&bytes)
}

/// A string made mostly of `?` is what non-ASCII text turns into when a Windows pipe
/// encodes it as ASCII. Returns the first such string's JSON path, if any.
///
/// Only meaningful for stdin: a file is written by the agent as bytes it chose, so it is
/// trusted. A real value that happens to be mostly question marks can be sent with
/// `--input-file`.
pub(crate) fn suspected_encoding_loss(value: &Value) -> Option<String> {
    fn walk(value: &Value, path: &mut String) -> Option<String> {
        match value {
            Value::String(text) => {
                let total = text.chars().count();
                let marks = text.chars().filter(|c| *c == '?').count();
                (total >= 3 && marks * 10 >= total * 6).then(|| path.clone())
            }
            Value::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
                let keep = path.len();
                path.push_str(&format!("[{index}]"));
                let found = walk(item, path);
                path.truncate(keep);
                found
            }),
            Value::Object(map) => map.iter().find_map(|(key, item)| {
                let keep = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                let found = walk(item, path);
                path.truncate(keep);
                found
            }),
            _ => None,
        }
    }
    walk(value, &mut String::new())
}

/// The message for a payload rejected by [`suspected_encoding_loss`]; the offending field is
/// reported separately in the error's `field`.
pub(crate) const ENCODING_LOSS_MESSAGE: &str = "non-ASCII text in the payload looks like it was replaced with '?' on \
     the way in (Windows PowerShell pipes use ASCII by default). Nothing was saved. Write the JSON to a UTF-8 file \
     and pass it with `--input-file <path>`, or set `$OutputEncoding = [System.Text.UTF8Encoding]::new($false)` \
     before piping.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_utf8_with_and_without_a_bom() {
        assert_eq!(decode_payload_bytes("上海".as_bytes()).unwrap(), "上海");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("上海".as_bytes());
        assert_eq!(decode_payload_bytes(&with_bom).unwrap(), "上海");
    }

    #[test]
    fn strips_a_doubled_bom_as_windows_powershell_emits() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF, 0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(r#"{"n":"上海"}"#.as_bytes());
        let text = decode_payload_bytes(&bytes).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["n"], "上海");
    }

    #[test]
    fn decodes_utf16_in_both_byte_orders() {
        let units: Vec<u16> = "上海 ok".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        let mut be = vec![0xFE, 0xFF];
        be.extend(units.iter().flat_map(|u| u.to_be_bytes()));
        assert_eq!(decode_payload_bytes(&le).unwrap(), "上海 ok");
        assert_eq!(decode_payload_bytes(&be).unwrap(), "上海 ok");
    }

    #[test]
    fn rejects_bytes_that_are_not_text() {
        assert!(decode_payload_bytes(&[0xC3, 0x28]).is_err());
        assert!(decode_payload_bytes(&[0xFF, 0xFE, 0x41]).is_err());
    }

    #[test]
    fn a_bom_payload_parses_as_json() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"name":"x"}"#);
        let text = decode_payload_bytes(&bytes).unwrap();
        assert!(serde_json::from_str::<Value>(&text).is_ok());
    }

    #[test]
    fn flags_text_that_was_hollowed_out_into_question_marks() {
        let hollow = json!({ "name": "????????", "message": "查询天气" });
        assert_eq!(suspected_encoding_loss(&hollow).as_deref(), Some("name"));
        let nested = json!({ "jobs": [{ "title": "??? ??" }] });
        assert_eq!(suspected_encoding_loss(&nested).as_deref(), Some("jobs[0].title"));
    }

    #[test]
    fn leaves_ordinary_text_with_a_question_mark_alone() {
        let fine = json!({ "name": "Weather?", "message": "What is the weather in Shanghai today?", "a": "??" });
        assert_eq!(suspected_encoding_loss(&fine), None);
    }
}
