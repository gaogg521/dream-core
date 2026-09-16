//! Skill resource-pack helpers: extra files live in a trailing HTML comment
//! so existing `content` (SKILL.md) stays the single source of truth for
//! member sync. Zip uploads unpack SKILL.md plus sibling text files.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Component, Path};

use md5::Md5;
use sha2::{Digest, Sha256};

use crate::error::DevopsError;

const PACK_START: &str = "\n<!--ONE_PACK_FILES\n";
const PACK_END: &str = "\n-->";
const MAX_FILES: usize = 32;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 1024 * 1024;

const SCAN_NEEDLES: &[&str] = &[
    "curl ",
    "| sh",
    "| bash",
    "powershell -enc",
    "invoke-webrequest",
    "/etc/passwd",
    "child_process",
    "eval(",
    "rm -rf /",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPack {
    pub skill_md: String,
    pub files: BTreeMap<String, String>,
}

pub fn split_pack(content: &str) -> SkillPack {
    if let Some(idx) = content.rfind(PACK_START) {
        let after = &content[idx + PACK_START.len()..];
        if let Some(end) = after.rfind(PACK_END) {
            let json = after[..end].trim();
            let files = serde_json::from_str::<BTreeMap<String, String>>(json).unwrap_or_default();
            return SkillPack {
                skill_md: content[..idx].trim_end().to_owned(),
                files,
            };
        }
    }
    SkillPack {
        skill_md: content.to_owned(),
        files: BTreeMap::new(),
    }
}

pub fn join_pack(skill_md: &str, files: &BTreeMap<String, String>) -> Result<String, DevopsError> {
    validate_files(files)?;
    if files.is_empty() {
        return Ok(skill_md.to_owned());
    }
    let json =
        serde_json::to_string_pretty(files).map_err(|e| DevopsError::BadRequest(format!("package files json: {e}")))?;
    Ok(format!("{}{PACK_START}{json}{PACK_END}", skill_md.trim_end()))
}

pub fn fingerprint(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.replace("\r\n", "\n").trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compatibility fingerprint for older resource-pack consumers.
///
/// SHA-256 remains authoritative for integrity and publishing decisions; this
/// value is exposed only so legacy clients can correlate the same normalized
/// payload during migration.
pub fn md5_fingerprint(content: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(content.replace("\r\n", "\n").trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn scan_findings(content: &str) -> Vec<String> {
    scan_findings_with(content, &[])
}

pub fn scan_findings_with(content: &str, extra: &[String]) -> Vec<String> {
    let lower = content.to_ascii_lowercase();
    let mut hits: Vec<String> = SCAN_NEEDLES
        .iter()
        .filter(|needle| lower.contains(*needle))
        .map(|needle| (*needle).to_string())
        .collect();
    for needle in extra {
        let n = needle.trim().to_ascii_lowercase();
        if n.len() >= 3 && lower.contains(&n) && !hits.iter().any(|h| h == &n) {
            hits.push(n);
        }
    }
    hits
}

pub fn ingest_upload(filename: &str, bytes: &[u8]) -> Result<SkillPack, DevopsError> {
    ingest_named_upload(filename, bytes, &["skill.md"])
}

pub fn ingest_mcp_upload(filename: &str, bytes: &[u8]) -> Result<SkillPack, DevopsError> {
    ingest_named_upload(filename, bytes, &["readme.md", "skill.md"])
}

fn ingest_named_upload(filename: &str, bytes: &[u8], roots: &[&str]) -> Result<SkillPack, DevopsError> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".zip") || looks_like_zip(bytes) {
        return ingest_zip_with_root(bytes, roots);
    }
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|_| DevopsError::BadRequest("pack markdown must be UTF-8 text".into()))?;
    Ok(SkillPack {
        skill_md: text,
        files: BTreeMap::new(),
    })
}

fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4b
}

