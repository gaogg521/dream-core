# 2026-09-18：上下文窗口 1M + 团队调度三个 bug + 服务商额度拒绝停全队

> **新会话/新 AI 首读**：本文档记录 dream-core 与 dream-engine 两侧。前端（上下文指示器、
> 压缩提示 i18n、团队额度提示、#4198 等待确认图标）见 dream-ui 同名文档
> `session-2026-09-18-context-compaction-and-team-provider-block.zh-CN.md`。
>
> 上下文窗口/压缩阈值的**权威定义在 dream-engine**，不要再在 dream-core 打补丁——本次就是
> 先走了这条弯路然后回退的，见第一节。

## 一句话总结

三件事，起因都是真实用户现场：

1. 模型窗口默认从 200k 提到 **1M**、80% 自动压缩，并且服务商说"prompt 太长"时能**自愈**；
2. 团队调度三个并发/状态 bug（finalize 去重形同虚设、失败被吞 5 秒、陈旧信号把槽位卡在
   "正在处理中"）；
3. 服务商因**额度**拒绝时**暂停整个团队**，而不是拿队列去撞墙——用户一觉醒来主对话堆了
   87 条无效排队，而真正的活儿已经被丢掉了。

---

## 一、上下文窗口与压缩（dream-engine 权威）

### 改了什么

| 位置 | 改动 |
| --- | --- |
| `dream-engine-config/src/compact.rs` | `default_context_window` 200_000 → **1_000_000**；`default_autocompact_threshold_pct` `None` → **`Some(80)`** |
| `dream-engine-agent/src/compact/emergency.rs` | 新增 `emergency_limit()`，把 `emergency_buffer` **夹到「触发点以上余量的一半」** |
| `dream-engine-providers/src/error.rs` | `is_context_overflow()` / `parse_context_limit()`，**按关键词短语定位**上限数字 |
| `dream-engine-agent/src/engine.rs` | `run_turn` 接住 `PromptTooLong` → 收窄窗口（只收不放）→ 强制压缩 → **只重试一次**；`output_budget()` 按窗口钳制 `max_tokens` |
| `dream-core-ai-agent/.../dream_engine/agent.rs` | `apply_context_window_policy` 砍到只传"用户声明的窗口"，阈值逻辑全部删除 |

### 三个真实的坑

**绝对值缓冲在非 200k 窗口上全是坑。** `output_reserve`/`autocompact_buffer`/
`emergency_buffer` 都是按 200k 调的绝对 token 数：1M 窗口下 autocompact 落在 96.7%，
离紧急阻断只剩 30k；**8k 本地窗口下紧急阻断线（5192）比 80% 触发点（6553）还低**，压缩
永远没机会跑。后者是写测试时真跑出来的，不是推演。改任何一个阈值都要重新检查
`trigger < emergency` 这个不变式。

**解析上限别扫数字。** OpenAI 一条报错里有四个数：`maximum context length is 128000 ...
you requested 130500 (125000 in the messages, 5500 in the completion)`——取最小值会学成
5500。必须按关键词短语定位；Anthropic 那种 `> 200000 maximum` 数字在关键词**前面**，要
单独处理。

**`max_tokens` 从不按窗口钳制（真机挖出来的独立 bug）。** OpenAI 默认输出预算 32000，而
服务商把**输出也算进窗口**。32768 窗口的模型，prompt 才 12055 也照样被拒：
`you requested about 44055 tokens (7451 text + 4604 tool + 32000 output)`。压缩救不了——
不存在小到能通过的 prompt。已在 `output_budget()` 里按「窗口 − 当前上下文 − 窗口/10」
钳制，只降不升。**钳制时要给 extended thinking 留底**（`thinking_floor()`），否则
`max_tokens < thinking.budget_tokens` 在 Anthropic 上是硬 400——这是我自己引入又修掉的。

### 验压缩的快路子

会话 workspace 根目录放 `.dream.toml`，`Config::resolve` 会读，dream-core 用
`get_or_insert` 不会覆盖：

```toml
[compact]
context_window = 60000
autocompact_threshold_pct = 10   # 10% = 6000 tokens，一轮就触发
```

不用改代码、不用重编。

---

## 二、团队调度的三个 bug

都在 `dream-core-team`，都做过**回滚验证**（把修复关掉，测试立刻红）。

### 1. finalize 去重形同虚设（`scheduler/dedup.rs`）

原实现是"先查再插"，两个线程能同时通过。**`#[tokio::test]` 默认单线程，所以原来的测试
根本测不出并发**——换成 `flavor = "multi_thread", worker_threads = 8` 之后，回滚验证显示
8 次 claim 里漏了 5/2/3 次。改成 `entry()` 原子占位。

