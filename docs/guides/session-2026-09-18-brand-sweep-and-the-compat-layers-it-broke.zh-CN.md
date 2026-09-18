# 上游品牌残留清理，以及这轮 sweep 自己弄坏的兼容层（2026-09-18）

> 本轮的**结论比过程重要**：批量改名这件事，代价不在"改漏了"，在"改多了"。
> 这份文档主要记录 **哪些值绝对不能改**，以及**怎么自动发现自己改错了**。
> dream-ui 侧的同名文档记录前端/安装器部分。

## 一、起因

用户要求把上游品牌残留"彻底改掉"，只保留兼容行为、不保留可见的旧名字。
前一轮已经改过两遍，这轮是第三遍 —— 也正是这一点让它危险：前两遍留下的
`LEGACY_*` 兼容层，在第三遍眼里长得就像"没改干净的残留"。

## 二、这轮 sweep 自己造成的真实缺陷（全部已修）

按危害从高到低。**每一条都是绿着的测试没拦住、或者被 sweep 连同实现一起改绿的。**

| # | 位置 | 被改成了什么 | 后果 |
| - | ---- | ------------ | ---- |
| 1 | `dream-core-app/src/config.rs` `derive_encryption_key` | 密钥派生的域分隔前缀被改名 | **所有存量安装的全部凭据永久无法解密**（provider、渠道配置、远程 agent）。这正是 Sentry ELECTRON-3T0 那次事故的失效模式 |
| 2 | `dream-domain-sso/src/routes.rs` `sanitize_deep_link_scheme` | 允许列表里的旧 scheme 被删、兜底值被改 | 改名前的客户端只向 OS 注册了旧 scheme，**SSO 回调落到没人监听的协议上**，浏览器里登录成功、应用永远收不到。CLAUDE.md 里写着的那条警告，被原样重现了一遍 |
| 3 | 11 个 `crates/dream-domain-*/migrations*/*.sql` | 注释里的 crate 名，以及 `WHERE agent_type = '<旧值>'` | 迁移的 WHERE 子句**匹配不到任何行**（迁移从此什么都不做），且**已应用迁移的 checksum 变了**，存量库启动校验会失败 |
| 4 | `dream-core-common/src/enums.rs` | `ConversationSource::DreamUi` / `McpSource::DreamUi` 的 `serde(alias)` | `conversations.source` 是持久化列（更早的 SQLite CHECK 约束里还写死了旧值），**旧行反序列化直接失败** |
| 5 | `dream-core-extension/src/types.rs` `EngineConfig` | manifest 的 `engine.<key>` 字段名 | 磁盘上已安装扩展声明的是旧 key，静默解析成 `None`，**版本门禁从"检查"变成"不检查"** |
| 6 | `dream-core-channel/src/message_service.rs` | —（这条是**既有 bug**，sweep 让它暴露出来） | 会话标题直接用了 `agent_config.agent_type` 这个**存量列原值**，改名前配置的渠道，新建会话的名字里会带着旧品牌给用户看 |
| 7 | `dream-core-api-types/src/team_mcp.rs` | `TEAM_MCP_SERVER_NAME` | 这个名字是 `mcp_servers` 行的 `name`，多处 `row.name == ...` 靠它识别"这是 team 服务器"。改名后**旧行不再被识别**，会作为普通 MCP 暴露给用户勾选 |
| 8 | `dream-core-extension/src/constants.rs` | `SKILLS_MARKET_PATH` | 这个 URL 是写进用户 `custom_skill_paths.json` 的**标识符**，`disable_skills_market` 按它删除条目。只改新值 → 旧条目删不掉，UI 里留一条关不掉的技能源 |

1/2/3/4/5 已经全部**还原**；6/7/8 改成了"新值写入 + 旧值兼容读"：

- 6：调用方改传 `agent_type.serde_name()`（规范值），并补了一条专门盯它的测试
- 7：新增 `LEGACY_TEAM_MCP_SERVER_NAME` + `is_team_mcp_server_name()`，6 个比较点全部改走它
- 8：新增 `LEGACY_SKILLS_MARKET_PATH`，`disable_skills_market` 两个都删

## 三、绝对不能改的值（清单在代码里，不在这里）

不要在这份文档里维护清单 —— 清单会过期。**权威清单是
`crates/dream-core-common/tests/brand_residue.rs` 的 `PINNED_VALUES`**，每一项都必须
带一句"为什么钉死"的理由，且有测试强制它带理由。

目前钉住的类别：

