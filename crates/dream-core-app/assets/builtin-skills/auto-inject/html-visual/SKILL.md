---
name: html-visual
description: Make pictures whose text and dates must be right (日历 年历 小报 海报 信息图 菜单 课程表 证书) - verify facts, lay out HTML, render PNG with render_html_to_image. Text-free art goes to image generation.
---

# HTML Visual — text-heavy pictures that are actually correct

An image model cannot reason or count. Asked for a 2027 calendar it drew February with 29 days and
a garbled zodiac year; told "2 月错了", it painted a new picture of February alone. So for any
picture whose **content** matters, you think the request through, own the facts and the layout in
HTML, and the app's Chromium turns it into a PNG. The HTML stays editable; every later fix is an
edit + re-render.

## Workflow

### 1. Understand and verify — out loud, briefly

The user should see that you understood the brief and checked the facts before anything is drawn.
Write short progress lines in your reply as you go (one or two sentences each, no essays):

1. **Read the brief.** One line naming the deliverable and what makes it hard, e.g.
   「这是一份数学周的四季年历小报：要把数学+科学+劳动三科融进去，日期错一格就是知识性错误，
   所以先核对 2027 年的日历数据，不能凭记忆写。」 Pick up every requirement in the prompt (themes,
   subjects, season split, size, audience) — they become sections of the page. Missing details get a
   sensible default stated in that line, not a question.
2. **Lock the facts.** Classify each fact:
   - **Calendar / lunar / 节气 / festivals → run the bundled script, never recall them:**
     ```
     node "<base directory of this skill>/scripts/calendar-data.cjs" <year> -o outputs/calendar-<year>.json
     ```
     It prints a summary (平年/闰年、天数、每月天数、干支生肖、春节、节日、24 节气) and writes every
     day to the JSON. Its source is the Hong Kong Observatory's official 公历农历对照表. Do **not** use
     JavaScript's `Intl` Chinese calendar as truth — it puts 2027 春节 on 2/7; the correct date is 2/6.
     If the script prints `⚠未核验` (offline), say so to the user and mark lunar data as unverified.
   - **Must be looked up** (official holiday 调休 schedules, event facts, prices) → search if you have
     a web tool and cite it in one line; next year's 调休 is usually not published yet — say so and leave
     it out rather than invent it.
   - **Computable** (sums, ages, countdowns, percentages) → compute in code.
   - **User-given** (names, slogans, season boundaries) → copy exactly.
   Then report in one line what is now locked, e.g.
   「日历数据已锁定：2027 平年 365 天、2 月 28 天，丁未羊年，春节 2/6（周六），端午 6/9、中秋 9/15、
   冬至 12/22（来源：香港天文台对照表）。」
3. **Plan.** Show the build steps as a short task list (use your task-list tool if you have one),
   e.g. 生成排版母版 → 渲染 PNG 并自检 → 交付. Then build.

### 2. Write one self-contained `<name>.html`

In the workspace's `outputs/` folder (create it if missing), with the file-write tool:

- Wrap the whole picture in `<main id="canvas">` with a fixed pixel size, e.g. A4 portrait
  `1240×1754`, poster `1080×1440`, phone long image `1080×` auto height, 16:9 `1920×1080`.
- All CSS inline in `<style>`; no external fonts, CDNs or network images (the renderer may be
  offline). Font stack: `"PingFang SC","Microsoft YaHei","Noto Sans CJK SC",sans-serif`.
- Calendar grids come from the script's JSON: paste the parts you need into a `<script>` constant
  (or `months` for grids, `days[i].lunar` / `term` / `festivals` for labels) and build the grid in
  JavaScript, synchronously. Never hand-type day numbers or lunar dates into the HTML.
- Decoration through CSS gradients, inline SVG, emoji. If the user wants real illustrations and
  `one_image_generation` is available, generate them as separate artwork (no text inside them) into
  `outputs/assets/` and reference them with relative paths — all text stays in HTML.
- Content quality matters as much as layout: for a 小报/作业, write real, age-appropriate content
  for each required subject (e.g. 数学: 平年闰年怎么算、各月天数规律; 科学: 节气与地球公转; 劳动:
  每季一项家务/种植任务), not filler.

### 3. Render and check

Call `render_html_to_image` with `source_type: "html_file"`, the HTML's absolute path,
`output_path` = same name `.png`, `width` = the canvas width, `selector: "#canvas"`.

Look at the PNG yourself (view the image if you have a view-image tool): text cut off or
overlapping, empty areas, and re-check the facts against step 1 (every month's day count, the
festival dates). Fix the HTML and re-render; one or two passes, not more.

### 4. Deliver

Give both paths (PNG first, then the editable HTML) and the one-line fact summary from step 1, in
3–4 lines, and invite changes. A follow-up such as "2 月错了" or "换个颜色" is a request to **check
and edit this HTML and re-render it** — re-verify the disputed fact with the script before
answering, and if the page was already right, say so with the source instead of changing it.

If `render_html_to_image` is not available, deliver the HTML and say in one line that PNG export
could not run here (the user can open it in a browser and print or screenshot it).

## Do not

- Do not call an image-generation model for the whole page, and do not ask it to "fix" text.
- Do not write calendar, lunar or 节气 data from memory, and do not trust `Intl` for lunar dates.
- Do not ask clarifying questions you can answer with a sensible default (size, style, palette).
