# Architecture

dream-core is the local backend server for the One Work platform, built with Rust (Axum + Tokio + SQLite).
It provides HTTP REST APIs and WebSocket real-time events for the dream-ui desktop client, compiling to a single `dreamcore` executable.

## Tech Stack

| Component | Technology |
|-----------|------------|
| Web framework | Axum 0.8 |
| Async runtime | Tokio |
| Database | SQLite (via sqlx, async) |
| Authentication | JWT + CSRF (Double Submit Cookie) |
| Real-time | WebSocket + event broadcasting |

## High-Level Architecture

```
┌─────────────────────────────────────────────────┐
│                  dream-core-app                      │
│         (binary entry, router assembly)          │
├──────────┬──────────┬──────────┬────────────────┤
│conversa- │ channel  │  team    │  ... (domain)  │
│  tion    │          │          │                 │
├──────────┴──────────┴──────────┴────────────────┤
│   dream-core-auth          dream-core-realtime           │
│  (JWT, CSRF, middleware) (WebSocket, events)     │
├─────────────────────────────────────────────────┤
│  dream-core-db    dream-core-api-types   dream-core-runtime  │
│ (repositories) (API contracts)  (runtime/process)│
├─────────────────────────────────────────────────┤
│       dream-core-common          dream-core-assets       │
│  (error types, enums, crypto)  (embedded data)   │
└─────────────────────────────────────────────────┘
```

Dependencies flow strictly downward. Domain crates must not depend on dream-core-app,
and dream-core-common has zero internal dependencies.

## Crate Hierarchy

The project is organized as a Cargo workspace with 33 crates. They belong to two independent naming lineages:

- **`dream-core-*`** (26 crates, formerly `aionui-*`) — the product core: conversations, channels, team collaboration, files, Office, MCP, etc. This is the whole surface of the personal/open-source edition.
- **`dream-domain-*`** (7 crates, formerly `one-*`) — the enterprise/commercialization layer: org tenants, digital employees, billing, SSO, the enterprise-org dimension, deployment reservations, the requirements/DevOps board. **Not part of the original AionCore lineage** — it's a separate family added later, layered on top of `dream-core-*` in the dependency graph (see "Enterprise" below).

The four foundational layers below are the pre-existing `dream-core-*` layering; Enterprise is a new fifth layer inserted between Domain and Composition.

### Foundation

Depended on by nearly all other crates. Changes require careful impact assessment.

| Crate | Responsibility |
|-------|----------------|
| `dream-core-common` | Shared error types (ApiError), enums, ID generation, crypto utilities, timestamps, pagination |
| `dream-core-api-types` | All HTTP/WebSocket request and response types — the single source of truth for API contracts |
| `dream-core-db` | SQLite database layer, defines Repository traits and implementations |
| `dream-core-assets` | Embedded static assets (agent metadata, prompts) |
| `dream-core-runtime` | Managed Node, subprocess spawning, PATH enhancement |
| `dream-core-process` | Supervised subprocess lifecycle, containment, and startup cleanup for direct CLI sessions |

### Capability

Cross-cutting capabilities used by domain crates.

| Crate | Responsibility |
|-------|----------------|
| `dream-core-auth` | JWT authentication, password hashing, CSRF protection, cookie management, auth middleware |
| `dream-core-realtime` | WebSocket connection management, event broadcasting (BroadcastEventBus), message routing |

### Domain

Each crate owns an independent business domain. They remain loosely coupled from each other.

