---
name: ppt-studio
description: Default for making a NEW PowerPoint deck (PPT/slides/presentation/路演/汇报). Write one deck.json, the bundled engine renders a designed .pptx in seconds. Editing an existing .pptx → officecli-pptx.
---

# PPT Studio — designed decks in one shot

You write **content and structure**; the bundled engine owns **design and geometry** (themes, layouts,
font sizing, gradients, charts). Never place shapes by coordinates, never pick colors or font sizes —
that is what made decks slow and ugly. A typical 12-page deck is: one `deck.json` write + one command.

Use this skill for any request to create a deck from scratch. Use `officecli-pptx` only to read or
modify an existing .pptx, and `morph-ppt` / `officecli-pitch-deck` only when the user explicitly asks
for Morph animation or an investor pitch deck built with officecli.

## Workflow (do not add steps)

1. **Plan silently.** Decide the storyline and the exact page list in your head — if the user named a
   page count, plan exactly that many slides (cover and closing included) before writing anything. Don't ask clarifying questions
   unless the topic itself is unknowable; pick sensible defaults (10–14 pages, theme by scenario).
   If the user gave source material (files, data, a doc), read it first and build from it.
2. **Write `deck.json`** into the workspace with the file-write tool (UTF-8). One write.
3. **Render:**
   ```
   node "<base directory of this skill>/scripts/build-deck.cjs" deck.json -o <名字>.pptx
   ```
   Paths with spaces need the quotes. Same command on Windows/macOS/Linux.
4. **Read the output.** `OK N 页` = success. Any `警告` line names a page whose text cannot fit even
   at the minimum size, a page missing its content field, or a page-count mismatch → fix exactly those
   pages in deck.json (one edit pass covering all warnings) and re-run (re-runs take <1s). Errors name the
   bad field/value; fix and re-run.
5. **Deliver**: tell the user the file name, page count and theme in 2–3 lines. Don't paste the JSON.
   Keep deck.json next to the .pptx — later edits ("把第 5 页改成…", "换个主题") are deck.json edits
   + re-render, not officecli surgery.

If `node` is not found, fall back to the `officecli-pptx` skill and tell the user in one line.

## deck.json

```json
{
  "title": "文件标题（写入属性）",
  "theme": "business-blue",
  "footer": "页脚文字，可省略",
  "pages": 20,
  "slides": [ { "type": "cover", ... }, ... ]
}
```

`pages` = the page count the user asked for (omit if they didn't); the engine warns when the slide
count differs. Every slide accepts `"notes": "演讲者备注"`. Content slides accept `"kicker"` (short label above the
title, e.g. `增长趋势`) and `"subtitle"`. Full worked example covering every layout:
`reference/example-deck.json` — read it once if unsure about a field.

### Layouts

| type | Use for | Fields |
|---|---|---|
| `cover` | First page | `title`, `subtitle`, `kicker`, `author`, `date`, `image`? |
| `agenda` | Table of contents | `items: [string]` (3–8) |
| `section` | Chapter divider | `title`, `subtitle`, `no`? (auto 01, 02…) |
| `bullets` | Points / arguments | `bullets: [string \| {text, sub:[string]}]` (≤6 top-level), plus optional `highlight: {value, title, text}` callout **or** `image` |
| `cards` | 2–6 parallel items (features, pillars) | `cards: [{icon, title, text}]` — `icon` is 1–2 chars or emoji, e.g. `"AI"`, `"安"` |
| `stats` | 2–4 headline numbers | `stats: [{value, label, detail}]`, `summary`? |
| `chart` | Data | `chart: {kind, categories, series:[{name, values}], unit?}`, `insights: [string]` (0–4) |
| `table` | Structured comparison | `columns: [..]`, `rows: [[..]]` (≤8 rows), `highlight`: column index to emphasize |
| `timeline` | Roadmap / history | `steps: [{label, title, text}]` (3–6) |
| `process` | Sequential method / flow | `steps: [{title, text}]` (3–5) |
| `compare` | Before/after, us vs them | `left: {title, points}`, `right: {title, points}` (right side is emphasized) |
| `matrix` | SWOT / 2×2 | `quadrants: [{tag, title, points}]` ×4 |
| `funnel` | Conversion stages | `stages: [{label, value, text}]` (3–6) |
| `quote` | A key statement | `quote`, `author` |
| `image-text` | Photo + explanation | `image`, `title`, `text`, `bullets`?, `imageSide: "left"\|"right"` |
| `image` | Full-bleed picture | `image`, `title`?, `caption`? |
| `closing` | Last page | `title` (default 谢谢), `subtitle`, `contact` |

`chart.kind`: `bar` `bar-h` `stacked` `stacked-h` `line` `area` `pie` `doughnut` `radar`.
Values are plain numbers (`52`, not `"52万"`); put the unit in the series name or `unit`.

`image` must be a local file path (absolute, or relative to deck.json). If the user wants pictures and
an image-generation tool is available, generate them into the workspace first; otherwise use layouts
without images — never invent paths or URLs.

### Themes

| theme | Fits |
|---|---|
| `business-blue` | 工作汇报、方案、通用商务（默认） |
| `midnight-tech` | 发布会、AI/技术分享、开发者大会（深色） |
| `aurora` | 产品发布、创意、品牌、年轻化（深色渐变） |
| `minimal-mono` | 咨询报告、设计、高级感 |
| `warm-terracotta` | 消费、文旅、人文、教育 |
| `forest-green` | ESG、金融、医疗健康、农业 |
| `china-red` | 政务、党建、年终总结、庆典 |
| `ocean-teal` | 医疗、教育、互联网、清新风 |

The user naming a color or mood overrides the table.

## Content rules (this is where quality comes from)

- **Structure**: `cover` → `agenda` (≥8 pages) → per chapter a `section` + 2–4 content pages → `closing`.
- **Titles are conclusions**, not topics: "季度营收持续加速，Q3 环比增长 38%" beats "营收情况".
- **Vary layouts.** Never use the same content layout on two consecutive pages; a deck that is mostly
  `bullets` is a failure. Numbers → `stats`/`chart`; parallel items → `cards`; sequence →
  `process`/`timeline`; contrast → `compare`/`table`.
- **Brevity**: bullet ≤ 30 字, card text ≤ 45 字, insight ≤ 40 字, ≤ 6 bullets per page. Concrete
  numbers and names over adjectives. If you have more, split the page.
- **Real data only.** Use figures from the user or their files. When illustrating with invented
  numbers, say so in your reply (and keep them plausible).
- Match the user's language for all slide text; keep `kicker` short (2–6 字 or 1–3 English words).
