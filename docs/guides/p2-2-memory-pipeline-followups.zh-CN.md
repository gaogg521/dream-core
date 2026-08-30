# P2-2 记忆系统管线接线实施方案（EXTRACT + INJECT）

> 目标：把 `dream-domain-memory` 从「空壳」接成真管线 —— 对话轮次完成后**抽取**要点写进
> `MemoryService::add_item`；轮次开始前**检索**可读记忆并注入 agent 上下文。
> 权威计划：`D:\dream\dream-en\docs\align-openocta-2026-08-29.zh-CN.md` §3「记忆系统」+ P2-2 行。
> 现状缺口见 `parity-check-2026-08-30.zh-CN.md` §三-1：「有记忆系统，但没有东西往里写」。
>
> 全部改动遵循红线：策略只加不减；个人版 = 无 memory crate = `None` seam = 零行为变化 = 逐字节等同现状；
> 新增 `dream_domain_memory` 引用必须在 `#[cfg(feature = "enterprise")]` 内（`dream-core/CLAUDE.md` §改这块三条规则-1）。

---

## 0. 关键事实（已在代码中核对，file:line）

### 0.1 现有「热路径 fire-and-forget seam」先例 —— 必须照抄的形状

| seam | trait 定义 | `ConversationService` 槽位 | setter | 调用点 | app 侧 impl（enterprise-gated） |
|---|---|---|---|---|---|
| P0-3 计量 | `UsageRecorder` `crates/dream-core-conversation/src/state.rs:26` | `usage_recorder: Arc<RwLock<Option<Arc<dyn UsageRecorder>>>>` `service.rs:349` | `with_usage_recorder` `service.rs:479` | `turn_orchestrator.rs:452-461` → `meter_attempt` `:934` | `BillingUsageRecorder` `crates/dream-core-app/src/router/routes.rs:538`；wiring `routes.rs:2432-2437` |
| P2-5 逐次 trace | `LlmCallTraceRecorder` `state.rs:61` + `LlmCallTrace` struct `state.rs:43` | `llm_trace_recorder` `service.rs:367` | `with_llm_trace_recorder` `service.rs:488` | `turn_orchestrator.rs:467-471` → `trace_attempt` `:973` | `BillingLlmCallTrace` `routes.rs:979`；wiring `routes.rs:2441-2444` |
| T3 发送闸门 | `SendGate` `state.rs:104` | `send_gate` `service.rs:360` | `with_send_gate` `service.rs:496` | `service.rs:3916`（`send_message`）+ `service.rs:4156`（`run_agent_turn`） | `EnterpriseSendGate` `routes.rs`；wiring `routes.rs:2454-2464` |

seam trait 共同规则（`state.rs:6-25,57-63` 注释 + `CLAUDE.md` §改这块-3）：
- **fire-and-forget**：实现自己 `tokio::spawn` 异步工作，绝不阻塞/失败 send 路径。
- **`Option` + `None` 语义**：个人版 = `None` = 零行为。不写 no-op 实现。
- 槽位是 `Arc<RwLock<Option<...>>>`，因为 `ConversationService` 有很多 clone（HTTP routes / cron / channel / team），
  wiring 必须打到所有 clone（`service.rs:342-359` 注释），所以走 interior-mutability setter，不走 per-router `ConversationRouterState` builder chain。

### 0.2 `dream-domain-memory` 当前能力（`crates/dream-domain-memory/src/service.rs`）

- `MemoryService::new(pool: SqlitePool)` `:199` —— 只吃一个 pool。
- `resolve_actor(user_id) -> Option<MemoryActor{tenant_id, role}>` `:205` —— 查 `one_user_org` +
  `one_active_tenant`（跨 crate 裸 SQL，与 one-platform `resolve_actor` 同款）。`no such table` → `Ok(None)`（个人版安全）。
- `add_item(tenant_id, caller_id, caller_role, collection_id, content, importance: f64, source_conversation_id: Option<&str>, tags: &[String])` `:570`
  —— **要求 collection 已存在**（`:585`）+ 通过 `writable_by` 授权（`:592`，personal owner-only / global·department 需 admin 或 write grant）。
  content 空白 → `BadRequest`。SHA-256 hash 落列（refine job 去重用）。
- `search_items(tenant_id, caller_id, caller_role, query, collection_id: Option<&str>, limit) -> Vec<MemoryItemDto>` `:667`
  —— 当前是 `content LIKE '%' || ? || '%'`（`:699`），只回 `status='active'`，只在 caller 可读的 collection 内
  （`readable_collection_ids` `:338`：global=全员 / department=同部门或 grant / personal=owner）。空 query → `Ok(vec![])`。