| Crate | Responsibility |
|-------|----------------|
| `dream-core-conversation` | Conversation management, messaging, confirmations, streaming responses |
| `dream-core-session` | Unified session state, capabilities, commands, and events across direct CLI and ACP backends |
| `dream-core-channel` | Multi-channel integration (WeChat, DingTalk, Lark), plugin system, pairing sessions |
| `dream-core-team` | Team collaboration, task scheduling, mailbox system |
| `dream-core-team-prompts` | Shared team role prompts, governance rules, and tool authorization metadata |
| `dream-core-cron` | Scheduled job execution, cron expressions, event triggering |
| `dream-core-file` | File operations, watching, snapshots, git operations, compression |
| `dream-core-project` | Project/folder bindings, Project Explorer, filesystem monitoring, and resource containment |
| `dream-core-office` | Office document handling (Excel, PPT, Word), preview, conversion |
| `dream-core-system` | System settings, provider management, version checking, model fetching |
| `dream-core-mcp` | MCP protocol integration, OAuth, multi-platform adapters |
| `dream-core-ai-agent` | Agent lifecycle management, worker task queues, ACP/auxiliary skills |
| `dream-core-extension` | Extension registry, hub management, skill discovery and installation |
| `dream-core-shell` | Shell command execution, speech-to-text |
| `dream-core-assistant` | Assistant configuration and management |
| `dream-core-claude-bridge` | Settings-only storage for the Claude Code custom-provider bridge (no protocol translation needed; reuses the litellm-internal gateway) |
| `dream-core-codex-bridge` | Local OpenAI Responses API compatibility bridge for Codex CLI (`wire_api="responses"` translation layer) |

### Enterprise

The former `one-*` family, 7 crates, forming the enterprise/commercialization surface. Follows the same conventions as the `dream-core-*` domain crates: all state lives in its own migration-ledger-managed `one_*` tables, and the only touch points with upstream (`dream-core-*`) are a single route merge in `dream-core-app`, plus read-only use of `dream-core-auth`/`dream-core-db` public APIs.

| Crate | Responsibility |
|-------|----------------|
| `dream-domain-org` | Enterprise tenants (project groups) / membership / invite codes / RBAC |
| `dream-domain-employee` | Digital employee definitions and run orchestration (depends on `dream-core-ai-agent`/`dream-core-conversation`/`dream-core-cron`/`dream-core-team`) |
| `dream-domain-billing` | Billing plane: subscription tier, seat cap enforcement, per-turn usage metering, plus a stubbed payment-provider seam (`BillingProvider`) |
| `dream-domain-sso` | SSO providers (Feishu/DingTalk/WeCom/LDAP), OAuth callbacks, JIT user provisioning |
| `dream-domain-platform` | Deployment/platform infrastructure config reservations: containerized execution, realtime collaboration (both "reserved adapter" patterns, defaulting to a Noop implementation) |
| `dream-domain-devops` | Requirements board (issues) + enterprise collaboration registries (skills/MCP/RAG document metadata); depends on `dream-domain-employee` |
| `dream-domain-enterprise` | The real enterprise-org dimension (department/title/membership) — orthogonal to `dream-domain-org`'s project-group tenants, its own independent set of tables |

There is one internal dependency within the Enterprise layer: `dream-domain-devops` → `dream-domain-employee`. Otherwise every Enterprise crate only depends on Foundation (`dream-core-common`/`dream-core-db`/`dream-core-api-types`/`dream-core-auth`); `dream-domain-employee` additionally depends on several `dream-core-*` Domain crates. **No `dream-core-*` crate depends back on `dream-domain-*`** — the Enterprise layer is purely additive, and the personal/open-source edition never needs to compile it in.

### Composition

| Crate | Responsibility |
|-------|----------------|
| `dream-core-app` | Top-level binary entry point (`dreamcore`), assembles all crates into the Axum server |

### Dependency Direction Rules

```
Composition → Enterprise → Domain → Capability → Foundation
              Enterprise → Foundation (cross-layer allowed)
              Domain → Foundation (cross-layer allowed)
```

- ✅ Upper layers may depend on lower layers
- ✅ Same-layer interaction through trait abstractions (e.g., conversation uses ai-agent capability via IWorkerTaskManager trait)
- ❌ Lower layers must not depend on upper layers
- ❌ Circular dependencies are forbidden

## Domain Crate Anatomy

Every domain crate follows a consistent internal organization. Using dream-core-conversation as a reference:

