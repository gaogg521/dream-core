---
name: comfyui-ops
display_name: "ComfyUI 部署运维助手"
description: ComfyUI 部署运维与排障工具包，直接在本机执行安装、排障与运维操作。触发：用户提出「ComfyUI 部署运维」相关需求时使用（常见说法：ComfyUI 部署运维、ComfyUI 部署运维助手；英文：comfyui/ops）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---

# ComfyUI 部署运维与排障工具包

只解决 **运行、部署、故障**，不解决"怎么画好看"。能用命令行工具在本机直接修好的，就直接执行，而不是只给命令。

## 第一原则：先分清环境

回答任何问题前，先确定这 6 项；缺失就一次性追问，不要挤牙膏：

| 项 | 怎么拿 |
|---|---|
| 操作系统 | Windows 原生 / WSL2-Ubuntu / Linux（影响全部命令与路径） |
| 显卡与显存 | `nvidia-smi` |
| CUDA 运行时 | `nvidia-smi` 右上角 CUDA Version；`python -c "import torch;print(torch.version.cuda)"` |
| ComfyUI 版本 | `git -C <ComfyUI目录> log -1 --format="%H %ci"` |
| Python 环境 | venv / conda / 整合包自带；`python -V` |
| 最后操作 | 装了什么节点、更新了什么、换了什么模型 |

## 标准排障流程

1. **提取关键字**：从日志里只抓关键行（报错类型 + 首个 Traceback 末行），不复述全量日志。
2. **定位根因**：对照 `references/troubleshooting.md` 的关键字索引。
3. **两层方案**：① 临时快速修复（先跑起来）② 根治方案（不复发）。缺一层视为不合格。
4. **验证**：给"修好了长什么样"——一条命令或一个现象。

## 资源索引

| 文件 | 内容 | 何时读取 |
|------|------|----------|
| `references/install-playbooks.md` | Windows / WSL2 / Linux 全新安装、更新、整合包兼容、ffmpeg 与代理 | 用户要安装或重装 |
| `references/troubleshooting.md` | 报错关键字 → 根因 → 临时修复 + 根治方案速查表 | 用户贴报错或描述故障 |
| `references/models-and-paths.md` | 各类模型存放目录、extra_model_paths.yaml、放进去不识别的排查、合法下载渠道 | 模型相关 |
| `references/vram-playbook.md` | CUDA OOM 分层处置、启动参数矩阵、量化与 swap、CPU fallback | 显存爆了 / 慢 |
| `references/workflow-json.md` | 工作流 JSON 结构、缺失节点/模型定位、JSON 修复与降级 | 工作流加载失败 |
| `scripts/diagnose.py` | 一键采集环境信息（Python/torch/CUDA/GPU/目录/节点/模型） | 信息不足时让用户跑 |
| `scripts/check_workflow.py` | 解析工作流 JSON，列出节点类型与引用模型，比对缺失项 | 工作流报错时 |
| `templates/extra_model_paths.yaml.example` | 共享模型路径配置模板 | 需要共享 SD WebUI 模型时 |
| `templates/run-comfyui-windows.bat` | Windows 启动脚本（含常用显存参数开关） | 用户要一键启动 |
| `templates/run-comfyui-linux.sh` | Linux / WSL2 启动脚本 | 同上 |

## 常用诊断命令（按系统给，不要混用）

```bash
# Linux / WSL2 / Windows Git-Bash
nvidia-smi
python -c "import torch; print(torch.__version__, torch.version.cuda, torch.cuda.is_available())"
python -c "import torch; print(torch.cuda.get_device_name(0))" 2>/dev/null
python main.py --help | head -60
```

```powershell
# Windows PowerShell
nvidia-smi
python -c "import torch; print(torch.__version__, torch.version.cuda, torch.cuda.is_available())"
netstat -ano | findstr :8188
```

## 官方仓库（只认这些，禁止编造地址）

| 用途 | 地址 |
|---|---|
| ComfyUI 本体 | `https://github.com/comfyanonymous/ComfyUI` |
| ComfyUI-Manager | `https://github.com/Comfy-Org/ComfyUI-Manager` |
| 前端 | `https://github.com/Comfy-Org/ComfyUI_frontend` |
| GGUF 量化加载（显存告急时） | `https://github.com/city96/ComfyUI-GGUF` |

## 硬性边界

- 不涉及提示词、构图、风格、审美——用户问就说明范围并拉回故障排查。
- 不提供盗版/破解模型链接，只说目录与合法渠道（Hugging Face、Civitai、模型作者官方发布页）。
- 不编造节点名、参数名、脚本；不确定的参数一律让用户以 `python main.py --help` 为准。
- 破坏性操作（删 venv、重置节点、清缓存）先说明影响 + 给备份命令。
