# Dream Core — 迁移设置说明

对应旧仓库 `1oneCore`：本地 HTTP/WebSocket 服务、SQLite 领域后端，目标二进制名 `dreamcore`（取代 `aioncore`）。

这是一份迁移期间的说明文件，不是产品 README（本目录下的 `README.md`/`ARCHITECTURE.md` 是从 `1oneCore` 原样复制过来的，之后随改造逐步更新）。完整决策背景见 `D:\aionui-m0\DREAM-PLATFORM-DIRECTION.md`。

## 本次复制说明（2026-08-23）

源码已从 `D:\aionui-m0\1oneCore` 原样复制过来（不含改名），排除 `.git`、`target/`、`node_modules/`、`out/`、`dist/`、`coverage/`、`.turbo/`。复制过程干净，无文件丢失。

## Rust crate 映射（P0 初稿，共 34 个 package）

**26 个 `aionui-*` → `dream-core-*`**：`dream-core-ai-agent`、`dream-core-api-types`、`dream-core-app`（内含二进制 `aioncore` → `dreamcore`）、`dream-core-assets`、`dream-core-assistant`、`dream-core-auth`、`dream-core-channel`、`dream-core-claude-bridge`、`dream-core-codex-bridge`、`dream-core-common`、`dream-core-conversation`、`dream-core-cron`、`dream-core-db`、`dream-core-extension`、`dream-core-file`、`dream-core-mcp`、`dream-core-office`、`dream-core-process`、`dream-core-project`、`dream-core-realtime`、`dream-core-runtime`、`dream-core-session`、`dream-core-shell`、`dream-core-system`、`dream-core-team`、`dream-core-team-prompts`。

**7 个 `one-*` domain crate → `dream-domain-*`**：`dream-domain-billing`、`dream-domain-devops`、`dream-domain-employee`、`dream-domain-enterprise`、`dream-domain-org`、`dream-domain-platform`、`dream-domain-sso`。

⚠️ `dream-core-process`/`dream-core-mcp` 与 `dream-engine`（原 aionrs-local）里的 `aion-process`/`aion-mcp` 是真实存在的命名冲突，`dream-core-*` 与 `dream-engine-*` 前缀必须分开维护，不能简化成同名。

## 好消息：数据库表结构不需要因改名而改动

实测 SQL migration 里没有 `aion` 前缀的 `CREATE TABLE`，业务表统一是 `one_*` 前缀（1ONE 品牌层遗留，不是原始 `aion` 命名）。需要处理的只是表内 JSON 字段值/枚举字符串里可能残留的字面量，不需要重命名表结构本身。

## Cookie / HTTP 头（单一来源，低风险，可以较早改）

`crates/dream-core-common/src/constants.rs`：
```rust
pub const COOKIE_NAME: &str = "dream-core-session";       // → dream-session
pub const CSRF_COOKIE_NAME: &str = "aionui-csrf-token"; // → dream-csrf-token
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";
```

## 已完成（2026-08-23）

- [x] `cargo check --workspace` 通过（依赖已切换为 git 引用新推送的 `gaogg521/dream-engine` 仓库，`branch = "main"`）
- [x] 33 个 crate 目录 + Cargo.toml + 全部 `.rs` 模块路径改名：26 个 `aionui-*` → `dream-core-*`，7 个 `one-*` → `dream-domain-*`（`one-*` 用了词边界匹配，避免误伤 `one_tenants`/`one_enterprise_members` 这类数据库表名）
- [x] 二进制名 `aioncore` → `dreamcore`
- [x] 引用 dream-engine 的跨仓库 git 依赖（`aion-agent`/`aion-mcp` 等 6 个）已切到 `dream-engine-*` 新包名 + 新仓库地址
- [x] Cookie 常量 `COOKIE_NAME`/`CSRF_COOKIE_NAME` 已从 `aionui-session`/`aionui-csrf-token` 改为 `dream-session`/`dream-csrf-token`（单一来源 `dream-core-common/src/constants.rs`），相关测试断言与 `ARCHITECTURE.md`/`ARCHITECTURE.zh-CN.md` 文档同步更新
- [x] `factory/aionrs.rs`/`manager/aionrs/`/`adapters/aionrs.rs`/`adapters/aionui.rs` 等实现"跟 dream-engine 通信"和"host 自身"的模块文件已改名为 `dream_engine.rs`/`dream_engine/`/`dream_ui.rs`；`AgentType::Aionrs`、`AionrsAdapter`、`AionuiAdapter` 等 Rust 标识符已改为 `AgentType::DreamEngine`、`DreamEngineAdapter`、`DreamUiAdapter`
- [x] 5 处相对路径字符串字面量（`include_dir!("../aionui-app/...")` 等）已同步改为新目录名，这类字符串不会被模块路径改名自动捕获，是本轮唯一编译报错的原因
- [x] 尚未提交前做了 5 轮 `cargo check --workspace` 回归，全部通过

## ⚠️ 刻意不改的部分（按第 13 节原则，不是遗漏）

- `AgentType::DreamEngine => "aionrs"`（`dream-core-common/src/enums.rs`）——Rust 标识符已改，序列化到 `conversations.type` 列的字符串字面量刻意保留 `"aionrs"`，因为存量数据库已经写着这个值，改字符串需要走第 4.1 节的前向 migration，不能只改代码
- `ConversationSource::Aionui`（`conversation.source` 列）、`McpSource::Aionui` 同理，字符串字面量未动
- `JWT_ISSUER = "aionui"`、`JWT_AUDIENCE = "aionui-webui"`（`dream-core-auth/src/jwt.rs`）——改了会让所有已登录用户的 token 立刻失效，需要业务侧确认是否接受一次全局强制重新登录
- `dirs::cache_dir().join("aionui")` 等多处后端缓存/临时目录名——与 Electron 侧 userData 目录是同一类"不看品牌看身份延续"的问题，维持现状
- `DEFAULT_CLIENT_ID = "aionui"`（`dream-core-mcp/src/oauth_service.rs`）、扩展 manifest 里 `engine.aionui` 兼容字段——对应决策文档第 8 节尚未拍板的待决事项，不在本轮擅自处理
- `dream-domain-sso` 里深链 scheme 白名单默认值 `"aionui"`——按第 12 节已确认不改

## 已知仍有残留、留待后续处理（非本轮阻塞项）

`grep -rio aion` 在排除 `target/`、`migrations/`、`CHANGELOG.md`、`Cargo.lock` 后仍有约 4000+ 处，主要集中在：历史设计文档（`crates/dream-core-team/docs/phase1/*.md`）、内置 SKILL.md 内容、CLI `--help` 文案（`dream-core-app/src/cli.rs`）、部分测试文件里重复断言持久化字符串值。这些要么是本节"刻意不改"的范畴，要么是文档/文案层面的收尾工作，不影响编译和当前判定的功能正确性，建议作为独立的后续任务清理，不要和这次的 crate/模块重命名混在一起验收。

## 尚未做的事

- [ ] 尚未提交、尚未推送到 `https://github.com/gaogg521/dream-core.git`

## appId / 深链协议：已决定不改

决策文档第 12 节：`com.huanle.oneone.ai` / `aionui://` 维持现状，不生成 Dream 版本。dream-domain-sso 里 `desktop_callback_page` 拼的 `aionui://sso-callback?token=...` 深链不用改。