### Standard Directory Structure

```
crates/dream-core-conversation/src/
├── lib.rs       # Module exports, defines the crate's public API
├── routes.rs    # HTTP route handlers
├── service.rs   # Business logic layer
├── state.rs     # RouterState struct (holds services and dependencies)
├── error.rs     # Domain-specific error types (optional)
├── types.rs     # Domain models (optional)
└── [modules]    # Feature-specific submodules (e.g., streaming.rs)
```

### File Responsibilities

**lib.rs** — Crate entry point, only module declarations and public API exports:
- Exports the `domain_routes()` function
- Exports `Service` and `RouterState`
- Contains no business logic

**routes.rs** — HTTP route definitions and handler functions:
- Exports a single `domain_routes(state: RouterState) -> Router` function
- Each handler: extract parameters → call service → construct response
- Handlers contain no business logic, only request/response transformation

**service.rs** — The sole location for business logic:
- Dependencies injected via constructor (Repository trait objects, EventBroadcaster, etc.)
- All business rules, validation, and orchestration logic lives here
- Does not import axum or touch HTTP types directly

**state.rs** — Router state, the carrier for dependency injection:
- Holds service instances and Arc references to other dependencies
- Implements Clone (required by Axum)

### Handler Signature Convention

```rust
async fn handler(
    State(state): State<RouterState>,       // Dependency injection
    Extension(user): Extension<CurrentUser>, // Authenticated user
    Path(id): Path<String>,                  // Path parameter
    Json(body): Json<RequestType>,           // Request body
) -> Result<(StatusCode, Json<ApiResponse<ResponseType>>), ApiError>
```

### When to Create a New Crate vs. Extend an Existing One

**Create a new crate when:**
- It represents an independent business domain (with its own data models and lifecycle)
- It needs an independent route prefix (e.g., `/api/new-domain/...`)
- It has no strong coupling with existing domains

**Extend an existing crate when:**
- The feature is a sub-feature of an existing domain
- It shares the same data models
- Routes are sub-paths of an existing prefix

## API Conventions

### RESTful Path Naming

```
/api/{resources}                   # Collection operations (GET list, POST create)
/api/{resources}/{id}              # Item operations (GET detail, PATCH update, DELETE)
/api/{resources}/{id}/{subresources} # Nested resources
/api/{resources}/{id}/{action}     # Action operations (only when CRUD cannot express it)
```

Rules:
- Always use the `/api/` prefix
- Resource names and path segments use kebab-case (e.g., `ai-agents`, `qr-login`)
- Action routes use verbs or verb phrases (e.g., `reset`, `stop`, `run`)

### Unified Response Format

**Success response (`ApiResponse<T>`):**
```json
{
  "success": true,
  "data": { ... },
  "message": "optional message"
}
```
Both `data` and `message` are optional fields, omitted from serialization when null.

**Error response (`ErrorResponse`):**
```json
{
  "success": false,
  "error": "Human-readable error message",
  "code": "ERROR_CODE"
}
```

All response types are defined in `dream-core-api-types` — the single source of truth for API contracts.

### HTTP Status Code Mapping

| ApiError Variant | Status Code | Error Code | Use Case |
|------------------|-------------|------------|----------|
| BadRequest | 400 | BAD_REQUEST | Invalid request parameters |
| Unauthorized | 401 | UNAUTHORIZED | Not authenticated or token expired |
| Forbidden | 403 | FORBIDDEN | No permission to access |
| NotFound | 404 | NOT_FOUND | Resource does not exist |
| Conflict | 409 | CONFLICT | Resource conflict |
| UnprocessableEntity | 422 | UNPROCESSABLE_ENTITY | Semantic error |
| RateLimited | 429 | RATE_LIMITED | Request rate exceeded |
| Internal | 500 | INTERNAL_ERROR | Internal server error |
| BadGateway | 502 | BAD_GATEWAY | Upstream service failure |
| Timeout | 502 | TIMEOUT | Upstream service timeout |

