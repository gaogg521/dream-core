---
name: weekly-literature-briefing
display_name: "每周文献热点简报"
description: 每周医学文献热点简报系统（SkeMex 自进化增强版）。功能：(1) 检索PubMed获取指定领域热点文献；(2) AI翻译标题和摘要为中英对照；(3) 生成简报发送到邮箱；(4)。触发：用户提出「每周文献热点简报」相关需求时使用（常见说法：每周文献热点简报、每周文献热点简报；英文：weekly/literature/briefing）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 每周医学文献热点简报（SkeMex 自进化增强版）

## 概述

本技能帮助医学工作者追踪指定研究领域的最新文献热点。**核心升级：集成 SkeMex 自进化技能记忆框架，追踪用户的文献兴趣演变，蒸馏每次简报对话中的有效信息，实现从交互中持续学习的个性化文献推荐。**

## ⚠️ 安全原则

- **config.json 中不得预填任何个人敏感信息**（邮箱地址、SMTP授权码、API密钥等）。技能发布时只包含 `config.template.json`（占位符模板），不包含 `config.json`。
- 首次使用时通过 Phase 1 引导流程，由用户自己提供个人信息并写入 config.json。
- **严禁在脚本中硬编码邮箱、密码、API密钥等凭证**，所有敏感配置必须从 config.json 动态读取。
- **`.skillignore` 文件**确保发布时不打包 `config.json` 和 `.backup/` 等敏感文件。
- **skill_memory/ 中的记忆数据**随用户使用累积，发布时仅包含空白初始模板。

## 你的工作方式

```
0. Skill Memory Read — 加载用户兴趣画像和历史经验（新增）
1. 首次使用引导 / 配置检查（Phase 1）
2. 检索 PubMed（记忆增强检索）（Phase 2）
3. 翻译标题和摘要（Phase 3）
4. 生成简报 + 写入IMA笔记 + 发送邮件（Phase 4）
5. Literature Recommendation — 个性化文献推荐（新增）
6. Skill Memory Write — 蒸馏对话经验、追踪兴趣变化（新增）
```

---

## Phase 0：Skill Memory Read（🆕 SkeMex 增强）

在开始任何检索之前，先从技能记忆库中加载用户的兴趣画像和历史交互经验。

### 0.0 首次运行检测

> **触发条件：** 检查 `skill_memory/` 目录下的文件是否为空白初始模板（`literature_interests.json` 中 `topics` 为空数组、`governance_state.json` 中 `total_interactions` 为 0）。

如果检测到首次运行（记忆库为空白初始状态）：
1. **跳过记忆加载**，直接进入 Phase 1（首次使用引导）
2. 在 Phase 1.2 引导完成后，根据用户配置的检索主题和偏好，**初始化记忆层**（见 Phase 1.2 第2步）
3. 在简报输出中标注「🆕 首次运行 — 记忆层将在后续使用中逐步建立」

### 0.1 记忆库位置

```
<技能目录>/skill_memory/
├── general.json              ← 通用阅读偏好和简报格式记忆
├── literature_interests.json ← 文献兴趣画像 + 兴趣变化追踪
├── briefing_history.json     ← 简报交付记录 + 对话蒸馏
├── recommendation_skills.json← 文献推荐策略记忆
├── evolution_log.md          ← 进化日志
└── governance_state.json     ← 治理状态（上次治理时间、容量等）
```

> **路径说明：** 技能目录即 SKILL.md 所在的目录。通过 `__file__` 或运行环境自动定位。新用户首次使用时，这些文件为空白初始模板。

### 0.2 检索策略：兴趣路由 + 价值感知

**Step 1：兴趣路由**

根据记忆库中的用户兴趣画像，动态调整检索策略：

| 路由信号 | 来源 | 调整动作 |
|----------|------|----------|
| `high_interest` | literature_interests.json 中 interest_level ≥ 0.8 的主题 | 扩大检索量（+50%），优先推送到顶部 |
| `emerging` | interest_level 0.4-0.8 或 trend="increasing" | 正常检索量，标注"新兴关注" |
| `declining` | interest_level < 0.4 或 trend="decreasing" | 缩减检索量（-50%），仅保留高分文献 |
| `new_keyword` | briefing_history.json 中近3次对话新出现的关键词 | 追加检索子主题 |
| `format_pref` | general.json 中的阅读偏好 | 调整翻译深度和排版格式 |

**Step 2：价值感知评分**