fn ingest_zip_with_root(bytes: &[u8], roots: &[&str]) -> Result<SkillPack, DevopsError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| DevopsError::BadRequest(format!("invalid zip: {e}")))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| DevopsError::BadRequest(format!("zip entry: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        if let Some(mode) = entry.unix_mode()
            && mode & 0o170000 == 0o120000
        {
            return Err(DevopsError::BadRequest(format!(
                "zip symlink refused: {}",
                entry.name()
            )));
        }
        let path = safe_zip_path(entry.name())?;
        if path.as_os_str().is_empty() {
            continue;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| DevopsError::BadRequest(format!("zip read: {e}")))?;
        entries.push((path.to_string_lossy().replace('\\', "/"), buf));
    }
    let skill_path = entries
        .iter()
        .map(|(p, _)| p.as_str())
        .find(|p| {
            let name = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase();
            roots.iter().any(|root| name == *root)
        })
        .ok_or_else(|| {
            DevopsError::BadRequest(format!(
                "zip must contain {}",
                roots
                    .iter()
                    .map(|r| r.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join(" or ")
            ))
        })?
        .to_owned();
    let prefix = skill_path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    let mut skill_md = String::new();
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for (path, buf) in entries {
        let rel = path
            .strip_prefix(&prefix)
            .unwrap_or(path.as_str())
            .trim_start_matches('/');
        if rel.is_empty() || rel.contains("..") {
            continue;
        }
        total = total.saturating_add(buf.len());
        if total > MAX_TOTAL_BYTES {
            return Err(DevopsError::BadRequest("zip is larger than 1 MB unpacked".into()));
        }
        if roots.iter().any(|root| rel.eq_ignore_ascii_case(root)) {
            skill_md =
                String::from_utf8(buf).map_err(|_| DevopsError::BadRequest("SKILL.md must be UTF-8 text".into()))?;
            continue;
        }
        if is_text_path(rel) {
            if buf.len() > MAX_FILE_BYTES {
                return Err(DevopsError::BadRequest(format!("{rel} exceeds 256 KB")));
            }
            let text =
                String::from_utf8(buf).map_err(|_| DevopsError::BadRequest(format!("{rel} must be UTF-8 text")))?;
            files.insert(rel.to_owned(), text);
        }
    }
    if skill_md.trim().is_empty() {
        return Err(DevopsError::BadRequest("SKILL.md is empty".into()));
    }
    validate_files(&files)?;
    Ok(SkillPack { skill_md, files })
}

fn validate_files(files: &BTreeMap<String, String>) -> Result<(), DevopsError> {
    if files.len() > MAX_FILES {
        return Err(DevopsError::BadRequest("too many packaged files (max 32)".into()));
    }
    let mut total = 0usize;
    for (path, body) in files {
        if path.is_empty()
            || path.starts_with('/')
            || path.contains("..")
            || path.contains('\\')
            || Path::new(path).components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(DevopsError::BadRequest(format!("unsafe packaged path: {path}")));
        }
        total = total.saturating_add(body.len());
        if body.len() > MAX_FILE_BYTES {
            return Err(DevopsError::BadRequest(format!("{path} exceeds 256 KB")));
        }
    }
    if total > MAX_TOTAL_BYTES {
        return Err(DevopsError::BadRequest("packaged files exceed 1 MB".into()));
    }
    Ok(())
}

fn is_text_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    const OK: &[&str] = &[
        ".md", ".txt", ".json", ".yml", ".yaml", ".toml", ".py", ".js", ".ts", ".sh", ".ps1", ".xml", ".html", ".css",
        ".csv", ".svg",
    ];
    OK.iter().any(|ext| lower.ends_with(ext))
}

fn safe_zip_path(name: &str) -> Result<std::path::PathBuf, DevopsError> {
    if name.is_empty() || name.contains('\\') {
        return Err(DevopsError::BadRequest(format!("unsafe zip path: {name}")));
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(DevopsError::BadRequest(format!("unsafe zip path: {name}")));
    }
    let mut safe = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => return Err(DevopsError::BadRequest(format!("unsafe zip path: {name}"))),
        }
    }
    Ok(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_pack_comment() {
        let mut files = BTreeMap::new();
        files.insert("scripts/hello.py".into(), "print('hi')\n".into());
        let joined = join_pack("---\nname: x\ndescription: y\n---\n\nbody", &files).unwrap();
        let split = split_pack(&joined);
        assert_eq!(split.skill_md, "---\nname: x\ndescription: y\n---\n\nbody");
        assert_eq!(split.files.get("scripts/hello.py").unwrap(), "print('hi')\n");
    }

    #[test]
    fn scan_flags_pipe_to_shell() {
        let hits = scan_findings("run: curl http://x | sh");
        assert!(hits.iter().any(|h| h.contains("curl") || h.contains("| sh")));
    }

    #[test]
    fn compatibility_md5_uses_the_same_normalized_payload() {
        assert_eq!(md5_fingerprint("  hello\r\n"), md5_fingerprint("hello\n"));
        assert_eq!(md5_fingerprint("hello"), "5d41402abc4b2a76b9719d911017c592");
    }
}