### Pagination

Uses offset-based pagination (`PaginatedResult<T>`):

```json
{
  "items": [...],
  "total": 100,
  "hasMore": true
}
```

Field descriptions:
- `items` — Current page data
- `total` — Total record count
- `hasMore` — Whether more data is available

Note: JSON field names use camelCase (via `#[serde(rename_all = "camelCase")]`).

### WebSocket Event Conventions

**Entry point:** Single `/ws` endpoint

**Message format (`WebSocketMessage<T>`):**
```json
{
  "name": "domain.actionName",
  "data": { ... }
}
```

**Event naming convention:**
- Format: `{domain}.{actionName}`, two-level structure
- domain uses camelCase (e.g., `conversation`, `fileWatch`)
- actionName uses camelCase (e.g., `listChanged`, `statusChanged`)
- Examples: `conversation.listChanged`, `cron.jobExecuted`, `extensions.stateChanged`

⚠️ **Legacy note:** Some existing events use kebab-case (e.g., `channel.pairing-requested`)
or three-level naming (e.g., `team.agent.status`). These are historical artifacts.
New events must follow the two-level camelCase convention above.
Existing inconsistencies will be unified incrementally during related module iterations.

### ACP Tool Output Sanitization

ACP agent tool-call events enter the unified `AgentStreamEvent` stream through
the `dream-core-ai-agent` translation boundary. That boundary keeps WebSocket and
SQLite message payloads bounded instead of forwarding large binary or inline
base64 results unchanged.

For example, Codex image generation may return both `saved_path` and PNG/JPEG/WebP
base64 in `raw_output.result`. Before forwarding and persistence, the translation
layer removes the inline `result` while retaining small structured fields such as
`saved_path`, `image.path`, `result_omitted`, and `result_bytes`. When the image is
already stored on disk, the tool status is normalized to `completed`. Clients load
the image from its file path on demand and must not depend on inline base64.

## Data Layer

### Repository Trait Pattern

All database access goes through trait abstractions defined in `dream-core-db`:

```rust
#[async_trait]
pub trait IConversationRepository: Send + Sync {
    async fn get(&self, id: &str) -> Result<Option<ConversationRow>, DbError>;
    async fn create(&self, row: &ConversationRow) -> Result<(), DbError>;
    async fn update(&self, id: &str, params: &UpdateConversationParams) -> Result<(), DbError>;
    async fn delete(&self, id: &str) -> Result<(), DbError>;
    // ...
}
```

Rules:
- Each domain entity has a corresponding Repository trait (e.g., `IConversationRepository`, `IUserRepository`)
- Trait names are prefixed with `I` to denote an interface
- Concrete implementations use the `Sqlite` prefix (e.g., `SqliteConversationRepository`)
- Service layer depends only on traits, never on concrete implementations

### Type Distribution

The project has three categories of data types, each with its own home:

| Type | Location | Purpose | Example |
|------|----------|---------|---------|
| Row models | `dream-core-db/src/models/` | Database row mapping | `ConversationRow` |
| Params objects | `dream-core-db/src/repository/` | Database write parameters | `UpdateConversationParams` |
| Request/response types | `dream-core-api-types` | API contracts and shared DTOs | `CreateConversationRequest`, `ConversationResponse` |

**The service layer may directly use types from `dream-core-api-types`.** This crate contains
pure data structure definitions with no HTTP framework dependencies, essentially serving as a shared DTO layer.

⚠️ **Critical constraint: `dream-core-api-types` must not depend on axum, tower, or any HTTP framework.
Only serde and basic type dependencies are allowed.** This is the prerequisite for services to safely use it.

### Responsibility Boundaries

- **Handler (routes.rs):** Request validation, parameter extraction, error mapping, constructing `ApiResponse`
- **Service (service.rs):** Business logic, rule validation, orchestrating Repository calls, Row ↔ Response conversion
- **Repository (dream-core-db):** Pure database operations, no business logic