- `create_collection(tenant_id, caller_id, caller_role, scope, department_id, owner_user_id, name, description)` `:361`
  —— **非 admin 只能建自己的 personal collection**（`:412-420`）。没有「自动为每个用户建 personal collection」。
- `MEMORY_SCOPES = ["global","department","personal"]` `:22`；`MEMORY_REFINE_MIN_IMPORTANCE = 0.3` `:25`。
- migration `crates/dream-domain-memory/migrations/001_init.sql` —— `one_memory_items.source_conversation_id TEXT`（可空，`:51`）已就位。**无需新迁移**（除非做 §A.5 去重键）。
- `MemoryItemDto` 含 `importance: f64`、`source_conversation_id: Option<String>`、`content_hash`（`models.rs`）。
- 无 embedding 检索（`search_items` 是 LIKE）。P3-2 内置 embedding 端点存在于 `dream-domain-devops`，但 memory crate 未接。

### 0.3 注入侧现状 —— agent 上下文如何组装

- 会话创建时把 assistant 快照的 rules 写进 `conversation.extra`：ACP/agy → `extra.preset_context`，DreamEngine → `extra.preset_rules`
  （`crates/dream-core-conversation/src/service.rs:1263-1287`；fallback 读取 `service.rs:1811-1815`）。
- turn 启动：`send_message`（`service.rs:4083` `build_task_options` → `:4108` `apply_conversation_runtime_context` → `:4113` `spawn_user_turn`）
  和 `run_agent_turn`（`service.rs:4218` → `:4245` → `:4249` `run_user_turn`）。
- `build_task_options` `service.rs:4857` → `SessionContextBuilder::build_options`（`session_context.rs:44`），
  产出 `BuildTaskOptions{ context: AgentSessionContext }`。`preset_context` 落在
  `context.kind` 里：`AcpSessionBuildContext.config.preset_context`（`dream-core-ai-agent/src/session_context.rs:49`，类型 `AcpBuildExtra`），
  `AntigravitySessionBuildContext.config.preset_context`，`AionrsSessionBuildContext.config.preset_rules`（`DreamEngineBuildExtra`）。
- ai-agent 消费：
  - ACP：`AcpSessionParams.preset_context` `crates/dream-core-ai-agent/src/factory/acp_assembler.rs:31`（`compose_preset_context` `:113` 只 trim）
    → `SessionNewPreludeHook` `crates/dream-core-ai-agent/src/manager/acp/hooks.rs:18`（**一次性**，`take_pending_session_new_prelude` `:20`）
    → `inject_first_message_prefix` `crates/dream-core-ai-agent/src/capability/first_message_injector.rs:33`
    → `[Assistant Rules]\n{ctx}\n[/Assistant Rules]\n\n{content}`。pipeline 构造 `manager/acp/agent.rs:742`（hook 列表硬编码）。
  - agy：`session_agent.rs:1732` `preset_context: config.preset_context.clone()` 进 `SessionInit`。
  - DreamEngine：`factory/dream_engine.rs:48` 把 `preset_rules` 合进 `system_prompt`（session build 时）。
- **层规则**：`dream-core-ai-agent`（capability 层）**不能**依赖 `dream-domain-memory`（domain 层）。
  `dream-core-conversation` 的 `Cargo.toml`（已核对）也只依赖 foundation/capability 层，无任何 domain crate。
  所以注入 seam 必须定义在 `dream-core-conversation`（或更低），impl 在 `dream-core-app`。

### 0.4 已有的「便宜 LLM 调用」原语（用于 §A 抽取的 LLM 方案）

`crates/dream-core-ai-agent/src/capability/image_description.rs` 的 vision delegate 已经在 dream-core 内做一次性模型调用：
- `dream_engine_providers::{LlmProvider, create_provider}` + `dream_engine_types::llm::LlmRequest`（`image_description.rs:27-28`）。
- `create_provider(&Config)` → `provider.stream(&LlmRequest{ model, system, messages, max_tokens, ... })`（`:152,197-216`）。
- 带超时（`describe_with_provider_with_timeout` `:169`）、失败返回 `Err` 不编造。
- provider 配置从 `VisionModelConfig`（billing allowlist gate 解析）或用户 provider 行来。

全仓 **没有 `sqlx::query!` 编译期宏**（grep 确认 0 处），所有查询是运行时 `query`/`query_as`/`query_scalar`。
全仓也**没有**「对话标题生成」之类的独立 LLM 助手 —— 标题由 agent CLI 自己发（`codex_title.rs` / claude adapter）。

---

## A. EXTRACT（轮次完成 → 写记忆）

### A.1 trait —— 放 `crates/dream-core-conversation/src/state.rs`

紧跟 `LlmCallTraceRecorder`（`state.rs:63`）之后新增：

