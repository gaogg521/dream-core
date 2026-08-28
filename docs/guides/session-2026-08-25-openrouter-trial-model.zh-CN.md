# 2026-08-25：OpenRouter 一键体验免费模型（dream-core 侧）

> **新会话/新 AI 首读**：本文档只记录 dream-core 这一侧的实现细节。完整背景（为什么这么
> 设计、跨三仓的整体架构、dream-ui 前端改动、验证情况）见 dream-ui 仓库的同名文档
> `docs/guides/session-2026-08-25-openrouter-trial-model.zh-CN.md`。

## 一句话总结

新增 `POST /api/providers/trial-key`：无请求体、无需用户身份，dream-core 自己解析/生成一个
稳定的本机 `install_id`，转发给一个新建的独立云端服务 `dream-trial-broker`（持有 OpenRouter
Management Key，dream-core 从不接触它），拿到一把每日 $1 硬顶的真实 OpenRouter key 后原样
返回给调用方。

## 改了什么

| 文件 | 改动 |
| --- | --- |
| `crates/dream-core-api-types/src/provider.rs` | 新增 `TrialKeyResponse`（`key`/`base_url`/`models`），无对应的 Request 类型——请求体为空 |
| `crates/dream-core-system/src/trial_key.rs` | 新文件，`TrialKeyService`：`get_or_create_install_id()` 用 `IClientPreferenceRepository`（`system_default_user` 作用域，key=`trial_broker_install_id`）持久化一次性生成的 id；`request_trial_key()` 转发给 broker 并把 HTTP 状态码映射成 `SystemError` |
| `crates/dream-core-system/src/error.rs` | `SystemError` 新增 `RateLimited` / `ServiceUnavailable(String)` |
| `crates/dream-core-system/src/routes.rs` | 新路由 `POST /api/providers/trial-key`（注册在 `/api/providers/fetch-models` 之后、`/api/providers/{id}` 之前，避免被当成 provider id）；`SystemRouterState` 新增 `trial_key_service` 字段；`From<SystemError> for ApiError` 补两条映射（`RateLimited` 直通，`ServiceUnavailable` 用 `ApiError::coded(503, "daily_budget_exhausted", ...)`） |
| `crates/dream-core-app/src/router/state.rs` | `build_system_state()` 里用 `std::env::var("DREAM_TRIAL_BROKER_URL").ok()` 构造 `TrialKeyService`；**注意构造顺序**——必须在 `client_pref_repo` 被移动进 `client_pref_service` 的 if/else 之前完成，否则借用检查会报 E0382（本轮踩过一次，已修） |
| 6 个 `crates/dream-core-system/tests/*_routes.rs` | 同步补上 `SystemRouterState` 新增的 `trial_key_service` 字段（`TrialKeyService::new(None, http_client.clone(), Arc::new(SqliteClientPreferenceRepository::new(...)))`），否则编译不过 |

## 为什么 `install_id` 由 dream-core 自己生成，不由调用方传入

如果请求体带一个 `install_id` 字符串，等于信任调用方（Electron 渲染进程）诚实上报——一个
本地可调试、可修改内存的进程，没有任何东西阻止它每次换一个新 id 来绕过 broker 的"一台设备
一次"限制。dream-core 是这台机器上跑着的唯一本地服务实例，用它自己的 `client_pref` 表
持久化一个一次性生成、之后永不改变的 id，天然比信任调用方传参更可信——即使前端被完全绕过
直接 curl 这个本地端口，拿到的也是同一个 id。

## 为什么不复用 `managed_provider.rs`

`ManagedProviderSync` 的语义是"服务端持续下发完整渠道列表，客户端做增量对账（新增/更新/因
不在列表中而删除）"，专为企业 SSO 场景设计——参见该文件顶部文档注释。这里的场景是"点一次
按钮、领一次 key"的一次性事件，没有"持续同步"的需求，硬套上去（比如把 trial key 包装成一个
`ManagedChannelPayload` 走 `sync-model-channels`）除了徒增复杂度没有任何好处。改为让
dream-ui 直接调用已有的普通 `POST /api/providers`，生成一条用户可编辑的普通 provider。

## 验证

- `cargo test -p dream-core-system`：全绿，含 `trial_key` 模块两个单测
  （`no_broker_configured_reports_plainly`、`install_id_is_generated_once_and_then_stable`）
- `cargo check --workspace`：全绿
- **没有**跑真实的 OpenRouter Management API 调用（需要真实的 Management Key 和网络），
  `TrialKeyService` 里的 OpenRouter 调用逻辑靠 `dream-trial-broker` 那边的 mock 测试覆盖，
  dream-core 这一侧只测到"broker 返回什么状态码，我们映射成什么错误"这一层
- `DREAM_TRIAL_BROKER_URL` 环境变量目前未在任何地方配置，本地/生产环境这个端点会直接返回
  "trial key issuance is not configured on this deployment"（400），这是预期行为，不是 bug

## 2026-08-28 补充：已部署 + 已接线

- **broker 已部署**：`43.163.105.71`，systemd（非 Docker），公网入口
  `https://work.1oneclaw.com/trial-broker`。完整细节见 dream-ui 仓库同名文档的
  "2026-08-28 补充"一节，以及 `dream-trial-broker` 仓库的 `deploy/DEPLOY.md`。
- **`DREAM_TRIAL_BROKER_URL` 已接线**：由 dream-ui `packages/web-host` 在 spawn aioncore 时
  注入默认值 `https://work.1oneclaw.com/trial-broker`。dream-core 这一侧代码**未改动**，
  仍然是"env 有值就用、没值就报未配置"的原有逻辑。
- **真实链路已验证**：当天用真实 Management Key + 打包用的 bundled aioncore 二进制跑通
  `POST /api/providers/trial-key` → 真实签发 → 二次 409；测试 key 已清理。
- dream-core 侧后续无待办；剩下的是 dream-ui 发版 + 真实 UI 冒烟。
