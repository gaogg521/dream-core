# 2026-08-26 媒体 env 契约修复 + 路径别名化

> **一句话**：品牌迁移只改了 dream-ui 一半、没改本仓库一半，把媒体 MCP 的环境变量契约改断了。
> 前端侧现象与完整验证清单见 dream-ui 的
> `docs/guides/session-2026-08-26-media-output-and-brand-cleanup.zh-CN.md`。

---

## 1. 根因：`media_workspace.rs` 的常量没跟着改名

`dream-core-mcp/src/media_workspace.rs` 一直在发 `AIONUI_MEDIA_WORKSPACE_DIR` /
`AIONUI_MEDIA_CONVERSATION_ID`，而 dream-ui 的 `imageGenServer.ts` 早已改读
`DREAM_MEDIA_*`。两边永不匹配 → 媒体工具退回自己的 cwd（app 数据目录），
生成的图片视频落在会话工作区之外，前端的媒体卡片、缩略图、成本行全部消失。

**编译通过、测试全绿、没有任何日志** —— 工具只是「什么都没被告知」，安静地走兜底分支。
这就是 CLAUDE.md 里「跨仓协议改名必须两侧同步，只改一边会在运行时悄悄失效而不报错」
所指的那类 bug。

修法：常量改为 `DREAM_MEDIA_*`，并新增把字面量钉死的测试
`env_names_match_the_typescript_media_server` —— 下次改名必须同时动两侧才能过。

同时 `BUILTIN_MEDIA_MCP_NAME` 改为 `one-image-generation`，
新增 `LEGACY_MEDIA_MCP_NAMES` 兼容三个历史名。**这一步不能漏**：env 注入是靠 server 名匹配的，
只认新名会让存量安装（DB 里存的还是旧名，要等前端配置迁移改写）重新掉进同一个坑。

## 2. 上传根目录：新增 `dream_core_common::upload_paths`

原先是 6 处硬编码 `temp_dir().join("aionui")`，而
`dream-core-conversation/src/service.rs` 已经单方面改成了 `join("dream")` ——
**上传写 A、发送边界校验 B，`ChatFileRef::Upload` 会被 `path_within()` 判越界拒绝**。

现在统一走：

- `upload_root()` —— 新文件写这里（`<tmp>/dream`）
- `upload_roots()` —— 校验/读取接受的全部目录（`<tmp>/dream` + `<tmp>/aionui`）

`ProjectService::resolve_chat_message` / `resolve_chat_file_ref` 的
`upload_root: &Path` 参数改成 `upload_roots: &[PathBuf]`，命中任一即可。

**为什么读取要接受两个根**：改名会让改名前已暂存的文件（用户挑好的附件、媒体 job 仍指向的
参考图）全部失效。所以只单向改写入端，读取端两个都认，老目录自然随 OS 清理 temp 而消亡。

## 3. 数据目录：`dream_core_common::data_paths` 别名解析

不搬任何文件。`resolve_with_legacy(parent, current, legacy)`：

> 当前名存在 → 用当前名；否则老名存在 → 用老名；都不存在（新装）→ 用当前名。两者都在时当前名优先。

| 新名 | 老名 | 用在哪 |
| --- | --- | --- |
| `one-backend.db` | `aionui-backend.db` | `AppConfig::database_path()`、`cmd_doctor`、`cmd_resetpass` |
| `one-sessions/` | `aionrs-sessions/` | `factory/dream_engine.rs` |
| `runtime/one-process/` | `runtime/aionui-process/` | `registry_store::registry_dir()`、`spawner::local_machine_id()` |

**为什么不做 rename 迁移**：SQLite 打开一个不存在的路径**不会失败**，它会建一个空库。
用户升级后打开 app 会发现会话、助手、技能全没了。别名化没有这个窗口。

`registry_store::SUBDIR`（`&str` 常量）因此变成了 `registry_dir(data_dir)` 函数，
`instance_lock.rs` 与 `lib.rs` 的导出同步调整。

