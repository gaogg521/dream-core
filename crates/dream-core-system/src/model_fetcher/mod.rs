mod fetchers;
mod url_fixer;

use std::sync::Arc;

use dream_core_api_types::{BedrockConfig, FetchModelsAnonymousRequest, FetchModelsRequest, FetchModelsResponse};
use dream_core_common::decrypt_string;
use dream_core_db::IProviderRepository;

use crate::error::SystemError;
use crate::provider::deserialize_opt;

/// Internal configuration extracted from a provider row for model fetching.
#[derive(Debug)]
pub(crate) struct FetchConfig {
    pub platform: String,
    pub base_url: String,
    pub api_key: String,
    pub bedrock_config: Option<BedrockConfig>,
}

/// When api_key contains multiple keys separated by newlines (the official
/// multi-key rotation format), extract only the first key for model fetching.
/// This avoids sending a multi-line Authorization header, which the HTTP client
/// rejects.
fn extract_first_key(api_key: &str) -> String {
    api_key.split('\n').next().unwrap_or(api_key).trim().to_string()
}

/// Service for fetching model lists from remote provider APIs.
#[derive(Clone)]
pub struct ModelFetchService {
    repo: Arc<dyn IProviderRepository>,
    encryption_key: [u8; 32],
    http_client: reqwest::Client,
}

impl ModelFetchService {
    pub fn new(repo: Arc<dyn IProviderRepository>, encryption_key: [u8; 32], http_client: reqwest::Client) -> Self {
        Self {
            repo,
            encryption_key,
            http_client,
        }
    }

    /// Fetch models for a provider by ID. If `try_fix` is true and the
    /// initial request fails on an OpenAI-compatible platform, attempt
    /// URL auto-correction with parallel probing.
    pub async fn fetch_models(
        &self,
        user_id: &str,
        provider_id: &str,
        req: &FetchModelsRequest,
    ) -> Result<FetchModelsResponse, SystemError> {
        let config = self.load_provider_config(user_id, provider_id).await?;
        self.fetch_with_config(&config, req.try_fix).await
    }

    /// Fetch models using credentials supplied in the request, without a
    /// persisted provider row. Powers the pre-create "Fetch Models" preview
    /// in the Add-Platform form.
    pub async fn fetch_models_anonymous(
        &self,
        req: &FetchModelsAnonymousRequest,
    ) -> Result<FetchModelsResponse, SystemError> {
        validate_anonymous_request(req)?;
        let config = FetchConfig {
            platform: req.platform.clone(),
            base_url: req.base_url.clone(),
            api_key: extract_first_key(&req.api_key),
            bedrock_config: req.bedrock_config.clone(),
        };
        self.fetch_with_config(&config, req.try_fix).await
    }

    /// Shared fetch+try_fix branch used by both the by-id and anonymous
    /// entry points.
    async fn fetch_with_config(&self, config: &FetchConfig, try_fix: bool) -> Result<FetchModelsResponse, SystemError> {
        match fetchers::fetch_for_platform(&self.http_client, config).await {
            Ok(models) => Ok(FetchModelsResponse {
                models,
                fixed_base_url: None,
            }),
            Err(err) if try_fix && supports_url_fix(&config.platform) => {
                url_fixer::try_fix_url(&self.http_client, config).await.map_err(|_| err)
            }
            Err(err) => Err(err),
        }
    }

    /// Extract and decrypt provider configuration from DB.
    async fn load_provider_config(&self, user_id: &str, provider_id: &str) -> Result<FetchConfig, SystemError> {
        let row = self
            .repo
            .find_by_id(user_id, provider_id)
            .await?
            .ok_or_else(|| SystemError::NotFound(format!("Provider {provider_id} not found")))?;

        let api_key = decrypt_string(&row.api_key_encrypted, &self.encryption_key)?;
        // The third copy of this rule, and the one that still disagreed: it
        // exempted Ollama but not Bedrock, so refreshing the model list of a
        // SAVED Bedrock provider — which correctly stores no key — failed with
        // "API key is empty". Now shared, like the other two.
        if api_key.trim().is_empty() && !crate::platform_authenticates_without_api_key(&row.platform) {
            return Err(SystemError::BadRequest("API key is empty".into()));
        }

        let bedrock_config: Option<BedrockConfig> = deserialize_opt(&row.bedrock_config, "bedrock_config")?;

        Ok(FetchConfig {
            platform: row.platform,
            base_url: row.base_url,
            api_key: extract_first_key(&api_key),
            bedrock_config,
        })
    }
}

