# aion 残留全量清理 + `onework.exe` 改名 + moltbook 移除（2026-09-01）

> 一次会话完成三件事：①把 V2 兼容白名单之外**所有** aion 家族残留清干净；
> 可执行文件名定为 **`onework.exe`**（无空格，三平台统一）；②移除 moltbook 集成；
> ③对"生态/外部契约"类残留逐项判定——**不会坏的改掉，会坏的说明为什么保留**。
> 新会话接手先读本文，再按需跳 MIGRATION-STATUS.md。

## 一、背景：为什么改完 P1–P5 还有残留

用户在运行日志里看到 `[aioncore]` 前缀和 `aionui_feedback_diagnostics` 事件名（截图）。
排查结论：P4 的注释清理脚本 `REPLACEMENTS` 映射表里**没有小写 `aioncore`**
（有 `aionui`/`aionrs`/`AionUi`/`AionCore`，但 `aioncore` 被最后一条 `Aion→Dream`
漏掉——大小写敏感），所以：

- 注释里大量 `aioncore` 存活至今；
- 更重要的是一批**运行时字符串**当年就不在清理范围（日志标签、tracing target、
  stdout 握手标记、内部 HTTP 头、NSIS 安装器、CI 资产名、打包脚本、web-cli、
  Agent 可见资产、Sentry 标签）。

## 二、改动总览

| 仓库 | 文件数 | 主要内容 |
|---|---|---|
| dream-core | 141 | 运行时契约字符串、Rust 标识符、错误文案、capabilities 文档、日志文件名、CI 资产名 |
| dream-ui | 196 | launcher 标签、binaryResolver、NSIS/安装器、打包链、TS 标识符/CSS/i18n、web-cli、Sentry |
| dream-en | 1 | deploy/docker-compose.yml 环境变量名 |
| one-work-content | 3 | 移除 2 个 moltbook 生态技能（trawl、doppel-social-outreach） |

## 三、分层改动明细（改了什么、怎么改的）

### 3.1 用户截图的直接来源

- `[aioncore]` 日志标签：`web-host/src/backend-launcher.ts` 约 20 处 console 前缀
  → `[dreamcore]`。
- `aionui_feedback_diagnostics`：dream-core 7 个 crate 共 24 处 tracing
  `target:` → `dream_feedback_diagnostics`；事件名
  `feedback.runtime.aionrs_error` → `feedback.runtime.dream_engine_error`。

### 3.2 跨进程契约（必须双端同改，全部按"新值写入 + 旧值兼容读"处理）

| 契约 | 新值 | 兼容策略 |
|---|---|---|
| stdout 就绪标记 | `DREAMCORE_READY` / `DREAMCORE_LISTENING ` | 前端同时接受旧 `AIONCORE_*`（防新旧版本混跑卡启动）；后端只发新值 |
| 内部 HTTP 头 | `x-dream-user-id` / `x-dream-conversation-id` / `x-dream-runtime-token` / `x-dream-internal` | cron 路由同时接受旧 `x-aionui-internal`；桌面端请求双发 |
| 附件标记 | `[[DREAM_FILES]]`（`dream-core-common/constants.rs` 的 `FILES_MARKER`，新增 `LEGACY_FILES_MARKER`） | 后端注入新标记；`media.rs`/`dream_engine/content.rs`/`vision_image_hook.rs` 的剥离/切分逻辑 `rsplit_once(新).or_else(旧)` 双匹配；前端 `MessageText` 用 `ALL_FILES_MARKERS` 双匹配——老会话不受影响 |
| 启动引导 | `DREAMCORE_BOOTSTRAP_SECRET`、`x-dreamcore-bootstrap-secret` | **旧名不再读**（部署侧必须改，启动时会报错提示新名字） |
| 日志文件 | `dreamcore.log` / `dream-engine.log` | 前端 `logs.ts` 的 `LOG_SUFFIXES` 保留旧后缀（读历史日志） |
| clap | `#[command(name = "dreamcore", about = "One Work Backend Server")]` | — |
| 引擎日志 target 过滤 | 常量改名 `ENGINE_TARGETS` | 过滤项里的 `aion_*` 四个旧 target **保留**（捕获改名前编译的旧引擎二进制日志） |

### 3.3 `onework.exe`（本次拍板，覆盖 8-31 的"删除回落 One Work"方案）