The boundary between Handler and Service is defined by **responsibility**, not by types —
Handlers do not make business decisions, Services do not handle HTTP concerns.

### Migration Management

Using sqlx's embedded migrations (`sqlx::migrate!()`):
- Migration files are located in `crates/dream-core-db/migrations/`
- Naming format: `NNN_descriptive_name.sql` (sequential numbering)
- Migrations run automatically on application startup
- New tables or schema changes must go through migration files — manual database modifications are forbidden
- Use `IF NOT EXISTS` to ensure idempotency

### Error Propagation

```
DbError (database layer)
  ↓ From trait implementation (dream-core-db/src/error.rs)
ApiError (unified error type)
  ↓ IntoResponse implementation
HTTP response (status code + ErrorResponse JSON)
```

Mapping rules:
- `DbError::NotFound` → `ApiError::NotFound` (preserves semantics)
- `DbError::Conflict` → `ApiError::Conflict` (preserves semantics)
- `DbError::Query` / `Migration` / `Init` → `ApiError::Internal` (hides internal details)

## Dependency Injection

### Injection Chain

The application uses Axum's `with_state()` pattern for dependency injection in three steps:

**Step 1: Centralized service construction (AppServices)**

`dream-core-app` defines `AppServices`, which holds all shared dependencies centrally:

```rust
pub struct AppServices {
    pub database: Database,
    pub jwt_service: Arc<JwtService>,
    pub user_repo: Arc<dyn IUserRepository>,
    pub cookie_config: Arc<CookieConfig>,
    pub qr_token_store: Arc<QrTokenStore>,
    pub ws_manager: Arc<WebSocketManager>,
    pub event_bus: Arc<BroadcastEventBus>,
    pub worker_task_manager: Arc<dyn IWorkerTaskManager>,
    pub agent_registry: Arc<AgentRegistry>,
    pub conversation_repo: Arc<dyn IConversationRepository>,
    pub acp_session_sync: Arc<AcpSessionSyncService>,
    pub jwt_secret_raw: String,
    pub data_dir: String,
    pub local: bool,
    pub app_version: String,
    pub skill_paths: Arc<SkillPaths>,
    pub guide_mcp_config: Option<GuideMcpConfig>,
    // ...
}
```

**Step 2: Build RouterState per domain**

`build_module_states()` constructs all domain RouterStates from `AppServices`.
Each domain receives only the dependencies it needs:

```rust
// Simple domain — only needs one service
pub struct CronRouterState {
    pub cron_service: Arc<CronService>,
}

// Complex domain — needs multiple services
pub struct OfficeRouterState {
    pub watch_manager: Arc<OfficecliWatchManager>,
    pub snapshot_service: Arc<SnapshotService>,
    pub conversion_service: Arc<ConversionService>,
    pub proxy_service: Arc<ProxyService>,
}
```

All RouterStates are `#[derive(Clone)]` and hold Arc-wrapped dependencies.

**Step 3: Handlers extract dependencies via State**

```rust
async fn create(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    body: Result<Json<CreateConversationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ApiResponse<ConversationResponse>>), ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let conversation = state.conversation_service.create(&user.id, req).await?;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(conversation))))
}
```

### Router Assembly

Router assembly is done through three layered functions:

1. `create_router()` — Async entry point, builds all states then calls the next layer
2. `create_router_with_states()` — Allows custom ModuleStates (useful for testing)
3. `create_router_with_all_state()` — Final assembly, merges all routes and middleware

Middleware stack (outermost to innermost):

```
CORS (local mode only)
  → Security Headers (all requests)
    → CSRF (non-local mode only)
      → Auth Middleware (selectively applied per route group)
        → Handler
```

Key points:
- Auth middleware is not global — it is selectively applied per route group via `route_layer()`
- Public routes (login, status check) do not have auth middleware attached
- The WebSocket `/ws` route does not use HTTP auth middleware — it uses independent token validation callbacks
- In local mode, CSRF checking is skipped and a default system user is injected

### Rules