**我自己在这里又埋了一个**：清理用 `duration_since` 判断是否是自己那次 claim，而它在
"存储的时间比参照点新"时会饱和成 0，导致新 claim 看起来和要过期的那次一模一样。改成
`*claimed_at == now` 精确比对。

### 2. 失败的 finalize 被吞 5 秒（`session.rs`）

`on_agent_finish` 的退出路径里，只有成功那条会 `clear_finalized_turn`。拆开之后走
`finalize_claimed_turn`，**每条退出路径都释放**。

### 3. 陈旧信号把槽位卡在"正在处理中"（`work_coordinator/coordinator.rs` + `event_loop.rs`）

`complete_signals` 原本是**全有全无**：一批 intent 里只要有一个对不上（已经 Completed、
或落在别的 slot、或带着 mailbox 消息），整批 `Rejected`。而 `event_loop` 的
`SettleSignals` 分支**不 break**，于是死循环重试同一批——槽位永远停在 `Queued`，UI 永远
显示"正在处理中"。

改成**逐个 intent 判定**：不存在/已 Completed 算已落地；落在别的 slot 或带 mailbox 的跳过；
只有一个都没落地才 `Rejected`。事件循环那边 `if !handle_signal_intents(...).await { break; }`。

**这同一个缺陷也解释了"队长收不到成员消息"**——信号不落地，`prepare_next_batch` 就永远
返回不了 `Execute`。用户反复反馈的"每次需要我去提醒队长"是这个，不是唤醒逻辑的问题。

---

## 三、服务商因额度拒绝 → 暂停整个团队（本次重点）

提交 `fix(team): stop the team when the provider refuses on spend grounds`。

### 现场

用户截图：主对话「正在处理中……已排队 87 条」，成员会话里是
`USER_LLM_PROVIDER_RATE_LIMITED` + `AccountQuotaExceeded`（5 小时配额耗尽）。一夜之间攒了
87 条。

### 三个缺陷叠在一起

**1. 服务商的错误分类在团队层被丢掉了。** 供应商层明确把限流标成 `retryable: true`
（UI 上那个绿色"可重试"标签就是它），但团队层拿到的 `AgentTurnOutcome.status` 只有
`Completed / Failed / Skipped` 三个值——调度器**根本分不清**"服务商没额度了"和"这个成员
干砸了"。

**2. 失败后没有任何退避。** `fail_batch` 之后返回 `ContinueDraining`，循环立刻抓下一个批次
再打一次。整个 crate 里搜不到一处对失败的 sleep/backoff。配额耗尽是**毫秒级快速失败**的，
所以它整夜在满速空转。

**3. 报错反而往队长队列里灌。** 成员失败 3 次（`MAX_MESSAGE_DELIVERY_FAILURES`）后，
`notify_leader_delivery_exhausted` 往队长信箱**再写一条消息并唤醒队长**。队长自己也在被
限流、排不动队，于是每个成员的每次失败都在给一个本来就堵死的队列加料。

**更坏的副作用**：那 3 次用完之后消息被 `mark_read` **直接丢弃**。一次配额中断会**真的
吃掉队列里的工作内容**——不是延后，是没了。这就是"87 条堆着但活儿没了"的根。

### 怎么修的

把分类从会话层一路透传到调度器：

```
ProviderError::RateLimited
  → AgentErrorCode::UserLlmProviderRateLimited      (已有)
  → ErrorEventData.code                              (已有)
  → ConversationTurnResult.error_code                (新)
  → ConversationAgentTurnOutcome.error_code          (新)
  → 团队 AgentTurnOutcome.error_code                 (新)
  → event_loop 分流
```

`AgentErrorCode::is_provider_spend_block()` 只认三个码：`RateLimited` /
`QuotaExhausted` / `BillingRequired`。**刻意窄**：auth / permission 也要用户出手，但需要的
是完全不同的话术；network / timeout 是真会自己好的，交给现有的 3 次投递预算。有测试钉死
这个集合（`agent_error.rs` 的 `only_spend_refusals_halt_an_automated_driver`）。

命中时走 `halt_team_on_provider_spend_block`：

- **不重试**，不再拿队列撞墙；
- **不消耗投递预算**（`DeliveryOutcome::NotFailed`），消息保持未读，由
  `reconcile_mailbox_snapshot` 在恢复后重新派发；
- **不通知队长**——那条正是放大器；
- **暂停每一个槽位**，并记住是**哪个槽位**撞的墙
  （`provider_spend_blocked_by: Option<String>`）。