```rust
/// One completed conversation turn, handed to the enterprise memory
/// pipeline (P2-2) for salient-fact extraction. Same fire-and-forget
/// contract as [`UsageRecorder`] / [`LlmCallTraceRecorder`]: the
/// implementation spawns its own async work and MUST NOT block or fail
/// the turn. Wired to one-memory in dream-app; `None` in personal builds
/// (no extraction, no rows — bit-for-bit the pre-memory behaviour).
///
/// `user_message` / `assistant_message` are the plain-text bodies of the
/// turn that just finished. `assistant_message` is `None` when the turn
/// produced no persisted assistant text (empty end_turn, hard failure) —
/// the implementation should skip extraction rather than store a
/// half-turn.
pub trait TurnMemoryExtractor: Send + Sync {
    fn extract_from_turn(
        &self,
        user_id: String,
        conversation_id: String,
        user_message: String,
        assistant_message: Option<String>,
    );
}
```

### A.2 `ConversationService` 槽位 + setter —— `crates/dream-core-conversation/src/service.rs`

- 字段（紧跟 `llm_trace_recorder` `service.rs:367`）：
  ```rust
  pub(crate) turn_memory_extractor:
      Arc<RwLock<Option<Arc<dyn crate::state::TurnMemoryExtractor>>>>,
  ```
- 构造器初始化（`service.rs:450-451` 那一组 `Arc::new(RwLock::new(None))` 里加一行）。
- setter（照 `with_llm_trace_recorder` `service.rs:488`）：
  ```rust
  pub fn with_turn_memory_extractor(&self, extractor: Arc<dyn crate::state::TurnMemoryExtractor>) {
      if let Ok(mut guard) = self.turn_memory_extractor.write() { *guard = Some(extractor); }
  }
  ```
- `lib.rs:40-41` 的 `pub use` 列表加 `TurnMemoryExtractor`。

### A.3 拿到 assistant 文本 —— 给 `RelayOutcome` 加一个字段

`RelayOutcome`（`crates/dream-core-conversation/src/stream_relay.rs:120`）当前不带 assistant 正文。
`StreamRelay::finalize` 有 `full_text_buffer`（`stream_relay.rs:312,497,954-961`）。新增：

```rust
// stream_relay.rs, struct RelayOutcome
/// The assistant's finalized plain-text reply for this turn, when one was
/// persisted. `None` for an empty / errored turn. Consumed ONLY by the
/// P2-2 memory-extraction seam — no other reader.
pub final_text: Option<String>,
```

在 `finalize`（`stream_relay.rs:~950`，算出 `final_text` / `hidden` 之后）把非空、非 hidden 的 `final_text`
塞进返回的 `RelayOutcome`。`aggregate_summary.merge` 不碰这个字段（它是 attempt 级，取最后一次 attempt 的即可 ——
在 `turn_orchestrator.rs:442` `merge` 之后单独 `last_final_text = outcome.final_text.clone()`）。

> 备选（不改 stream_relay）：抽取 adapter 自己用 `conversation_repo.list_messages_page`
> （`crates/dream-core-db/src/repository/conversation.rs:88`）读最后一条 user + 最后一条 assistant。
> 但 adapter 在 dream-app，要多传一个 repo handle，且要处理「哪条是本轮」的时序。**推荐改 stream_relay**（一个字段、`finalize` 一处赋值，最干净）。

### A.4 调用点 —— `turn_orchestrator.rs::run_user_turn`（`turn_orchestrator.rs:514`）

在 loop 结束、`final_failed` 已知、构造 `ConversationTurnResult` 之前（`turn_orchestrator.rs:697-709` 那段，
`complete_released_turn` 调用之后）：

```rust
// P2-2 memory extraction: only on a turn that actually produced a reply.
// Fire-and-forget — the extractor spawns its own work; a failure here
// must never touch the turn's own result.
if !final_failed {
    if let Some(extractor) = self.service.turn_memory_extractor.read().ok().and_then(|g| g.clone()) {
        extractor.extract_from_turn(
            input.user_id.clone(),
            conv_id.clone(),
            input.content.clone(),          // the user message text for this turn
            last_final_text.take(),         // assistant reply, from §A.3
        );
    }
}
```

`input.content` 是本轮 user 文本（`TurnStartInput.content`，`turn_orchestrator.rs:521` 处进 `initial_send`）。
`run_user_turn` 是 `send_message`（HTTP + IM channel）**和** `run_agent_turn`（cron）唯一汇合点
（`service.rs:4113` `spawn_user_turn` 内部 spawn `run_user_turn`；`service.rs:4249` 直接 `.run_user_turn`），
所以这一处覆盖全部触发路径。

### A.5 app 侧 impl —— `crates/dream-core-app/src/router/routes.rs`（`#[cfg(feature = "enterprise")]`）