- **AppServices is the sole service construction center** — all Repository instantiation and Service assembly happens here
- **RouterState holds only necessary dependencies** — each domain's State includes only the services it uses
- **Dependencies are passed via `Arc<dyn Trait>`** — enables runtime polymorphism and test substitution
- **Domain crates do not construct their own dependencies** — they only define what they need (RouterState), `dream-core-app` handles assembly

## Security Model

### Middleware Stack (Outermost to Innermost)

```
CORS (local mode only, allows any origin)
  → Security Headers
      X-Frame-Options: DENY
      X-Content-Type-Options: nosniff
      X-XSS-Protection: 1; mode=block
      Referrer-Policy: strict-origin-when-cross-origin
    → CSRF (non-local mode only, Double Submit Cookie)
      → Auth Middleware (selectively applied per route group)
        → Handler
```

### JWT Authentication

- Algorithm: HMAC-SHA256
- Validity: 24 hours
- Payload: `user_id`, `username`, `iat`, `exp`, `iss` ("dream"), `aud` ("dream-webui")
- Secret source priority: environment variable → database → random generation (64 bytes, getrandom)
- Token extraction priority: `Authorization: Bearer` header → `dream-session` cookie
- Supports token blacklist (SHA-256 hash, DashMap storage)

### CSRF Protection

Uses the Double Submit Cookie pattern:
- Cookie name: `dream-csrf-token` (not HttpOnly — JavaScript must read it)
- Request header: `x-csrf-token`
- Validation: cookie value must exactly match header value
- Safe methods (GET, HEAD, OPTIONS) bypass validation
- Exempt paths: `/login`, `/api/auth/qr-login`

### Password Security

- Algorithm: bcrypt, cost factor 12
- Timing attack protection: minimum 50ms response time
- User enumeration protection: uses pre-computed dummy hash when user does not exist

### Cookie Configuration

| Cookie | HttpOnly | Secure | SameSite | Max-Age |
|--------|----------|--------|----------|---------|
| `dream-session` | ✅ | When HTTPS | Strict(HTTPS) / Lax(HTTP) | 30 days |
| `dream-csrf-token` | ❌ | When HTTPS | Strict(HTTPS) / Lax(HTTP) | 30 days |

### Rate Limiting

| Level | Limit | Window | Scope | Key |
|-------|-------|--------|-------|-----|
| Auth | 5 failures | 15 minutes | Login routes | Client IP |
| API | 60 requests | 1 minute | Public endpoints | Client IP |
| Action | 20 requests | 1 minute | Sensitive operations | User ID (falls back to IP) |

IP extraction priority: `X-Forwarded-For` → `X-Real-IP` → "unknown"

### Local Mode

Enabled via the `--local` startup flag, designed for Electron embedded scenarios:
- Skips JWT verification, injects a fixed user (`system_default_user`)
- Skips CSRF checking
- Enables fully open CORS
- WebSocket is also exempt from authentication

### Security Rules

- New endpoints must be evaluated for auth middleware requirement
- State-changing operations (POST/PUT/DELETE/PATCH) must be CSRF-protected
- Sensitive operations should have rate limiting configured
- Error responses must not leak internal implementation details (DbError::Query maps to generic Internal)
- Secrets must never be hardcoded in source code

## Testing Strategy

### Test Layers

| Layer | Location | Database Strategy | Purpose |
|-------|----------|-------------------|---------|
| Unit tests | `#[cfg(test)]` inline in each `.rs` file | None or Mock | Function-level logic verification |
| Integration tests | `crates/<crate>/tests/` | In-memory SQLite | Service and Repository behavior verification |
| E2E tests | `crates/dream-core-app/tests/` | In-memory SQLite | Full HTTP request chain verification |

### In-Memory Database

All tests requiring a database use `init_database_memory()`:
- Creates an SQLite in-memory database (`sqlite::memory:`)
- Single connection pool (`max_connections = 1`, ensures data consistency for in-memory DB)
- Automatically runs migrations
- Automatically creates the system default user (`system_default_user`)
- Each test gets an independent, fresh database instance

