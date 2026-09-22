# remove-watermark skill

一个去除图片水印 / logo / 角标 / 文字的 Claude Code skill，底层用传统图像修复 + AI 修复两条路线。

## 快速开始

```bash
cd D:\projects\tools
python -m pip install -r .claude/skills/remove-watermark/requirements.txt
```

然后在 Claude Code 里说："去掉 xxx.png 右下角的水印"，skill 会自动触发并引导完成。

## 直接命令行用（不经 Claude）

```bash
# 看图片信息
python .claude/skills/remove-watermark/scripts/remove_watermark.py photo.png --info

# 去掉右下角一块水印（归一化坐标：起点(0.7,0.8)，宽20%高15%）
python .claude/skills/remove-watermark/scripts/remove_watermark.py photo.png --region 0.7,0.8,0.2,0.15 -o out.png

# 用 AI（LaMa）高质量修复
python .claude/skills/remove-watermark/scripts/remove_watermark.py photo.png --region 0.7,0.8,0.2,0.15 --ai

# 用绝对像素坐标
python .claude/skills/remove-watermark/scripts/remove_watermark.py photo.png --box 1200,1800,400,250

# 交互式框选（弹窗，鼠标拖框后回车）
python .claude/skills/remove-watermark/scripts/remove_watermark.py photo.png --gui
```

## 定位水印的三种方式

| 方式 | 参数 | 适用 |
|------|------|------|
| 归一化坐标 | `--region fx,fy,fw,fh` | 知道水印相对位置（Claude 驱动主路径） |
| 像素坐标 | `--box x,y,w,h` | 知道精确像素 |
| 交互框选 | `--gui` | 人手动选 |

## 传统 vs AI

| | 传统（默认 telea/ns） | AI（--ai，LaMa） |
|---|---|---|
| 速度 | 秒级 | 数秒~数十秒（CPU） |
| 依赖 | opencv-python | simple-lama-inpainting（torch/torchvision） |
| 效果 | 小 logo / 简单文字好 | 大面积 / 复杂背景 / 纹理好 |
| 首次 | 无需下载 | 需下载约 200MB 模型 |

> AI 路线安装注意：`simple-lama-inpainting` 的依赖 pin 过旧（numpy<2 / pillow<10 / opencv<5），直接装会强制降级现有库。请用
> `python -m pip install simple-lama-inpainting --no-deps`（torch、torchvision 需已装好）。

> AI 模型下载慢（国内）：模型默认从 GitHub 下载。可先用镜像手动下载后放到缓存目录，脚本会直接复用：
> ```bash
> curl -L -o "$HOME/.cache/torch/hub/checkpoints/big-lama.pt" \
>   "https://ghfast.top/https://github.com/enesmsahin/simple-lama-inpainting/releases/download/v0.1.0/big-lama.pt"
> ```

## 全局安装为 skill

项目级 skill 只在 `D:\projects\tools` 下可用。想在所有项目里用，把整个目录复制到个人 skill 目录：

```bash
cp -r D:/projects/tools/.claude/skills/remove-watermark ~/.claude/skills/
```

（脚本用相对路径，迁移后无需改动；调用时把脚本路径换成 `~/.claude/skills/remove-watermark/scripts/remove_watermark.py`，或直接在 SKILL.md 里按实际路径调整示例。）
