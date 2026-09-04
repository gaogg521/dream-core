//! Application configuration parsed from CLI arguments + key derivation.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    Local,
    WebUi,
    DreamPro,
}

impl IdentityMode {
    pub fn auth_label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::WebUi => "webui",
            Self::DreamPro => "aionpro",
        }
    }

    pub fn is_local(self) -> bool {
        self == Self::Local
    }
}

/// Application configuration parsed from CLI arguments.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub work_dir: PathBuf,
    pub app_version: String,
    /// Run in local embedded mode (skip authentication, use system_default_user).
    pub local: bool,
    pub identity_mode: IdentityMode,
    pub bootstrap_secret: Option<String>,
    /// Dump prompt diagnostics under `data_dir/prompt-dumps`.
    pub dump_prompts: bool,
    /// Explicitly authorize backup and rebuild for corruption-like local databases.
    pub recover_corrupted_database: bool,
}

impl AppConfig {
    pub fn effective_identity_mode(&self) -> IdentityMode {
        if self.local {
            IdentityMode::Local
        } else {
            self.identity_mode
        }
    }

    /// Format as `host:port` for socket binding.
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Local URL helpers should use to call this backend from the same machine.
    pub fn local_base_url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" | "::" => "127.0.0.1",
            other => other,
        };
        format!("http://{host}:{}", self.port)
    }

    /// Path to the SQLite database file.
    ///
    /// Resolves through `data_paths` so an install created before the rename
    /// keeps opening the catalog it already has instead of silently starting a
    /// new empty one beside it.
    pub fn database_path(&self) -> PathBuf {
        dream_core_common::backend_db_path(&self.data_dir)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            host: dream_core_common::constants::DEFAULT_HOST.to_string(),
            port: dream_core_common::constants::DEFAULT_PORT,
            data_dir: PathBuf::from("data"),
            work_dir: PathBuf::from("data"),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            local: false,
            identity_mode: IdentityMode::WebUi,
            bootstrap_secret: None,
            dump_prompts: false,
            recover_corrupted_database: false,
        }
    }
}

/// Derive a 32-byte encryption key from the JWT secret using SHA-256.
pub fn derive_encryption_key(jwt_secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"aionui-encryption-key:");
    hasher.update(jwt_secret.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 25808);
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(config.identity_mode, IdentityMode::WebUi);
        assert!(config.bootstrap_secret.is_none());
        assert!(!config.dump_prompts);
        assert!(!config.recover_corrupted_database);
    }

    #[test]
    fn test_app_config_socket_addr() {
        let config = AppConfig {
            host: "0.0.0.0".to_string(),
            port: 3000,
            ..Default::default()
        };
        assert_eq!(config.socket_addr(), "0.0.0.0:3000");
    }

    #[test]
    fn local_base_url_uses_loopback_for_wildcard_host() {
        let config = AppConfig {
            host: "0.0.0.0".to_string(),
            port: 49152,
            ..Default::default()
        };
        assert_eq!(config.local_base_url(), "http://127.0.0.1:49152");
    }

    #[test]
    fn test_app_config_database_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        assert_eq!(config.database_path(), dir.path().join("one-backend.db"));
    }

    /// An upgrade must not point the backend at a name nothing on disk uses:
    /// SQLite would create it, the catalog would come up empty, and the user
    /// would open the app to find every conversation gone.
    #[test]
    fn database_path_keeps_using_a_pre_rebrand_catalog() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aionui-backend.db"), b"existing catalog").unwrap();
        let config = AppConfig {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        assert_eq!(config.database_path(), dir.path().join("aionui-backend.db"));
    }
}
