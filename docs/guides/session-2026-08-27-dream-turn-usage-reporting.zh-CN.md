# 2026-08-27 dream 会话上报每轮 token 用量（上下文指示器此前一直空白）

> **一句话**：`dream`（1ONE CLI）类型会话的上下文/花费指示器**从来没显示过**，
> 因为后端根本不上报用量——唯一携带 token 计数的发射点只被独立 CLI 调用，而
> 进程内这条路把引擎返回值直接丢掉了。已修：接住返回值，在 `Finish` 之前发一帧
> `AcpContextUsage`。
>
> **范围**：仅 `dream-core`（`manager/dream_engine/agent.rs`）。**不需要改
> `dream-engine`**——`AgentEngine::context_status()` 本来就是公开的。dream-ui
> 侧有三个配套缺口，在那边另修（见文末）。
>
> **提交**：`8d3e8bc`。⚠️ **必须重编内嵌二进制才在 dev 生效**，见第五节。

---

## 一、现象与它的误导性

用户说"我记得做过一个上下文花费和 token 使用的小功能，好像没找到了"，并附了一张
截图，里面那个 popover 是好的：

```
16.2% · 162.5K / 1M 上下文已使用
会话成本 ≈ US$12.2492
输入 280 · 输出 7.3K · 缓存读 1.6M · 缓存写 146.1K · 思考 2.7K
```

**功能没丢。** 那张截图是 ACP 会话（Claude Code / Codex CLI）——1M 上下文 +
缓存读写明细正是 Claude 经 ACP 的形状。真正的事实是：**它对 `dream` 类型会话
从来不显示**，而用户日常用的正是 1ONE CLI。

另一个容易误判的点：用户的两张截图看起来像"对话模式有、媒体模式没有"，其实差别
是**欢迎页 vs 会话内**（一张写"全自动"，另一张写"**权限 ·** 全自动"）。渲染条件
是 `tokenUsage ? <Indicator/> : undefined`，与媒体模式无关。

---

## 二、真机抓帧：整轮没有任何用量帧

在 dev 里挂上 `ipcBridge.conversation.responseStream` 的监听，发一轮正常完成的
对话（"3+3=?"），整轮收到的帧类型只有：

```
start · thinking · content · finish
```

而 `finish` 的 data 是：

```json
{ "session_id": null }
```

`FinishEventData` 上 `model` / `input_tokens` / `output_tokens` 三个字段都带
`#[serde(default, skip_serializing_if = "Option::is_none")]`，而 `session_id`
只有 `#[serde(default)]`。所以**键整个不存在**（而不是为 null）就证明构造时那三个
字段全是 `None`——排除了"发了但是 0"这种可能。

`GET /api/conversations/{id}/usage` 对所有会话都返回 200 但 `used` 为空，包括
刚跑完两轮对话的那个——**后端侧也没落下任何东西**。

---

## 三、根因：唯一带用量的发射点只被独立 CLI 调用

`BackendOutputSink::emit_stream_end`（`capability/backend_output_sink.rs`）是
唯一会填 `input_tokens`/`output_tokens`/`model` 的地方。查它的调用者，全部在
**`dream-engine-cli`**：

- `crates/dream-engine-cli/src/run.rs:222`
- `crates/dream-engine-cli/src/json_stream/message.rs:53`

CLI 的做法是：`engine.run()` 返回后，从 `run_result.usage` 里手动取四个数调
`emit_stream_end`。

而本仓是**进程内**跑同一个引擎（`manager/dream_engine/agent.rs:147` 把
`BackendOutputSink` 作为 `OutputSink` 传给引擎），并且在 `run_with_blocks()`
返回后只做了：

```rust
Some(Ok(_)) => {                       // ← 返回值被丢掉
    self.runtime.emit_finish(None);    // ← 不带任何用量
    Ok(())
}
```

`AgentResult.usage` 里 `input_tokens` / `output_tokens` /
`cache_creation_tokens` / `cache_read_tokens` **全都在那儿**，从来没人读。

### 三个 Finish 发射点，两个不带用量

排查时值得知道的地图：

