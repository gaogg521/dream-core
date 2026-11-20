# 2026-11-20：企业模型价格表 + 通道缓存计量修复

面向后续维护者。用户侧说明见 dream-en `docs/model-prices.zh-CN.md`。

## 一、缓存读取恒为 0（`05d006f`）

- 现象：企业后台「消费日志」对企业模型通道的「缓存读取 / 写入」恒为 0，而账单管理里直连 Key 有数。两页读同一张 `one_llm_calls`，差别在**写入链路**。
- 根因：`dream-domain-devops/src/proxy_usage.rs` 的 `UsageTap::absorb_event` 漏认 `usage.prompt_tokens_details.cached_tokens`（火山方舟 / GLM / Qwen / Kimi / OpenAI Chat 的标准位置）和 DeepSeek 原生 `prompt_cache_hit_tokens`。识别顺序对齐 `dream-engine-providers/src/openai.rs`。
- 口径：`input_tokens` 统一为「不含缓存的新输入」。OpenAI 系 `prompt_tokens`、Responses 的 `input_tokens` **含**缓存，代理会减掉；Anthropic 本来就不含。与桌面 `client-usage` 上报一致，缓存不计入 `total_tokens`。
- 为什么在服务端读而不从客户端读：企业通道流量本来就过代理，看到的字节与客户端相同；服务端读不依赖客户端版本、不可伪造。`client-usage` 接口故意跳过企业通道（`is_company_channel_report`）防止重复计数。
- 历史行不回补。

## 二、模型价格表（`one_model_prices`，迁移 billing_014/015/016）

**设计取舍（被推翻过两次，别走回头路）**

1. 第一版把单价放通道 `model_settings` 里 → 删模型再加回就丢。**现在按模型名独立存**，与通道无关。
2. 价格不止 token：`billing_unit` = token / image / second / request，另有分辨率档位 `tier_prices_json`、来源 `source`（manual / official / sync）与 `source_note`。
3. 币种**每个价格自己选**（`currency`，默认 USD）。成本、花费上限、报表全站是美元口径，改成全站切币种牵涉所有报表，所以 RMB 按**固定常量 `CNY_PER_USD = 7.0`**（`dream-core-common/src/license.rs`）折成美元。**刻意没有汇率设置**（用户要求）。字段名里的 `*_usd_*` / `*_micros` 是旧名：价格行里它们是「该行币种」的数，`admin_model_price()` 读出后立即 `cny_to_usd()`，下游一律美元。
   - 中途试过「公司级币种开关 + 全站换符号」，被用户否掉（要的是录入时选币种）；那版建过 `one_price_settings` 表并占用了 `billing_016` 的名字。**该版从未提交 / 发布**，只污染过本地测试库；若在自己的测试库里遇到 `currency` 列缺失，删掉 `_one_migrations` 里的 `billing_016_price_currency` 行和 `one_price_settings` 表再启动。

**计费路径（统一查表）**：`record_metered_turn`（代理；先按请求名再按上游回显名）、`record_turn`、`record_media_usage`（价格表 > 客户端上报的 `unit_price_micros` > 内置表）。`MediaUsage.tier` / 请求体 `tier` 可选。显式 0 价 = 免费，不触发「未定价」告警。

**防溢出**：媒体用量的 `count`、`duration_seconds` 由客户端上报，`count × 秒数 × 单价` 一律 `saturating_mul`（之前是普通乘法，异常大值在发布版会回绕成负数 = 绕过花费上限）。

## 三、查价（`price_catalog.rs`，`POST /model-prices/lookup`）

- 来源：公司 LiteLLM / 中转站 → 公开 LiteLLM 价格库（github raw + jsdelivr，6h 缓存，失败记忆 60s，单源超时 10s）。
- 解析器 `parse_catalog` 递归找「模型名 + 价格」：LiteLLM 价格表、`/model/info`、中转站 `pricing.*CostPerMillion`。公司网关 Joy MaaS 的真实数据在 `/api/admin/model-list`，**所有类别都按百万 token 计价**。
- 匹配 `find_match` 保守：精确 → 去服务商前缀 → 去日期快照 → 中转站 `backendNames`（去末尾线路标签）。第三方网关条目与后两种命中一律 `exact=false`。
- 安全：`check_fetch_url` 只放 http/https，拒 link-local / 元数据地址；自定义重定向策略逐跳复查（带测试）；响应上限 8MB；`catalogJson` 粘贴不访问来源；凭证只用于这一次请求，不落库不进日志。
- **登录**：来源需要登录时返回 `auth_required` + `customAuthUrls`。**不做服务端远程浏览器**（评估过，要在企业镜像里内置 Chromium，已否决）。改为管理员自己的浏览器登录，管理后台用 `probeBrowser` 轮询读取，读不到再复制粘贴。自动继续要求来源返回具体 `Access-Control-Allow-Origin` + `Access-Control-Allow-Credentials: true`（现在的 `*` 会被浏览器拒绝）。

## 踩坑

- `reqwest` 现在是 billing crate 的依赖（rustls-native-roots，与 devops 一致）。
- 轮询曾经每 2 秒打 4 个候选地址 × 5 分钟（约 600 次）：现在只打确实存在的接口，指数放慢，连续两次读不到任何响应即停。
- 内置浏览器面板里 `window.open` 会开成独立弹窗并阻塞页面：用普通链接新标签。
- Bash 工具会吞反斜杠；含 `\'` 的 TS 字符串用 Write 工具，别走 heredoc。
