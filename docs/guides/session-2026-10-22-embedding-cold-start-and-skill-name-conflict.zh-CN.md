# Embedding 首次检索慢 + 官方技能与自建技能重名冲突

日期：2026-10-22。来源：`outputs/module-fixes-20261020/FIX-REGRESSION.zh-CN.md` “仍未覆盖的边界” 第 6、7 条。

## 1. 首次检索慢（第 7 条）

**现象**：演示服升级后首轮 embedding 健康探测约 17.5 秒；搜索只给 embedding 2 秒预算，超时就降级成关键词检索，所以自然语言整句（和库里文字没有字面重合）首次返回空；随后同句约 0.66 秒就能命中。

**根因（代码层，已验证）**：`search_rag` 用 `tokio::time::timeout(2s, embed(..))` 包住 embedding 调用，超时即 **丢弃** 这个 future——请求被取消，冷的模型/端点（模型要加载、首次 TLS、本地 ONNX 引擎初始化）永远没机会跑完，每次“早期搜索”都重新付一次冷启动，并且结果整句无关键词命中时直接空。
冷态在远端端点内部**为什么**慢（模型被换出？资源竞争？）仍未确认——这点没有变，下面是缓解而不是对远端的修复。

**改动**（`dream-domain-devops`）：

- 新模块 `query_embedding.rs`：查询向量请求超过预算后**不取消**，在后台跑完并写入 LRU 缓存（64 条，键 = 端点 + 模型 + 查询文本）；相同查询并发时共享同一个请求；在途请求上限 8，防止端点挂死时堆积任务。同一句话再搜一次直接命中缓存，走完整语义排序。
- `DevopsService::spawn_embedding_warmup`：进程启动 20 秒后，若库里已有索引分块则预热一次；之后每 4 分钟预热一次，但仅限最近 30 分钟内有人搜索过（空闲服务器不会一直 ping）。本地模型尚未下载到磁盘时**不会**从后台触发下载（`embedding::local_model_ready`）。
- 接线：`dream-core-app` 完整服务端构建处（`router/routes.rs`）。

**测试**：`rag_cold_embedding_finishes_in_the_background_and_the_next_search_is_semantic`（首个请求故意慢 2.6 秒：首搜在 4 秒内返回空；等待后同句命中且只发过 1 次 embedding 请求）、`rag_embedding_warmup_calls_the_endpoint_only_when_there_is_something_to_search`；缓存键隔离/LRU/活跃窗口的单测。

**没做、也不能声称**：没有在真实的演示服上量化冷启动前后的差别；预热是否消除远端的 17.5 秒要部署后再看 `knowledge embedding warmed` 日志里的 `elapsed_ms`（info 级）。

## 2. 官方技能与自建技能重名（第 6 条）

**现象**：官方“测试用例设计”与自建技能重名，同步报 `name '测试用例设计' is already taken by a self-built row; rename one of them`，被算作 1 项失败，每次同步都重复（先下载并校验 SKILL.md，再失败）。

**第一版（9ca4d28，已被取代）**：重名条目直接跳过、连 payload 都不下载，只在报告里列为冲突。用户否决了这个做法：官方内容会就此永远进不来，等于被隐藏。

**现行做法（5a06479）**：照常下载、照常导入，名字冲突时给市场这一份加区分标识，自建那份一个字不动。

- 技能：`原名（来源名）`，来源名截前 24 个字符，例如「测试用例设计（官方 One Work 内容）」；同时改写 SKILL.md frontmatter 里的 `name`。团队技能在成员端按注册表 ID 建目录，但引擎按 frontmatter 的 `name` 识别技能，不改这一处的话成员端会出现两个同名技能。
- MCP：`原名-market`（ASCII）。MCP 名会成为各 Agent CLI 配置里的键和工具名前缀，只能用 `[A-Za-z0-9_-]`。
- 标签名本身也被占用时，追加序号（`（来源名 2）`、`-market-2`）。
- 之后的同步更新内容时保留标签名，frontmatter 也同步改写（同步映射按上游原名记录，不受影响）。
- 每次同步的报告都会在 `conflicts` 里列出 `{item, importedAs, reason}`，不会因为已经导入过就从报告里消失；冲突不算失败。
- `upsert_new` 里原有的拒绝保留，作为并发下的最后防线。

**管理后台（dream-en，2966a4c）**：同步报告两处显示「原名 → 导入后的名称」，13 种语言文案改为“已加来源标签导入”。旧版管理后台不认识 `conflicts`，会忽略它（条目照常导入，只是看不到说明）。

**测试**：`a_self_built_name_collision_imports_the_market_entry_under_a_labelled_name`、`a_labelled_entry_keeps_its_label_on_update_and_stays_listed_as_a_conflict`、`labels_are_readable_for_skills_and_identifier_safe_for_mcp`、`only_the_top_level_frontmatter_name_is_rewritten`；admin-web `marketSourcesTab.test.tsx`。

## 3. 验证记录

- `cargo nextest run -p dream-domain-devops --no-fail-fast`：244 通过、1 跳过。
- dream-en `admin-web`：`tsc --noEmit` 通过；市场相关 2 个测试文件 10 项通过。
- 未做：真机/演示服验证；企业镜像与管理后台尚未重新构建发布。
