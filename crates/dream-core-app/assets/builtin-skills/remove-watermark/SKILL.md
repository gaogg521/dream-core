---
name: remove-watermark
display_name: "去水印"
description: 去除图片上的水印、logo、角标、文字、字幕。当用户说"去水印""去掉这个 logo/角标""把这行字抹掉""图片上有水印帮我弄掉""去掉右下角那个标"等时使用。触发：用户提出「去水印」相关需求时使用（常见说法：去水印、去水印；英文：remove/watermark）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 去水印（remove-watermark）

用传统图像修复（OpenCV inpaint）或 AI 修复（LaMa）抹除图片中的水印、logo、角标、文字。

## 依赖

首次使用前确保已安装依赖（缺什么装什么）：

```bash
python -m pip install -r requirements.txt
```

- `opencv-python`：传统路线必需，缺失时脚本会自动降级为 Pillow 模糊兜底（质量较低）。
- `simple-lama-inpainting`：仅 `--ai` 路线需要，首次运行会下载约 200MB 的 LaMa 模型。**安装须用 `--no-deps`**：`python -m pip install simple-lama-inpainting --no-deps`（它的依赖 pin 过旧，会强制降级 numpy/Pillow/opencv 导致构建失败）。
- **AI 模型下载慢（国内）**：模型默认从 GitHub 下载，国内可能极慢。可先用镜像手动下载后放到缓存目录，脚本会直接复用：
  ```bash
  curl -L -o "$HOME/.cache/torch/hub/checkpoints/big-lama.pt" \
    "https://ghfast.top/https://github.com/enesmsahin/simple-lama-inpainting/releases/download/v0.1.0/big-lama.pt"
  ```
  （`ghfast.top` 为 GitHub 加速镜像，若失效可换其他 gh-proxy；也可用 `LAMA_MODEL=/path/to/big-lama.pt` 环境变量指向本地模型文件。）

## 操作流程（务必按顺序）

1. **看图片**：用 Read 工具读取用户给的图片，观察水印在哪、大概多大、是否复杂。
2. **拿尺寸**：跑 `python scripts/remove_watermark.py <图片> --info` 得到宽高。
3. **换算区域**：把水印位置估成归一化比例 `fx,fy,fw,fh`（相对宽高的 0~1），例如"右下角、占宽约 20%、高约 15%"对应 `--region 0.75,0.8,0.2,0.15`。起点 fx/fy 是区域**左上角**的相对位置。
4. **执行**：调用脚本。**选方法**：
   - 小 logo / 简单文字水印 → 默认传统法（快）：`python scripts/remove_watermark.py <图片> --region 0.75,0.8,0.2,0.15`
   - 大面积 / 复杂背景 / 传统法效果差 → 加 `--ai`（质量高、较慢）。
5. **检查结果**：用 Read 看输出图。水印残留就调大区域（fw/fh 适当加 0.03~0.05 或外扩 `--pad`）；把周围背景也抹掉了就调小区域。可换方法重试，直到干净自然。
6. **交付**：告诉用户输出文件路径。

## 脚本位置与调用

脚本在本 skill 目录下，从项目根 `D:\projects\tools` 调用时相对路径为：

```bash
python .claude/skills/remove-watermark/scripts/remove_watermark.py <图片> [参数]
```

常用参数：

| 参数 | 说明 |
|------|------|
| `--info` | 只打印宽高/模式 |
| `--region fx,fy,fw,fh` | 归一化区域（主路径） |
| `--box x,y,w,h` | 绝对像素区域 |
| `--gui` | 交互式框选（备选，会弹窗） |
| `--method telea/ns/ai` | 方法；`--ai` 等价于 `--method ai` |
| `--radius N` | 传统 inpaint 半径（默认 3） |
| `--pad N` | 区域四周外扩像素（默认 8，让过渡更自然） |
| `-o 路径` | 输出路径（默认 `<原名>_clean.<后缀>`） |

## 注意事项

- 图片如果是 RGBA（含透明），脚本会自动保留透明通道；输出为 jpg 时透明会被丢弃。
- 半透明水印直接用 inpaint 可能留痕，此时优先试 `--ai`，或把区域框得比水印稍大一圈。
- 传统法处理带纹理/照片背景的大水印效果有限，预期会留痕时直接上 `--ai`，别反复试传统法浪费时间。
