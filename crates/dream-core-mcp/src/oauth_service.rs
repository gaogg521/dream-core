use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dream_core_api_types::{OAuthLoginResponse, OAuthStatusResponse};
use dream_core_common::{TimestampMs, now_ms};
use dream_core_db::{IOAuthTokenRepository, UpsertOAuthTokenParams};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken,
    TokenResponse, TokenUrl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use crate::error::McpError;
use crate::oauth_discovery::{self, ClientInfo, Metadata};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default timeout for the OAuth callback server waiting for the redirect.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);

/// Default OAuth client ID for MCP servers (public client, no secret).
const DEFAULT_CLIENT_ID: &str = "one";

/// Token expiry safety margin (refresh 5 minutes before expiration).
const EXPIRY_MARGIN_MS: i64 = 5 * 60 * 1000;

type LoginAttempts = Arc<std::sync::Mutex<HashMap<(String, String), std::time::Instant>>>;
type ResourceLocks = Arc<Mutex<HashMap<(String, String), std::sync::Weak<Mutex<()>>>>>;

/// Release on preparation errors, callback completion and cancellation alike.
struct LoginAttempt {
    attempts: LoginAttempts,
    key: (String, String),
    started: std::time::Instant,
}

impl Drop for LoginAttempt {
    fn drop(&mut self) {
        if let Ok(mut attempts) = self.attempts.lock()
            && attempts.get(&self.key) == Some(&self.started)
        {
            attempts.remove(&self.key);
        }
    }
}

// ---------------------------------------------------------------------------
// Pending login state
// ---------------------------------------------------------------------------

/// State held while waiting for the OAuth callback redirect.
///
/// Stores endpoint URLs rather than the typed `BasicClient` to avoid
/// complex generic type parameters from the `oauth2` crate.
struct PendingLogin {
    csrf_token: CsrfToken,
    pkce_verifier: PkceCodeVerifier,
    auth_url: String,
    token_url: String,
    redirect_url: String,
    client_info: ClientInfo,
    cancelled: Arc<Notify>,
}

struct PreparedLogin {
    authorization_url: String,
    listener: TcpListener,
    state: String,
    cancelled: Arc<Notify>,
}

// ---------------------------------------------------------------------------
// McpOAuthService
// ---------------------------------------------------------------------------

/// Service for MCP server OAuth 2.0 PKCE authentication.
///
/// Manages the full lifecycle: discovery → authorize → callback → token
/// exchange → storage → refresh → logout.
#[derive(Clone)]
pub struct McpOAuthService {
    token_repo: Arc<dyn IOAuthTokenRepository>,
    /// Mutex protecting pending login state by (user_id, oauth_state).
    pending: Arc<Mutex<HashMap<(String, String), PendingLogin>>>,
    login_attempts: LoginAttempts,
    resource_locks: ResourceLocks,
}

