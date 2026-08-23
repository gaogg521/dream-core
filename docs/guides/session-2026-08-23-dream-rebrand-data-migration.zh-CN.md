# Dream 品牌独立化：后端持久化数据迁移收尾

**日期**：2026-08-23 · **范围**：dream-core（本文档），完整的跨仓叙述（含前端 bug、
CodeMirror 排查、验证记录）见 dream-ui 的
[session-2026-08-23-dream-rebrand-data-migration.zh-CN.md](https://github.com/gaogg521/dream-ui/blob/main/docs/guides/session-2026-08-23-dream-rebrand-data-migration.zh-CN.md)
——本仓库这份文档只收录后端特有的、值得单独存档的细节，避免两边重复维护同一份内容。

## 一、本仓库改了什么（文件级）

| 文件 | 改动 |
| --- | --- |
| `crates/dream-core-common/src/enums.rs` | `AgentType::DreamEngine`/`ConversationSource::DreamUi`/`McpSource::DreamEngine`/`McpSource::DreamUi` 加 `#[serde(rename="dream", alias="旧值")]`；`AgentType::id()` 对 `DreamEngine` 分支硬编码冻结返回历史哈希 `"632f31d2"` |
| `crates/dream-core-db/migrations/052_dream_rebrand_persisted_values.sql` | 新增正向迁移：UPDATE `conversations.type`/`conversations.source`/`agent_metadata.agent_type`/`assistant_sessions.agent_type` 四处旧值→`dream` |
| `crates/dream-domain-employee/migrations/005_dream_rebrand_agent_type.sql` | 新增正向迁移：UPDATE `one_personal_agents.agent_type` |
| `crates/dream-core-auth/src/middleware.rs` | `WEBUI_PROXY_HEADER`/`CLIENT_IP_HEADER` 两个内部 HTTP 头常量值从 `x-aionui-*` 改为 `x-dream-*`（真实 bug，见下节 2.1；`RUNTIME_TOKEN_HEADER` 等其余三个头常量确认无需改动，未动） |
| `crates/dream-core-conversation/src/service.rs` | `aionrs_capability_agent_id()` 内部硬编码的查找 key `"aionrs"` → `"dream"`（真实 bug，见 2.2） |
| 约 10 个 crate 的测试文件 | fixture/断言里写死的旧字符串值（`"aionrs"`/`.aionrs/skills`/`bare-aionrs` 等）改成 `"dream"`/`.dream/skills`/`bare-dream`，详见二节列表 |

## 二、本仓库特有的两个真实 bug（不是测试断言过时）

### 2.1 内部 HTTP 头名跨仓不同步（`dream-core-auth::middleware`）

详细背景见 dream-ui 文档 3.2 节。本仓库侧的修复点：

```rust
// crates/dream-core-auth/src/middleware.rs
pub const WEBUI_PROXY_HEADER: &str = "x-dream-forwarded-origin"; // was x-aionui-forwarded-origin
pub const CLIENT_IP_HEADER: &str = "x-dream-client-ip";          // was x-aionui-client-ip
```

同文件里 `RUNTIME_TOKEN_HEADER`/`RUNTIME_USER_ID_HEADER`/`RUNTIME_CONVERSATION_ID_HEADER`
三个头常量**确认保留不变**——已用 grep 核实两仓都没有任何代码期望这三个头改名，
不要在后续"顺手"把它们也改掉。

### 2.2 会话分叉（fork）功能对 dream 会话必现失败

详细背景见 dream-ui 文档 3.3 节。本仓库侧的修复点：

```rust
// crates/dream-core-conversation/src/service.rs
async fn aionrs_capability_agent_id(&self, user_id: &str, conversation_id: &str) -> Result<String, ConversationError> {
    Ok(self
        .resolve_assistant_agent_binding(user_id, "dream") // was "aionrs"
        .await?
        .map(|binding| binding.agent_id)
        .unwrap_or_default())
}
```

`resolve_assistant_agent_binding` 底层调用 `dream-core-db::resolve_agent_binding_from_rows`，
是严格字符串匹配（`row.agent_type == value`），不做别名归一化——这个函数族（还包括
`resolve_agent_binding`、`resolve_agent_binding_for_user`）**是本次迁移里 bug 命中率
最高的一类**，因为它们的调用点如果硬编码传旧值当 key，编译和 `cargo check` 完全
测不出来，只能靠跑完整的集成测试暴露。除了这处 `service.rs`，
`crates/dream-core-db/tests/agent_binding_resolver.rs` 里
`resolves_internal_agent_type_when_backend_is_null` 测试本身也一度还在用
`resolve_agent_binding(db.pool(), "aionrs")` 当调用参数，同一类错误在测试代码里
又复现了一次，一并改成 `"dream"`（连同 `.expect("aionrs should resolve")` 的消息文本
和两处断言 `resolved.agent_type`/`resolved.runtime_backend` 也改成 `"dream"`）。

## 三、测试断言/fixture 修复清单（过时值，非真实 bug）

以下改动都是"生产逻辑本来就是对的，只是测试写死了迁移前的旧值"，按 crate 分组：

- `dream-core-assistant/src/service.rs`：`assistant_lineage_extracts_aionrs_preset_id` 里 `lineage.agent_type` 断言、`mk_agent_row`/`builtin.agent_ref` 构造参数
- `dream-core-channel/src/channel_settings.rs`：`make_definition("bare-aionrs", "aionrs")` 两处调用
- `dream-core-channel/src/action.rs`、`dream-core-channel/tests/session_action_integration.rs`：`assert!(text.contains("aionrs"))` → `"dream"`
- `dream-core-channel/tests/message_service_integration.rs`：`bare_assistant_definition_params(...)` 第三参、`agent_type` 字面量、会话名断言 `tg-aionrs-70880480` → `tg-dream-70880480`
- `dream-core-conversation/src/service_test.rs`：`seed_aionrs_conversation_with_snapshot` 里 `r#type: "aionrs".into()`；`warmup_restores_skill_links_for_recreated_auto_workspace` 里三处 `.aionrs/skills/cron` 路径断言
- `dream-core-conversation/src/session_context.rs`：`workspace_empty_uses_auto_path_and_is_not_custom` 里工作区路径断言 `aionrs-temp-conv-1` → `dream-temp-conv-1`（根因是 `conversation_label()` 现在解析出 "dream"）
- `dream-core-cron/tests/service_integration.rs`：两处 `job.agent_type` 断言（`seeded_agent_id()` 测试专用字典本身不改，那是纯测试标签映射）
- `dream-core-db/tests/aionrs_fork_capability_migration.rs`、`agent_binding_resolver.rs`：见二节

## 四、迁移文件生命周期踩坑记录

写 052/005 两个新迁移之前，一度误以为 `assistants.preset_agent_type`、
`assistant_overrides.preset_agent_type`、`cron_jobs.agent_type` 也需要同样的 UPDATE
语句——只看了最早建表的迁移文件就下结论。实际上后续某个中间迁移已经把这几张表重建
成 JSON blob 存储，这些列早就不再以独立列的形式存在，按错误假设写的 SQL 一执行就报
`no such column`，级联炸出无关 crate 的 74 个测试失败。**结论：判断一个字段现在是否
还存在、以什么形式存在，必须顺着这张表的全部迁移历史（包括 `ALTER TABLE`/整表重建）
逐条追下来，只看最早的建表语句是不可靠的。**

## 五、测试与验证

```powershell
cargo install cargo-nextest --locked   # 一次性
cargo nextest run --workspace --no-fail-fast
```

本次收尾从第一次全量红测试到全绿，一共经过约 10 余轮"跑一遍 → 修当轮暴露的失败 →
再跑一遍"的迭代——`cargo test --workspace` 默认 `--fail-fast`，每个测试二进制独立
fail-fast，所以不加 `--no-fail-fast` 报出来的失败数会偏小、误导判断"是不是已经全部
修完了"。改用 `cargo nextest run --workspace --no-fail-fast` 后，一轮就能拿到全 workspace
的完整失败列表，收尾效率明显更高。

最后两个测试文件（`aionrs_fork_capability_migration.rs`、`agent_binding_resolver.rs`）
修复后已提交推送；随后又跑了一轮 `cargo nextest run --workspace --no-fail-fast` 做
最终确认，结果见本次提交历史里紧随其后的验证记录（如仍有失败，按本文档第四节的方法论
——先查是测试断言过时还是 strict-match 查找函数踩坑——继续修复，不要假设"应该没问题了"）。