- `electron-builder.yml`：顶层 `executableName: onework` + `linux.executableName: onework`
  （此前回落 productName 会产生带空格的 `One Work.exe`；`onework` 进程/安装目录无空格，
  Dock/窗口/关于页仍读 CFBundleName = "One Work"）。
- 同步链：`resources/installer.nsh`（默认安装目录钉到 `Programs\onework`，旧目录列入
  收敛清单）、`installer-observability.nsh` 的唯一 define
  `AIONUI_APP_EXECUTABLE_FILENAME = "onework.exe"`（6 处出荷校验自动跟随）、
  `query-lockers.ps1`（known lockers 加 onework 系 + bundle 路径加
  `bundled-dreamcore\...\dreamcore.exe`，旧路径保留；内部 PS 命名空间
  `AionUi.RestartManager` → `OneWork.RestartManager`）、`packaged-launch.mjs` /
  `dev-bootstrap.mjs` / `build-with-builder.js` 的 kill 列表（新名加最前，旧名全保留）、
  `sentry.ts` installDirs、`tests/e2e/fixtures.ts` 候选名。
- **NSIS 幂等坑**（见 §八.5）：本机 node_modules 里 electron-builder 的
  installUtil.nsh 已被旧模板补丁过，改名后 patcher 的"已打过补丁"分支匹配不上会直接
  throw；给 copied/in-place 两处 ExecWait 加了**旧变量名模板的升级分支**
  （`$AionUiSessionLogPath/$AionUiSessionId` → `$OneWorkSession*`），
  bundled-uninstaller 覆盖块同理。所有 `.nsh` 的会话变量名同步改名。

### 3.4 打包/发布链

- **dream-core workflows**：`release.yml` 删掉"build 出 dreamcore 后 mv 成 aioncore 再
  发布"的垫片，资产直接 `dreamcore-v*.tar.gz/zip` + `dreamcore-checksums.txt`；
  `build-manual.yml` 同理（artifact `dreamcore-manual-*`）。
- **dream-ui**：`prepare-aioncore.js` → `prepare-dreamcore.js`（文件改名），
  输出 `resources/bundled-dreamcore/<plat-arch>/dreamcore[.exe]`；下载优先
  `dreamcore-*` 资产名、失败回退旧 `aioncore-*`（改名前发布的版本），旧包里的
  `aioncore` 二进制解压后复制规范化为 `dreamcore`；Actions artifact 同样双名回退。
  `verify-bundled-aioncore-resources.js` → `verify-bundled-dreamcore-resources.js`，
  安装器侧 `verify-bundled-aioncore-install.ps1` → `verify-bundled-dreamcore-install.ps1`。
- `binaryResolver.ts`：`BINARY_NAME='dreamcore'`，候选 =
  {env 覆盖目录, cwd/resources, resourcesPath} × {bundled-dreamcore, bundled-aioncore}
  × {dreamcore[.exe], aioncore[.exe]}；diagnostics 只记录"当前名 × 标准目录"的候选，
  避免把 legacy 回退尝试写进诊断。
- `backendInstallDiagnostics.ts` / `backendStartupFailure.ts`：识别新目录名，legacy
  目录存在时记 `legacyBundledDirPath` 字段且不误报 missing。
- 根 `package.json`：`aioncoreVersion` → `dreamcoreVersion`（含
  `resolveDreamcoreVersion.js`、两条 workflow、imageGenServer 注释）。
- **Dockerfile**：`AIONUI_PORT/DATA_DIR/ALLOW_REMOTE/LOG_JSON` → `DREAM_*`（这是
  一个真实 bug：代码只认新名，容器部署之前会静默错端口/错数据目录）；ENTRYPOINT
  `./dream-web`。
- **dream-en deploy/docker-compose.yml**：`AIONUI_HOST/DATA_DIR/ADMIN_PORT` →
  `ONE_HOST/ONE_DATA_DIR/ONE_ADMIN_PORT`（后端 cli.rs 实际读的名字；同样是真实 bug）。
- web-cli：`aionui-web` → `dream-web`（bin 文件、package.json bin、console 标签、
  tarball 内容目录、Dockerfile COPY 路径、compose volume `aionui-data`→`dream-data`）；
  数据目录 `~/.aionui-web` → `~/.dream-web`（旧目录存在则继续用，不丢数据）；
  内部 bundled 目录 `bundled-aioncore` → `bundled-dreamcore`（带旧目录回退）。

