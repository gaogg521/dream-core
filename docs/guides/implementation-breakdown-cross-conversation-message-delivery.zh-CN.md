# 实施拆解：跨会话消息投递（@@）

> 日期：2026-09-19 ｜ 状态：**拆解完成，未实现，等通知开工**
> 设计依据：[`design-cross-conversation-message-delivery.zh-CN.md`](design-cross-conversation-message-delivery.zh-CN.md)（分层决定 + 产品决定 §6 已拍板）。
> 本文只回答"动手时改哪些文件、按什么顺序、每个包怎么验收"，不重复设计论证。

---

## 0. 范围与不做清单

做：设计文档分期 **2、3、4、5**（转义 → session CLI → 投递核心 → 前端）。

**明确不做**（防止范围蔓延）：

| 不做 | 理由 |
| --- | --- |
| 期 1 归档 | 独立工作包；不做归档时 4.3 走 `is_deleting` 丢弃分支，功能完整 |
| 新建 crate / 数据库迁移 | 队列是内存的（设计 §3.2 刻意如此：待投递不是用户数据，重启即丢） |
| 常驻注入的 `session` 技能 | 设计 §4.2：挂进已有 `domains` 索引，auto-inject 预算刚压到 197 字符且有 200 字符守卫 |
| 回信默认 | §6.2：单向投递，回信地址仅在 `reply_requested=true` 时出现在收件块里 |

## 1. 现有挂点（都核实过，带位置）

| 挂点 | 位置 | 用途 |
| --- | --- | --- |
| 发送边界（唯一注入点） | `dream-core-conversation/src/service.rs` `send_message`（≈3963 起）；`resolve_message_attachments`（≈692）是 `[[DREAM_FILES]]` 原子解析的现成范本 | `@@` 解析、标记串转义都在这里做 |
| 团队会话拒绝（免费边界） | 同上，`team_id_from_extra` 检查 | 结构上已成立，需一条测试钉住（§4.4） |
| RuntimeState | `dream-core-conversation/src/runtime_state.rs`：`is_claimed / is_cancelling / is_restarting / is_deleting / mark_deleting / clear_conversation` | drainer 的全部目标状态判断 |
| `next_run` 同款排队语义参考 | 团队路径 `team_conversation_adapters.rs:98` 用 `run_agent_turn` + `wait_until_unclaimed` 阻塞 | 我们**反着来**：`send_message` + 非阻塞轮询（差异一） |
| CLI 域索引 | `dream-core-app/src/commands/cmd_capabilities.rs:58` 的 `domains`（config/diagnose/team 三域） | 加 `session` 域（只读 list）；运行时 env（`ONE_HELPER_BIN` 等）已注入 |
| 用户偏好存储 | `dream-core-system` `ClientPrefService`（lib.rs:80） | 用户侧开关（默认开） |
| 会话列表查询 | `ConversationService::list`（service.rs:2351） | `@@` 引用解析 + `session list` CLI 的数据源 |
| 启动挂载点 | `dream-core-app/src/router/routes.rs` `create_router_with_runtime`（channel 插件恢复等后台任务都在这里 spawn） | drainer 常驻任务的挂载位置 |
| 事件广播 | 现有 event_bus → WebSocket（`forward_event_bus_to_websocket`） | 投递成功/限流触发事件的通道 |

## 2. 工作包分解（建议 4 个 commit，按序）

### WP1：发送边界标记串转义（期 2，小，先做不吃亏）

- 新文件 `dream-core-conversation/src/markers.rs`：
  - 常量：三个标记前缀 `[[DREAM_FILES]]` / `[[DREAM_SESSIONS]]` / `[[DREAM_SESSION_MESSAGE]]`；
  - `escape_marker_text(content) -> String`：把用户文本里的这些字面量打断（建议在 `[[DREAM_` 的 `[[` 与 `DREAM_` 之间插 U+200B 零宽空格——视觉无差、解析必失配）；
  - 顺带收掉 `[[DREAM_FILES]]` 的既有伪造面（设计 §3.3 建议前者）。
- 接入点：`send_message` 里 `resolve_message_attachments` 之前，只对**用户原始文本**转义（服务端自己生成的块在转义之后注入，不受影响）。
- 测试：模块单测——三种标记都被打断、正常 `[[`/`]]` 文本不受影响、转义后引擎侧不再识别（负向验证）。

### WP2：`session` 域 CLI（期 3，中）

- `cmd_capabilities.rs`：`domains` 加第四项 `session`（mode: read-only；`contract_command: "session capabilities"`）。
- 新文件 `crates/dream-core-app/src/commands/cmd_session.rs`：只读 `session list`——当前用户的会话（id、标题、workspace 标记、更新时间），复用 `ConversationService::list` 的过滤语义；走 `ONE_RUNTIME_TOKEN` 鉴权（与 team 域同款）。
- 测试：域出现在 capabilities 索引；list 只返回调用者自己的会话；无 token 拒绝。

