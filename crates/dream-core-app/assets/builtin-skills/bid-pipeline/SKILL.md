---
name: bid-pipeline
display_name: "招投标内容生产流水线"
description: 招投标/专家库内容生产技能。当用户需要把征集公告改写成公众号/头条/小红书/视频号/抖音发布内容、生成封面贴图视频、发布前做合规校验或选题去重时触发。提供封面/贴图/视频/HTML。触发：用户提出「招投标内容生产流水线」相关需求时使用（常见说法：招投标内容生产流水线、招投标内容生产流水线；英文：bid/pipeline）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# bid-pipeline 技能

本技能是「招投标内容智作团」的共享工具箱，包含封面/配图/视频/纯文本生成脚本、合规与平台参考文档、字体与品牌资产。团队成员在各自阶段按需调用，主理人负责中转产物。

## 目录

- `scripts/`：生成与校验脚本（Python/Shell）
- `references/`：视觉规范 `cover_spec.md`、平台口径 `platform_guide.md`、品牌与红线 `brand_compliance.md`
- `assets/`：`AI形象.png`（品牌形象）、`cover_bg_h.png` / `cover_bg_v.png`（封面背景占位图，可替换为正式生成图）
- `fonts/`：**不随独立技能包分发**（单字体 19MB，超技能包 3MB 限制）。脚本内置回退：`/tmp/LXGWWenKai.ttf` → 系统 `/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc`；如需霞鹜文楷效果，自行下载放到 `/tmp/` 即可

## 环境变量

`BID_SHARED_DIR`：脚本产物与共享资源的根目录。未设置时默认指向本技能目录（`skills/bid-pipeline/`，内含 `fonts/`、`assets/`），保证开箱即用。各脚本产物默认落 `<BID_SHARED_DIR>/<date>/`。

## 脚本用法

- `build_html.py`：公众号 HTML 卡片式排版（`gen <blocks.json> <out.html> --deep --accent ...` / `refit <old.html> <out.html>`）
- `build_plaintext.py <平台md> [xhs|toutiao]`：小红书/头条纯文本可复制版
- `build_covers.py`：公众号横版 + 小红书竖版封面（参数化，禁止手改结构）
- `build_xhs_cards.py`：小红书三图组合
- `build_videos.py`：视频号/抖音 竖版 mp4（配音+字幕）
- `check_no_leak.py` / `check_topic_dup.py` / `check_ai_taste.py` / `check_wx_tt_depth.py`：发布前四项校验
- `repair_websearch.sh`：qclaw 网关 web 搜索健康巡检（环境专用，非必须）

## 调用约定（团队成员）

- 封面设计师：用 build_covers / build_xhs_cards / build_videos 出图出视频。
- 发布管家：用 build_html / build_plaintext 出排版与纯文本，并跑 4 项 check_* 校验。
- 所有参数化调用，禁止手改脚本内部结构与尺寸。
- 配色与版式严格遵循 `references/cover_spec.md`；平台口径与红线遵循 `references/platform_guide.md`、`references/brand_compliance.md`。

## 依赖

- Python 3 + Pillow（封面/配图/视频脚本）；`edge_tts`（视频配音，可选）。
- 字体依赖 `fonts/LXGWWenKai.ttf`；缺失时脚本回退系统 Noto CJK。