### 3.5 dream-core 运行时字符串/文案

- 用户可见错误：`"AionUI failed while sending the message"` 等 3 条 →
  `"One Work …"`；`"another aioncore already owns this data directory"` → dreamcore；
  antigravity hook 拒绝理由（`aionui hook …` → `dream hook …`、`approved in AionUi` →
  `approved in One Work`）；QR 登录页 `<h1>AionUI</h1>` → `One Work`。
- Agent 可见（进模型上下文）：`cmd_capabilities.rs`/`team_capabilities.rs`/
  `diagnose_capabilities.rs` 的 `aioncore …` 命令示例 → `dreamcore`；
  `<aionui-image-description>`/`<aionui-local-ocr-skill>` 注入标签 → `<dream-*>`；
  codex `clientInfo.name "aionui-title"` → `dream-title`；钉钉 UA `aioncore` →
  `dreamcore`；Discord identify `browser/device: aionui` → `dream`；
  内部 tool_call id 前缀 `aionrs-{id}` → `dream-engine-{id}`（会话内对称派生，无持久化）。
- **版本检查**：`version.rs` 的 `DEFAULT_REPO: "iOfficeAI/AionUi"` →
  `"gaogg521/dream-core"`（否则应用会拿上游发布当更新源）、UA → dreamcore、
  两处测试 mock 路径同步。
- builtin skills/assistants 文档（注入 Agent 上下文）：`aioncore` CLI 引用 →
  `dreamcore`、`~/.aionui/tools` → `~/.dream/tools`、小红的 chrome profile 目录
  `.aionui` → `.dream`、`aionui_image_generation` 工具名 → `one_image_generation`。
- 其他：`instance_lock` `"aionui.instance.lock"` → `"dream.instance.lock"`（锁文件是
  易逝物，无数据迁移问题）；`send_error.rs` 的 `classify_aionui_state` 等函数名、
  `AionrsAgentError/AionrsAgentManager/AionrsResolvedConfig/…` 等标识符 →
  `DreamEngine*`/`engine_*`；`IdentityMode/AuthIdentityMode/ExternalUserType/UserType`
  的 `AionPro/Aionpro` 变体 → `DreamPro`，**wire 值用
  `#[serde(rename="aionpro")]`/`#[sqlx(rename="aionpro")]`/`#[value(alias="aionpro")]`
  钉死**（持久化行与部署脚本不受影响）。

### 3.6 dream-ui TS 层

- 标识符（词法器分类后整替换）：`Aionrs*` → `DreamEngine*`（328 处）、
  `AionModal/AionSelect/AionCollapse/AionSteps/AionScrollArea` 组件与 Props →
  `Dream*`、`__AionResizeObserverPatched__` → `__DreamResizeObserverPatched__`、
  CSS 类 `aionui-modal*`/`aion-select`/`aion-url-viewer-toolbar` →
  `dream-*`（TSX 与 CSS 1:1 同步，改名前确认 HEAD 里没有同名类冲突）。
- i18n：`cron.page.form.aionrsModelRequired` → `dreamEngineModelRequired`
  （全部语言 cron.json + 代码引用）。
- data-testid：`aionrs-model-selector`/`aionrs-model-option-*`/`aionrs-mode-option-*`/
  `aionrs-attach-folder-btn`/`aionrs-file-upload-input`/`aionrs-file-tag-*`/
  `agent-mode-selector-aionrs` → `dream-engine-*`（组件 + e2e specs 同步）。
- dev console 标签：`[AionUi:process]`/`[AionUi:init]` → `[OneWork:*]`；
  更新器临时目录 `AionUi-update-*` → `OneWork-update-*`。

### 3.7 Sentry / 遥测

- 27 个 `aionui.*` 标签键 → `dream.*`（`sentry.ts` 写入/过滤 +
  `InstallationIntegrityDialog.tsx`/`main.tsx` + 两个测试文件）。
  ⚠️ Sentry 面板上旧事件仍是旧标签，已保存的搜索/仪表盘要改用新标签。
- localStorage `aionui.gpuAutoDisableNoticeAckAt` → `dream.*`（老用户会再看一次
  GPU 提示，无其他影响）。`aionui.backend_startup.missing_bundled_dir` 这类
  **历史事故档案**（`sentry-feedback-resolutions/*.json`）不改——那是事实记录。