impl McpOAuthService {
    pub fn new(token_repo: Arc<dyn IOAuthTokenRepository>, _http_client: reqwest::Client) -> Self {
        // OAuth uses dedicated bounded, no-redirect clients. Keep the argument
        // for source compatibility with existing application composition.
        Self {
            token_repo,
            pending: Arc::new(Mutex::new(HashMap::new())),
            login_attempts: Arc::default(),
            resource_locks: Arc::default(),
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Check whether the given server URL has a valid (non-expired) OAuth token.
    pub async fn check_oauth_status(&self, user_id: &str, server_url: &str) -> Result<OAuthStatusResponse, McpError> {
        let authenticated = self.has_valid_token(user_id, server_url).await?;
        Ok(OAuthStatusResponse { authenticated })
    }

    /// Start the OAuth PKCE login flow for the given MCP server URL.
    ///
    /// 1. Discover authorization/token endpoints
    /// 2. Generate PKCE challenge
    /// 3. Start local callback server on a random port
    /// 4. Return the authorization URL for an explicit client-side Login click
    /// 5. Wait for the redirect with the authorization code
    /// 6. Exchange code for tokens and persist them
    pub async fn login(&self, user_id: &str, server_url: &str) -> Result<OAuthLoginResponse, McpError> {
        let key = (user_id.to_owned(), server_url.to_owned());
        let started = std::time::Instant::now();
        {
            let mut attempts = self
                .login_attempts
                .lock()
                .map_err(|_| McpError::OAuth("Login state unavailable".into()))?;
            if attempts.contains_key(&key) {
                return Ok(OAuthLoginResponse {
                    success: false,
                    pending: true,
                    authorization_url: None,
                    error: None,
                });
            }
            attempts.insert(key.clone(), started);
        }
        let attempt = LoginAttempt {
            attempts: self.login_attempts.clone(),
            key,
            started,
        };
        let prepared = tokio::time::timeout(Duration::from_secs(35), self.prepare_login_flow(user_id, server_url))
            .await
            .map_err(|_| McpError::OAuth("OAuth discovery timed out; explicit user retry required".into()))??;
        let still_current = self
            .login_attempts
            .lock()
            .map_err(|_| McpError::OAuth("Login state unavailable".into()))?
            .get(&(user_id.to_owned(), server_url.to_owned()))
            == Some(&started);
        if !still_current {
            self.pending.lock().await.remove(&(user_id.to_owned(), prepared.state));
            return Err(McpError::OAuth("OAuth login was cancelled".into()));
        }
        let authorize_url = prepared.authorization_url;
        let oauth_state = prepared.state;
        let listener = prepared.listener;
        let cancelled = prepared.cancelled;
        let service = self.clone();
        let user_id = user_id.to_owned();
        let server_url = server_url.to_owned();
        let resource_host = reqwest::Url::parse(&server_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        info!(
            resource_host,
            event = "mcp_oauth_explicit_login",
            "MCP OAuth awaiting explicit browser authorization"
        );
        tokio::spawn(async move {
            let _attempt = attempt;
            let flow = async {
                match service.wait_for_callback(&user_id, &oauth_state, listener).await {
                    Ok((code, state)) => service.exchange_code(&user_id, &server_url, code, state).await,
                    Err(error) => Err(error),
                }
            };
            let result = tokio::select! {
                biased;
                _ = cancelled.notified() => Err(McpError::OAuth("OAuth login cancelled".into())),
                result = flow => result,
            };
            service.pending.lock().await.remove(&(user_id.clone(), oauth_state));
            if result.is_err() {
                warn!(
                    resource_host,
                    event = "mcp_oauth_incomplete",
                    "MCP OAuth login did not complete; explicit user retry required"
                );
            } else {
                info!(
                    resource_host,
                    event = "mcp_oauth_authenticated",
                    "MCP OAuth authorization completed"
                );
            }
        });
        // The client opens this URL only in response to the user's Login click.
        // A hosted Core must never open a browser on its server machine.
        Ok(OAuthLoginResponse {
            success: false,
            pending: true,
            authorization_url: Some(authorize_url),
            error: None,
        })
    }

    /// Logout from the given MCP server URL (delete stored token).
    ///
    /// Idempotent: returns Ok even if no token was stored.
    pub async fn logout(&self, user_id: &str, server_url: &str) -> Result<(), McpError> {
        let resource = self.resource_lock(user_id, server_url).await;
        let _resource = resource.lock().await;
        // Serialize with the final token write. A late callback cannot recreate
        // credentials after logout, and its listener/request is cancelled.
        let mut pending = self.pending.lock().await;
        pending.retain(|(owner, _), login| {
            let keep = owner != user_id || login.client_info.resource != server_url;
            if !keep {
                login.cancelled.notify_one();
            }
            keep
        });
        if let Ok(mut attempts) = self.login_attempts.lock() {
            attempts.remove(&(user_id.to_owned(), server_url.to_owned()));
        }
        match self.token_repo.delete(user_id, server_url).await {
            Ok(()) => {
                debug!(server_url, "OAuth token deleted");
                Ok(())
            }
            Err(dream_core_db::DbError::NotFound(_)) => {
                debug!(server_url, "No OAuth token to delete (idempotent)");
                Ok(())
            }
            Err(e) => Err(McpError::Database(e)),
        }
    }

    /// Return the list of server URLs that have stored OAuth tokens.
    pub async fn get_authenticated_servers(&self, user_id: &str) -> Result<Vec<String>, McpError> {
        let urls = self.token_repo.list_authenticated_urls(user_id).await?;
        Ok(urls)
    }

    /// Get a valid access token for the given server URL.
    ///
    /// If the stored token is expired and a refresh token is available,
    /// automatically refreshes before returning.
    /// Returns `None` if no token is stored for this URL.
    pub async fn get_token(&self, user_id: &str, server_url: &str) -> Result<Option<String>, McpError> {
        let resource = self.resource_lock(user_id, server_url).await;
        let _resource = resource.lock().await;
        let row = match self.token_repo.get_by_url(user_id, server_url).await? {
            Some(row) => row,
            None => return Ok(None),
        };

        // Check if token is expired (with safety margin).
        if let Some(expires_at) = row.expires_at {
            let now = now_ms();
            if now >= expires_at.saturating_sub(EXPIRY_MARGIN_MS)
                && let Some(ref refresh_token) = row.refresh_token
            {
                match self
                    .refresh_token(user_id, server_url, refresh_token, row.client_info.as_deref())
                    .await
                {
                    Ok(new_token) => return Ok(self.has_valid_token(user_id, server_url).await?.then_some(new_token)),
                    Err(_) => {
                        warn!("MCP OAuth token refresh failed; expired credentials will not be returned");
                    }
                }
            }
            if now_ms() >= expires_at {
                return Ok(None);
            }
        }

        Ok(Some(row.access_token))
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    async fn resource_lock(&self, user_id: &str, server_url: &str) -> Arc<Mutex<()>> {
        let mut locks = self.resource_locks.lock().await;
        locks.retain(|_, lock| lock.strong_count() > 0);
        let key = (user_id.to_owned(), server_url.to_owned());
        if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    /// Discover endpoints, build OAuth client, generate PKCE, bind callback
    /// server, store pending state, and return the authorization URL + listener.
    async fn prepare_login_flow(&self, user_id: &str, server_url: &str) -> Result<PreparedLogin, McpError> {
        let metadata = self.discover_endpoints(server_url).await?;

        let auth_url_str = metadata.authorization_endpoint.clone();
        let token_url_str = metadata.token_endpoint.clone();

        let auth_url = AuthUrl::new(metadata.authorization_endpoint.clone())
            .map_err(|e| McpError::OAuth(format!("Invalid auth URL: {e}")))?;
        let token_url = TokenUrl::new(metadata.token_endpoint.clone())
            .map_err(|e| McpError::OAuth(format!("Invalid token URL: {e}")))?;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| McpError::OAuth(format!("Failed to bind callback server: {e}")))?;
        let callback_port = listener
            .local_addr()
            .map_err(|e| McpError::OAuth(format!("Failed to get callback port: {e}")))?
            .port();

        let redirect_url_str = format!("http://127.0.0.1:{callback_port}/callback");
        let redirect = RedirectUrl::new(redirect_url_str.clone())
            .map_err(|e| McpError::OAuth(format!("Invalid redirect URL: {e}")))?;

        let client_info =
            oauth_discovery::register(&Self::build_no_redirect_client()?, &metadata, &redirect_url_str).await?;
        let client = BasicClient::new(ClientId::new(client_info.client_id.clone()))
            .set_auth_type(oauth2::AuthType::RequestBody)
            .set_auth_uri(auth_url)
            .set_token_uri(token_url)
            .set_redirect_uri(redirect);

        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let (authorize_url, csrf_token) = client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge)
            .add_extra_param("resource", server_url)
            .url();
        let oauth_state = csrf_token.secret().clone();
        let cancelled = Arc::new(Notify::new());

        {
            let mut pending = self.pending.lock().await;
            let state = csrf_token.secret().clone();
            pending.insert(
                (user_id.to_string(), state),
                PendingLogin {
                    csrf_token,
                    pkce_verifier,
                    auth_url: auth_url_str,
                    token_url: token_url_str,
                    redirect_url: redirect_url_str,
                    client_info,
                    cancelled: cancelled.clone(),
                },
            );
        }

        Ok(PreparedLogin {
            authorization_url: authorize_url.to_string(),
            listener,
            state: oauth_state,
            cancelled,
        })
    }

    /// Check if a valid (non-expired) token exists for the URL.
    async fn has_valid_token(&self, user_id: &str, server_url: &str) -> Result<bool, McpError> {
        let row = match self.token_repo.get_by_url(user_id, server_url).await? {
            Some(row) => row,
            None => return Ok(false),
        };

        if let Some(expires_at) = row.expires_at
            && now_ms() >= expires_at
        {
            return Ok(false);
        }

        Ok(true)
    }

    async fn discover_endpoints(&self, server_url: &str) -> Result<Metadata, McpError> {
        oauth_discovery::discover(&Self::build_no_redirect_client()?, server_url, None).await
    }

    /// Wait for the OAuth callback redirect on the given listener.
    async fn wait_for_callback(
        &self,
        user_id: &str,
        expected_state: &str,
        listener: TcpListener,
    ) -> Result<(String, String), McpError> {
        tokio::time::timeout(
            CALLBACK_TIMEOUT,
            Self::handle_callback_connection(user_id, expected_state, listener, self.pending.clone()),
        )
        .await
        .map_err(|_| McpError::OAuth("OAuth callback timed out; explicit user retry required".into()))?
    }

    /// Handle a single HTTP connection on the callback server.
    async fn handle_callback_connection(
        user_id: &str,
        expected_state: &str,
        listener: TcpListener,
        pending: Arc<Mutex<HashMap<(String, String), PendingLogin>>>,
    ) -> Result<(String, String), McpError> {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|e| McpError::OAuth(format!("Failed to accept connection: {e}")))?;

            let request = tokio::time::timeout(Duration::from_secs(5), async {
                let mut buffer = Vec::new();
                while buffer.len() < 8192 {
                    let mut chunk = [0u8; 1024];
                    let count = stream.read(&mut chunk).await.ok()?;
                    if count == 0 {
                        return None;
                    }
                    buffer.extend_from_slice(&chunk[..count]);
                    if buffer.windows(4).any(|part| part == b"\r\n\r\n") {
                        return String::from_utf8(buffer).ok();
                    }
                }
                None
            })
            .await;
            let request = match request {
                Ok(Some(request)) => request,
                _ => continue,
            };
            let target = request
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("GET "))
                .and_then(|line| line.split_whitespace().next());
            let parsed = target
                .filter(|path| path.split('?').next() == Some("/callback"))
                .and_then(|path| reqwest::Url::parse(&format!("http://127.0.0.1{path}")).ok())
                .filter(|url| {
                    let mut keys = std::collections::HashSet::new();
                    url.query_pairs()
                        .filter(|(key, _)| matches!(key.as_ref(), "code" | "state" | "error"))
                        .all(|(key, value)| !value.is_empty() && keys.insert(key.into_owned()))
                });
            let denied = parsed.as_ref().is_some_and(|url| {
                let pairs = url.query_pairs().collect::<Vec<_>>();
                pairs
                    .iter()
                    .any(|(key, value)| key == "state" && value == expected_state)
                    && pairs.iter().any(|(key, _)| key == "error")
            });
            if denied {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nAuthorization cancelled. Return to One Work to retry.").await;
                return Err(McpError::OAuth(
                    "Authorization was declined; explicit user retry required".into(),
                ));
            }
            let valid =
                parsed.is_some() && parse_callback_query(&request).is_ok_and(|(_, state)| state == expected_state);
            if !valid {
                let _ = stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nInvalid OAuth callback")
                    .await;
                continue;
            }
            let (code, state) = parse_callback_query(&request)?;

            // Validate CSRF state.
            let guard = pending.lock().await;
            let pending_login = guard
                .get(&(user_id.to_string(), state.clone()))
                .ok_or_else(|| McpError::OAuth("No pending login state".to_string()))?;

            if state != *pending_login.csrf_token.secret() || state != expected_state {
                return Err(McpError::OAuth("CSRF state mismatch".to_string()));
            }
            drop(guard);

            // Acknowledge receipt; the UI reports success only after token exchange.
            let response = "HTTP/1.1 200 OK\r\n\
            Content-Type: text/html; charset=utf-8\r\n\
            Connection: close\r\n\r\n\
            <html><body><h1>Authorization received</h1>\
            <p>Return to One Work to check the connection status.</p>\
            </body></html>";

            let _ = stream.write_all(response.as_bytes()).await;

            return Ok((code, state));
        }
    }