照 `BillingLlmCallTrace`（`routes.rs:979`）+ `PlatformConfigResolver`（`routes.rs:1572`）的形状：

```rust
#[cfg(feature = "enterprise")]
struct OneMemoryTurnExtractor {
    memory: std::sync::Arc<dream_domain_memory::MemoryService>,
    org: std::sync::Arc<dream_domain_org::OrgService>,
    // §A.6 决定放不放 provider_repo / encryption_key（LLM 方案才需要）
}

#[cfg(feature = "enterprise")]
impl dream_core_conversation::TurnMemoryExtractor for OneMemoryTurnExtractor {
    fn extract_from_turn(
        &self,
        user_id: String,
        conversation_id: String,
        user_message: String,
        assistant_message: Option<String>,
    ) {
        let (memory, org) = (self.memory.clone(), self.org.clone());
        tokio::spawn(async move {
            let Some(assistant_message) = assistant_message else { return; };
            // 1) tenant + role
            let actor = match memory.resolve_actor(&user_id).await {
                Ok(Some(a)) => a,           // MemoryActor{tenant_id, role}
                Ok(None) => return,         // 不在企业 —— 不写
                Err(e) => { tracing::debug!(error=%e, "memory extract: resolve_actor failed"); return; }
            };
            // 2) 目标 collection：本人 personal collection，懒建
            let collection_id = match ensure_personal_collection(&memory, &actor, &user_id).await {
                Ok(id) => id,
                Err(e) => { tracing::debug!(error=%e, "memory extract: ensure collection failed"); return; }
            };
            // 3) 抽取（§A.6）
            let facts = extract_salient_facts(&user_message, &assistant_message /*, provider */).await;
            // 4) 写入
            for f in facts {
                if let Err(e) = memory.add_item(
                    &actor.tenant_id, &user_id, &actor.role,
                    &collection_id, &f.content, f.importance,
                    Some(&conversation_id), &f.tags,
                ).await {
                    tracing::debug!(error=%e, "memory extract: add_item failed");
                }
            }
        });
    }
}
```

`ensure_personal_collection`：`memory.list_collections(tenant_id, user_id, role)` 里找 `scope=="personal" && owner_user_id==Some(user_id)`；
无则 `memory.create_collection(tenant_id, user_id, role, "personal", None, None, "对话记忆", "")`（走 i18n 流程定 key，别内联中文 —— 红线 8）。
用一个进程内 `Mutex<HashSet<String>>` 或 `dashmap` 去抖并发首建（两个并发轮次同时建 → 二者都 ok，但会造两个 collection；
可接受，或加 per-user 锁）。

wiring（`routes.rs:2432-2445` 那个 `#[cfg(feature = "enterprise")]` 块内，紧跟 `with_llm_trace_recorder`）：
```rust
states.conversation.service.with_turn_memory_extractor(std::sync::Arc::new(OneMemoryTurnExtractor {
    memory: one_memory_service.clone(),
    org: one_org_service.clone(),
}));
```
注意 `one_memory_service` 现在只在 `build_governance_plane` 内局部构造（`routes.rs:2056`）；需要把 `Arc<MemoryService>`
提到 `GovernancePlane` 结构体上返回（加一个 `pub memory_service:` 字段，`routes.rs:2065-2079` 那个 struct literal），
或在 wiring 处重新 `MemoryService::new(services.database.pool().clone())`（cheap，只是 pool 包装）。**推荐后者**（零结构改动）。

### A.6 抽取策略 —— 启发式 vs 便宜 LLM（两个都评估，含结论）

**背景约束**：任务说明「坏启发式产生垃圾记忆，比空更糟」。记忆会被注入进*未来所有轮次*的 agent 上下文，
垃圾记忆是持续污染。宁可少写、写准。

#### 方案 H：纯启发式
- 规则示例：user 消息里含「我叫/我的名字/我在/我用/我喜欢/以后/记住/我们的项目/我负责」等偏好/事实触发词的句子；
  或 assistant 回复里「已记录/我会记住/明白了，你偏好」之类确认句。抽出对应句子做 item。
- 优点：零成本、零延迟、无外部依赖、确定性、易测。
- 缺点：**中文触发词表脆弱**，召回和精度都差；跨句指代解析不了；很容易把闲聊句、代码片段、错误信息当「事实」写进去。
- 判断：**不达标**。OpenOcta 的记忆是「个人内容炼化 + 偏好学习」，启发式给不出这个质量。可作为**兜底/前置过滤**
  （先用触发词判断「这轮值不值得抽」，值得再走 LLM），但不能作为唯一抽取器。