### 3.8 moltbook 移除（用户拍板"不需要了"）

- 删除：`assets/builtin-skills/moltbook/` 全套、3 语言 assistant rules、头像、
  `assistants.json` 的 moltbook 助手条目（剩 21 个内置助手）。
- `migrateAssistants.ts` 的 `PRESET_ID_WHITELIST` 移除 `'moltbook'`：老用户已装的
  moltbook 助手此后按自定义助手保留，不丢数据。
- 市场源 `one-work-content`：删 `trawl`（纯 Moltbook 背景扫描）与
  `doppel-social-outreach`（主要渠道即 Moltbook）两个技能 + index.json 条目；
  其余 10 个只是正文顺带提到该外部平台的技能保留。
- 顺带：`publish_xiaohongshu.py` profile 目录、`RemotionVideoExpert.md` 工具名等
  资产清理同轮完成。

### 3.9 其他

- `dream-ui/.aionui/`（无代码引用的开发档案目录）→ `.dev-archive`。
- `packages/desktop/node_modules/@aionui/web-host` 孤儿符号链接（P4 改包名后
  bun 未清的陈旧链接）删除；源码无任何 `@aionui/*` import。
- 历史事实记录一律不改：`docs/guides/session-*.md`、
  `sentry-feedback-resolutions/*.json`、已发布 DB 迁移 SQL、`CHANGELOG.md`。

## 四、顺手修复的真实 bug（不是测试断言过时）

1. **cron 模型解析比较旧值**：`executor.rs::resolve_model` 条件是
   `job.agent_type != "aionrs"`（注释却写着 Only dream…），dream 类型定时任务全部
   拿不到 model → 改为 `!= "dream"`，4 个测试夹具与用户可见错误文案同步。
2. **Dockerfile 设置旧环境变量**：`AIONUI_PORT/DATA_DIR/ALLOW_REMOTE/LOG_JSON` →
   `DREAM_*`（部署此前会静默错端口/错数据目录）。
3. **dream-en compose 同类问题**：`AIONUI_HOST/DATA_DIR/ADMIN_PORT` →
   `ONE_*`（后端实际读 `ONE_HOST/ONE_DATA_DIR/ONE_ADMIN_PORT`）。
4. **自动更新默认仓库指向上游**：`version.rs DEFAULT_REPO = iOfficeAI/AionUi` →
   `gaogg521/dream-core`（否则应用把上游发布当更新源）。
5. **NSIS patcher 对已补丁 node_modules 不兼容**：改名后幂等分支失配会让打包
   直接失败 → 加旧模板升级分支（§3.3）。
6. **binaryResolver legacy 回退缺 `.exe` 后缀**（win32 下会去找无后缀的
   `aioncore`）——重构候选循环时一并修掉。

## 五、刻意保留的 aion 痕迹（白名单，动不得/不值得动）

- **持久化 wire 值与别名**：枚举 serde/sqlx alias（`aionrs`/`aionui`）、
  `user_type "aionpro"`、已发布 DB 迁移 SQL、`data_paths.rs` 的 legacy fallback
  （`aionui-backend.db`/`aionrs-sessions`/`aionui-process`）、`upload_paths.rs`
  `LEGACY_UPLOAD_ROOT_DIR`、`_aionui_` 时间戳分隔符（历史文件名格式）、
  legacy 环境变量名（`legacy_env.rs`、`AIONUI_MEDIA_*`、`AIONUI_*_USER_ID` 等
  concat 拆写防误改）、`initStorage` 旧配置文件名、`legacyStorageKeys.ts`、
  `__aionui_theme` 回退、legacy MCP 名（`aionui-image-generation` 等，
  与后端 `AUTO_INJECTED_BUILTIN_NAMES` 对齐）、`LEGACY_BUTLER_ASSISTANT_ID`、
  `engine.aionui` manifest 兼容字段、`LEGACY_PROTOCOL_SCHEMES`（`aionui://` 冻结）。
- **身份冻结项**：appId、`aionui://` scheme 注册、历史安装目录探测
  （`/Applications/AionUi/aioncore` 等）、`PROD_USERDATA_APP_NAME` 首启迁移、
  kill 列表里的旧进程名。