    /// Build a no-redirect reqwest client for OAuth token exchange.
    fn build_no_redirect_client() -> Result<reqwest::Client, McpError> {
        reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| McpError::OAuth(format!("Failed to build HTTP client: {e}")))
    }

    /// Exchange the authorization code for tokens and persist them.
    async fn exchange_code(
        &self,
        user_id: &str,
        server_url: &str,
        code: String,
        state: String,
    ) -> Result<(), McpError> {
        let (auth_url_str, token_url_str, redirect_url_str, pkce_verifier, client_info) = {
            let guard = self.pending.lock().await;
            let pending = guard
                .get(&(user_id.to_string(), state.clone()))
                .ok_or_else(|| McpError::OAuth("No pending login state".to_string()))?;
            (
                pending.auth_url.clone(),
                pending.token_url.clone(),
                pending.redirect_url.clone(),
                PkceCodeVerifier::new(pending.pkce_verifier.secret().clone()),
                pending.client_info.clone(),
            )
        };

        let auth_url = AuthUrl::new(auth_url_str).map_err(|e| McpError::OAuth(format!("Invalid auth URL: {e}")))?;
        let token_url = TokenUrl::new(token_url_str).map_err(|e| McpError::OAuth(format!("Invalid token URL: {e}")))?;
        let redirect =
            RedirectUrl::new(redirect_url_str).map_err(|e| McpError::OAuth(format!("Invalid redirect URL: {e}")))?;

        let client = BasicClient::new(ClientId::new(client_info.client_id.clone()))
            .set_auth_type(oauth2::AuthType::RequestBody)
            .set_auth_uri(auth_url)
            .set_token_uri(token_url)
            .set_redirect_uri(redirect);

        let http_client = Self::build_no_redirect_client()?;

        let token_result = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .add_extra_param("resource", server_url)
            .request_async(&http_client)
            .await
            .map_err(|_| McpError::OAuth("OAuth token exchange failed".into()))?;

        let resource = self.resource_lock(user_id, server_url).await;
        let _resource = resource.lock().await;
        let mut pending = self.pending.lock().await;
        if !pending.contains_key(&(user_id.to_owned(), state.clone())) {
            return Err(McpError::OAuth("OAuth login was cancelled".into()));
        }
        self.persist_token(user_id, server_url, &token_result, &client_info)
            .await?;
        pending.remove(&(user_id.to_owned(), state));
        debug!(server_url, "OAuth tokens stored successfully");
        Ok(())
    }

    /// Refresh an expired access token using the refresh token.
    async fn refresh_token(
        &self,
        user_id: &str,
        server_url: &str,
        refresh_token_value: &str,
        client_info_json: Option<&str>,
    ) -> Result<String, McpError> {
        let registration: Option<ClientInfo> = client_info_json
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| McpError::OAuth("Invalid saved OAuth client binding; explicit login required".into()))?;
        if registration.as_ref().is_some_and(|info| info.resource != server_url) {
            return Err(McpError::OAuth(
                "Saved OAuth client is bound to another MCP resource".into(),
            ));
        }
        let http_client = Self::build_no_redirect_client()?;
        let metadata = oauth_discovery::discover(
            &http_client,
            server_url,
            registration.as_ref().and_then(|info| info.issuer.as_deref()),
        )
        .await?;
        if metadata.issuer.as_deref() != registration.as_ref().and_then(|info| info.issuer.as_deref()) {
            return Err(McpError::OAuth(
                "Authorization server changed; explicit login required".into(),
            ));
        }
        let token_url =
            TokenUrl::new(metadata.token_endpoint).map_err(|e| McpError::OAuth(format!("Invalid token URL: {e}")))?;

        let client = BasicClient::new(ClientId::new(
            registration
                .as_ref()
                .map_or(DEFAULT_CLIENT_ID, |info| info.client_id.as_str())
                .to_owned(),
        ))
        .set_auth_type(oauth2::AuthType::RequestBody)
        .set_token_uri(token_url);

        let refresh_token = RefreshToken::new(refresh_token_value.to_string());
        let token_result = client
            .exchange_refresh_token(&refresh_token)
            .add_extra_param("resource", server_url)
            .request_async(&http_client)
            .await
            .map_err(|_| McpError::OAuth("OAuth token refresh failed".into()))?;

        let new_access_token = token_result.access_token().secret().clone();

        let expires_at: Option<TimestampMs> = token_result.expires_in().map(|d| now_ms() + d.as_millis() as i64);

        // Prefer new refresh_token if provided, otherwise keep the old one.
        let new_refresh = token_result
            .refresh_token()
            .map(|t| t.secret().as_str())
            .unwrap_or(refresh_token_value);

        self.token_repo
            .upsert(UpsertOAuthTokenParams {
                user_id,
                server_url,
                access_token: &new_access_token,
                refresh_token: Some(new_refresh),
                client_info: client_info_json,
                token_type: "bearer",
                expires_at,
            })
            .await?;

        debug!(server_url, "OAuth token refreshed successfully");
        Ok(new_access_token)
    }

    /// Persist token response to DB.
    async fn persist_token<TR: TokenResponse>(
        &self,
        user_id: &str,
        server_url: &str,
        token_result: &TR,
        client_info: &ClientInfo,
    ) -> Result<(), McpError> {
        let expires_at: Option<TimestampMs> = token_result.expires_in().map(|d| now_ms() + d.as_millis() as i64);
        let client_info = serde_json::to_string(client_info)
            .map_err(|_| McpError::OAuth("Could not encode OAuth client binding".into()))?;

        self.token_repo
            .upsert(UpsertOAuthTokenParams {
                user_id,
                server_url,
                access_token: token_result.access_token().secret(),
                refresh_token: token_result.refresh_token().map(|t| t.secret().as_str()),
                client_info: Some(&client_info),
                token_type: "bearer",
                expires_at,
            })
            .await?;

        Ok(())
    }

    /// Clear the pending login state.
    #[cfg(test)]
    async fn clear_pending_for_user(&self, user_id: &str) {
        let mut guard = self.pending.lock().await;
        guard.retain(|(pending_user_id, _), _| pending_user_id != user_id);
    }
}