- 持久化 wire 值（`AgentType` 的 serde alias 所对应的那个字符串）
- 磁盘上已存在的目录名 / 日志后缀 / 会话目录
- 迁移 001 种进去的展示名与图标路径（迁移状态 fixture 必须逐字复现）
- 构建期从第三方 hub 下载的包名

## 四、为什么普通测试拦不住（以及新的护栏）

**改名会同时改实现和断言。** 上面 1/2/4/5 全都是"实现和测试一起被改绿"的。
编译器只保护标识符，不保护字符串字面量 —— 而所有持久化契约都是字符串。

这轮加了三层护栏：

1. **`brand_residue.rs`**（新增，已进仓）——"没有 `legacy`/`pre-rebrand`/`pre-fork`
   标记的旧名字一律报错"。它**跳过任意层级的 `migrations/`、`migrations_mysql/`、
   `docs/`**，并把迁移状态 fixture（`**/tests/*migration*.rs`）整体豁免：这类文件
   必须逐字复现迁移前的库，改一个字就等于让迁移的 WHERE 落空。
2. **"消失的字面量"审计**（一次性脚本，见下）——把 HEAD 里所有含旧品牌的字符串
   字面量与工作区做差集，**在全仓范围内彻底消失的那些**就是失去最后一个读者的
   兼容层。上面 1/2/4 就是这么找出来的。这一步应当在任何一次批量改名后跑一遍。
3. **跑测试之前先跑 `cargo test --workspace --no-run`**：`cargo check` 不编译
   `#[cfg(test)]`，批量改名最容易在测试代码里留下坏标识符。

### "消失的字面量"审计怎么做

```text
对每个 .rs：取 HEAD 版本与工作区版本的字符串字面量集合
  gone = HEAD字面量 - 全仓工作区字面量        # 注意是「全仓」，不是「同文件」
  只看 gone 里含 aion/ 旧品牌 的项
```

只按文件做差集会漏 —— 一个值可能从实现里消失、但还留在别处的测试里。必须对
**整个工作区**取并集再做差。

## 五、另外顺手修掉的既有缺陷

- `scripts/just/cargo.{sh,ps1}` 的 `crates=()` 列表还是引擎改名前的包名，
  设了 `DREAM_ENGINE` 想用本地引擎的话**必然报 "missing aion-agent" 退出**
- `README.md` 里让人跑 `node scripts/prepareAioncore.js`，实际文件叫
  `prepareDreamcore.js`
- 内置技能（会被 `rust_embed` 打进二进制、模型直接读）里：
  - `one-troubleshooting` 的 macOS 日志目录示例指向旧应用名
  - `one-webui-setup` 的防火墙命令、三平台配置文件路径全指向旧应用名
  - `xiaohongshu-recruiter/scripts/generate_images.js` 的字体目录回退指向旧 userData
    目录（`fs.existsSync` 兜底，所以一直静默不生效）
  - 两个 auto-inject 技能里"不要设置 `AIONUI_...` 环境变量"这条规则，保护的是一个
    已经不存在的前缀
- `dream-core-mcp` 的 MCP 连接失败提示里还在让用户"重启 AionUI"（用户可见）
- `dream-core-system/src/diagnostics.rs` 的脱敏说明同上（用户可见）
- 迁移 `057_drop_legacy_brand_logo_reference.sql`（新增）：把仍指向旧品牌图标的行
  改到 `1one.png`，这样那个 svg 资源才能真的从二进制里删掉

## 六、遗留 / 待定

- `AgentType::display_name()` 对 `DreamEngine` 返回 **`1ONE CLI`**，迁移 019 也把
  库里的展示名改成了 `1ONE CLI`。这不是上游品牌，是**我们自己更早的品牌**，
  用户当前在界面上看到的就是它。要不要改成 `Dream CLI` / `One Work CLI` 是产品
  决策，本轮没动。
- `resources/hub/` 仍在构建期从第三方 hub 下载（用户已明确接受）。
- OfficeCLI 相关技能里的手动安装兜底链接改成了厂商域名 `https://officecli.ai`；
  该域名 301 跳到它自己的 GitHub 仓库，跳转目标仍是第三方组织。

## 七、验证

- `cargo test --workspace --no-run` 通过
- `cargo nextest run --workspace -E 'not binary_id(~e2e)'` 全绿（e2e 交给 CI，
  见 memory「本地别跑全量 e2e」）
- `brand_residue` 两个测试绿
- **没有做真机验证**：本轮改动集中在兼容层还原与构建脚本，改完的运行时行为
  （SSO 回调、凭据解密）恰恰是需要存量数据才能验证的，建议在有真实 profile 的
  机器上过一遍登录 + 打开模型设置。
