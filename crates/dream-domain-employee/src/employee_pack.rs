//! Digital-employee ZIP packs (OpenOcta-compatible): `README.md` + `config.json`,
//! optional `assets/icon.png`.

use std::io::{Cursor, Read};
use std::path::{Component, Path};

use serde::Deserialize;
use serde_json::Value;

use crate::error::EmployeeError;

const MAX_ZIP: usize = 2 * 1024 * 1024;
const MAX_ICON: usize = 650 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct EmployeePackConfig {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub mcp_servers: Option<Value>,
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers_camel: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct EmployeePack {
    pub name: String,
    pub description: Option<String>,
    pub agent_type: String,
    pub readme: Option<String>,
    pub icon_data_url: Option<String>,
    pub mcp_servers: Option<Value>,
    pub pack_id: Option<String>,
    pub pack_files: std::collections::BTreeMap<String, String>,
}

pub fn parse_employee_zip(bytes: &[u8]) -> Result<EmployeePack, EmployeeError> {
    if bytes.len() > MAX_ZIP {
        return Err(EmployeeError::BadRequest("employee zip exceeds 2 MB".into()));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| EmployeeError::BadRequest(format!("invalid zip: {e}")))?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| EmployeeError::BadRequest(format!("zip entry: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let path = safe_zip_path(entry.name())?;
        let rel = path.to_string_lossy().replace('\\', "/");
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| EmployeeError::BadRequest(format!("zip read: {e}")))?;
        files.push((rel, buf));
    }
    let config_path = files
        .iter()
        .map(|(p, _)| p.as_str())
        .find(|p| p.eq_ignore_ascii_case("config.json") || p.to_ascii_lowercase().ends_with("/config.json"))
        .ok_or_else(|| EmployeeError::BadRequest("zip must contain config.json".into()))?
        .to_owned();
    let prefix = config_path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    let mut config_raw = None;
    let mut readme = None;
    let mut icon = None;
    let mut pack_files = std::collections::BTreeMap::new();
    for (path, buf) in files {
        let rel = path.strip_prefix(&prefix).unwrap_or(path.as_str());
        if rel.eq_ignore_ascii_case("config.json") {
            config_raw = Some(String::from_utf8(buf).map_err(|_| {
                EmployeeError::BadRequest("config.json must be UTF-8".into())
            })?);
        } else if rel.eq_ignore_ascii_case("readme.md") {
            readme = Some(String::from_utf8(buf).map_err(|_| {
                EmployeeError::BadRequest("README.md must be UTF-8".into())
            })?);
        } else if rel.eq_ignore_ascii_case("assets/icon.png") || rel.eq_ignore_ascii_case("icon.png")
        {
            if buf.len() > MAX_ICON {
                return Err(EmployeeError::BadRequest("icon.png exceeds 650 KB".into()));
            }
            icon = Some(format!(
                "data:image/png;base64,{}",
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf)
            ));
        } else if is_text_pack_path(rel) {
            if let Ok(text) = String::from_utf8(buf) {
                pack_files.insert(rel.to_owned(), text);
            }
        }
    }
    let config: EmployeePackConfig = serde_json::from_str(
        config_raw
            .as_deref()
            .ok_or_else(|| EmployeeError::BadRequest("config.json missing".into()))?,
    )
    .map_err(|e| EmployeeError::BadRequest(format!("config.json: {e}")))?;
    let name = config.name.trim();
    if name.is_empty() {
        return Err(EmployeeError::BadRequest("config.json name is required".into()));
    }
    Ok(EmployeePack {
        name: name.to_owned(),
        description: config.description.filter(|s| !s.trim().is_empty()),
        agent_type: config
            .agent_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("claude")
            .to_owned(),
        readme,
        icon_data_url: icon,
        mcp_servers: config.mcp_servers.or(config.mcp_servers_camel),
        pack_id: config.id,
        pack_files,
    })
}

fn is_text_pack_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".txt", ".json", ".yml", ".yaml", ".py", ".js", ".ts"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn safe_zip_path(name: &str) -> Result<std::path::PathBuf, EmployeeError> {
    if name.is_empty() || name.contains('\\') {
        return Err(EmployeeError::BadRequest(format!("unsafe zip path: {name}")));
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(EmployeeError::BadRequest(format!("unsafe zip path: {name}")));
    }
    let mut safe = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => return Err(EmployeeError::BadRequest(format!("unsafe zip path: {name}"))),
        }
    }
    Ok(safe)
}