// ---------------------------------------------------------------------------
// Query parameter parsing
// ---------------------------------------------------------------------------

/// Parse `code` and `state` from the first line of an HTTP request.
///
/// Expects: `GET /callback?code=xxx&state=yyy HTTP/1.1`
fn parse_callback_query(request: &str) -> Result<(String, String), McpError> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| McpError::OAuth("Empty HTTP request".to_string()))?;

    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| McpError::OAuth("Malformed HTTP request line".to_string()))?;

    let query_str = path
        .split_once('?')
        .map(|(_, q)| q)
        .ok_or_else(|| McpError::OAuth("No query parameters in callback".to_string()))?;

    let mut code = None;
    let mut state = None;

    for pair in query_str.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "code" => code = Some(url_decode(value)),
                "state" => state = Some(url_decode(value)),
                _ => {}
            }
        }
    }

    let code = code.ok_or_else(|| McpError::OAuth("Missing 'code' in callback".to_string()))?;
    let state = state.ok_or_else(|| McpError::OAuth("Missing 'state' in callback".to_string()))?;

    Ok((code, state))
}

/// Minimal percent-decoding for query parameter values.
fn url_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.bytes();

    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                let hex = [h, l];
                if let Ok(s) = std::str::from_utf8(&hex)
                    && let Ok(byte) = u8::from_str_radix(s, 16)
                {
                    result.push(byte as char);
                    continue;
                }
                // Malformed percent-encoding: keep as-is.
                result.push('%');
                result.push(h as char);
                result.push(l as char);
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_USER_ID: &str = "user-1";

    // -- parse_callback_query ------------------------------------------------

    #[test]
    fn parse_valid_callback_query() {
        let request = "GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\nHost: localhost\r\n";
        let (code, state) = parse_callback_query(request).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn parse_callback_query_reversed_params() {
        let request = "GET /callback?state=s1&code=c1 HTTP/1.1\r\n";
        let (code, state) = parse_callback_query(request).unwrap();
        assert_eq!(code, "c1");
        assert_eq!(state, "s1");
    }

    #[test]
    fn parse_callback_query_with_extra_params() {
        let request = "GET /callback?code=c&foo=bar&state=s HTTP/1.1\r\n";
        let (code, state) = parse_callback_query(request).unwrap();
        assert_eq!(code, "c");
        assert_eq!(state, "s");
    }

    #[test]
    fn parse_callback_query_missing_code() {
        let request = "GET /callback?state=s HTTP/1.1\r\n";
        let err = parse_callback_query(request).unwrap_err();
        assert!(err.to_string().contains("Missing 'code'"));
    }

    #[test]
    fn parse_callback_query_missing_state() {
        let request = "GET /callback?code=c HTTP/1.1\r\n";
        let err = parse_callback_query(request).unwrap_err();
        assert!(err.to_string().contains("Missing 'state'"));
    }

    #[test]
    fn parse_callback_query_no_query_string() {
        let request = "GET /callback HTTP/1.1\r\n";
        let err = parse_callback_query(request).unwrap_err();
        assert!(err.to_string().contains("No query parameters"));
    }

    #[test]
    fn parse_callback_query_empty_request() {
        let err = parse_callback_query("").unwrap_err();
        assert!(err.to_string().contains("Empty HTTP request"));
    }

    // -- url_decode ----------------------------------------------------------

    #[test]
    fn url_decode_no_encoding() {
        assert_eq!(url_decode("hello"), "hello");
    }

    #[test]
    fn url_decode_percent_encoded() {
        assert_eq!(url_decode("hello%20world"), "hello world");
    }

    #[test]
    fn url_decode_plus_sign() {
        assert_eq!(url_decode("hello+world"), "hello world");
    }

    #[test]
    fn url_decode_special_characters() {
        assert_eq!(url_decode("%3D%26%3F"), "=&?");
    }

    #[test]
    fn url_decode_mixed() {
        assert_eq!(url_decode("a%20b+c%3Dd"), "a b c=d");
    }

    // -- McpOAuthService construction ----------------------------------------

    #[test]
    fn service_clone_is_independent() {
        let repo: Arc<dyn IOAuthTokenRepository> = Arc::new(MockTokenRepo);
        let http = reqwest::Client::new();
        let svc = McpOAuthService::new(repo, http);
        let _clone = svc.clone();
    }

    #[tokio::test]
    async fn clear_pending_for_user_keeps_other_user_same_oauth_state() {
        let svc = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        insert_pending_login(&svc, "user-a", "shared-state").await;
        insert_pending_login(&svc, "user-b", "shared-state").await;

        svc.clear_pending_for_user("user-a").await;

        let pending = svc.pending.lock().await;
        assert!(!pending.contains_key(&("user-a".to_string(), "shared-state".to_string())));
        assert!(pending.contains_key(&("user-b".to_string(), "shared-state".to_string())));
    }

    // -- Mock repositories ---------------------------------------------------

    async fn insert_pending_login(svc: &McpOAuthService, user_id: &str, state: &str) {
        let (_, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut pending = svc.pending.lock().await;
        pending.insert(
            (user_id.to_string(), state.to_string()),
            PendingLogin {
                csrf_token: CsrfToken::new(state.to_string()),
                pkce_verifier,
                auth_url: "https://auth.example.com/authorize".to_string(),
                token_url: "https://auth.example.com/token".to_string(),
                redirect_url: "http://127.0.0.1/callback".to_string(),
                client_info: ClientInfo {
                    client_id: "one".into(),
                    issuer: None,
                    resource: "https://example.com".into(),
                },
                cancelled: Arc::new(Notify::new()),
            },
        );
    }

    struct MockTokenRepo;

    #[async_trait::async_trait]
    impl IOAuthTokenRepository for MockTokenRepo {
        async fn get_by_url(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<dream_core_db::models::OAuthTokenRow>, dream_core_db::DbError> {
            Ok(None)
        }

        async fn upsert(
            &self,
            _: UpsertOAuthTokenParams<'_>,
        ) -> Result<dream_core_db::models::OAuthTokenRow, dream_core_db::DbError> {
            unimplemented!()
        }

        async fn delete(&self, _: &str, _: &str) -> Result<(), dream_core_db::DbError> {
            Ok(())
        }

        async fn list_authenticated_urls(&self, _: &str) -> Result<Vec<String>, dream_core_db::DbError> {
            Ok(vec![])
        }
    }

    struct IdempotentDeleteRepo;

    #[async_trait::async_trait]
    impl IOAuthTokenRepository for IdempotentDeleteRepo {
        async fn get_by_url(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<dream_core_db::models::OAuthTokenRow>, dream_core_db::DbError> {
            Ok(None)
        }

        async fn upsert(
            &self,
            _: UpsertOAuthTokenParams<'_>,
        ) -> Result<dream_core_db::models::OAuthTokenRow, dream_core_db::DbError> {
            unimplemented!()
        }

        async fn delete(&self, _: &str, url: &str) -> Result<(), dream_core_db::DbError> {
            Err(dream_core_db::DbError::NotFound(format!(
                "OAuth token for '{url}' not found"
            )))
        }

        async fn list_authenticated_urls(&self, _: &str) -> Result<Vec<String>, dream_core_db::DbError> {
            Ok(vec![])
        }
    }

    struct ValidTokenRepo;

    #[async_trait::async_trait]
    impl IOAuthTokenRepository for ValidTokenRepo {
        async fn get_by_url(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<dream_core_db::models::OAuthTokenRow>, dream_core_db::DbError> {
            Ok(Some(dream_core_db::models::OAuthTokenRow {
                user_id: TEST_USER_ID.to_string(),
                server_url: "https://example.com".to_string(),
                access_token: "valid_access_token".to_string(),
                refresh_token: None,
                client_info: None,
                token_type: "bearer".to_string(),
                expires_at: Some(now_ms() + 3_600_000),
                created_at: now_ms(),
                updated_at: now_ms(),
            }))
        }

        async fn upsert(
            &self,
            _: UpsertOAuthTokenParams<'_>,
        ) -> Result<dream_core_db::models::OAuthTokenRow, dream_core_db::DbError> {
            unimplemented!()
        }

        async fn delete(&self, _: &str, _: &str) -> Result<(), dream_core_db::DbError> {
            Ok(())
        }

        async fn list_authenticated_urls(&self, _: &str) -> Result<Vec<String>, dream_core_db::DbError> {
            Ok(vec!["https://example.com".to_string()])
        }
    }

    struct ExpiredTokenRepo;

    #[async_trait::async_trait]
    impl IOAuthTokenRepository for ExpiredTokenRepo {
        async fn get_by_url(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<dream_core_db::models::OAuthTokenRow>, dream_core_db::DbError> {
            Ok(Some(dream_core_db::models::OAuthTokenRow {
                user_id: TEST_USER_ID.to_string(),
                server_url: "https://example.com".to_string(),
                access_token: "expired_token".to_string(),
                refresh_token: None,
                client_info: None,
                token_type: "bearer".to_string(),
                expires_at: Some(1000),
                created_at: 500,
                updated_at: 500,
            }))
        }

        async fn upsert(
            &self,
            _: UpsertOAuthTokenParams<'_>,
        ) -> Result<dream_core_db::models::OAuthTokenRow, dream_core_db::DbError> {
            unimplemented!()
        }

        async fn delete(&self, _: &str, _: &str) -> Result<(), dream_core_db::DbError> {
            Ok(())
        }

        async fn list_authenticated_urls(&self, _: &str) -> Result<Vec<String>, dream_core_db::DbError> {
            Ok(vec![])
        }
    }

    struct NoExpiryTokenRepo;

    #[async_trait::async_trait]
    impl IOAuthTokenRepository for NoExpiryTokenRepo {
        async fn get_by_url(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<dream_core_db::models::OAuthTokenRow>, dream_core_db::DbError> {
            Ok(Some(dream_core_db::models::OAuthTokenRow {
                user_id: TEST_USER_ID.to_string(),
                server_url: "https://example.com".to_string(),
                access_token: "no_expiry_token".to_string(),
                refresh_token: None,
                client_info: None,
                token_type: "bearer".to_string(),
                expires_at: None,
                created_at: now_ms(),
                updated_at: now_ms(),
            }))
        }

        async fn upsert(
            &self,
            _: UpsertOAuthTokenParams<'_>,
        ) -> Result<dream_core_db::models::OAuthTokenRow, dream_core_db::DbError> {
            unimplemented!()
        }

        async fn delete(&self, _: &str, _: &str) -> Result<(), dream_core_db::DbError> {
            Ok(())
        }

        async fn list_authenticated_urls(&self, _: &str) -> Result<Vec<String>, dream_core_db::DbError> {
            Ok(vec!["https://example.com".to_string()])
        }
    }

    #[tokio::test]
    async fn concurrent_login_returns_one_url_without_reopening_or_cross_user_deduplication() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let endpoint = base.clone();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = requests.clone();
        let fixture = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                let read = socket.read(&mut request).await.unwrap();
                assert!(read > 0);
                if !String::from_utf8_lossy(&request[..read]).contains("/.well-known/oauth-authorization-server ") {
                    socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await
                        .unwrap();
                    continue;
                }
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = serde_json::json!({"authorization_endpoint":format!("{endpoint}/authorize"),"token_endpoint":format!("{endpoint}/token")}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let service = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        let mut calls = Vec::new();
        for _ in 0..16 {
            let service = service.clone();
            let base = base.clone();
            calls.push(tokio::spawn(
                async move { service.login("alice", &base).await.unwrap() },
            ));
        }
        let mut urls = Vec::new();
        for call in calls {
            let result = call.await.unwrap();
            assert!(result.pending);
            if let Some(url) = result.authorization_url {
                urls.push(url);
            }
        }
        assert_eq!(urls.len(), 1);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(service.login("bob", &base).await.unwrap().authorization_url.is_some());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        let url = reqwest::Url::parse(&urls[0]).unwrap();
        let redirect = url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let response = reqwest::get(format!("{redirect}?code=denied&state=wrong")).await;
        assert!(response.map_or(true, |response| !response.status().is_success()));
        assert!(service.login("alice", &base).await.unwrap().authorization_url.is_none());
        fixture.abort();
    }

    // -- Service behavior tests ----------------------------------------------

    #[tokio::test]
    async fn check_status_no_token_returns_false() {
        let svc = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        let status = svc
            .check_oauth_status(TEST_USER_ID, "https://example.com")
            .await
            .unwrap();
        assert!(!status.authenticated);
    }

    #[tokio::test]
    async fn check_status_with_valid_token() {
        let svc = McpOAuthService::new(Arc::new(ValidTokenRepo), reqwest::Client::new());
        let status = svc
            .check_oauth_status(TEST_USER_ID, "https://example.com")
            .await
            .unwrap();
        assert!(status.authenticated);
    }

    #[tokio::test]
    async fn check_status_with_expired_token() {
        let svc = McpOAuthService::new(Arc::new(ExpiredTokenRepo), reqwest::Client::new());
        let status = svc
            .check_oauth_status(TEST_USER_ID, "https://example.com")
            .await
            .unwrap();
        assert!(!status.authenticated);
    }

    #[tokio::test]
    async fn check_status_no_expiry_treated_as_valid() {
        let svc = McpOAuthService::new(Arc::new(NoExpiryTokenRepo), reqwest::Client::new());
        let status = svc
            .check_oauth_status(TEST_USER_ID, "https://example.com")
            .await
            .unwrap();
        assert!(status.authenticated);
    }

    #[tokio::test]
    async fn logout_idempotent_for_nonexistent() {
        let svc = McpOAuthService::new(Arc::new(IdempotentDeleteRepo), reqwest::Client::new());
        svc.logout(TEST_USER_ID, "https://nonexistent.example.com")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_authenticated_servers_empty() {
        let svc = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        let urls = svc.get_authenticated_servers(TEST_USER_ID).await.unwrap();
        assert!(urls.is_empty());
    }

    #[tokio::test]
    async fn get_authenticated_servers_returns_urls() {
        let svc = McpOAuthService::new(Arc::new(ValidTokenRepo), reqwest::Client::new());
        let urls = svc.get_authenticated_servers(TEST_USER_ID).await.unwrap();
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[tokio::test]
    async fn get_token_returns_none_when_no_token() {
        let svc = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        let token = svc.get_token(TEST_USER_ID, "https://example.com").await.unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn get_token_returns_access_token() {
        let svc = McpOAuthService::new(Arc::new(ValidTokenRepo), reqwest::Client::new());
        let token = svc.get_token(TEST_USER_ID, "https://example.com").await.unwrap();
        assert_eq!(token.as_deref(), Some("valid_access_token"));
    }

    #[tokio::test]
    async fn get_token_rejects_expired_when_no_refresh() {
        let svc = McpOAuthService::new(Arc::new(ExpiredTokenRepo), reqwest::Client::new());
        // An expired credential must never be sent to the MCP server.
        let token = svc.get_token(TEST_USER_ID, "https://example.com").await.unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn discovery_failure_can_be_explicitly_retried_immediately() {
        let service = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        assert!(service.login(TEST_USER_ID, "not a URL").await.is_err());
        assert!(service.login(TEST_USER_ID, "not a URL").await.is_err());
        assert!(service.login_attempts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_discovery_releases_the_login_attempt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let service = McpOAuthService::new(Arc::new(MockTokenRepo), reqwest::Client::new());
        let task_service = service.clone();
        let task = tokio::spawn(async move { task_service.login(TEST_USER_ID, &url).await });
        let (_connection, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(service.login_attempts.lock().unwrap().is_empty());
    }
}
