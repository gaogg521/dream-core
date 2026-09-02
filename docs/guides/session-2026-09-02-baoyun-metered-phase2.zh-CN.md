# 2026-09-02：宝云计量代理（模式 B）—— dream-core 侧接线（Phase 2）

> **新会话/新 AI 首读**：本文档只记录 dream-core 这一侧。完整背景（为什么走模式 B、
> broker 的架构与已实现的 Phase 1、未决问题）见 `dream-trial-broker/docs/
> baoyun-metered-proxy-handoff.zh-CN.md`。模式 A（OpenRouter 一键体验）的 dream-core 侧
> 见同目录 `session-2026-08-25-openrouter-trial-model.zh-CN.md`——本次是它的平行兄弟。

## 一句话总结

新增 `MeteredAccessService`：模式 A 让 broker 发一把限额上游 key，模式 B 让 broker
**开一个它自己代理并本地计费的账户**。dream-core 转发本机的 claim / quota / order 请求，
原样relay broker 的答复；**从不接触 master key，也不经手模型流量**。默认关闭
（`DREAM_TRIAL_BROKER_URL` 未配置时报"未配置"）。

## 改了什么（提交 `feat(system): relay metered-proxy (mode B) trial access to the broker`）

| 文件 | 改动 |
| --- | --- |
| `crates/dream-core-api-types/src/provider.rs` | 新增 `MeteredAccessResponse`（`vendor`/`base_url`/`device_token`/`models`/`currency`/`free_grant_cents`/`remaining_cents`）、`MeteredQuotaStatusResponse`（free_grant/purchased/consumed/remaining/exhausted）、`MeteredOrderResponse`（`payment: Option<Value>` 只在创建时有）+ 三个 Request/Query DTO。**刻意不复用 `TrialKeyResponse`**——模式 B 没有可下发的上游 key，金额是厂商币种不是 USD |
| `crates/dream-core-system/src/install_id.rs` | 新文件。把原来 `TrialKeyService` 私有的 `get_or_create_install_id` 抽成 `pub(crate)` 自由函数，pref key/owner 完全不变（`system_default_user` / `trial_broker_install_id`）。模式 A / B 必须给 broker 同一个 install_id——一台设备两种模式是同一台设备 |
| `crates/dream-core-system/src/trial_key.rs` | `get_or_create_install_id` 方法改成一行委托 `crate::install_id::…`，其余不动，测试全绿 |
| `crates/dream-core-system/src/metered_access.rs` | 新文件。`MeteredAccessService { broker_base_url, http_client, client_pref_repo }`，方法 `claim` / `read_quota_status` / `create_order` / `get_order`，打 broker `/v1/metered/*`。broker 状态码映射：404→`NotFound`（按 `error` 字段细分 vendor/account/order）、400 `metered_package_unknown`→`BadRequest`、429→`RateLimited`、其余→`BadGateway` |
| `crates/dream-core-system/src/routes.rs` | `SystemRouterState` 新增 `metered_access_service`；4 条路由 `POST /api/providers/metered/claim`、`GET /api/providers/metered/quota?vendor=`、`POST /api/providers/metered/orders`、`GET /api/providers/metered/orders/{id}`（literal 段注册在 `/{id}` 之前，同模式 A） |
| `crates/dream-core-app/src/router/state.rs` | `build_system_state` 构造 `MeteredAccessService`，读同一个 `DREAM_TRIAL_BROKER_URL`——broker 一个服务同时提供 `/v1/trial-keys` 和 `/v1/metered/*` |
| 6 个 `tests/*_routes.rs` | 同步补 `metered_access_service` 字段（`MeteredAccessService::new(None, …)`）。**注意：只加这 5 行，别对这些文件跑 `rustfmt`**——它们有大量 pre-existing 格式漂移，全量 rustfmt 会重写整个文件 |
| `crates/dream-core-system/tests/metered_access_routes.rs` | 新文件，7 个集成测试，用 `wiremock` 假 broker：claim 转发 install_id、404/400 状态映射、无 broker 报"未配置" |

## 错误分类（单独提交 `fix(errors): map the broker's structured QUOTA_EXHAUSTED to spent-allowance`）

模式 B 余额耗尽时 broker 直接返回结构化 `402 {"code":"QUOTA_EXHAUSTED","error":"quota_exhausted",…}`。
两条分类路径都认它：

- `protocol/send_error.rs` `looks_like_spent_allowance` 加了 `"quota_exhausted"`
  （broker 的稳定机器码，不是上游散文）和它的 message。
- `classify_provider_text` 里 `looks_like_spent_allowance` 检查**移到 402/billing 块之前**
  （原来只在 403 块之前）：有效 key 没预算 ≠ 账户要充钱（402→BillingRequired）≠ 拒绝访问
  （403）。OpenRouter 报 403、broker 报 402，都得先被 spent-allowance 拦下。
- `manager/dream_engine/error.rs` 那条路径本来就先查 `looks_like_spent_allowance`，扩展
  predicate 后自动覆盖。
- 保留纪律：一个 predicate 两条路径共享，不会再分叉（历史见 send_error.rs 里的注释）。

普通账户 402（没有 spent-allowance 信号，如 "credit balance is too low"）仍然映射到
`BillingRequired`——reorder 没有吞掉它，有测试钉死。

## 还没做

- **Phase 3（dream-ui）**：泛化 claim hook、余额显示、购买弹窗（对接 broker 的
  `MockGateway`）、i18n。购买弹窗要在 `structuredError.code === 'USER_LLM_PROVIDER_QUOTA_EXHAUSTED'`
  **且 provider 是我们发的 metered-trial provider** 时弹出——需要一个字段区分。
- **Phase 4**：broker 侧接真实支付宝/微信商户号（阻塞在商户号申请）。
- broker 侧 `BAOYUN_TRIAL_MODELS` 目前是未验证的占位 `deepseek-chat`，推广前要核对真实
  catalog。

## 踩坑

- `cargo fmt --all -- --check` 在这个仓库当前分支上有一堆 pre-existing 漂移（`agent_task.rs`
  等我没碰的文件）。只 `rustfmt` 自己改的文件，别 `--all`，也别对上面那 6 个测试文件跑
  （见表格备注）。
- `cargo clippy -p dream-core-ai-agent -- -D warnings` 会因为 `manager/acp/agent.rs` 里两个
  pre-existing dead-code（`claim_announcement` / `announce_session_id_after_prompt`）失败，
  跟本次改动无关——用 plain `git push` + `cargo test -p <crate>`，别用 `just push`。