对匹配到的记忆技能，计算综合得分：

```
Score = λ_sim × 语义匹配度 + λ_u × 历史效用值 + λ_h × 记忆强度(含时间衰减)
```

- 语义匹配：当前检索主题 vs 记忆中的兴趣标签
- 历史效用：该推荐策略过去的正面采纳率
- 记忆强度：随时间衰减，近期交互获得的兴趣信号权重更高

**Step 3：兴趣注入方式**

将检索到的兴趣画像以结构化方式注入到 Phase 2 的检索上下文中：

```markdown
## 🧠 已加载用户兴趣画像（来自历史交互记忆）

### 高兴趣领域
- VTE / 静脉血栓栓塞症 ⭐兴趣0.95 | 趋势：稳定 | 子主题：DOAC、远端DVT、肿瘤相关VTE
- AI辅助医学教育 ⭐兴趣0.82 | 趋势：上升 | 子主题：ChatGPT、前后测设计、模拟教学

### 新兴关注
- 外周动脉疾病腔内治疗 | 兴趣0.55 | 趋势：上升 | 近2周开始讨论

### 阅读偏好
- 翻译深度：完整翻译（保留结构化标记）
- 推送偏好：Q1/Q2期刊优先
- 互动偏好：简报后经常讨论临床应用场景
```

> **使用原则**：记忆画像是辅助参考，最终检索策略仍以用户 config.json 中明确配置的主题为主。当记忆与配置冲突时，以配置为准，但在输出中提示用户可考虑调整配置。

---

## Phase 1：首次使用引导 / 配置检查

### 1.1 检测 config.json

> **触发条件：** 当用户首次使用本技能，或 `config.json` 不存在时，执行以下引导流程。

当检测到 `config.json` 不存在时，**逐步**向用户确认以下问题（不要一次全部列出，每次 1-2 个，等用户回复后继续）：

### 引导步骤

1. **个人信息**
   - 提示：「请问怎么称呼你？你的职称或单位是什么？」
   - 记录到 `profile.name` 和 `profile.title`。

2. **文献来源确认**
   - 提示：「本技能默认使用 PubMed 作为文献检索来源。是否需要其他来源（如 arXiv、Google Scholar）？」
   - 目前仅支持 PubMed，记录用户偏好备用。

3. **检索领域和主题词（用户输入）**
   - 提示：「请告诉我你希望追踪的研究领域或主题关键词，支持多个板块。例如："静脉血栓栓塞症"、"AI辅助医学教育"、"颈动脉狭窄" 等。」
   - 仅记录用户提供的自然语言主题名称和关键词，**暂不要求用户提供 PubMed 检索表达式**。

4. **PubMed 检索策略自动生成（Agent 生成 + 用户确认）** ⭐
   - 根据上一步用户提供的主题关键词，Agent **自动生成** PubMed 检索表达式（MeSH + tiab 组合），逐个主题展示给用户确认。
   - 展示格式示例：
     ```
     📌 主题：静脉血栓栓塞症
     建议检索表达式：
       Query 1: ("venous thromboembolism"[MeSH] OR "VTE"[tiab] OR "deep vein thrombosis"[tiab] OR "pulmonary embolism"[tiab])
       Query 2: ("anticoagulation"[MeSH] OR "anticoagulant"[tiab] OR "DOAC"[tiab] OR "low molecular weight heparin"[tiab])
       建议期刊白名单: J Vasc Surg, Eur J Vasc Endovasc Surg, Blood, Circulation, Thromb Res
     ```
   - 提示：「以上是根据你的主题自动生成的检索策略，是否需要调整？可以直接告诉我修改意见，或回复"确认"继续。」
   - 用户确认后，写入 `search.topics[].queries` 和 `search.topics[].journal_filter`。
   - 参考 `references/setup-guide.md` 中的检索策略编写要点。
   - **每个主题都要单独生成和确认。**

5. **每主题推送篇数** ⭐
   - 提示：「每个主题每次推送几篇文献？可选：5篇、10篇、20篇（默认20篇）。如果某个主题文献量很大，建议设小一些，保证推送质量。」
   - 写入 `search.max_results_per_topic`（全局默认值）。
   - 如用户对不同主题有差异化需求，可在对应 topic 中单独设置 `max_results` 覆盖全局值。
   - 提醒：「检索时会先从 PubMed 获取该数量的文献，再经过分区/期刊筛选，最终推送的篇数可能少于设定值。」

