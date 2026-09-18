# 设计：跨会话消息投递（与团队邮箱的关系）

> 日期：2026-09-18 ｜ 状态：**设计，未实现**
>
> 这份文档只回答一个必须先拍板的问题：**团队邮箱和「会话对会话」投递，是合并成一套，还是分层？**
> 这个问题不定，写代码一定返工——两套消息语义各做一半，正是"做得不好"的来源。
>
> 前置调研见 dream-ui `docs/guides/session-2026-09-18-upstream-harvest-and-cross-session-archive-research.zh-CN.md` §4。

---

## 1. 现状事实（都带代码位置，不是印象）

### 1.1 团队成员本身就是会话

`TeamAgent`（`dream-core-team/src/types.rs:95`）带 `conversation_id`。**一个 team = 一组 conversation + 一个邮箱 + 一个调度器/事件循环**。
所以"团队内 agent 互发消息"在物理上**已经是**会话对会话的投递了。

### 1.2 团队投递已经落在 `ConversationService` 上

`dream-core-app/src/router/team_conversation_adapters.rs:98` —— 团队的 `AgentTurnExecutionPort` 实现最终调的是：

```rust
self.conversation_service.run_agent_turn(ConversationAgentTurnRequest {
    user_id, conversation_id, content, files,
    inject_skills: Vec::new(),
    persist_user_message: false,     // ← 注意
    user_message_hidden: false,
    on_started,
})
```

目标忙时的处理是 `runtime_state().wait_until_unclaimed(&conversation_id).await` —— **阻塞等到目标空出来再重试**。

### 1.3 普通发送路径已经在拒绝团队会话

`ConversationService::send_message`（`dream-core-conversation/src/service.rs:3963`）开头就有：

```rust
if let Some(team_id) = team_id_from_extra(&row.extra) {
    return Err(ConversationError::Forbidden {
        reason: "Team-owned conversations must be sent through Team API".into(),
    });
}
```

**这条边界是免费的**：一个 agent 哪怕从别处拿到 team 会话 id，走普通投递照样被拒。

### 1.4 发送边界已经有原子的标记块解析

同一个 `send_message` 里，文件附件在**发送边界**被原子解析成内联的 `[[DREAM_FILES]]`（`service.rs:4040` 附近，"a bad reference fails the whole send"）。`@@` 要插入的就是这个点，**不需要新建边界**。

### 1.5 RuntimeState 已经能回答"目标现在能不能收"

`dream-core-conversation/src/runtime_state.rs`：`is_claimed` / `is_cancelling` / `is_restarting` / `is_deleting` / `wait_until_unclaimed` / `active_turn_id_for` 全都在。

### 1.6 agent 可调的 CLI 表面已经有域索引

`cmd_capabilities.rs:58` 的 `domains` 已经列了 `config` / `diagnose` / `team`，加一个 `session` 域是自然扩展。每个会话的进程里 `ONE_HELPER_BIN` / `ONE_CONVERSATION_ID` / `ONE_RUNTIME_TOKEN` 都已注入。

---

## 2. 结论：**分层，不合并**

虽然 §1.1 / §1.2 看起来"两者本来就是一回事、合并即可"，但有三条**结构性差异**，合并会同时弄坏两边。

### 差异一：并发模型相反

| | 团队邮箱 | 跨会话投递 |
|---|---|---|
| 驱动 | **每个 slot 一个独立事件循环**（`EventLoop::spawn(slot_id, ...)`） | 一个**共享 drainer** |
| 目标忙时 | `wait_until_unclaimed().await` 阻塞等 | **绝不能阻塞** |

团队那边阻塞是安全的，因为堵住的只是**那一个 slot 自己的循环**。跨会话投递如果照抄，一个跑十分钟的目标会把**所有用户的所有待投递**堵死。

