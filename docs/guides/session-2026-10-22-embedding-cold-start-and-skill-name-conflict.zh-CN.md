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

**决策**：自建内容继续优先，不覆盖、不自动改名（名称是技能的身份，自动加后缀会破坏同步映射和去重）。改变的是**怎么报告**：

- 同步报告新增 `conflicts: [{item, reason}]`，与 `errors` 分开；冲突不再算失败。
- 在批量预取和逐条处理两处都**提前识别**（已导入过的条目不受影响），冲突条目不再下载 payload。
- 管理员把任意一方改名后，下次同步自动导入官方版本（有测试）。
- `upsert_new` 里原有的拒绝保留，作为并发下的最后防线。

**管理后台（dream-en）**：同步报告弹窗两处（市场来源页、后台同步条）新增“与自建内容同名”分组和一句操作提示；顶部状态条追加“N 项与自建内容同名，已保留自建版本”；13 种语言文案。旧版管理后台不认识 `conflicts` 字段，会忽略它（条目被跳过但看不到提示），所以管理后台需要随后端一起升级。

**测试**：`a_self_built_name_collision_is_a_reported_conflict_not_a_failure_or_takeover`（含“未下载 payload”断言）、`renaming_the_self_built_row_lets_the_next_sync_import_the_official_one`；admin-web `marketSourcesTab.test.tsx` 新增冲突渲染用例。

## 3. 验证记录

- `cargo nextest run -p dream-domain-devops --no-fail-fast`：244 通过、1 跳过。
- dream-en `admin-web`：`tsc --noEmit` 通过；市场相关 2 个测试文件 10 项通过。
- 未做：真机/演示服验证；企业镜像与管理后台尚未重新构建发布。
