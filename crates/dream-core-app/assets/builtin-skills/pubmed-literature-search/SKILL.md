---
name: pubmed-literature-search
display_name: "pubmed搜文献助手"
description: PubMed 关键词文献检索助手。用于「搜文献 / 查文献 / 检索 PubMed / 帮我找论文 / 出文献报告」等需求：按客户关键词与时间窗（必填）在 PubMed 全量检索，。触发：用户提出「pubmed搜文献」相关需求时使用（常见说法：pubmed搜文献、pubmed搜文献助手；英文：pubmed/literature/search）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# pubmed搜文献助手

输入「客户关键词 + 限制条件」，产出标准文献报告 `pubmed_<主题slug>_report.html`。
本技能自包含（样式/IF 表/检索脚本/验证器全在本包内），不依赖任何其他技能或外部服务。

## 铁律（违反即红线，一票否决）

0. **全面性优先于标注（最高优先级）**：文献**准确且全面**是第一位，影响因子只是附加标注。
   - 期刊未被 IF 表收录（预印本、新刊、中文刊、已除名刊等）→ **文章一律保留**，不显示 IF、打「IF未收录」徽章，**严禁因查不到 IF 而删除文章**。
   - IF 阈值只用于筛掉「**已确定低于阈值**」的文章；无法判定的一律进报告并单独计数。
   - 召回链条（窗口内命中 / 取回 / 入选 / 未收录）必须完整可核对，缺任一环不许出报告。

1. **时间窗客户必填**。客户没给时间范围就先问，不得自设默认窗口。
2. **限制如实转换**：客户给的关键词与限制（文献类型/语言/主题词等）如实组装为 PubMed 语法，不得私自放宽或收窄。
3. **翻译 ≥95%**：标题中文翻译逐篇给出；通讯作者单位必须有中译。期刊名/专业缩写可保留英文。
4. **IF 来源唯一且口径固定**：只用本包 `scripts/journal_if_map.json`（**JCR2025 年度版**全量 22643 刊），查表键 = 脚本输出的 `journal_canon` 规范名（大写、去地名括号、去冠词 The、句点转空格；如 `Nature reviews. Drug discovery` → `NATURE REVIEWS DRUG DISCOVERY`）**精确命中**；未收录期刊（含预印本）不显示指标，绝不编造。禁用 OpenAlex、IF≈、影响因子约 等估算表述。**报告必须标注「IF 为 JCR2025 年度值，非文章发表当年 IF」**——同一篇文章在不同年份 IF 不同，本技能不做逐年追溯。
5. **作者全名**：作者列表写全名，禁 `et al.`；通讯作者加 `__CORR__` 前缀高亮。
6. **样式逐字节**：`<style>` 从 `scripts/report-style.css` 复制，禁改任何选择器/变量/字号。
7. **门禁退出码 0 才可交付**：交付前必跑 `scripts/report_validator.py`。

## 工作流

### 第 1 步 · 明确需求
向客户确认/汇总：① 核心关键词（中英均可）② 时间窗（必填）③ 其他限制（文献类型、语言、篇数上限，默认 100、上限 500）。

### 第 2 步 · 组装检索式并检索
把客户需求组装成 PubMed 语法，运行（零第三方依赖，任意 Python 3.8+）：

```
python scripts/pubmed_search.py \
  --query "toxoplasma gondii AND bradyzoite" \
  --mindate 2025/01/01 --maxdate 2026/09/06 \
  --retmax 100 --sort relevance \
  --out articles.json
```

- `--sort`：`relevance`（相关度）或 `pub_date`（时间倒序）。
- 可选 `--api-key`（NCBI API key，提升限速）。
- 客户给了 IF 阈值时**必须**加 `--if-gt <阈值>`：由脚本内完成查表与筛选，并打印**召回审计**（窗口内命中/取回/IF 表命中/入选/已确定低于阈值/IF 表未收录）。
- 输出 JSON 中 `selected` = 已确定 IF>阈值；**`unknown_if` = IF 表未收录期刊的文章，必须一并写进报告并标「IF未收录」，不得丢弃**（报告内该字段置 `ifv:null` 即不显示 IF）。
- 输出 JSON 每篇含：`pmid/year/title/journal/journal_full/journal_canon/ptype/authors/corr/aff/aff_src/doi`。

**检索式必须覆盖同义词与变体**（首单教训：只写 `CtBP[tiab]` 漏掉 `CtBP3`/连字符写法/`BARS`），组装完后自查：缩写、全称、连字符变体、亚型编号、旧称/别名是否已纳入。