### Mock Strategy

**Prefer real in-memory databases. Mocks are only for isolating unneeded dependencies.**

- Integration and E2E tests: use real Sqlite implementations + in-memory database
- Unit tests: mock unrelated dependencies (e.g., `MockBroadcaster`, `MockConversationRepo`)
- Mock implementations use `Mutex<Vec<T>>` for in-memory storage with manual trait implementations

### E2E Test Pattern

`dream-core-app/tests/common/mod.rs` provides shared test utilities:

```rust
// Build the complete application
let (app, services) = build_app().await;

// Create a user and log in, obtaining auth credentials
let (token, csrf) = setup_and_login(&services, "testuser", "password").await;

// Make an authenticated request
let response = app.oneshot(
    get_with_token("/api/conversations", &token, &csrf)
).await;
```

Login flow:
1. Create user directly via Repository (bypassing the API)
2. GET `/api/auth/status` to extract the CSRF token
3. POST `/login` to obtain the session token
4. Subsequent requests carry `Authorization: Bearer` + `x-csrf-token` headers

### Test File Naming

| Suffix | Purpose | Example |
|--------|---------|---------|
| `*_test.rs` | Unit/functional tests | `extension_loading_test.rs` |
| `*_integration.rs` | Integration tests | `acp_agent_integration.rs` |
| `*_e2e.rs` | End-to-end tests | `auth_e2e.rs`, `conversation_e2e.rs` |

### Test Failure Handling Rules

When a test fails, do NOT modify the test to make it pass. First determine:

1. **Test assertion still represents correct behavior** → fix the implementation, not the test
2. **Requirements or interface intentionally changed, test reflects old behavior** → may update the test, but must:
   - Confirm the change is intentional (not an unintended side effect)
   - Ensure new assertions still validate meaningful behavior
3. **Uncertain** → stop, trace back the change, clarify before proceeding

Prohibited:
- ❌ Deleting failing tests to "fix" the problem
- ❌ Weakening specific assertions to vague ones (e.g., `assert_eq!(status, 201)` → `assert!(status.is_success())`)

## Adding a New Feature

### When to Create a New Crate

**Create a new crate when:**
- It represents an independent business domain (with its own data models and lifecycle)
- It needs an independent route prefix (`/api/new-domain/...`)
- It has no strong coupling with existing domains

**Extend an existing crate when:**
- The feature is a sub-feature of an existing domain
- It shares the same data models
- Routes are sub-paths of an existing prefix

### Complete Steps for Creating a New Domain Crate

Using `dream-core-my-feature` as an example (for an enterprise/commercialization feature, follow the same steps with a `dream-domain-my-feature` prefix instead, placed in the Enterprise layer rather than Domain):

**Step 1: Create the crate and register it in the workspace**

1. Create the directory `crates/dream-core-my-feature/`
2. Add the workspace member in root `Cargo.toml`:
   ```toml
   members = [
       # ... existing members
       "crates/dream-core-my-feature",
   ]
   ```
3. Register in `[workspace.dependencies]` of root `Cargo.toml`:
   ```toml
   dream-core-my-feature = { path = "crates/dream-core-my-feature" }
   ```
4. Use `.workspace = true` for shared dependency versions within the crate

**Step 2: Write the crate following the standard structure**

```
crates/dream-core-my-feature/
├── Cargo.toml
├── src/
│   ├── lib.rs        # Export my_feature_routes, MyFeatureService, MyFeatureRouterState
│   ├── routes.rs     # pub fn my_feature_routes(state: ...) -> Router
│   ├── service.rs    # Business logic
│   └── state.rs      # #[derive(Clone)] pub struct MyFeatureRouterState { ... }
└── tests/
    └── my_feature_test.rs
```

**Step 3: If database access is needed, add to dream-core-db**