## 4. 其它

- 快照目录前缀 `aionui-snapshot-` → `one-snapshot-`，但 stale 清理**两个前缀都扫**，
  否则改名前建的目录会永远留在用户 temp 里没人删。签名改成 `1ONE` / `snapshot@1one.local`
  （只影响新提交）。
- `dream-core-cron` 的 `system_resume` 现在同时接受 `x-dream-internal` 与
  `x-aionui-internal`。桌面端配的是固定版本 `aioncore`，UI 比后端老是常态；
  只认新名会让休眠唤醒后的 cron 静默 403。

## 5. 验证

```bash
cargo test -p dream-core-common -p dream-core-mcp -p dream-core-project -p dream-core-file \
           -p dream-core-office -p dream-core-team -p dream-core-conversation \
           -p dream-core-process -p dream-core-app -p dream-core-cron -p dream-core-ai-agent
```

改完必须重编 `dreamcore` 并让 dream-ui 的 bundled 目录同步更新才生效
（dream-ui 侧对 `DREAM_MEDIA_*` 做了旧名兜底，所以在后端发版之前功能已经是好的）。

## 6. 已知问题（既有）

HEAD 不是 fmt-clean —— `cargo fmt --all` 会连带重排上百个无关文件的 import 与换行。
本次把格式化拆成了独立的 `style:` 提交，功能改动单独一个提交。
注意 `just push` 的门禁本来就跑 `cargo fmt --all -- --check`。

---

## 7. 后续：内置管家/技能改名 + `AIONUI_*` env 家族（同日）

### 迁移 053

`aionui-assistant` → `one-assistant`，四个技能 `aionui-{config,troubleshooting,webui-public,webui-setup}`
→ `one-*`。与 Electron 侧那些只在遗留库上跑的迁移不同，**本仓库的迁移在新库上全跑**，
所以不改的话全新用户依然拿到旧品牌。

`source_ref` 是最要命的一列：它是清单身份列（与 `source` 唯一），而清单每次启动按**新** id
重新播种 —— 漏改的行不会匹配、会被再播种一遍，用户看到**两个管家**。
JSON 列（`default_skill_ids` 等）的替换针对带引号 token，否则 `aionui-config-extra` 会被误伤。

测试见 `tests/builtin_assistant_rebrand_migration.rs`（5 用例）。两个写测试的坑：
手搓 `Migrator` 跑全链路会挂在 042；改用 `init_database_memory()` 后它已跑过 053，
sqlx 版本账本会让 `Migrator` **跳过**，断言全部假通过 —— 最终改成 `include_str!` 迁移文件直接执行 SQL。

### env 家族

38 个 `AIONUI_*` → `ONE_*`。新增 `dream_core_common::legacy_env::adopt_legacy_env()`，
在 `main()` / `admin.rs` 最开头把 `AIONUI_X` 采纳为未设的 `ONE_X`——
一处解决 clap `#[arg(env=)]` 和所有 `std::env::var`，读取点全部只认新名。

**安全回归警告**：`registry.rs::is_blocked_override_env_key` 靠 `AIONUI_` 前缀阻止用户
env override 覆盖内部变量。改名时必须同步加 `ONE_` 前缀，否则用户能覆盖 `ONE_RUNTIME_TOKEN`。

**批量改名的通用坑**：正则扫描会把「负责兼容旧值」的代码本身也改掉。本次中了两次
（`legacy_env.rs` 的测试、`types.rs` 的 `LEGACY_*` 常量），都改用 `concat!("AIONUI", "_X")` 构造。
改完批量改名，务必回头单独检查这类文件。

### 测试执行

见 `AGENTS.md` 新增的「NEVER run two builds against the same `target/` at once」。
本次因为并发跑出过 LNK1104、计时测试超时等假故障。串行跑的干净结果：
`cargo nextest run --workspace` **9636 测试全过**。