### WP3：投递核心（期 4，大——本次的主体）

新文件 `dream-core-conversation/src/session_delivery.rs`（类型 + 队列 + 限流，可单测）+ `dream-core-app` 侧接线：

1. **发送侧解析**：`send_message` 边界把 `@@<token>` 原子解析为 `[[DREAM_SESSIONS]]` 块——按 `user_id` + conversation_id 查库验证归属，**标题/workspace 一律由服务端从库行生成**（设计 §3.3 红线）；一个坏引用失败整条消息。token 语法在动工时定死（建议 `@@conv:<id>`，composer 负责插入）。
2. **入队**：解析出的每个目标 → `PendingDelivery { from_conversation, to_conversation, user_id, content, reply_requested }` 进内存队列。**固定校验顺序**：内容空 → 开关（两层，§6.3 默认全开）→ **限流闸** → 目标存在性（限流排在目标查询前，刷垃圾不付 DB 读）。
3. **drainer**：1 秒 tick、不退避、非阻塞。每轮对每条 pending：
   - `is_deleting(to)` → **丢弃**（硬删除等不到复活）；
   - `is_claimed / is_cancelling / is_restarting(to)` → 保留，下轮再看；
   - 否则调 `send_message`（`persist_user_message` 默认路径——语义 ≡ 用户按发送），内容为服务端生成的 `[[DREAM_SESSION_MESSAGE]]` 块：发件人名/id、**双方 workspace 是否一致**、`reply_requested=true` 时才附回信地址；成功即出队 + 双方广播事件。
   - `send_message` 失败（409 抢占失败等）→ 保留重试；**连续失败计数**超阈值丢弃并广播，防永久毒丸。
4. **限流**：两个滑动窗口——会话级出站、会话对往返（A→B 后 B→A 的翻转频率）。触发：拒绝新入队 + 广播事件（UI 给"停止"）。已入队的是否清空 = 跟随 UI 的"停止"。
5. **取消/重启**：用户停止会话 X → 清空 `to==X` 与 `from==X` 的 pending（否则"停止"是假的）；运行时重启（`is_restarting`）→ **保留**（重启后回到 idle 正是投递在等的状态）。
6. **开关两层**：企业能力闸（默认开，无租户 fail-open）+ `ClientPrefService` 用户偏好（默认开）。关掉后：`@@` 解析、入队、drainer 全停；**`session list` CLI 是否保留**见 §4 待确认。
7. **drainer 挂载**：`create_router_with_runtime` 启动段 spawn（持有 `Arc<ConversationService>` + `Arc<dyn IWorkerTaskManager>`）。
8. **测试**（设计 §4.4 的钉子必须在这批里）：
   - 投递目标为 team 会话 → 被 `send_message` 的团队拒绝挡住（结构性质钉死）；
   - 转义后的用户文本里的伪 `[[DREAM_SESSION_MESSAGE]]` 不会被收件侧当真；
   - 忙目标保队、`is_deleting` 丢弃、空闲即投递且**目标历史里出现一条真实用户消息**；
   - `reply_requested=false` 的收件块**不含**回信地址；
   - 跨 workspace 投递的块带 workspace 不一致标记；
   - 限流先于目标查询（无 DB 读即可拒绝）；
   - 取消清空、重启保留。

### WP4：前端 `@@`（期 5，中，dream-ui）

- SendBox：`@@` 触发会话选择器（数据走现有会话列表 bridge），选中插入 `@@conv:<id>` token；
- 有 token 时显示"要求回信"开关（默认关 → `reply_requested`）；
- 新事件监听：投递成功（双方 toast/状态）、限流触发（给"停止"按钮 → 调取消接口）；
- 设置页加用户开关（默认开）；
- i18n：13 种语言全量（沿用本轮 3.0.6 的教训，更新记录也要列这条）。

## 3. 验收清单（发版前必查，ACP §2.6 同款地位）

1. team 会话拒绝的测试存在且绿（§4.4 的钉子）；
2. 用户文本里的伪标记串转义后不被引擎识别（负向验证做过）；
3. 限流闸在目标 DB 查询之前；
4. `reply_requested=false` 时收件块无回信地址（结构性收口，不是文案承诺）;
5. 跨 workspace 的收件块带不一致标记；
6. 开关三层默认全开 = 上线即生效（所以本清单 1–5 是上线硬前置，设计 §6.3）；
7. 更新记录：按 S1 全量 commit 检测流程列入本轮条目。

## 4. 动工前要拍板的两个小决定（不阻塞准备工作）

| # | 问题 | 建议 |
| --- | --- | --- |
| 1 | `@@` token 的文本形态 | `@@conv:<id>`；composer 插入，服务端解析后从用户可见文本中替换为块 |
| 2 | 开关关闭时 `session list` CLI 是否保留 | 保留（只读、只出调用者自己的数据）；上游关的是"两个列表出口"指 UI 出口。需要你确认 |
