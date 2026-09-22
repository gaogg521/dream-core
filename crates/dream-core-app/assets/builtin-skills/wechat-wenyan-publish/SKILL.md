---
name: wechat-wenyan-publish
display_name: "公众号全流程发布"
description: 微信公众号全流程发布技能：写稿规范 → 封面生成（900×383 无署名）→ SVG 转 PNG 配图 → wenyan 自定义主题排版（redwhite/green/blue/g。触发：用户提出「公众号全流程发布」相关需求时使用（常见说法：公众号全流程发布、公众号全流程发布；英文：wechat/wenyan/publish）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# wechat-wenyan-publish：微信公众号全流程发布

把成熟的公众号发布链路固化成一步到位流程：
**写稿 → 自检 → 封面 → 配图 → 排版 → 发布 → 核实**。

## 何时使用

用户要写公众号文章 / 发草稿 / 排版发布 / 给公众号配图封面（不是简单排版，是完整发布流程）。

## 技能结构

```
wechat-wenyan-publish/
├── SKILL.md                  # 本文件（全流程说明）
├── scripts/
│   ├── preflight.py          # 发布前自检（字节/图片/元数据）
│   ├── gen_cover.py          # 编辑风封面（900×383，无署名）
│   ├── gen_diagrams.py       # SVG→PNG 配图（白底中文标签）
│   └── publish_wenyan.py     # 一键渲染+直连 API 发布（--verify 自动核实）
├── themes/                   # 4 套自定义主题（redwhite/green/blue/gray）
└── references/
    ├── wechat-api-limits.md        # 微信 API 硬约束
    ├── cover-and-image-style.md    # 封面/配图风格规范
    ├── workflow-checklist.md       # 逐项清单与常见坑
    └── theme-tuning.md             # 主题色板映射与调优
```

## 凭证与前置条件（执行前必读）

- **必须由用户提供自己的公众号 AppID / AppSecret**，通过环境变量传入：
  `export WECHAT_APP_ID=wx...` / `export WECHAT_APP_SECRET=...`
- 本技能**不内置、不落盘任何凭证**；没有凭证时只执行到自检/封面/配图/渲染为止，如实告知用户无法发布。
- **IP 白名单**：微信 API 要求调用方服务器 IP 在公众号后台「设置与开发 → 安全中心」白名单内；在沙箱/容器环境运行时若报 40164（IP 不在白名单），如实转告用户，不要重试硬闯。
- `wenyan` CLI（`@wenyan-md/cli`，npm 安装）用于主题渲染；图片工具依赖 Pillow + cairosvg（`pip install Pillow cairosvg`）。

## 执行流程

### 第 0 步：写稿（联网核实事实）
- 写前核实关键事实，标注来源意识；观点性判断按观点处理，不要写成硬事实。
- 用全角标点；不用「本文看点」；章节标题 4-5 字为宜；工具类文章文末附可复制安装命令。
- 文末落款按用户账号的惯用风格，没有则给一个简洁的中性落款。

### 第 1 步：frontmatter（微信 API 硬约束）
```yaml
---
title: 标题（≤64 字节，中文≈3B/字，先数后写）
cover: /绝对路径/cover.png
author: 作者名（≤8 字节）
description: 摘要（≤120 字节，会渲染成文首导语引用块，写一句有钩子的）
---
```
正文图片引用一律 `![](assets/xx.png)`（PNG/JPG，**严禁 SVG**）。

### 第 2 步：自检 + 封面 + 配图
```bash
python scripts/preflight.py article.md              # 字节与图片校验，不过就先修
python scripts/gen_cover.py --out cover.png \
    --eyebrow "栏目 · 日期" \
    --title "主标题" --subtitle "副标题" \
    --pills "卖点1,卖点2,卖点3" [--right-svg 自定义.svg]
python scripts/gen_diagrams.py 图1.svg 图2.svg      # 正文配图（SVG 源建议存 article 目录）
```
- **封面一律不加作者署名**（不放"作者名·"式水印，保持版面干净）。
- 配图统一白底、中文标签、#10a37f 主色，几何要素（箭头/对齐）用坐标算清楚，别写死估数。
- 字体：优先 Noto Sans CJK；脚本内置多路径查找（Linux/macOS/Windows），容器无 CJK 字体时安装 `fonts-noto-cjk`。

### 第 3 步：一键发布（wenyan 主题渲染 → 直连微信 API）
```bash
python scripts/publish_wenyan.py article.md --theme redwhite --dry-run   # 先试跑
python scripts/publish_wenyan.py article.md --theme redwhite --verify    # 正式发+核实
```
- 主题：`redwhite`(默认红白 #DC2626)、`green`(摸鱼绿 #059669)、`blue`(科技蓝 #2563EB)、`gray`(石墨灰 #52525B)。新主题注册：`wenyan theme --add <css>`。
- 脚本自动：wenyan render → 正文图片经 `media/uploadimg` 换成微信链接 → 封面经 `material/add_material` 上传 → `draft/add` 建草稿；`--verify` 再调 `draft/get` 核实。
- 零第三方依赖：API 调用用 Python 标准库 urllib 实现。

### 第 4 步：核实
- `--verify` 自动调用 `draft/get`，核对图数、中文字数、标题/摘要字节。
- 也可以用 `--dry-run` 只渲染不发布，把 HTML 交给用户手动粘贴。

## 常见坑（详见 references/workflow-checklist.md）

1. title/summary 超字节 → 微信 API 直接拒；用 preflight.py 先把关。
2. wenyan 输出正文图为相对路径 → publish_wenyan.py 已自动处理并上传换取微信链接。
3. SVG 进正文 → 微信拒收，一律转 PNG。
4. 40164 IP 不在白名单 → 让用户把当前出口 IP 加进公众号后台白名单，不要反复重试。
5. cairosvg 报 CAIRO_STATUS_WRITE_ERROR → 输出目录不存在，先 mkdir。
6. 40001 invalid credential → access_token 过期或 secret 错误，重取 token 或核对凭证。

## 能力边界

- 只支持**公众号草稿箱**链路（建草稿、查草稿、删草稿）；不负责群发、评论管理、素材库整理。
- 需要**已认证的公众号账号**（个人订阅号即可调 draft API）；测试号无草稿箱能力。
- 排版主题基于 wenyan 渲染器；对 `<style>` 支持有限，复杂交互组件不支持。