| 发射点 | 带用量 | 谁在用 |
| --- | --- | --- |
| `BackendOutputSink::emit_stream_end` | ✅ | 只有 `dream-engine-cli` |
| `AgentRuntime::emit_finish` | ❌ `..Default::default()` | 本条路径 + `manager/acp/agent.rs` |
| `SessionAgent::emit_finish_once` | ❌ `..Default::default()` | `session_agent.rs`，三处**全是取消/kill 路径** |

⚠️ 顺带排除一条歧路：`session_agent.rs:3325` 的注释说这条路径"故意不把用量放在
Finish 上，而是由 pump 把每个 `UsageDelta` 持久化到 `context_usage` 并直接广播"。
那是**另一条**（ACP/codex/claude backend）的设计；`UsageDelta` 只由
`dream-core-session` 的 `adapter/claude.rs` / `backend/{acp,claude,codex}_conn.rs`
产生，**没有 dream 的**。所以本轮一个用量帧都没广播不是过滤掉了，是压根没产出。

---

## 四、修法

### 4.1 接住返回值，在 Finish 之前发用量帧

```rust
Some(Ok(run_result)) => {
    // BEFORE the Finish: relay 一看到 Finish 就停止转发这一轮
    self.emit_turn_usage(&engine, &run_result);
    self.runtime.emit_finish(None);
    Ok(())
}
```

⚠️ **顺序是硬要求**：relay 在 `Finish` 处结束对该轮的转发，发在后面会被丢掉。

窗口大小取 `engine.context_status()`——它是 `pub`，所以**不需要改
dream-engine**。`ContextStatus` 带 `context_usage`（占用）与
`context_window`（窗口），正是把一个裸 token 数变成百分比所需的两个值。

### 4.2 为什么发 `AcpContextUsage` 而不是往 `Finish` 上加字段

- 名字是历史遗留（`session_agent.rs` 的 `broadcast_usage_frame` 注释自己写着
  "Fires for every backend"），语义上它就是**会话级**用量报告；
- `{used, size, _meta}` 这个形状是渲染层**已经在读**的；
- 和 `Finish` 不同，它能携带上下文**窗口**——而窗口正是这个指示器存在的意义。

### 4.3 ⚠️ 两个 `input_tokens` 语义相反（本节最重要）

| 来源 | `input_tokens` 的含义 |
| --- | --- |
| 引擎 `TokenUsage`（`dream-engine-types/src/message.rs:223`） | **完整** provider 输入，**含** cache 读与 cache 写 |
| 渲染层 breakdown | 缓存**没有**覆盖的那部分**新**输入 |

渲染层之所以是后者，是因为它的缓存命中率要拿它当分母：
`cached_read / (cached_read + input)`。**原样透传会把缓存 token 双算，把命中率
报得远低于真实值**——一个看起来完全合理的错数字。

所以帧构造里先减掉：

```rust
let fresh_input = usage.input_tokens
    .saturating_sub(usage.cache_read_tokens)
    .saturating_sub(usage.cache_creation_tokens);
```

`saturating_sub` 不是防御性冗余：厂商报出比自己输入总量还大的缓存数时，普通减法
会让 u64 下溢成天文数字；钳到 0 读作"全部来自缓存"，是那种（不可能的）报告的
诚实读法。

这一条是把帧构造抽成纯函数 `build_turn_usage_frame(context_usage,
context_window, usage)` 的**唯一理由**——否则它就是一行 `json!`。

### 4.4 `used` 是占用而不是本轮总和

指示器回答的是"窗口还剩多少"，用每轮的 token 总和会**每条消息都归零**。所以
`used` 取 `context_status().context_usage`，`_meta` 里才是本轮的收支明细。
`size: 0` 是渲染层自己约定的"窗口未知"编码，也正是没配窗口时解析出的值。

---

## 五、验证

### 5.1 单测（5 条，`manager/dream_engine/agent_test.rs::turn_usage_frame`）

- `reports_fresh_input_not_the_cache_inclusive_total`——4.3 那条语义反转
- `keeps_the_uncached_remainder_when_there_is_one`——有余量时正常相减
- `clamps_instead_of_underflowing_on_an_impossible_report`——饱和减
- `carries_context_occupancy_and_window_rather_than_a_turn_total`——4.4
- `passes_an_unknown_window_through_as_zero`——`size: 0` 的约定

