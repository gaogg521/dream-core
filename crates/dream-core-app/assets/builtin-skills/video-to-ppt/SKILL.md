---
name: video-to-ppt
display_name: "视频转PPT"
description: 把演示/会议/课程录屏视频自动切页、抽帧、画质增强，导出成每页一张图铺满的图片型 PPTX。支持四档画质（无损 PNG / 高清 / 压缩 / 原尺寸）、切页灵敏度与相邻相似页去重。触发：用户提出「视频转PPT」相关需求时使用（常见说法：视频转PPT、视频转PPT；英文：video/to/ppt）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 视频转 PPT

## 用途

把 PPT 演示录屏、讲课录像、会议汇报视频自动切成一页页幻灯片，经画质增强后导出成「每页一张图铺满」的图片型 PPTX。

## 何时使用

- 用户有一段演示/讲课视频，想要回PPT 文件。
- 要从视频里把每一页幻灯片单独提取成图片。
- 录屏没有原始 PPT，只能从视频还原课件。

## 环境准备（首次使用必做）

脚本依赖 OpenCV、NumPy、Pillow、python-pptx。用隔离环境的 Python 安装：

```bash
pip install -r scripts/requirements.txt
```

装完先跑内置自测确认环境可用：

```bash
python scripts/video2ppt.py --selftest
```

输出 `SELFTEST OK pages=3 pptx=True` 即环境正常，再处理用户视频。

> 依赖未装就跑会直接报 ImportError，先装依赖再执行，不要跳过自测。

## 如何使用

### 第一步：问清需求，选参数

默认参数（`--quality lossless --sample 0.3 --thresh 12`）适合大多数 1080p 录屏。用户有明确偏好时再调：

| 用户诉求 | 参数 |
|---|---|
| 要最清晰，不介意体积 | `--quality lossless`（默认） |
| 要微信/邮件能发 | `--quality compress`（约 25MB / 62 页） |
| 要清晰又要小一点 | `--quality high`（约 30MB / 62 页） |
| 要每页单独图片 | 加 `--keep-png` |
| 要双击就能放、零依赖分享 | 加 `--keep-html`（图片内嵌单文件，可直接浏览器放映，也可发布上线分享） |

### 第二步：执行

```bash
python scripts/video2ppt.py "E:\视频\演示.mp4" --out "E:\输出目录" --quality lossless
```

输出目录默认在视频同级的 `<视频名>_ppt/`，产物为 `<视频名>_导出.pptx`。

长视频（1GB / 50 分钟）约需 5~7 分钟，属正常，跑之前告诉用户别以为卡死。

### 第三步：按结果调参

结果不满意时，按 `references/使用与调参说明.md` 第四章「按症状开药」调整，不要瞎试：

- **漏页**（两页合成一页）→ 减小 `--sample`（0.3 → 0.15）
- **多页**（动画被当翻页）→ 增大 `--thresh`（12 → 20~30）
- **重复页**（演讲者来回翻）→ 确认 `--dedup` 开启（默认开）
- **体积太大** → 改 `--quality compress` 或 `high`

## 判定纪律

- **产物是图片型 PPT**，每页是一张图，**不做 OCR、不还原可编辑文字**。用户若要可编辑文本，明确说明需另做 OCR 或人工录入，不要含糊承诺。
- **不要建议加全局白平衡**：早期版本这么做会把 PPT 背景和图表搞偏色，已刻意移除。
- **偏色/发虚等问题先查版本**，确认用的是当前 `scripts/video2ppt.py`（v3.1）。
- **产物体积大是正常的**：62 页 1080p 无损 PNG 约 55MB，不要当成故障。
- **纯色页可能漏切**：已加双判据兜底，极端情况仍可能漏；先调低 `--thresh` 重跑，别直接判定工具坏了。
- 视频路径含空格或中文时，命令行务必加引号。

## 参考

- `scripts/video2ppt.py` — 主程序（606 行单文件），可直接运行。
- `scripts/requirements.txt` — 依赖清单（含打包成 exe 用的 pyinstaller 说明）。
- `references/使用与调参说明.md` — 完整参数表、画质档位、调参指南、画质处理链、已知坑与版本记录。