#### 方案 L：便宜一次性 LLM 调用（推荐）
- 用 §0.4 的 `dream_engine_providers::create_provider` + `LlmRequest`，system prompt 固定为
  「你是记忆抽取器。从下面这轮对话里提取应当长期记住的用户事实/偏好，每条一行 JSON `{content, importance:0..1, tags:[]}`；
  没有值得记的就输出空。不要提取一次性的任务指令、代码、报错、寒暄。」，`max_tokens` ~256，超时 10s。
- provider 配置来源（按优先级）：
  1. 企业管理员在记忆设置里指定的「抽取模型渠道」（**建议新增**：`dream-domain-memory` 加一个 tenant 级
     `extraction_channel_id` 配置 + 迁移一列 / 一张 `one_memory_config` 表 + admin 端点。最干净，管理员可控成本）。
  2. 回落：该轮跑的 provider（`build_options.context.model: ProviderWithModel`）—— 但这需要把 model 也透传进 seam
     （给 `extract_from_turn` 加一个 `turn_model: Option<ProviderWithModel>` 参数，从 `turn_orchestrator` 的
     `input.build_options.context.model` 取）。provider 行（api_key 密文 + base_url）用 `services.provider_repo` +
     `decrypt_string(row.api_key_encrypted, encryption_key)`（`factory/dream_engine.rs:97`）解出。
  3. 都没有 → 跳过（不写）。
- 失败/超时/空输出 → 不写。解析失败的行丢弃（不写半条）。
- 成本控制：只在「本轮 user+assistant 合计长度 > N 且触发词命中」时才调用（方案 H 当门控）；
  每会话每 M 轮最多抽一次（在 adapter 里用 `conversation_id` + 计数器节流）。
- 优点：质量达标、口径对齐 OpenOcta、复用现成原语。
- 缺点：有 token 成本（管理员可关 = 不配 channel 就退化成「有记忆系统但不自动写」，仍可手动 add）；有一次额外 LLM 往返（但在 spawn 里，不上热路径）。

**结论**：采用 **L 为主 + H 做门控/节流**。第一版可以先只做 L（system prompt 足够严格 + `importance` 阈值过滤
`< 0.5` 的不写），H 门控作为紧接着的成本优化。若产品决定「先不引入抽取 LLM 成本」，则**该半边先只留 seam + 手动
`add_item` 端点**，`extract_from_turn` 内部直接 return（诚实的「管线就绪、抽取器待配」，对齐 `handoff` 里其它「诚实边界」的处理方式）。

### A.7 去重（可选，第二版）
`add_item` 已按 `content` SHA-256 落 `content_hash`，refine job（`service.rs:776`）按 hash 去重。
但抽取器每轮可能产出近似句。第一版靠 refine job 兜底即可。第二版可在 `add_item` 前加一次
`search_items(query = fact.content 前缀)` 命中就跳过。

### A.8 测试（EXTRACT）
- `crates/dream-core-conversation/src/turn_orchestrator.rs` `#[cfg(test)]`：
  - `RecordingTurnMemoryExtractor`（照 `RecordingUsageRecorder` `turn_orchestrator.rs:1052`）；
  - 断言成功轮次调用一次、内容 = `input.content` + assistant 文本；失败轮次**不调用**（照
    `usage_recorder_not_invoked_on_...` 之类的现有反证测试思路，`CLAUDE.md` 提到「证明 cron/channel 的 agent 从未被调用」的测试）。
  - `RelayOutcome.final_text` 在 empty/hidden 时为 `None` → extractor 传 `None`。
- `crates/dream-core-app/tests/`（enterprise feature）：端到端 —— seed 企业成员 → 跑一轮 →
  `OneMemoryTurnExtractor` 建出 personal collection 且 `add_item` 有行、`source_conversation_id` 正确。
  非企业成员（`resolve_actor` → None）→ 零行。
- `crates/dream-domain-memory/src/service.rs` `#[cfg(test)]`：`add_item` 到刚建的 personal collection 成功（已有类似测试 `personal_collection_is_owner_only` `:1323`）。
- 双 feature `cargo test --no-run`（红线 5）。

---

## B. INJECT（轮次开始 → 检索并注入）

### B.1 trait —— 放 `crates/dream-core-conversation/src/state.rs`

```rust
/// Retrieves the caller's readable enterprise memory (P2-2) for injection
/// into an agent turn's context. Same `None`-in-personal-builds contract
/// as the other seams; unlike them this one IS awaited on the turn-start
/// path, so the implementation MUST be fast (one indexed query) and MUST
/// degrade to an empty Vec on any error — a memory lookup that fails or
/// stalls must never delay or block a turn.
#[async_trait::async_trait]
pub trait MemoryContextProvider: Send + Sync {
    /// Returns memory snippets relevant to `query` that `user_id` may read,
    /// most relevant first, already length-bounded. Empty = inject nothing.
    async fn recall(&self, user_id: &str, query: &str) -> Vec<String>;
}
```