`cargo test -p dream-core-ai-agent turn_usage_frame` → 5 passed。

### 5.2 真机（必须先重编内嵌二进制）

⚠️ **改了本仓代码不重编内嵌，dev 里跑的还是旧后端**：

```powershell
cd dream-core
cargo build -p dream-core-app --release
cd ..\dream-ui
$env:DREAM_BACKEND_LOCAL_PATH = '..\dream-core\target\release\dreamcore.exe'
node scripts/prepareAioncore.js
```

**`prepareAioncore.js` 的两个坑**：

1. **dev 在跑时会 `EPERM`**（二进制被占用）。必须先停 dev，并确认
   `Get-Process electron` / `dreamcore` 都清零再执行。
2. 拷完二进制后它还会去 npm 拉 managed resources。本轮那步**网络超时失败**了，
   但**二进制已经同步好**——比对内嵌目录与 `target/release` 的时间戳和字节数即可
   确认，那一步失败不影响本仓改动生效。

重启 dev 后发一轮对话，帧序列变成：

```
start · thinking · content · acp_context_usage · finish
                            ^^^^^^^^^^^^^^^^^^ 在 finish 之前
```

帧内容：

```json
{ "used": 7459, "size": 200000,
  "_meta": { "input_tokens": 20200, "output_tokens": 12700,
             "cached_read_tokens": 23800, "cached_write_tokens": 0 } }
```

渲染层显示：

```
3.7% · 7.5K / 200K 上下文已使用
缓存命中率 54.1%
输入 20.2K · 输出 12.7K · 缓存读 23.8K
```

命中率验算：`23.8K / (23.8K + 20.2K) = 54.1%` ✓——分母是未缓存输入，没有双算，
说明 4.3 那个减法接对了。

---

## 六、边界与已知项

- **成本（会话花费）仍然为空**。dream 这条路的成本来自
  `oneBilling.getConversationCost`，而个人版没挂载 billing 域（`/api/one/...`
  返回 404，`governance.ts` 的 `checkMediaPolicy` 注释明写 "fails open on
  transport errors"）。**不是缺陷，别去"修"它**——企业版挂了 billing 域才有数。
- **`thought_tokens` 没有**。引擎的 `TokenUsage` 只有四个字段，没有单独的思考
  token；渲染层那一行对 dream 会话不显示，是"没有"而不是"漏了"。
- **取消/失败的轮次不发用量帧**。`Some(Err(_))` 和 `None`（被 stop 打断）两条
  分支保持原样：一轮没有正常完成时，`AgentResult` 不存在。代价是用户取消的那轮
  不计入显示。
- **未持久化到 `context_usage` 存储**。本轮只做了实时广播；`GET /usage` 对 dream
  会话仍然返回空。dream-ui 侧改为在收到帧时写 `extra.last_token_usage` +
  `last_context_limit`，重开会话靠那个恢复。要让 `GET /usage` 也有数，需要在本仓
  加一条持久化，本轮未做。

---

## 七、dream-ui 侧的三个配套缺口（在那边修）

任何一个单独存在都会让指示器继续空白：

1. `useDreamEngineMessage` **不处理 `acp_context_usage` 帧**——后端对着空房间喊；
2. 它**从不调用 `getUsage`**，所以没有后端快照恢复路径（ACP 那个 hook 有）；
3. `ContextUsageIndicator` 的 `context_limit` 被**硬编码成 0**（原注释说"dream
   从不报告窗口大小"——现在报告了）。

外加新增的缓存命中率显示。完整记录见 dream-ui 的
[`docs/guides/session-2026-08-27-media-endpoint-fallback.zh-CN.md`](https://github.com/gaogg521/dream-ui/blob/main/docs/guides/session-2026-08-27-media-endpoint-fallback.zh-CN.md)
第十一、十二节（那份文档同时记录了本轮媒体协议 fallback、AGNES 视频、oxlint
门禁、媒体计费分档等其余改动）。