/// Validate a `FetchModelsAnonymousRequest` — platform / base_url / api_key
/// must all be non-empty after trim.
fn validate_anonymous_request(req: &FetchModelsAnonymousRequest) -> Result<(), SystemError> {
    if req.platform.trim().is_empty() {
        return Err(SystemError::BadRequest("platform is required".into()));
    }
    // Bedrock is the exception: the AWS SDK derives its endpoint from the
    // region, so the dialog sends no base URL and `fetch_bedrock` never reads
    // one. Requiring it here failed the Bedrock model list outright.
    if !crate::platform_has_no_base_url(&req.platform) && req.base_url.trim().is_empty() {
        return Err(SystemError::BadRequest("baseUrl is required".into()));
    }
    // Bedrock uses bedrock_config for credentials; a local Ollama daemon has
    // no credentials at all. Empty api_key is allowed for both — see
    // `platform_authenticates_without_api_key`, which provider creation now
    // shares rather than keeping its own, stricter copy of this rule.
    if !crate::platform_authenticates_without_api_key(&req.platform) && req.api_key.trim().is_empty() {
        return Err(SystemError::BadRequest("apiKey is required".into()));
    }
    Ok(())
}

/// Platforms that support URL auto-fix (OpenAI-compatible). Ollama's native
/// `/api/tags` has no `/v1` variant to probe for, so auto-fix would only
/// mislead.
fn supports_url_fix(platform: &str) -> bool {
    !matches!(
        platform,
        "anthropic"
            | "claude"
            | "gemini"
            // Same pair as the fetch dispatch: `gemini-vertex-ai` is the value
            // real rows carry, `vertex-ai` is kept for legacy ones. Listing
            // only the short name let URL auto-fix probe `/v1` against Vertex,
            // which has no such variant to find.
            | "gemini-vertex-ai"
            | "vertex-ai"
            | "bedrock"
            | "minimax"
            | "dashscope-coding"
            | "ollama"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_common::encrypt_string;
    use dream_core_db::{CreateProviderParams, SqliteProviderRepository, init_database_memory};

    const TEST_KEY: [u8; 32] = [0x42; 32];
    const TEST_USER_ID: &str = "user-1";

    async fn setup() -> (ModelFetchService, dream_core_db::Database) {
        let db = init_database_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, user_type, username, password_hash, status, session_generation, created_at, updated_at) \
             VALUES (?, 'local', ?, '', 'active', 0, 1, 1)",
        )
        .bind(TEST_USER_ID)
        .bind(TEST_USER_ID)
        .execute(db.pool())
        .await
        .unwrap();
        let repo = Arc::new(SqliteProviderRepository::new(db.pool().clone()));
        let svc = ModelFetchService::new(repo, TEST_KEY, reqwest::Client::new());
        (svc, db)
    }

    async fn create_provider(db: &dream_core_db::Database, platform: &str, base_url: &str, api_key: &str) -> String {
        let repo = SqliteProviderRepository::new(db.pool().clone());
        let encrypted = encrypt_string(api_key, &TEST_KEY).unwrap();
        let row = repo
            .create(CreateProviderParams {
                id: None,
                user_id: TEST_USER_ID,
                platform,
                name: "Test",
                base_url,
                api_key_encrypted: &encrypted,
                models: "[]",
                enabled: true,
                capabilities: "[]",
                context_limit: None,
                model_protocols: None,
                model_enabled: None,
                model_health: None,
                model_settings: "{}",
                bedrock_config: None,
                is_full_url: false,
                managed_by: None,
            })
            .await
            .unwrap();
        row.id
    }

    #[test]
    fn supports_url_fix_openai_compatible() {
        assert!(supports_url_fix("openai"));
        assert!(supports_url_fix("new-api"));
        assert!(supports_url_fix("some-custom-provider"));
    }

    #[test]
    fn supports_url_fix_non_openai() {
        assert!(!supports_url_fix("anthropic"));
        assert!(!supports_url_fix("claude"));
        assert!(!supports_url_fix("gemini"));
        assert!(!supports_url_fix("bedrock"));
        assert!(!supports_url_fix("vertex-ai"));
        assert!(!supports_url_fix("minimax"));
        assert!(!supports_url_fix("dashscope-coding"));
        assert!(!supports_url_fix("ollama"));
    }

    #[tokio::test]
    async fn load_config_nonexistent_provider_returns_not_found() {
        let (svc, _db) = setup().await;
        let err = svc.load_provider_config(TEST_USER_ID, "no_such_id").await.unwrap_err();
        assert!(matches!(err, SystemError::NotFound(_)));
    }

    #[tokio::test]
    async fn load_config_empty_api_key_returns_bad_request() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "openai", "https://api.openai.com", "   ").await;
        let err = svc.load_provider_config(TEST_USER_ID, &id).await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    #[tokio::test]
    async fn load_config_decrypts_api_key() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "openai", "https://api.openai.com", "sk-test-key").await;
        let config = svc.load_provider_config(TEST_USER_ID, &id).await.unwrap();
        assert_eq!(config.api_key, "sk-test-key");
        assert_eq!(config.platform, "openai");
        assert_eq!(config.base_url, "https://api.openai.com");
        assert!(config.bedrock_config.is_none());
    }

    #[tokio::test]
    async fn fetch_models_vertex_ai_returns_hardcoded() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "vertex-ai", "https://unused", "fake-key").await;
        let req = FetchModelsRequest { try_fix: false };
        let resp = svc.fetch_models(TEST_USER_ID, &id, &req).await.unwrap();
        assert_eq!(resp.models.len(), 2);
        assert!(resp.fixed_base_url.is_none());
    }

    #[tokio::test]
    async fn fetch_models_minimax_returns_hardcoded() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "minimax", "https://unused", "fake-key").await;
        let req = FetchModelsRequest { try_fix: false };
        let resp = svc.fetch_models(TEST_USER_ID, &id, &req).await.unwrap();
        assert_eq!(resp.models.len(), 3);
    }

    #[tokio::test]
    async fn fetch_models_nonexistent_provider() {
        let (svc, _db) = setup().await;
        let req = FetchModelsRequest { try_fix: false };
        let err = svc.fetch_models(TEST_USER_ID, "no_such_id", &req).await.unwrap_err();
        assert!(matches!(err, SystemError::NotFound(_)));
    }

    #[tokio::test]
    async fn fetch_models_anonymous_minimax_returns_hardcoded() {
        let (svc, _db) = setup().await;
        let req = FetchModelsAnonymousRequest {
            platform: "minimax".into(),
            base_url: "https://unused".into(),
            api_key: "fake-key".into(),
            bedrock_config: None,
            try_fix: false,
        };
        let resp = svc.fetch_models_anonymous(&req).await.unwrap();
        assert_eq!(resp.models.len(), 3);
        assert!(resp.fixed_base_url.is_none());
    }

    #[tokio::test]
    async fn fetch_models_anonymous_rejects_empty_api_key() {
        let (svc, _db) = setup().await;
        let req = FetchModelsAnonymousRequest {
            platform: "openai".into(),
            base_url: "https://api.openai.com".into(),
            api_key: "   ".into(),
            bedrock_config: None,
            try_fix: false,
        };
        let err = svc.fetch_models_anonymous(&req).await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    #[tokio::test]
    async fn fetch_models_anonymous_rejects_empty_platform() {
        let (svc, _db) = setup().await;
        let req = FetchModelsAnonymousRequest {
            platform: "".into(),
            base_url: "https://api.openai.com".into(),
            api_key: "sk-test".into(),
            bedrock_config: None,
            try_fix: false,
        };
        let err = svc.fetch_models_anonymous(&req).await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    #[tokio::test]
    async fn fetch_models_anonymous_bedrock_allows_empty_api_key() {
        // Bedrock uses bedrock_config for credentials, not api_key.
        // With no bedrock_config attached the fetcher itself will fail,
        // but validate_anonymous_request must not reject up-front.
        //
        // The base URL below is invented — see
        // `the_bedrock_dialogs_actual_payload_is_accepted` for the body the
        // client really sends, which carries none. Keeping a fabricated value
        // here is what let this test agree with itself while the real request
        // was refused.
        let (_svc, _db) = setup().await;
        let req = FetchModelsAnonymousRequest {
            platform: "bedrock".into(),
            base_url: "https://bedrock.example".into(),
            api_key: "".into(),
            bedrock_config: None,
            try_fix: false,
        };
        assert!(validate_anonymous_request(&req).is_ok());
    }

    /// The exact JSON the settings dialog posts when the user opens the model
    /// dropdown for Bedrock (`EditModeModal`'s onFocus handler): a platform,
    /// an empty key, a `bedrock_config` — and no `base_url` key at all.
    ///
    /// This body used to be rejected twice over: serde had no default for
    /// `base_url`, so it never deserialized, and the validator required the
    /// field even though `fetch_bedrock` builds its endpoint from the region
    /// and never reads it. The user saw "Failed to fetch models" with an empty
    /// dropdown and no way forward.
    #[tokio::test]
    async fn the_bedrock_dialogs_actual_payload_is_accepted() {
        let body = serde_json::json!({
            "platform": "bedrock",
            "api_key": "",
            "bedrock_config": {
                "auth_method": "accessKey",
                "region": "us-east-1",
                "access_key_id": "AKIAEXAMPLE",
                "secret_access_key": "secret",
            },
        });
        let req: FetchModelsAnonymousRequest =
            serde_json::from_value(body).expect("the client's Bedrock body must deserialize");
        assert!(req.base_url.is_empty());
        assert!(validate_anonymous_request(&req).is_ok());
    }

    /// The exemption is Bedrock's alone. Every other platform is reached at an
    /// address the user supplies, so an absent one is still a 400 — otherwise
    /// this would just be deleting the check.
    #[test]
    fn a_platform_reached_over_http_still_needs_a_base_url() {
        let req = FetchModelsAnonymousRequest {
            platform: "openai".into(),
            base_url: String::new(),
            api_key: "sk-test".into(),
            bedrock_config: None,
            try_fix: false,
        };
        assert!(validate_anonymous_request(&req).is_err());
    }

    /// A SAVED Bedrock provider stores no API key — correctly, since its
    /// credentials live in `bedrock_config`. Refreshing its model list goes
    /// through `load_provider_config`, which kept its own copy of the
    /// empty-key rule that exempted Ollama and not Bedrock, and so rejected
    /// the row with "API key is empty" before the fetcher ever ran.
    ///
    /// The fetch itself is expected to fail here (no `bedrock_config` is
    /// attached to the row), so this asserts on WHICH error comes back: the
    /// gate must no longer be the thing that stops it.
    #[tokio::test]
    async fn a_saved_bedrock_provider_passes_the_empty_key_gate() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "bedrock", "", "").await;
        let err = svc
            .load_provider_config(TEST_USER_ID, &id)
            .await
            .err()
            .map(|e| e.to_string());
        assert!(
            !err.as_deref().is_some_and(|m| m.contains("API key is empty")),
            "the empty-key gate must not fire for Bedrock, got: {err:?}"
        );
    }

    /// The same gate for a saved Ollama daemon, which likewise has no key.
    #[tokio::test]
    async fn a_saved_ollama_provider_passes_the_empty_key_gate() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "ollama", "http://localhost:11434", "").await;
        let config = svc
            .load_provider_config(TEST_USER_ID, &id)
            .await
            .expect("a keyless Ollama row must load");
        assert_eq!(config.platform, "ollama");
        assert!(config.api_key.is_empty());
    }

    /// And it still fires for a platform that genuinely authenticates with a
    /// key, so the exemption did not become a blanket removal.
    #[tokio::test]
    async fn a_saved_keyed_provider_still_fails_the_empty_key_gate() {
        let (svc, db) = setup().await;
        let id = create_provider(&db, "openai", "https://api.openai.com/v1", "").await;
        let err = svc.load_provider_config(TEST_USER_ID, &id).await.unwrap_err();
        assert!(err.to_string().contains("API key is empty"), "got: {err}");
    }

    /// Vertex is the value real rows carry, spelled the way the client and
    /// `model_platforms` spell it.
    ///
    /// The fetch dispatch used to match `"vertex-ai"` alone — a string no
    /// provider row has ever contained — so every Vertex provider skipped its
    /// hard-coded catalogue and fell through to the OpenAI-compatible branch,
    /// which asks for `{base_url}/models` with a bearer token. The tests did
    /// not catch it because they built their fixtures from that same invented
    /// string.
    #[tokio::test]
    async fn vertex_resolves_under_the_platform_name_clients_actually_send() {
        let client = reqwest::Client::new();
        for platform in ["gemini-vertex-ai", "vertex-ai"] {
            let config = FetchConfig {
                platform: platform.into(),
                // Deliberately unroutable: reaching the network at all would
                // mean the hard-coded catalogue was not used.
                base_url: "http://127.0.0.1:1".into(),
                api_key: String::new(),
                bedrock_config: None,
            };
            let models = fetchers::fetch_for_platform(&client, &config)
                .await
                .unwrap_or_else(|e| panic!("{platform} must resolve from the built-in catalogue: {e}"));
            assert!(!models.is_empty(), "{platform} returned an empty catalogue");
        }
    }

    /// URL auto-fix probes `/v1` variants, which Vertex has none of — the
    /// exclusion list had the same invented spelling as the dispatch.
    #[test]
    fn vertex_is_excluded_from_url_auto_fix_under_both_spellings() {
        assert!(!supports_url_fix("gemini-vertex-ai"));
        assert!(!supports_url_fix("vertex-ai"));
        // The exclusion is still a list, not a blanket off-switch.
        assert!(supports_url_fix("custom"));
        assert!(supports_url_fix("new-api"));
    }

    /// `platform` crosses the wire as free text; a capitalised value must not
    /// quietly reinstate the rejection.
    #[test]
    fn the_base_url_exemption_does_not_depend_on_casing() {
        assert!(crate::platform_has_no_base_url("Bedrock"));
        assert!(crate::platform_has_no_base_url("  BEDROCK "));
        assert!(!crate::platform_has_no_base_url("ollama"));
        assert!(!crate::platform_has_no_base_url(""));
    }

    // ── extract_first_key ─────────────────────────────────────────────

    #[test]
    fn extract_first_key_single_key() {
        assert_eq!(extract_first_key("sk-test-key"), "sk-test-key");
    }

    #[test]
    fn extract_first_key_multi_key_newline() {
        assert_eq!(extract_first_key("sk-key1\nsk-key2"), "sk-key1",);
    }

    #[test]
    fn extract_first_key_multi_key_trailing_spaces() {
        assert_eq!(extract_first_key("  sk-key1  \nsk-key2"), "sk-key1",);
    }

    #[test]
    fn extract_first_key_empty_fallback() {
        assert!(extract_first_key("").is_empty());
    }

    #[test]
    fn extract_first_key_just_newlines() {
        assert!(extract_first_key("\n\n\n").is_empty());
    }

    #[tokio::test]
    async fn fetch_models_anonymous_multikey_uses_first_key() {
        let (svc, _db) = setup().await;
        // Multi-key api_key — must not fail with header parsing error
        let req = FetchModelsAnonymousRequest {
            platform: "minimax".into(),
            base_url: "https://unused".into(),
            api_key: "fake-key\nanother-key".into(),
            bedrock_config: None,
            try_fix: false,
        };
        let resp = svc.fetch_models_anonymous(&req).await.unwrap();
        assert_eq!(resp.models.len(), 3);
    }

    #[tokio::test]
    async fn load_config_multi_key_stores_first_key() {
        let (svc, db) = setup().await;
        // Save a provider with multi-key api_key
        let id = create_provider(&db, "openai", "https://api.openai.com", "sk-key1\nsk-key2").await;
        let config = svc.load_provider_config(TEST_USER_ID, &id).await.unwrap();
        // Must extract only the first key for model fetching
        assert_eq!(config.api_key, "sk-key1");
    }
}