（`async_trait` 已是 `dream-core-conversation` 依赖，见 `Cargo.toml:25`。`SendGate` 也是 `#[async_trait]`。）

### B.2 `ConversationService` 槽位 + setter

同 §A.2：字段 `memory_context_provider: Arc<RwLock<Option<Arc<dyn MemoryContextProvider>>>>`，
`with_memory_context_provider` setter，`lib.rs` 导出。

### B.3 注入点 —— `turn_orchestrator.rs::run_user_turn`，attempt loop 之前（`turn_orchestrator.rs:~530`）

在 `run_user_turn` 里，`initial_send` 构造之后、`loop` 之前，调用 provider 并把命中 prepend 到
`input.build_options.context` 的 preset 字段：

```rust
// P2-2 memory injection: prepend readable memory hits to the turn's
// preset context. Best-effort — an empty/failed recall leaves the
// context exactly as it was.
if let Some(provider) = self.service.memory_context_provider.read().ok().and_then(|g| g.clone()) {
    let hits = provider.recall(&input.user_id, &input.content).await;
    if !hits.is_empty() {
        let block = format!("[Relevant Memory]\n{}\n[/Relevant Memory]", hits.join("\n"));
        prepend_preset_context(&mut input.build_options.context, &block);
    }
}
```

`prepend_preset_context` 帮助函数（放 `turn_orchestrator.rs` 或 `session_context.rs`）：
```rust
fn prepend_preset_context(ctx: &mut dream_core_ai_agent::session_context::AgentSessionContext, block: &str) {
    use dream_core_ai_agent::session_context::AgentSessionKind::*;
    match &mut ctx.kind {
        Acp(c) | /* 需分开写，类型不同 */ => { c.config.preset_context = Some(join(block, c.config.preset_context.take())); }
        Antigravity(c) => { c.config.preset_context = Some(join(block, c.config.preset_context.take())); }
        DreamEngine(c) => { c.config.preset_rules = Some(join(block, c.config.preset_rules.take())); }
    }
}
// join(block, Some(existing)) => format!("{block}\n\n{existing}"); join(block, None) => block
```

（`AcpSessionBuildContext.config` / `AntigravitySessionBuildContext.config` 都是 `AcpBuildExtra`，
`AionrsSessionBuildContext.config` 是 `DreamEngineBuildExtra`；`preset_context` / `preset_rules` 字段见
`dream-core-api-types`。核对：`dream-core-ai-agent/src/factory/acp_assembler.rs:80` 读 `config.preset_context`，
`factory/dream_engine.rs:48` 读 `overrides.preset_rules`。）

### B.4 ⚠️ 已知限制 —— 只在会话首轮生效（必须向产品说清）

ACP 的 `preset_context` 只在 `session/new` 那一次通过 `SessionNewPreludeHook` 注入（一次性 flag
`take_pending_session_new_prelude`，`hooks.rs:20` / `session.rs:92-97`）。DreamEngine 的 `preset_rules`
在 session build 时合进 system_prompt（resume 时是否重注入取决于是否 rebuild）。**因此 §B.3 的注入对一个已经
跑过的会话的第 2、3…轮不会重新注入记忆。**

两个选择：
- **第一版（推荐）**：接受「首轮注入」语义，等价于「Assistant Rules」的现有行为。文档写清。
- **完整版（第二版，风险更高）**：在 ai-agent 加一个**每轮**都跑的 `PreSendHook`（不 gate 在一次性 flag 上），
  把 `MemoryContextProvider`（trait 定义下沉到 ai-agent 层或其依赖，impl 仍在 app）通过 `AcpSessionParams`
  （`acp_assembler.rs:23`）+ factory（`AgentFactoryDeps`）传进去，hook 在 `PromptCtx`（`prompt_pipeline.rs:16`）
  拿 `ctx.params.user_id` + 当前 prompt 文本做 recall，prepend 到 prompt。pipeline 注册在
  `manager/acp/agent.rs:742`。这条路碰 factory 装配 + ACP 热路径，是「碰对话轮次编排 + agent 上下文组装两处热路径」
  里更重的那半 —— 单独一轮做。

### B.5 app 侧 impl —— `crates/dream-core-app/src/router/routes.rs`（`#[cfg(feature = "enterprise")]`）

照 `PlatformConfigResolver`（`routes.rs:1572-1590`，它就是「resolve tenant via `org.tenant_of` → 调 platform service」）：