1. Add Row model in `models/`
2. Define Repository trait (`I` prefix) and Sqlite implementation in `repository/`
3. Add migration file in `migrations/` (`NNN_descriptive_name.sql`)

**Step 4: If API types are needed, add to dream-core-api-types**

Define request/response types in `dream-core-api-types` to keep API contracts centrally managed.

**Step 5: Wire into dream-core-app**

1. Add dependency in `dream-core-app/Cargo.toml`:
   ```toml
   dream-core-my-feature.workspace = true
   ```

2. Add field to `ModuleStates`:
   ```rust
   pub my_feature: MyFeatureRouterState,
   ```

3. Write the `build_my_feature_state()` function:
   ```rust
   pub fn build_my_feature_state(services: &AppServices) -> MyFeatureRouterState {
       let pool = services.database.pool().clone();
       let repo = Arc::new(SqliteMyFeatureRepository::new(pool));
       MyFeatureRouterState {
           my_feature_service: MyFeatureService::new(repo, services.event_bus.clone()),
       }
   }
   ```

4. Call it in `build_module_states()`:
   ```rust
   my_feature: build_my_feature_state(services),
   ```

5. Register routes in `create_router_with_all_state()`:
   ```rust
   let my_feature_authenticated = my_feature_routes(states.my_feature)
       .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
   
   let router = Router::new()
       // ... existing routes
       .merge(my_feature_authenticated)
       // ...
   ```

### Checklist

Before adding a new crate, confirm:
- [ ] Crate internal structure follows the standard pattern (lib/routes/service/state)
- [ ] Dependency direction is correct (does not depend on upper-layer or same-layer concrete implementations)
- [ ] Repository trait defined in dream-core-db, implementation uses Sqlite prefix
- [ ] API types defined in dream-core-api-types
- [ ] Routes use `/api/` prefix with kebab-case resource names
- [ ] Includes corresponding test files
- [ ] WebSocket events follow `domain.camelCaseAction` naming convention

## Runtime Infrastructure

### Managed Node Runtime

Builtin ACP adapters run through the managed Node runtime in
`crates/dream-core-runtime/src/node_runtime/`. Packaged builds activate Node
from the managed-resources bundle; download-mode builds install the pinned
runtime under `{data_dir}/runtime/node`. Adapter commands carry an explicit
Node executable and do not depend on the ambient `PATH`.

### Startup PATH Enhancement

`fn main()` calls `dream_core_runtime::enhance_process_path()` **before** the
tokio runtime starts, so every downstream `which::which(...)` and
`Command::new(...)` — including the existing spawn sites across the
workspace — inherits an enriched `PATH`. Three layers are merged in priority
order: interactive login-shell `$PATH` (Unix, 3 s timeout) → current inherited
PATH → platform fallback bins (`~/.cargo/bin`, `~/.local/bin`, version-manager
installations, Windows `%APPDATA%\npm`, Git, Scoop, …). The call is
`unsafe` because Rust 2024 requires a single-threaded precondition for
`env::set_var`; `main()` runs this as its very first statement to
satisfy the invariant. A `startup: PATH ready path_segments=… path_len=…`
info log confirms the enhancement at each run (no full PATH content is
logged at `info` level).

### Subprocess Spawn Builder

New subprocess spawn sites should go through
`dream_core_runtime::Builder::agent(program)` (for long-running agent CLIs
whose stdio the caller owns) or `dream_core_runtime::Builder::clean_cli(program)`
(for short-lived tools whose output we parse). Both set
`kill_on_drop(true)` and strip `NODE_OPTIONS`/`NODE_INSPECT`/`NODE_DEBUG`/
`CLAUDECODE` so debug-profile env doesn't leak into the child.
`clean_cli` additionally pipes stdio and sets `NO_COLOR=1` + `TERM=dumb`
to keep ANSI codes out of captured output.

Do NOT manually re-implement these behaviours with raw
`tokio::process::Command` — the centralised builder is the one place to
update policies (e.g. future `CARGO_*` cleanup, sandbox flags).