> 这也是上游明确选 `send_message` 而不是 `run_agent_turn` 的原因（前者排上轮次就返回）。**我们团队路径用的恰恰是 `run_agent_turn`——所以它不能直接复用。**

### 差异二：持久化与可见性语义相反

- 团队：`persist_user_message: false`，消息**不作为用户消息进入目标会话历史**，而是走 `mirror_unread_to_conversation`（`session.rs:1144`）投影成团队视图。
- 跨会话：语义必须是**"等价于用户打开那个会话按了发送"**，那就**必须**持久化成一条真实的用户消息，否则用户回头看历史会发现凭空多了一轮回复而没有提问。

这两个语义**不能由同一个调用点同时满足**。

### 差异三：寻址与权威不同

- 团队：`(team_id, slot_id)`，存在**调度器和 leader**，有编排语义（谁该干活、批次、重试耗尽通知 leader）。
- 跨会话：`(user_id, conversation_id)`，**没有编排者**，是对等投递，收件方自己决定要不要回。

把对等投递塞进有 leader 的模型里，就得凭空发明一个"谁是 leader"。

### 因此

> **团队邮箱保持原样，作为 team 内编排通道。跨会话投递是另一条更薄的路径，建在 `send_message` 上。两者靠 §1.3 那条已经存在的拒绝天然隔开。**

这不是妥协，是各自用对了自己的并发与持久化模型。**共享的是 `ConversationService`，不是投递层。**

---

## 3. 跨会话投递的设计

### 3.1 语义定义（一句话，其余都从它推导）

**一个 agent 给同一用户的另一个会话发消息 ≡ 用户打开那个会话按了发送。** 收件会话起一轮，自己决定要不要回。

从这一句推出来的：必须持久化成用户消息；必须走 `send_message`；必须继承 `send_message` 已有的全部闸门（团队拒绝、模型白名单、企业策略）——**不另写第二条发送路径**，否则这个等价关系要靠纪律维持，迟早失守。

### 3.2 投递循环

- **用 `send_message`，不用 `run_agent_turn`**（理由见差异一）。
- **固定 1 秒 tick，不做退避。** 退避在这里方向是反的：目标那轮跑得越久，等待就越长，结果正好在目标空出来的时候在睡觉。
- **排队由错误驱动，不由能力表驱动。** 一个"支持轮次中投递"的目标，如果那轮正卡在确认卡片上，照样得排队。按能力表推断"支持 ⇒ 直接投递"会漏掉这个 case。
- 队列在内存里；进程重启即丢。这是**刻意的**：待投递不是用户数据，重启后重放一条几分钟前的消息比丢掉它更糟。

### 3.3 两个标记块

- 发件侧：`@@` 在发送边界解析成 `[[DREAM_SESSIONS]]`，**原子**——一个引用错误就让整条消息失败（和 `[[DREAM_FILES]]` 同样的规则）。
- 收件侧：`[[DREAM_SESSION_MESSAGE]]` 带上发件人名字、id、回信地址、以及**双方 workspace 是否相同**。最后这个字段不是装饰：没有它，收件 agent 会把一个相对路径解到错误的目录上。
- **两个块的内容一律由服务端从数据库行生成，绝不接受客户端传入的名字/workspace。**

> ⚠️ **必须在设计阶段就答的问题**：标记块在提示里是**靠位置被信任的纯文本**。用户自己打一个 `[[DREAM_SESSION_MESSAGE]]`、或者 agent 从网页上读到一段再原样回显，下游分不出真假（这个性质 `[[DREAM_FILES]]` 已经有了，是既有面，不是新引入）。
> 跨会话把它**从"用户自己骗自己"升级成"A 会话能伪造成 B 会话的身份"**，所以这条路径落地时必须同时决定：是在发送边界转义用户文本里的标记串，还是给块加一个服务端签发的一次性随机标签。**建议前者**——转义比签名简单，且能顺带收掉 `[[DREAM_FILES]]` 的既有面。

### 3.4 防风暴与开关