```rust
#[cfg(feature = "enterprise")]
struct OneMemoryContextProvider {
    memory: std::sync::Arc<dream_domain_memory::MemoryService>,
    org: std::sync::Arc<dream_domain_org::OrgService>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_conversation::MemoryContextProvider for OneMemoryContextProvider {
    async fn recall(&self, user_id: &str, query: &str) -> Vec<String> {
        let actor = match self.memory.resolve_actor(user_id).await {
            Ok(Some(a)) => a, _ => return Vec::new(),
        };
        // search_items 已经按 caller 可读的 collection（global/自己部门/自己 personal + grant）过滤
        let items = match self.memory
            .search_items(&actor.tenant_id, user_id, &actor.role, query, None, 5)
            .await
        {
            Ok(v) => v, Err(e) => { tracing::debug!(error=%e, "memory recall failed"); return Vec::new(); }
        };
        items.into_iter()
            .map(|i| i.content)
            .map(|c| truncate(&c, 400))   // 单条封顶
            .collect()
    }
}
```

tenant 解析：`MemoryService::resolve_actor`（`service.rs:205`）内部已做（`one_active_tenant` 优先 + `one_user_org`），
**不需要**额外走 `org.tenant_of`（保留 `org` handle 只为与其它 adapter 形状一致，可以不要）。

wiring：`routes.rs` `#[cfg(feature = "enterprise")]` 块（同 §A.5）：
```rust
states.conversation.service.with_memory_context_provider(std::sync::Arc::new(OneMemoryContextProvider {
    memory: std::sync::Arc::new(dream_domain_memory::MemoryService::new(services.database.pool().clone())),
    org: one_org_service.clone(),
}));
```

### B.6 检索质量说明
- 当前 `search_items` 是 `content LIKE '%query%'`（`service.rs:699`），query = 整条 user 消息 →
  LIKE 整句几乎永远不命中。**必须**在 adapter 里把 query 降级为关键词，或（更好）给 `search_items` 加一个
  「按空白分词、OR 匹配、按命中词数排序」的分支。**建议本方案范围内**：`MemoryService` 加
  `search_items_tokenized`（或给 `search_items` 加 `tokenize: bool`），SQL 里 `content LIKE '%'||?||'%' OR ...` 多词 OR。
- 真·语义检索（embedding）留作后续：P3-2 的 `dream-domain-devops::embedding::embed` 已有内置端点，
  可以给 `one_memory_items` 加 embedding 列 + 迁移 + 在 `add_item` 时算向量 + `search_items` 走余弦。独立立项。

### B.7 测试（INJECT）
- `turn_orchestrator.rs` `#[cfg(test)]`：`StubMemoryContextProvider` 返回固定 hits → 断言
  `input.build_options.context` 的 `preset_context` / `preset_rules` 被 prepend；返回空 → context 逐字节不变；
  `recall` 返回 Err 语义（provider panic 不行，但返回空要测）。对 ACP / agy / DreamEngine 三种 kind 各一个用例。
- `crates/dream-core-ai-agent/tests/prompt_pipeline_integration.rs`（若走第二版每轮 hook）：hook 把 memory 块注进 prompt。
- `crates/dream-core-app/tests/`（enterprise）：seed 一条 global memory item → 跑一轮 →
  断言送给 agent 的首条 prompt 含 `[Relevant Memory]`（可用 mock agent 抓 prompt，见
  `dream-core-conversation` dev-dep `dream-core-ai-agent` `test-support` feature，`Cargo.toml:35`）。
- 个人版：`cargo test --no-run`（无 memory crate，seam = None，编译通过 + 行为不变）。

---

## C. Feature-gating checklist（红线 3 + CLAUDE.md §改这块）

- [ ] `TurnMemoryExtractor` / `MemoryContextProvider` trait + `ConversationService` 槽位 + setter：
      **不**门控（在 `dream-core-conversation`，个人版也编译，`Option` = `None`）。同 `UsageRecorder` 现状。
- [ ] `RelayOutcome.final_text`：不门控。
- [ ] `turn_orchestrator` 调用点：不门控（读 `Option`，`None` 直接跳过）。
- [ ] `OneMemoryTurnExtractor` / `OneMemoryContextProvider` + 全部 `dream_domain_memory::` / `dream_domain_org::` 引用：
      `#[cfg(feature = "enterprise")]`。
- [ ] wiring（`with_turn_memory_extractor` / `with_memory_context_provider` 调用）：在现有
      `#[cfg(feature = "enterprise")]` 块内（`routes.rs:2432-2445` 附近）。
- [ ] `GovernancePlane` 若加 `memory_service` 字段：该字段 `#[cfg(feature = "enterprise")]`（或直接重建 pool，见 §A.5）。
- [ ] 验证：`just check-editions`（两版都编译）+ 双 feature `cargo test --no-run` + `cargo nextest run -p dream-core-conversation -p dream-domain-memory`；
      收尾 `cargo nextest run --workspace`（红线 6：本地不跑 `-p dream-core-app` 全量 e2e，交 CI）。
