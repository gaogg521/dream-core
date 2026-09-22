# 模型存放路径与管理

## 1. 标准目录映射（相对 ComfyUI 根目录）

| 模型类型 | 目录 | 常见后缀 |
|---|---|---|
| Checkpoint（大模型） | `models/checkpoints/` | `.safetensors` `.ckpt` |
| LoRA | `models/loras/` | `.safetensors` |
| VAE | `models/vae/` | `.safetensors` `.pt` |
| ControlNet | `models/controlnet/` | `.safetensors` `.pth` |
| Embedding / Textual Inversion | `models/embeddings/` | `.safetensors` `.pt` |
| 放大模型 | `models/upscale_models/` | `.pth` `.safetensors` |
| CLIP | `models/clip/` | `.safetensors` |
| UNET（含 GGUF 量化） | `models/unet/` | `.safetensors` `.gguf` |
| CLIP Vision | `models/clip_vision/` | `.safetensors` |
| Style Model | `models/style_models/` | `.safetensors` |
| 模型补丁 | `models/model_patches/` | `.safetensors` |
| Diffusers 格式 | `models/diffusers/` | 目录形式 |

> 子目录允许嵌套：例如 `models/loras/SDXL/xxx.safetensors`，加载器里会以 `SDXL\xxx.safetensors` 形式出现（Windows 用 `\`，Linux 用 `/`）。**工作流引用的是这个带子目录的名字**，跨系统迁移工作流时要注意。

## 2. 与 SD WebUI 共享模型：extra_model_paths.yaml

把 `extra_model_paths.yaml.example` 复制为 `extra_model_paths.yaml` 并编辑：

```yaml
a111:
    base_path: D:/stable-diffusion-webui/     # 注意用正斜杠或双反斜杠
    checkpoints: models/Stable-diffusion
    vae: models/VAE
    loras: models/Lora
    embeddings: embeddings
    controlnet: models/ControlNet
    upscale_models: models/ESRGAN
```

- 也可以启动时指定：`python main.py --extra-model-paths-config D:/path/extra_model_paths.yaml`
- 改完**必须重启** ComfyUI。
- 路径含空格或中文时尽量用引号包裹，或先把目录改成英文无空格。
- 模板见 `templates/extra_model_paths.yaml.example`。

## 3. "模型放进去不显示" 排查顺序

1. **目录是否对**：Checkpoint 放 `models/checkpoints/`，不是 `models/Stable-diffusion/`（那是 WebUI 的）。
2. **是否刷新**：网页端 Refresh，或重启 ComfyUI；新版本也支持启动时自动扫描。
3. **文件是否完整**：对比下载源的文件大小；`.safetensors` 下载中断是头号原因。
4. **是否被 git 忽略**：在某些目录里 `.gitignore` 不会影响扫描，但磁盘空间要注意。
5. **是否用了 extra_model_paths**：`base_path` 写错会让整组路径失效。
6. **扩展名是否被改**：`xxx.safetensors.part` / `xxx.safetensors.zip` 不会被识别。
7. **权限**：Linux/WSL2 下 `ls -l` 确认当前用户有读权限。

## 4. 模型容量规划

| 模型 | 大约体积 | 建议显存 |
|---|---|---|
| SD1.5 checkpoint | 2-4 GB | 4 GB+ |
| SDXL checkpoint | 6.5 GB | 8 GB+ |
| SD3 / FLUX fp16 | 11-23 GB | 12-24 GB |
| FLUX GGUF Q5/Q8 | 6-12 GB | 8-12 GB |
| LoRA | 20-500 MB | — |

## 5. 合法获取渠道（只说渠道，不给盗版链接）

- **Hugging Face**：`https://huggingface.co/` —— 官方权重（stabilityai、black-forest-labs 等）首发地。
- **Civitai**：`https://civitai.com/` —— 社区模型，注意每个模型的授权协议（部分禁止商用）。
- **模型作者发布的官方页面 / GitHub Release**。
- 下载后核对文件大小与哈希；来源不明的 `.ckpt` 有 pickle 反序列化风险，**优先 `.safetensors`**。

## 6. 删除 / 迁移模型的注意事项

- 迁移前先备份 `extra_model_paths.yaml` 和 `workflows/`。
- 跨盘移动大模型用 `mv`（同盘瞬间完成），跨盘需真正拷贝，确认完成再删原文件。
- 不要直接删正在被 ComfyUI 加载的模型文件，先停 ComfyUI。