6. **easyScholar API（期刊分区）** ⭐
   - 提示：「是否需要期刊分区筛选（JCR分区、影响因子）？启用后简报会标注每篇文献的分区和IF，并自动过滤低分区期刊。」
   - 如用户需要，引导获取步骤：
     1. 访问 [easyScholar](https://easyscholar.cc/) 注册账号
     2. 进入「个人中心」→「API 接口」页面
     3. 复制你的 `secret_key`
   - 提示：「请将你的 easyScholar secret_key 提供给我。」
   - 如不需要，设置 `easyScholar.enabled: false`，并告知用户：「未启用分区筛选时，简报将不标注期刊分区，且不会按分区过滤文献。」

7. **SMTP 邮箱配置**
   - 提示：「请提供邮件推送的 SMTP 配置：邮箱地址、SMTP授权码（非登录密码）。常见邮箱：126邮箱(smtp.126.com)、QQ邮箱(smtp.qq.com)、Gmail(smtp.gmail.com)。」

8. **飞书推送（可选）**
   - 提示：「是否需要飞书推送？如不需要可跳过。」

9. **IMA 知识库/笔记**
   - 提示：「是否自动写入 IMA 笔记？」
   - 说明：「IMA 笔记功能由 ima.copilot 平台自动授权，**无需你手动配置 API 密钥**。只需告诉我笔记本名称即可。」
   - 如用户需要，追问：「请提供目标笔记本名称（如"文献简报"）。如果笔记本不存在，我可以帮你创建。留空则写入默认位置。」
   - 记录到 `output.write_ima_note: true` 和 `output.notebook_name`。
   - 如不需要，设置 `output.write_ima_note: false`。

10. **推送周期**
    - 提示：「推送周期：每周一？每周五？还是其他频率？默认每周一 07:00。」

11. **深入解读提示**
    - 提示：「每次发送简报后会提醒：如需深入解读某篇文献，请下载 PDF 原文后进行。是否确认？」
    - 默认开启，在邮件和笔记末尾添加此提示。

### 1.2 引导完成后

1. 将所有信息写入 `config.json`。
2. **初始化记忆层**：根据用户提供的主题和偏好，在 `skill_memory/` 中创建初始兴趣画像：
   - `literature_interests.json`：为每个配置的检索主题创建条目，设置 `interest_level: 0.5`（初始值）、`trend: "stable"`、`first_seen` 时间戳
   - `general.json`：记录翻译偏好（默认完整中英对照）、期刊偏好（根据 easyScholar 配置）
   - `governance_state.json`：设置 `total_interactions: 0`、`briefing_count: 0`
   - `evolution_log.md`：记录初始化事件
3. 告知用户：
   - 配置文件路径
   - 下次触发方式
   - 修改配置方式：直接编辑 `config.json` 或说「修改文献简报配置」
4. **自动提示试运行** ⭐：
   - 提示：「✅ 配置已完成！建议现在试运行一次，确认检索结果和邮件格式是否符合预期。是否立即试运行？」
   - 如用户同意，执行 Phase 2-4 的完整流程（检索时间范围缩短为最近 3 天，避免文献过多）。
   - 试运行完成后，提示用户检查邮件，并根据结果调整配置。

### 1.3 已有配置的用户

如 `config.json` 已存在，直接进入 Phase 0（记忆加载）→ Phase 2（检索）。

---

## Phase 2：检索 PubMed（记忆增强）

### 2.1 执行方式

```bash
python scripts/run_briefing.py config.json
```

### 2.2 流程

1. 读取 `config.json` 中的 `search.topics` 配置
2. **🆕 加载 Phase 0 记忆**：根据兴趣画像调整每个主题的检索策略
   - 高兴趣主题：可适当扩大 `max_results`
   - 新兴关键词：追加子主题检索
   - 下降趋势主题：缩减检索量，仅保留高分文献
3. 对每个主题，执行 PubMed E-utilities API 搜索
4. 获取文献元数据（标题、摘要、作者、期刊、DOI、PMID 等）
5. 如启用 easyScholar，查询期刊 JCR 分区和影响因子
6. 按期刊白名单和分区门槛筛选
7. **🆕 兴趣加权排序**：基于记忆中的用户兴趣，对筛选后的文献进行个性化排序
8. 保存 JSON 数据到 `output.report_dir`

### 2.3 兴趣加权排序算法

```
排序分 = 0.3 × 主题兴趣度 + 0.3 × 期刊质量分 + 0.2 × 子主题匹配度 + 0.2 × 新颖度
```

- **主题兴趣度**：来自 `literature_interests.json` 中该主题的 `interest_level`
- **期刊质量分**：JCR 分区映射（Q1=1.0, Q2=0.8, Q3=0.5, Q4=0.3）
- **子主题匹配度**：文献标题/摘要是否包含用户近期讨论过的关键词
- **新颖度**：发表时间越近分值越高

### 2.4 关键检查点

- 如 `config.json` 不存在 → 进入 Phase 1 引导
- 如 SMTP 配置不完整 → 仅生成报告，不发送邮件
- 如 PubMed 检索无结果 → 提示用户调整检索策略
- 如记忆库中显示用户兴趣已显著变化 → 主动提示用户是否需要更新检索主题

---

## Phase 3：翻译标题和摘要（由 Agent 在对话中完成）

翻译服务由当前对话中的大语言模型完成，无需额外 API。

### 3.1 翻译内容

- 文献英文标题 → 中文标题
- 文献英文摘要 → 中文摘要（保留结构化标记如【目的】【方法】【结果】【结论】）

### 3.2 翻译要求

- 保留原文医学术语对照（如 "深静脉血栓(deep vein thrombosis, DVT)"）
- 统计学数据原文保留，不做意译
- 不确定的术语标注原文
- 药品名称保留通用名 + 商品名（如有）

### 3.3 输出格式：中英对照

翻译后的内容按以下格式组织，确保邮件和笔记中呈现中英对照：

```
## 📌 主题名称

### 1. 中文翻译标题
**English Title:** Original English Title
**期刊:** Journal Name | JCR Q1 | IF=X.X
**发表时间:** YYYY Mon DD
**第一作者:** Author Name
**DOI:** doi: xxx
**PubMed:** https://pubmed.ncbi.nlm.nih.gov/PMID/

**Abstract (English):**
> Original English abstract...

**摘要（中文翻译）:**
> 中文翻译摘要内容...

---
```

### 3.4 🆕 记忆增强翻译

- 检查 `general.json` 中的翻译偏好（如用户偏好"要点提取"而非全文翻译）
- 如果用户近期讨论过某些术语，确保翻译中一致使用用户偏好的译法
- 对于用户高兴趣领域的文献，可适当增加翻译详尽度（如补充研究设计的批判性注释）

---

## Phase 4：生成简报 + 写入IMA笔记 + 发送邮件

### 4.1 写入 IMA 笔记

**标题格式：** `每周文献热点简报（YYYY年M月D日 - YYYY年M月D日）`

**内容格式：**
```markdown
# 每周文献热点简报（****年*月*日 - ****年*月*日）

## 📊 本周摘要
| 板块 | 文献数 | 重点内容 |
|------|--------|----------|
| 主题名称 | N篇 | 关键词 |

## 📌 主题名称

### 1. 中文翻译标题
**English Title:** Original English Title
**期刊:** Journal Name | JCR Q1 | IF=X.X
**发表时间:** YYYY Mon DD
**第一作者:** Author Name
**DOI:** doi: xxx
**PubMed:** https://pubmed.ncbi.nlm.nih.gov/PMID/

**Abstract (English):**
> English abstract...

**摘要（中文翻译）:**
> 中文摘要内容...

---
```

> ⚠️ **IMA 笔记格式规范：** IMA 笔记必须使用**纯 Markdown**，**严禁使用 HTML 标签**（如 `<details>`、`<summary>`、`<div>` 等）。英文原文摘要和中文翻译摘要直接用 Markdown 加粗标题 + 引用块展示，不要做折叠处理。

### 4.2 发送中英对照 HTML 邮件

**执行方式：**
```bash
python scripts/email_sender.py "邮件主题" /path/to/briefing.html config.json
```

**邮件格式（中英对照）：**
- 每篇文献卡片包含：
  - 🇨🇳 中文标题 + 🇬🇧 English Title
  - 期刊分区徽章（JCR、中科院、影响因子）
  - **黄色背景区块**：英文原文摘要（Abstract）
  - **绿色背景区块**：中文翻译摘要
- 页脚注明数据来源和生成信息

**末尾提醒（第11项配置）：**
> 💡 **提示：** 如需对某篇文献进行深入解读，请下载 PDF 原文后提供给我，我将为您进行详细分析。

### 4.3 IMA 笔记操作流程

1. 使用 `push_note` 创建笔记
2. 使用 `rename_note` 设置规范标题
3. 如配置了 `notebook_name`，使用 `move_notes` 移入目标笔记本

---

## Phase 5：Literature Recommendation（🆕 个性化文献推荐）

**简报发送后，基于用户兴趣画像和记忆，主动推荐额外的高相关文献。**

### 5.1 推荐触发条件

以下任一条件满足时执行推荐：
- 用户在简报后主动询问某篇文献的详情
- 用户表达了对某个子主题的深入兴趣（如"有没有更多关于DOAC在肾功能不全中应用的文章？"）
- 记忆库显示某新兴主题的 interest_level 持续上升但尚未纳入正式检索主题
- 简报后的自然对话流中，用户讨论了与简报文献相关但未被检索覆盖的方向

### 5.2 推荐策略

**策略 A：兴趣延伸推荐**

基于当前简报中用户讨论最多的文献，延伸搜索：
1. 提取用户最关注的文献（通过对话热度判断）
2. 搜索该文献的引用文献（cited by）和参考文献（references）
3. 搜索同一作者/团队近期发表的其他文章
4. 用兴趣画像过滤结果，推荐最匹配的 3-5 篇

**策略 B：新兴主题推荐**

基于记忆中的新兴关键词，主动检索：
1. 从 `literature_interests.json` 中提取 trend="increasing" 且 interest_level 0.4-0.8 的主题
2. 执行 PubMed 检索，筛选近 1 个月的高质量文献
3. 推荐 2-3 篇，说明推荐理由："检测到您近期对 XXX 关注增加，以下是相关最新文献"

**策略 C：交叉领域推荐**

基于用户的多主题兴趣画像，推荐交叉领域文献：
1. 识别用户的高兴趣主题组合（如 VTE + AI）
2. 检索交叉领域文献
3. 推荐 1-2 篇，标注"跨领域推荐"

### 5.3 推荐输出格式

```markdown
## 📚 个性化文献推荐

基于您的阅读兴趣和近期互动，为您推荐以下文献：

### 推荐策略：兴趣延伸
**推荐理由：** 您在本期简报中对《XXXX》表现出浓厚兴趣，以下是该团队/方向的最新进展：

1. **中文标题**
   English Title | Journal | JCR Q1 | IF=X.X
   PubMed: https://pubmed.ncbi.nlm.nih.gov/PMID/
   📌 推荐理由：与您讨论的XXX问题直接相关...

2. ...

### 推荐策略：新兴主题
**推荐理由：** 检测到您近期对"XXX"的关注持续增加：

3. **中文标题**
   English Title | Journal | JCR Q2
   PubMed: https://pubmed.ncbi.nlm.nih.gov/PMID/
   📌 推荐理由：这是一个新兴交叉方向，值得跟踪...

---
💡 如需深入解读某篇推荐文献，请告诉我，或下载 PDF 后提供给我。
```

### 5.4 推荐质量保障

- **去重**：推荐文献不得与本期简报中的文献重复
- **时效性**：推荐文献优先选择近 3 个月内发表的
- **质量门槛**：推荐文献须满足用户的 `min_journal_rank` 要求
- **数量控制**：每次推荐不超过 5 篇，避免信息过载
- **可解释性**：每篇推荐文献必须标注推荐理由

---

## Phase 6：Skill Memory Write（🆕 对话蒸馏与兴趣追踪）

**这是核心升级：每次简报交互后，自动从对话中蒸馏有效信息，更新用户兴趣画像，追踪热点领域的变化。**

### 6.1 何时触发 Write

以下任一条件满足时触发记忆蒸馏：
- 用户在简报后讨论了某篇或某几篇文献（正面或负面反馈）
- 用户提出了新的研究方向或关键词
- 用户对简报格式、内容、推送频率提出了调整意见
- 用户对推荐文献做出了反馈（采纳/忽略/拒绝）
- 用户的讨论显示出兴趣焦点的迁移（如从"VTE抗凝"转向"VTE腔内治疗"）

**不触发的情况**：
- 用户只是说"收到"或"谢谢"，无实质讨论内容
- 标准流程化交互（确认配置→执行→完成）
- 基础操作问题（如何修改配置等）

### 6.2 蒸馏流程：三遍分析 + 审核门控

**第一遍：兴趣信号提取**

分析简报发送后的完整对话，提取以下信号：

| 信号类型 | 检测方式 | 记忆动作 |
|----------|----------|----------|
| `topic_engagement` | 用户对某主题的文献展开了讨论 | 提升该主题的 interest_level |
| `topic_dismissal` | 用户明确表示某主题"太多了/不感兴趣" | 降低该主题的 interest_level |
| `new_keyword` | 用户提到了新的子主题或关键词 | 记录到 literature_interests 的 keywords_emerged |
| `new_topic` | 用户表达了全新的研究兴趣方向 | 创建新兴主题条目 |
| `topic_shift` | 讨论焦点从A转向B | 更新趋势标记 |
| `format_feedback` | 用户对简报格式有意见 | 更新 general.json 中的偏好 |
| `recommendation_feedback` | 用户对推荐文献的反馈 | 更新 recommendation_skills.json 效用值 |
| `deep_read_request` | 用户请求深入解读某篇文献 | 记录高兴趣信号 + 该文献 PMID |

**第二遍：兴趣画像更新**

基于提取的信号，更新 `literature_interests.json`：

```json
{
  "topics": [
    {
      "name": "静脉血栓栓塞症",
      "interest_level": 0.95,
      "trend": "stable",
      "subtopics": ["DOAC", "远端DVT", "肿瘤相关VTE", "围手术期抗凝"],
      "keywords_emerged": ["edoxaban 30mg", "bridging anticoagulation", "distal DVT surveillance"],
      "last_discussed": "2026-06-15",
      "total_mentions": 18,
      "papers_discussed": ["PMID:12345678", "PMID:87654321"],
      "papers_deep_read": ["PMID:12345678"],
      "interest_history": [
        {"date": "2026-06-01", "level": 0.80},
        {"date": "2026-06-08", "level": 0.88},
        {"date": "2026-06-15", "level": 0.95}
      ]
    }
  ],
  "emerging_topics": [
    {
      "name": "AI辅助手术规划",
      "first_mentioned": "2026-06-10",
      "interest_level": 0.45,
      "trend": "increasing",
      "confidence": 0.6,
      "trigger_conversation": "用户在VTE简报后提到对AI辅助术前规划的兴趣"
    }
  ]
}
```

**兴趣级别调整规则：**
- 用户深入讨论某主题 → interest_level +0.03~0.05
- 用户请求深入解读 → interest_level +0.05~0.08
- 用户明确表示"太多了/不需要" → interest_level -0.10~0.15
- 用户完全忽略某主题的文献（连续3次无互动） → interest_level -0.05
- 新关键词出现且被多次提及 → 在 subtopics 中记录
- interest_level 变化超过 0.15（连续2次交互）→ 更新 trend 标记

**第三遍：简报对话蒸馏**

将本次简报交互中的有效信息蒸馏为结构化记录，写入 `briefing_history.json`：

```json
{
  "briefings": [
    {
      "date": "2026-06-15",
      "topics_covered": ["VTE", "AI辅助医学教育"],
      "papers_delivered": 12,
      "papers_discussed": 3,
      "papers_deep_read": 1,
      "conversation_insights": [
        {
          "type": "topic_engagement",
          "content": "用户对艾多沙班在老年VTE患者中的剂量选择展开了详细讨论",
          "source_paper_pmid": "PMID:12345678",
          "action": "VTE主题interest_level +0.05; subtopics追加'edoxaban dosing elderly'"
        },
        {
          "type": "new_keyword",
          "content": "用户提到关注'rivaroxaban vs apixaban real-world data'",
          "action": "keywords_emerged追加该关键词"
        },
        {
          "type": "recommendation_feedback",
          "content": "用户采纳了推荐文献#2，忽略#1和#3",
          "action": "策略A(兴趣延伸)效用+0.05; 记录用户偏好'同一团队文献>交叉领域'"
        }
      ],
      "user_satisfaction": "positive",
      "satisfaction_signals": ["主动追问3篇文献详情", "请求深入解读1篇", "无负面反馈"]
    }
  ]
}
```

**审核门控：**
- **新颖性检查**：蒸馏出的信号是否与已有记忆重复？若是，更新而非新增
- **质量检查**：信号是否足够明确？模糊的反馈（如"还行"）不写入
- **安全检查**：确保不记录用户的个人患者信息

### 6.3 写入位置

| 信号类型 | 写入文件 | 说明 |
|----------|----------|------|
| 兴趣级别变化 | `literature_interests.json` | 更新对应主题的 interest_level、trend、interest_history |
| 新关键词/子主题 | `literature_interests.json` | 追加到 topics[].subtopics 或 keywords_emerged |
| 新兴主题 | `literature_interests.json` | 追加到 emerging_topics |
| 简报对话蒸馏 | `briefing_history.json` | 追加到 briefings 数组 |
| 阅读偏好变化 | `general.json` | 更新对应偏好字段 |
| 推荐策略效用 | `recommendation_skills.json` | 更新对应策略的 utility 和采纳计数 |

同时追加到 `evolution_log.md`：
```markdown
### 2026-06-15 21:30 | DISTILL | 简报对话蒸馏
**简报日期：** 2026-06-15
**蒸馏信号：** 3条（topic_engagement × 1, new_keyword × 1, recommendation_feedback × 1）
**兴趣变化：** VTE 0.88→0.95 | 新增关键词'rivaroxaban vs apixaban real-world data'
**推荐策略效用：** 策略A +0.05（用户采纳延伸推荐）
```

### 6.4 效用追踪

每次推荐策略被 Phase 5 使用后：

| 反馈信号 | 效用更新 | 检测方式 |
|----------|----------|----------|
| 用户采纳推荐（请求详情/深入解读） | +0.05~+0.10 | 用户主动询问推荐文献 |
| 用户部分采纳（对部分感兴趣） | +0.00~+0.03 | 用户讨论了部分推荐但非全部 |
| 用户忽略推荐 | +0.00 | 用户未提及任何推荐文献 |
| 用户明确拒绝（"不需要这类"） | -0.10~-0.15 | 用户表示推荐不相关 |

### 6.5 定期治理

**治理频率**：每 20 次交互或每 7 天执行一次

治理操作：
1. **兴趣衰减**：超过 30 天未讨论的主题，interest_level 自动衰减 -0.02/周
2. **新兴主题晋升**：emerging_topics 中 interest_level ≥ 0.7 且 trend="increasing" 持续 3 次交互 → 建议用户纳入正式检索主题
3. **历史归档**：briefing_history.json 超过 50 条记录时，将旧记录归档到 `briefing_archive.json`
4. **推荐策略优化**：淘汰 utility < 0.2 的推荐策略，合并相似策略
5. **容量控制**：每个记忆文件最多保留合理条数，超出时淘汰最低效用项

---

## 文件结构

```
weekly-literature-briefing/
├── SKILL.md                         # 本文件 - 技能定义
└── （.skillignore：安装到本地后生效的发布排除列表，防止 config.json 意意外传；发布包本身不含此文件）
├── config.template.json             # 配置模板（首次使用时复制为 config.json）
├── config.json                      # 用户配置（首次引导后生成，被 .skillignore 排除）
├── scripts/
│   ├── run_briefing.py              # 主运行脚本
│   ├── pubmed_search.py             # PubMed 检索模块
│   ├── html_generator.py            # HTML 邮件生成模块
│   └── email_sender.py              # 邮件发送模块
├── skill_memory/                    # 🆕 SkeMex 记忆层（发布时为空白模板）
│   ├── general.json                 # 通用阅读偏好（初始为空）
│   ├── literature_interests.json    # 文献兴趣画像（初始为空）
│   ├── briefing_history.json        # 简报历史 + 对话蒸馏（初始为空）
│   ├── recommendation_skills.json   # 推荐策略记忆（仅含种子策略）
│   ├── evolution_log.md             # 进化日志（初始为空）
│   └── governance_state.json        # 治理状态（初始计数为0）
└── references/
    └── setup-guide.md               # 详细配置指南
```

---

## 使用示例

### 示例1：首次使用（完整引导流程）

> 用户：「帮我设置每周文献简报」
>
> Agent：检测到 config.json 不存在，进入 Phase 1 引导流程。
>
> **Step 1** Agent：「请问怎么称呼你？你的职称或单位是什么？」
> 用户：「张三，XX医院血管外科主治医师」
>
> **Step 2-4** 逐步确认检索主题、自动生成检索策略...
>
> **完成后** Agent：「✅ 配置已完成！建议现在试运行一次，确认检索结果和邮件格式是否符合预期。是否立即试运行？」
> 用户：「好，试运行一次」
> Agent：执行 Phase 0-4（检索最近 3 天），发送邮件，用户检查效果。
> **🆕 引导完成后初始化记忆层**，记录用户初始兴趣画像。

### 示例2：日常触发（含记忆增强）

> 用户：「运行本周文献简报」
>
> Agent：
> - **Phase 0**：加载用户兴趣画像，发现 VTE 兴趣度 0.95、AI教育 0.82，新兴关注"腔内治疗"
> - **Phase 2**：VTE 主题扩大检索量，腔内治疗追加子主题检索
> - **Phase 3-4**：翻译 + 写入笔记 + 发送邮件
> - **Phase 5**：基于本期内容 + 兴趣画像，推荐 3 篇延伸文献
> - **Phase 6**：等待用户反馈，蒸馏对话

### 示例3：简报后的对话蒸馏

> 用户（收到简报后）：「第3篇 VTE 的那篇 edoxaban 研究很有意思，他们的老年亚组分析怎么说的？」
>
> Agent：展开讨论该文献 → 提供老年亚组数据 → **Phase 5** 推荐延伸文献（同团队/同主题）
>
> **Phase 6 蒸馏**：
> - VTE 主题 interest_level +0.05
> - subtopics 追加 "edoxaban elderly subgroup"
> - papers_discussed 记录该 PMID
> - 简报对话蒸馏记录写入 briefing_history.json

### 示例4：兴趣变化检测

> 用户：「最近外周动脉疾病的腔内治疗有些新进展，帮我看看」
>
> Agent：
> - **Phase 0**：检测到记忆中 PAD 为新兴主题（interest_level 0.5, trend increasing）
> - **Phase 5 策略B**：主动检索 PAD 腔内治疗近 1 月文献，推荐 3 篇
> - **Phase 6**：PAD 兴趣度提升 → 如果持续上升，建议用户将 PAD 纳入正式检索主题

### 示例5：修改配置

> 用户：「把文献简报的推送邮箱改成 xxx@xxx.com」
>
> Agent：读取 config.json，修改 email.recipient 字段，保存并确认。

### 示例6：添加检索主题

> 用户：「文献简报加一个"颈动脉狭窄"的板块」
>
> Agent：自动生成 MeSH 检索策略，展示给用户确认后，在 search.topics 中添加新主题。**同步更新 literature_interests.json**。

---

## 质量检查

每次输出前自查：

| 检查项 | 标准 |
|--------|------|
| 记忆层加载 | Phase 0 已正确加载用户兴趣画像 |
| 检索策略 | 高兴趣主题已适当扩大检索量 |
| 翻译质量 | 中英对照，术语一致，统计学数据准确 |
| 推荐去重 | Phase 5 推荐文献不与简报内容重复 |
| 推荐可解释性 | 每篇推荐文献标注推荐理由 |
| 兴趣追踪 | Phase 6 已正确蒸馏对话信号并更新记忆 |
| 安全性 | 未包含任何个人敏感信息（邮箱、密码、API密钥等） |
| IMA 笔记格式 | 纯 Markdown，无 HTML 标签 |

---

## 配置说明

详细配置项说明请参考 `references/setup-guide.md`。

**必填配置：**
- `profile.name` - 用户名
- `search.topics` - 至少一个检索主题（含 PubMed 检索表达式）
- `email.smtp` + `email.recipient` - 邮件推送配置

**可选配置：**
- `easyScholar` - 期刊分区查询
- `feishu` - 飞书推送
- `zotero` - Zotero 文献管理
- `output.write_ima_note` - IMA 笔记写入

## 注意事项

1. **安全：** `config.json` 包含 API Key 和 SMTP 授权码，不应提交到公开版本库
2. **API 限速：** PubMed E-utilities 建议每秒不超过 3 次请求，easyScholar 每次请求间隔 0.3 秒
3. **翻译质量：** 由 Agent 在对话中使用大模型翻译标题和摘要，翻译质量取决于当前模型
4. **首次引导：** 只在 config.json 不存在时触发，已有配置的用户直接执行检索流程
5. **中英对照：** 所有输出（邮件、笔记）均为中英对照格式，英文原文 + 中文翻译并列展示
6. **MeSH 自动生成：** 首次引导时用户只需提供主题关键词，Agent 自动生成 PubMed 检索表达式供确认
7. **🆕 记忆隐私：** 记忆层仅记录用户的研究兴趣偏好和文献讨论内容，绝不记录个人身份信息或患者信息
8. **🆕 记忆治理：** 兴趣画像会随时间自然衰减（30天不讨论-0.02/周），确保记忆反映用户当前而非历史偏好