- [ ] i18n：collection 默认名（"对话记忆"）、任何前端新文案走 `dream-ui/.claude/skills/i18n` 流程，不内联中文（红线 8）。
- [ ] logging（AGENTS.md §Logging）：抽取/注入失败走 `debug!`（非诊断关键，fire-and-forget），
      与 `BillingUsageRecorder` 的 `tracing::debug!(error=%e, "... failed (non-fatal)")` 一致。注入命中数可加一条 `debug!`。

---

## D. 涉及文件清单

**改：**
- `crates/dream-core-conversation/src/state.rs` —— 2 个新 trait（+ `LlmCallTrace` 附近）。
- `crates/dream-core-conversation/src/service.rs` —— 2 个槽位字段、构造器初始化、2 个 setter。
- `crates/dream-core-conversation/src/lib.rs` —— `pub use` 导出 2 个 trait。
- `crates/dream-core-conversation/src/stream_relay.rs` —— `RelayOutcome.final_text` 字段 + `finalize` 赋值。
- `crates/dream-core-conversation/src/turn_orchestrator.rs` —— `run_user_turn` 内 2 处调用（extract 在 loop 后、inject 在 loop 前）+ `prepend_preset_context` helper + `#[cfg(test)]` 桩与用例。
- `crates/dream-core-app/src/router/routes.rs` —— `OneMemoryTurnExtractor` + `OneMemoryContextProvider` struct/impl（`#[cfg(feature="enterprise")]`）+ 2 处 wiring。
- `crates/dream-domain-memory/src/service.rs` —— （INJECT 检索质量）`search_items` 分词分支或新方法 + 测试。
- `crates/dream-domain-memory/src/lib.rs` —— 若加分词方法则导出。

**可能改（取决于抽取策略 L 的 provider 来源）：**
- `crates/dream-domain-memory/`（migrations + service + routes + models）—— tenant 级 `extraction_channel_id` 配置。
- `crates/dream-core-conversation/src/state.rs` + `turn_orchestrator.rs` —— `extract_from_turn` 增 `turn_model` 参数。
- `crates/dream-core-app/src/router/routes.rs` —— extractor 注入 `provider_repo` + `encryption_key`。

**新增：**
- `crates/dream-core-app/tests/memory_pipeline_e2e.rs`（enterprise）—— extract + inject 端到端。

---

## E. 最高风险点

1. **注入进热路径**（`run_user_turn` attempt loop 之前 `await` 一次 `recall`）。缓解：trait 契约要求
   「一次索引查询 + 出错返回空」；adapter 里 `search_items` 有 `LIMIT`；可加 `tokio::time::timeout(200ms)` 包一层，
   超时当空。**这是 parity-check §三-1 点名的「碰两处热路径，风险高」的那一处。**
2. **`preset_context` 只首轮注入**（§B.4）。若产品期望「每轮都带最新记忆」，第一版达不到，需第二版每轮 hook。
   必须提前对齐预期，否则验收时被当 bug。
3. **抽取质量**（§A.6）。坏抽取器 = 持续污染未来所有轮次的上下文。缓解：LLM 方案 + 严格 system prompt +
   `importance` 阈值过滤 + 「先不做就只留 seam」的诚实退路。
4. **`search_items` 的 LIKE 整句几乎不命中**（§B.6）。不修分词，INJECT 半边基本是死代码 —— 看起来接了、实际召回≈0。
   必须在本方案内一起改分词。
5. **personal collection 懒建的并发**（§A.5）——两个并发首轮造两个 collection。低危（功能不坏，只是多一行），
   但要么加 per-user 锁要么接受。
6. **`RelayOutcome.final_text` 与 replay/continuation 的交互**：一轮里有多次 attempt（system-response 续写、auto-replay），
   `turn_orchestrator.rs:442` 每次 `merge`。要确认取的是**最后一次成功 attempt** 的 `final_text`，不是中间续写的。
   （`last_outcome` 在 `turn_orchestrator.rs:500` 已是这个语义，跟着它取即可。）
7. **cron / IM channel 触发的轮次也会抽取**（`run_agent_turn` → `run_user_turn`）。通常是想要的（定时任务里用户说的话也该记），
   但 `input.content` 对 cron 可能是系统合成的 prompt（不是真用户输入）——抽取 system prompt 会产生垃圾记忆。
   缓解：seam 调用点可加 `input.conversation.source` / 是否 cron 的判断，cron 触发的轮次跳过抽取（注入仍做）。
   `run_agent_turn` 的 `ConversationAgentTurnRequest` 有 `persist_user_message` 标志可作信号。