### 第 3 步 · 相关性核查（强制，防"命中≠相关"）
对候选逐篇**抓摘要确认检索目标确有实质机制学/生物学角色**（`efetch` 取 abstract 定位关键词上下文），三类必须处理：
1. **命名关联**：目标词只出现在另一个蛋白/复合物的名字里（如 CtIP = CtBP-interacting protein）→ 剔除；
2. **词形误命中**：缩写与普通英文单词撞形（如 BARS/bars）→ 剔除；
3. **仅背景提及**：摘要只一笔带过、非本文研究对象 → 保留但需在交付说明中标注。

同时复核第 2 步的未收录清单：若其中有明显高 IF 期刊却查不到，是期刊名规范化问题（副题 ` : XXX`、分号副题、不闭合括号、and vs &、逗号、撇号等），**先修 `journal_canon` 再重跑**；修完仍查不到的，按铁律 0 **保留文章、标「IF未收录」**。

### 第 3 步续 · 翻译与查表补齐
逐篇补齐：`zh`（标题中译）、`aff_zh`（通讯作者单位中译）、`ifv/ifq`（`journal_full` 大写精确查 IF 表）。**aff 只取通讯作者本人名下单位**（脚本已按末位作者口径提取，`aff_src` 标记回退情况），单条通常 ≤400 字符。

### 第 4 步 · 组装报告（脚本参数化，无模板文件依赖）
第 3 步的富化 JSON 存盘后，运行构建器（HTML 骨架/样式引用/JS 渲染全部内嵌在脚本内）：

```
python scripts/build_report.py \
  --data articles_enriched.json \
  --topic "弓形虫入侵机制" \
  --query "组装后的PubMed检索式" \
  --window "2025/01/01 - 2026/09/06" \
  --audit "窗口内命中124｜取回124｜IF表未收录14｜IF>10入选13｜相关性剔除3" \
  --out pubmed_<主题slug>_report.html
```

- `--audit` **必填**（缺了脚本拒绝生成）：把第 2 步召回审计与第 3 步剔除数写成一行，打印在报告 hero 区，**让客户能自己核对有没有漏**。

- 报告结构（hero/stats 4 卡/筛选栏/卡片 DOM/JS 渲染）由 `build_report.py` 内嵌骨架**确定性生成**，不存在"以某模板文件为准"的外部依赖。
- AI 只负责第 3 步的翻译与 IF 查表补齐（`zh/aff_zh/ifv/ifq`），组装零自由发挥。
- 结构规格文档见 @references/文献报告模板规格.md（仅作人类可读说明，组装以 `scripts/build_report.py` 为准）。

### 第 5 步 · 门禁（退出码 0 才可交付）

```
python scripts/report_validator.py <报告.html>
```

验证器自动校验：结构闭合、净化词/红线词 0 命中、翻译达标、单位中译齐全、**IF 防编造**（逐篇与 IF 表核对）、PMID 进入链接齐全、**召回审计行存在且含 命中/取回/入选**（缺任一数字直接失败）。❌ 出现则回对应铁律修正后重跑。

## 文件结构

本技能按 本平台 开放平台规范封装，zip 内**只含以下文件**（多余的示例报告/图标/说明文档不进包，避免平台解析失败）：

```
pubmed-search-assistant/
├── SKILL.md                          # ★ 技能定义（必须）
├── references/
│   └── 文献报告模板规格.md            # 报告结构规格（人类可读文档, 组装以 build_report.py 为准）
└── scripts/
    ├── pubmed_search.py              # PubMed 检索（E-utilities, 零依赖）
    ├── build_report.py               # 报告构建器（HTML 骨架内嵌, 参数化生成）
    ├── report-style.css              # 报告样式母版（构建器读取, 禁改）
    ├── journal_if_map.json           # JCR2025 影响因子全量映射表（紧凑数组格式: 首键 _fields 声明字段序, load_ifmap 统一还原, 新旧格式兼容）
    └── report_validator.py           # 报告质量门禁
```

脚本均用 `os.path.dirname(__file__)` 定位同目录资源，解压到任意目录即可运行，无绝对路径依赖、无第三方依赖（Python 3.8+）。

## 边界

- 仅检索与报告生成，不代替客户做文献评价结论。
- PubMed 未收录的内容（非生物医学、部分会议文献）不保证命中；`total_in_window` 大于 `retmax` 时提示客户收窄条件或提高上限。