- **生态/外部契约**：hub 目录与 zips 来自上游 `iOfficeAI/AionHub`（打包时下载，
  本地快照改了会被覆盖——私有化需自建 hub 仓库，见 §七）；
  `@aionui/webui` npm 警告（指真实存在的外部包）；`iOfficeAI/OfficeCli`（外部依赖）；
  NSIS 安装器 `AIONUI_MSG_*`/`AIONUI_E_*` 宏名族（构建期符号，P4 即定规则不动，
  用户可见的是宏展开后的文案）。
- **历史记录**：session 文档、sentry-feedback-resolutions 事故档案、CHANGELOG。

## 六、验证记录

- **dream-core**：`cargo check --workspace` 绿；13 个受影响包 nextest 全绿——
  含 app 包 **874/874**（moltbook 移除后全量重跑）、ai-agent 1083/1083、
  cron 281、system 293、conversation/mcp/assistant/channel/common/project/auth/db/
  employee 全绿。资产相关 e2e（assistants/skills_builtin/assets）73/73。
- **dream-ui**：`tsc --noEmit` 绿；全量 vitest 5128+ 通过（过程中暴露的 6 个失败
  全是测试夹具未随改名同步，逐一修复复验；previews 两个为机器负载超时，隔离通过）。
- **dream-engine**：确认干净。
- 环境注意事项：这台机器虚拟内存偏紧——dreamcore.exe 运行时锁住
  `target/debug/dreamcore.exe` 导致 cargo 无法重链（`拒绝访问 os error 5`）、
  内存不足时 rustc 崩溃（`STATUS_STACK_BUFFER_OVERRUN`）或报
  `os error 1455 页面文件太小`。跑全量测试前先退出桌面 app，必要时加大页面文件。

## 七、遗留与后续

1. **hub 私有化**（独立基础设施任务）：fork AionHub → 重命名各 zip 内 manifest 的
   `name`/`author`/`description`（`aionext-*` → 自有前缀）→ 重算 integrity →
   `prepareHubResources.js` 的 `BASE_URLS` 两行指向新仓库 → 已装用户的扩展 ID
   需要一条 DB 迁移（同 AgentType 模式）。在此之前 hub 内容仍来自上游目录。
2. `user_type "aionpro"` 的 wire 值仍是冻结值（变体已是 DreamPro）；若要连值一起
   换，需要一条 SQL 迁移 + API 客户端确认，收益低（UI 不展示），暂缓。
3. 重新打包发布后，用户可见面（进程名 onework.exe、日志标签 [dreamcore]、日志
   文件名、安装目录）才会真正切换；服务器部署参照改好的 docker-compose/Dockerfile
   同步环境变量名。
4. 建议择机跑一次完整 `cargo nextest run --workspace`（本会话受内存限制按包跑绿）。

## 八、踩坑记录（下次直接绕开）

1. **本会话自建的扫描器**（`scratchpad/scan_aion.py`）：Rust raw-string 分支遇到
   `r#ident`（raw identifier）不前进会死循环——判断失败时必须 `i += 1`。
2. **Git Bash 的 ugrep**：`grep -rl` 输出反斜杠路径，管道给 sed 会丢路径分隔符；
   批量替换统一用 `scratchpad/codemod.py`（Windows 安全、带计数）。
3. **超长 heredoc 会被 bash 截断**：大段脚本一律先 Write 成文件再 `python file.py`，
   不要 `python - <<'EOF'`。
4. **grep 模式 `[[AION_FILES]]` 是字符集合**（匹配任一列字符），曾误中 ~300 文件；
   好在替换是字面量、内容不变。字面量匹配记得 `-F` 或转义。
5. **`include_dir!` 资产是编译期嵌入**：改 `assets/` 后必须重编译才生效，正在跑的
   测试二进制仍是旧资产。
6. **`.claude/worktrees/` 下的旧 worktree 副本不要改**（本会话曾差点点错
   query-lockers.ps1 的 worktree 副本）。
7. **注释改写的语义红线**：指"历史版本"的注释（pre-fork、legacy 名、older than
   the rename）不能盲改，本会话在 comment-only pass 里加了 legacy-note 守卫 + 事后
   diff 复核，仍人工修回了两处（"legacy `aioncore` name"、"(pre-fork) aioncore"）。