- **两个滑动窗口**：会话级出站、会话对之间往返。触发时广播事件，让 UI 能给出"停止"。
- **固定校验顺序**，让同时命中多个拒绝条件的请求总是返回同一个码；**限流闸排在目标查询之前**，这样刷垃圾请求不会每次都付一次 DB 读。
- **取消和重启是不同意图**：用户点停止要清掉发给该会话的待投递（否则"停止"是假的，drainer 马上又把它叫醒）；运行时重启要保留，因为重启后会话回到 idle，而 idle 正是待投递在等的状态。
- 目标正在被删除（`is_deleting`）时直接丢弃，不排队——我们的 `delete` 是硬删除，排队等一个即将不存在的行没有意义。

---

## 4. 四个「我们和上游不一样」的点

### 4.1 总开关在企业版必须是能力，不只是用户设置

上游是 `system_settings` 一个 per-user 列。我们有企业能力下发（`one_user_org` 是唯一凭据），所以这个开关得是**两层**：企业侧能力闸 + 用户侧偏好。
并且要注意我们既有的设计张力：**安全策略对无租户用户 fail-open**。跨会话投递如果也 fail-open，个人版就是"默认开"，那是对的；但要确认企业版关掉之后，**客户端跑的个人版构建**里这个闸是不是真的编进去了（见 memory `client-runs-personal-build-gates-absent`）。

### 4.2 别再加一条常驻注入的技能

上游说"auto-inject 技能是唯一能真正教会一个普通会话某个 CLI 的机制"。但我们刚把 auto-inject 描述预算压下去（`one-config` 677→197 字符），并加了 200 字符的测试守卫。

**建议**：先不新建 `session` 技能，而是把 `session` 域挂进**已有的 `domains` 索引**（`cmd_capabilities.rs:58`），让 agent 通过 `one-config` 已经教会的 `capabilities` 发现机制找到它。只有实测证明发现率不够，再考虑加技能——加的话必须 ≤200 字符，守卫会拦。

### 4.3 我们的 `delete` 是硬删除

目标会话被删时，队列里发给它的待投递必须丢弃。`RuntimeState::mark_deleting` / `is_deleting` 现成可用。上游没这个问题是因为它们同期上了归档（软删除）——**我们如果按调研建议先做归档，这一条会变简单：归档只是停运行时 + 留数据，待投递可以保留到取消归档。**

### 4.4 团队会话的边界是免费的，但要写成测试

§1.3 那条拒绝让"agent 拿到 team 会话 id 也投递不进去"自动成立。**这是结构性质，不是巧合，必须有一条测试钉住它**——否则哪天有人放松了那个拒绝，跨会话投递会跟着一起破防，而症状会出现在很远的地方。

---

## 5. 分期建议

| 期 | 内容 | 依赖 |
|---|---|---|
| 0 | **本文档这个决定**（分层 / 不合并） | 无 —— 已完成 |
| 1 | 归档（见调研 §4.3） | 无。**建议先做**，它让 4.3 变简单 |
| 2 | 发送边界的标记串转义（收掉 §3.3 那个既有面） | 无。独立小改动，先做掉不吃亏 |
| 3 | `session` 域 CLI + `capabilities`（只读：list） | 无 |
| 4 | 投递循环 + 两个标记块 + 限流 + 开关 | 期 2、3 |
| 5 | 前端 `@@` 输入与跨会话送达提示 | 期 4 |

---

## 6. 明确未决（需要产品拍板，不是技术问题）

1. **跨会话投递要不要允许跨 workspace？** 技术上可以（带 workspace-match 字段即可），但一个 agent 能驱动另一个目录里的 agent 干活，权限模型上是另一件事。
2. **收件方回信默认是回给发件会话，还是不回？** 上游是"自己决定"。我们要不要给一个显式的"需要回复/不需回复"标志。
3. **企业版是否默认关闭。**