### 两个容易写错的地方

**意图要标 `Cancelled` 而不是 `Failed`。** `run_summary_locked` 里
`failed_intent_count > 0` 会把整个 team run 翻成 `Failed` 并发 `team.runFailed`。暂停不是
失败。用 `Cancelled` 和用户手动暂停走同一套语义。有断言钉死不发 `team.runFailed`。

**解除只认真人。** `clears_provider_spend_block()` 比 `resumes_paused_slot()` 更窄——后者
还包含 `LeadIntervention`，但队长自己也被暂停着，它对服务商的额度无能为力。解除时**整队
一起解**，不能只解用户打字的那个槽位，否则其余成员还停着。

### 为什么是停全队，不是只停那一个

不同专家可以配**不同服务商**（用户现场就是队长 deepseek、成员 doubao），所以"doubao 爆了
把 deepseek 队长也停掉"直觉上像过度杀伤。**用户明确拍板保持全队停。**

理由：队长继续跑也只会不停派活给一个答不了的专家，最后照样卡住——而且正是那样攒出了
87 条。配套条件是提示**会点名**是哪个专家的额度被限制，所以"停全队"不会让用户不知道该去
查哪家的额度。

---

## 四、怎么在本地造一次真实的服务商报错

不用等真额度爆，也不用只靠读代码。起一个本地 HTTP 服务按 OpenAI 兼容格式恒返 429 +
**照抄用户截图原文**的 `AccountQuotaExceeded` 包体，再建一个 provider 指过去。

- `/v1/models` 要返 **200**，否则 provider 建不起来；只让 `/v1/chat/completions` 返错。
- 报错包体别自己编：分类逻辑按关键词短语匹配，编的包体可能刚好绕开要验的分支。
- dev 必须跑**改过的** `dreamcore.exe`：`DREAM_BACKEND_BIN=<path>`。
- **重编前先停 dev**，否则 `cargo build` 挂 `os error 5 拒绝访问`。
- 隔离用 `DREAM_MULTI_INSTANCE=1`（数据目录 `dream-ui-Dev-2`、跳过单实例锁），配
  `DREAM_DEVTOOLS_CDP_PORT=9230` 做 CDP。

本次真机结果：

```
Provider error: Rate limited, retry after 5000ms
code=Some(UserLlmProviderRateLimited) ownership=Some(UserLlmProvider)
team work batch terminal classification="provider_spend_blocked" exhausted_message_count=0
team halted: model provider refused on spend grounds already_blocked=false
team paused: the model provider refused on spend grounds; waiting for the user
```

只发 **1 次**真实请求（不是 3 次）；`exhausted_message_count=0`（活儿一条没丢）；再发一条
消息恢复后第二次 `unread_count=2`，被保住的原消息跟新消息一起重投。

---

## 五、测试与验证纪律

- `cargo nextest run -p dream-core-team -p dream-core-conversation -p dream-core-api-types`
  ——1897 条 **19 秒**。
- **别往 `-p` 里加 `dream-core-app`**：那是 e2e 大套件，`message_e2e`/`mcp_e2e` 每条
  60~80 秒，本次误加后 27 分钟才跑到 507/2203。app 那层如果只是结构体多个字段，编译器
  已经验过了。
- clippy 用 `--no-deps`：`dream-core-auth` / `dream-core-conversation` 有 5 条改动前就存在
  的告警，按 ratchet 规则不归本次修。要确认不是自己引入的，用 `git stash` 比对基线数量。
- 绿测试不等于有效测试。本次每条关键断言都做了**回滚验证**：把分流条件改成恒 `false`，
  测试立刻变红（3 次尝试 + 消息被丢），改回来才绿。

## 六、还没做 / 已知问题

- **成员因非额度原因反复失败时，`notify_leader_delivery_exhausted` 的放大仍在**：每条消息
  3 次预算是有界的，但新消息不断到达时总量无界。合并同一槽位的重复"已暂停"通知还没做。
- `engine.rs:1102` 截断工具调用时仍直接发英文串（没走 `emit_info_coded`），正常使用可达。
- `DEFAULT_CHAR_BUDGET` 那句 "2% of 200k × 4" 的注释已过时（`bootstrap.rs` 传的是 `None`，
  固定 16k 字符），**别照着它改成 80_000**。
- 队长自动下线不用的专家：用户明确说先不做。
- 团队不活动看门狗（inactivity watchdog）：上游 `gaogg521/dream-core` 也只有定义没接线
  （58 个 `dream-core-team/*.rs` 全扫过），用户决定跟着不做。
