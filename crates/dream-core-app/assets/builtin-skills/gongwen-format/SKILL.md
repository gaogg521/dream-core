---
name: gongwen-format
display_name: "文档格式化排版"
description: 把用户编好内容的 Word 或 Markdown 文档一键排版为符合党政机关公文国标（GB/T 9704-2012 / GB/T 33476.2-2016）的 docx：字体字号行。触发：用户提出「文档格式化排版」相关需求时使用（常见说法：文档格式化排版、文档格式化排版；英文：gongwen/format）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 文档格式化排版（gongwen-format v2.0）

**作者**：胡立敏 ｜ **依据**：GB/T 9704-2012《党政机关公文格式》、GB/T 33476.2-2016《党政机关电子公文格式规范 第 2 部分》

## 何时使用

用户给出 Word（.docx）或 Markdown（.md）文件，要求"排版 / 格式化 / 按公文国标刷新格式"时使用。
核心承诺：**只刷格式，不改文字**——不删字、不改字、不合并段落。

## 快速开始

### 路径一：Word 排版

```bash
python <skill>/scripts/format.py <输入.docx> -o <输出-formatted.docx> --report <变更报告.md>
```

- 自动识别 13 种段落角色（标题/发文字号/主送/正文/层次序数/图片段/图题表题/落款/抄送/印发等）
- 输出：排版后 docx + Markdown 变更清单；退出码 0=ERROR 清零，1=仍有 ERROR

### 路径二：Markdown 排版

```bash
python <skill>/scripts/md_parser.py <输入.md> -o <输出-formatted.docx> --report <变更报告.md>
```

- 段前用 `<!--role: TITLE-->` 等注释标注角色（不标默认 BODY）；支持 `|表格|` 语法和 `![图题](图片路径)` 插图
- 相对图片路径以 .md 所在目录为基准解析；alt 文字非空时自动生成图题段

### 路径三：只体检不改

```bash
python <skill>/scripts/check.py <输入.docx> --out <报告.md>
```

## 排版规则速览

| 要素 | 规则 | 依据 |
|---|---|---|
| 大标题 | 二号 22pt 方正小标宋，居中，**大纲 1 级** | §7.3.1 |
| 一级"一、" | 三号 16pt 黑体，**大纲 2 级** | §7.3.3.2 |
| 二级"（一）" | 三号 16pt 楷体，**大纲 3 级** | §7.3.3.2 |
| 三四级"1./①" | 三号仿宋，**大纲 4 级** | §7.3.3.2 |
| 正文 | 三号仿宋，28 磅固定行距，首行缩进 2 字 | §7.3.3 |
| **图片段** | 居中、**单倍行距（防裁剪）**、段前后 6 磅 | 排版惯例 |
| **图题/表题** | "图1 xxx"→四号仿宋居中、单倍行距 | 排版惯例 |
| **表格** | 表头黑体、数据仿宋（四号）、整体居中、单元格居中、单倍行距 | 排版惯例 |
| 落款/日期 | 右对齐、右空 2 字、月日不补零 | §8.3 |
| 页面 | A4，版心 156×225mm（默认 33476.2 软件坐标；`--strict` 走 9704 严格坐标） | §6.1 |

> 大纲级别 1-4 级 = Word「视图 → 导航窗格」层级树，可折叠/跳转，引用→目录也可一键生成。

## 使用约定（Agent 执行时遵守——效率优先，硬性要求）

**标准流程只有一条命令**。format.py 已内置「排版前校验 → 角色识别 → 排版 → 排版后校验 → 变更报告」全流程，一条命令跑完：

```bash
python <skill>/scripts/format.py <输入.docx> -o <输出-formatted.docx> --report <变更报告.md>
```

硬性禁令（违反即严重拖慢交付，每多一轮命令就是一次 Agent 往返）：

1. **禁止先跑 check.py 预检**——format.py 自带 before/after ERROR 对比。仅当用户明确说"只体检不要改"时才单独跑 check.py。
2. **禁止写临时检查脚本**（inspect_*.py 之类）——变更报告已含逐段角色/置信度/变更明细，直接读报告回答用户。
3. **禁止额外备份步骤**——输出永远是新文件（-formatted.docx），源文件天然不被覆盖，不要复制 .bak。
4. **Python 探测最多一次**——先直接用 `python`；报 ModuleNotFoundError 才提示 `pip install python-docx` 安装后重试；同一会话记住可用解释器，不重复探测。
5. **不主动逐段复核**——仅当变更报告中低置信段落涉及标题/主送等关键角色时，一句话提醒用户即可，不阻塞交付。

例外流程（仅三种）：

- Markdown 输入 → `python <skill>/scripts/md_parser.py <输入.md> -o <输出.docx>`（同样一条命令）
- 只体检不改 → `python <skill>/scripts/check.py <输入.docx> --out <报告.md>`
- 专用模板豁免：等保定级报告、红头模板等，用户要求保留原版式时只跑 check 不跑 format。

## 已知边界（如实告知用户）

- 不改正文页脚页码（避免破坏原页脚域，需手工核对：四号宋体、单页右/双页左）
- 不探测印章是否加盖；只调整落款日期右空字数
- 特定格式公文（信函/命令/纪要）专属版式暂不支持
- 22 行/页 与 28 字/行的实际行数需在 Word 中目测核对（不渲染字体）
- markdown 图片需本地文件路径（网络 URL 不下载）

## 目录结构

```
gongwen-format/
├── SKILL.md              本说明
├── references/           国标参数速查表
├── scripts/              排版引擎
│   ├── format.py         Word → 国标排版（主入口）
│   ├── md_parser.py      Markdown → 国标 docx
│   ├── check.py          合规体检（9 项检查器，只读）
│   ├── classify.py       段落角色识别（13 种）
│   ├── lib_docx.py       python-docx 封装层
│   ├── report.py         报告生成
│   ├── rewrites/styles.py 国标样式套用
│   └── checks/           9 项检查器实现
└── fixtures/             测试样本（good/bad.docx、sample-media.md）
```
