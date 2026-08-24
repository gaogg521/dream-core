@AGENTS.md

## 项目定位

**dream-core** 是 **One Work** 平台的本地后端服务（Rust），编译产物为单个可执行文件 `dreamcore`。本项目最初基于开源项目 [AionCore](https://github.com/iOfficeAI/AionCore) 二次开发，**现已完全独立成自有平台，不再跟随或合并上游**，技术与协议层统一使用小写前缀 `dream`。

> **代码溯源**：本仓库 2026-08-23 从旧仓库 `D:\aionui-m0\1oneCore`（原始最上游是开源项目
> [AionCore](https://github.com/iOfficeAI/AionCore)）**原样复制的一次性快照**，不含 `.git`
> 历史。如果在本仓库里发现某个功能/文件"应该存在但找不到"，先去 `D:\aionui-m0\1oneCore`
> 翻一下——很可能是快照时点之后才在旧仓库落地的，或者是旧仓库里还没合并进 `one-main`
> 主干的分支。`D:\aionui-m0` 三仓（`1oneUI`/`1oneCore`/`aionrs-local`）定位是只读归档，
> 不再往里提交新代码。

> **新会话/新 AI 首读**：本仓库持久化数据迁移的详细过程（改了哪些文件、发现的真实 bug、
> 迁移文件生命周期踩坑）记录在
> [session-2026-08-23-dream-rebrand-data-migration.zh-CN.md](./docs/guides/session-2026-08-23-dream-rebrand-data-migration.zh-CN.md)；
> 跨仓完整叙述（含前端 bug 与 CodeMirror 排查）见 dream-ui 同名文档。本 CLAUDE.md 只保留
> 长期有效的规则，过程性细节请去读那份文档。

## 三仓架构

| 仓库 | 角色 | 关键产物 |
| --- | --- | --- |
| **[dream-ui](https://github.com/gaogg521/dream-ui)** | Electron 桌面、React UI、WebUI 静态资源 | 安装包 |
| **dream-core**（本仓库） | Rust 本地服务，30+ 领域 crate | `dreamcore` 二进制 |
| **[dream-engine](https://github.com/gaogg521/dream-engine)** | Agent 引擎（CLI/TUI/Provider/工具） | `dream` 二进制，本仓库通过 `dream-engine-* = { git = "...", branch = "main" }` 依赖引入 |

推荐开发时三仓并列：

```text
dream/
├── dream-ui/
├── dream-core/     ← 本仓库
└── dream-engine/
```

改了 dream-engine 的代码后需要先推 dream-engine 仓库的 `main` 分支，再在本仓库 `cargo update -p dream-engine-*` 对齐依赖版本；改了本仓库代码后必须重编 `dreamcore` 并让 dream-ui 的 bundled 目录同步更新才生效（详见 dream-ui 的 [开发者上手指南](https://github.com/gaogg521/dream-ui/blob/main/docs/guides/fork-dev-onboarding.zh-CN.md)）。

## 品牌与技术身份分层

| 层级 | 值 | 说明 |
| --- | --- | --- |
| 用户可见产品名 | **One Work**（首字母大写、中间有空格） | 只在面向用户的文案/文档里出现；本仓库对外不直接展示品牌名，但错误提示等字符串若引用产品名，以此为准。**这个名字容易被口头/打字误传成 "OneWork"、"ONE WORK" 等变体，改动前以 dream-ui 的 `BRAND_DISPLAY_NAME` 常量实际值为准** |
| 技术/协议前缀 | **`dream`**（小写） | crate 名 `dream-core-*`/`dream-domain-*`、二进制名 `dreamcore`、Rust 枚举 serde 值、环境变量 `DREAM_*`、内部 HTTP 头 `x-dream-*` |

## 持久化线上取值改名的铁律（本仓库最容易踩的坑）

数据库枚举列、JWT claim、内部协议头这类**跨版本持久化或跨进程约定**的字符串，改名必须做兼容层，否则会破坏历史数据或跨仓协作：

- **Rust 枚举变体改名**：`#[serde(rename = "新值", alias = "旧值")]`，新值是当前规范线上值，旧别名保证历史数据仍可反序列化。参考 `dream-core-common::enums::AgentType`。
- **哈希派生的稳定 ID**（如 `AgentType::id()`）：改名前检查是否有代码把它当稳定值写死进种子数据（如 `builtin-assistants/assistants.json`）或断言；若有，需要在 `id()` 里对该分支硬编码冻结旧哈希，不能让改名连带改变 ID。
- **跨仓协议**（如内部 HTTP 头名 `x-dream-forwarded-origin`/`x-dream-client-ip`）：dream-core 的 `dream-core-auth::middleware` 与 dream-ui 的 `web-host/static-server.ts` 必须同步改，只改一边会让 WebUI 来源识别、客户端 IP 转发在运行时悄悄失效而不报错。
- **strict-match 的查找函数**（如 `resolve_agent_binding_from_rows`）：这类函数按精确字符串匹配 `row.agent_type == value`，不做别名归一化。任何地方硬编码传入旧值作为查找 key（例如某个"兜底找内置 agent"的调用点）都会在改名后悄悄查不到、返回空结果，且不报错——**这类 bug `cargo check` 测不出来，必须跑测试**，多个真实 bug（会话分叉功能、定时任务运行时类型推导）都是这样漏网的。
- 纯测试内部约定（fixture 里用来分支选择的字符串标签、非持久化的局部变量名）可以直接改，不需要兼容层。
- **改完不能只信编译通过**：本仓库有大量集成测试是运行时字符串比较，`cargo check --workspace` 甚至 `cargo test`（默认 fail-fast）都可能漏掉。批量改名后必须完整跑一遍 `cargo test --workspace`（或更快的 `cargo nextest run --workspace`），并且要跑到底、不能提前判定"看起来都过了"——workspace 有几十个 crate，某个未被前几轮触及的 crate 里可能还藏着引用旧值的测试断言。

## 测试

```powershell
cargo nextest run --workspace   # 推荐，比 cargo test 快很多
cargo test --workspace          # 也可以，但明显更慢
```

`cargo nextest` 需要先安装：`cargo install cargo-nextest --locked`。

统计失败数必须带 `--no-fail-fast`（`cargo test` 默认 fail-fast，lib 目标一挂就不跑后面的集成测试二进制，报出来的失败数会偏小）；`cargo nextest run` 本身没有这个问题。

## 数据库迁移

迁移文件按 crate 分别管理（如 `crates/dream-core-db/migrations/`、`crates/dream-domain-employee/migrations/`），应用启动时自动执行。**已发布的迁移文件历史内容不可修改**（会破坏 `_sqlx_migrations` 校验和，导致存量安装升级失败）——只能新增下一个编号的正向迁移。改 `migrations/*.sql` 后必须重新编译并让 dream-ui bundled 目录更新，否则桌面端仍跑旧 schema。
