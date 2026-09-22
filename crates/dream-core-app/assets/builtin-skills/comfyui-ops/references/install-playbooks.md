# 安装 / 更新 / 环境修复 作战手册

官方仓库只有两个：
- 本体 `https://github.com/comfyanonymous/ComfyUI`
- 管理器 `https://github.com/Comfy-Org/ComfyUI-Manager`

## 0. 安装前先确认的三件事

1. **显卡驱动**：`nvidia-smi` 能跑通且显示的 CUDA Version ≥ 12.x。跑不通 = 驱动没装，先装驱动再谈其它。
2. **Python 版本**：3.10 / 3.11 / 3.12 最稳。3.13 早期生态常缺 wheel，遇到 `ERROR: Could not find a version that satisfies the requirement` 直接退回 3.11。
3. **磁盘空间**：仅本体 + 基础依赖约 10GB；一个 SDXL checkpoint 约 6.5GB，一个 FLUX fp16 约 23GB，提前规划盘位。

## 1. Windows 原生安装（推荐路径）

> 以下步骤由本助手直接在本机用 **PowerShell** / **CMD** 执行；如你手动执行，路径带空格必须加引号。

```powershell
# 1) 选盘位（示例 D 盘，避免中文与空格路径）
cd /d D:\
git clone https://github.com/comfyanonymous/ComfyUI.git
cd ComfyUI

# 2) 建虚拟环境（与系统 Python 隔离，出问题可整个删掉重来）
python -m venv venv
.\venv\Scripts\activate

# 3) 装 PyTorch（先装它，否则 requirements.txt 可能拉到 CPU 版）
#    按 nvidia-smi 的 CUDA Version 选 index：cu124 / cu126 / cu128，不确定先试 cu126
pip install torch torchvision torchaudio --extra-index-url https://download.pytorch.org/whl/cu126

# 4) 装本体依赖
pip install -r requirements.txt

# 5) 验证 CUDA 可用 —— 必须输出 True
python -c "import torch; print(torch.__version__, torch.version.cuda, torch.cuda.is_available())"

# 6) 启动
python main.py
```

浏览器打开 `http://127.0.0.1:8188`。

**常见坑**：
- `python` 不是 3.10+：用 `py -3.11 -m venv venv` 指定版本。
- 执行策略拦截 activate：`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`。
- 杀软/OneDrive 把 `models/` 同步走，导致磁盘爆满：ComfyUI 目录不要放在 OneDrive 下。

## 2. WSL2-Ubuntu 安装

```bash
# 前提：Windows 已装 NVIDIA 官方驱动（Game Ready / Studio 均可），WSL2 内不装驱动
nvidia-smi          # WSL2 里能直接看到显卡才算通

sudo apt update && sudo apt install -y git python3-venv python3-pip build-essential

# 务必装到 Linux 文件系统，不要放 /mnt/c
cd ~
git clone https://github.com/comfyanonymous/ComfyUI.git
cd ComfyUI

python3 -m venv venv
source venv/bin/activate
pip install torch torchvision torchaudio --extra-index-url https://download.pytorch.org/whl/cu126
pip install -r requirements.txt
python -c "import torch; print(torch.__version__, torch.version.cuda, torch.cuda.is_available())"

python main.py --listen 0.0.0.0     # WSL2 需要 --listen，否则 Windows 浏览器可能连不上
```

**WSL2 专坑**：
- 放在 `/mnt/c/...` 下：文件权限混乱 + I/O 极慢，模型加载可能慢 5-10 倍。放 `~/`。
- Windows 浏览器访问不了：用 `--listen 0.0.0.0`，访问地址用 `http://localhost:8188`；仍不通则查 Windows 防火墙与 WSL 镜像网络模式（`wsl --status`）。
- 显存被 Windows 侧占用：`nvidia-smi` 看是否有宿主进程占显存，关掉占用程序。
- WSL 默认内存限制：编辑 `C:\Users\<用户名>\.wslconfig` 增加 `[wsl2] memory=32GB swap=32GB`。

## 3. Linux 原生安装

```bash
sudo apt update && sudo apt install -y git python3-venv python3-pip build-essential ffmpeg
cd ~
git clone https://github.com/comfyanonymous/ComfyUI.git
cd ComfyUI
python3 -m venv venv && source venv/bin/activate
pip install torch torchvision torchaudio --extra-index-url https://download.pytorch.org/whl/cu126
pip install -r requirements.txt
python main.py --listen 0.0.0.0
```

## 4. 安装 ComfyUI-Manager

```bash
cd <ComfyUI目录>/custom_nodes
git clone https://github.com/Comfy-Org/ComfyUI-Manager.git
```
重启 ComfyUI，页面出现 "Manager" 按钮即可。升级：
```bash
cd <ComfyUI目录>/custom_nodes/ComfyUI-Manager && git pull
```

## 5. 更新本体（git 方式）

```bash
cd <ComfyUI目录>
git pull
.\venv\Scripts\activate        # Windows
# source venv/bin/activate     # Linux/WSL2
pip install -r requirements.txt
```

`git pull` 报 `Your local changes would be overwritten`：
```bash
git stash && git pull && git stash pop     # 保留改动
# 或放弃本地改动（会丢未提交的修改）
git checkout -- . && git pull
```

更新后节点大面积变红：多半是本体 API 变了，先 `git pull` 所有节点再重启；仍不行按 `troubleshooting.md` 的"节点变红"分支处理。

## 6. 整合包（一键包 / Portable）兼容处理

不优先推荐，但用户已在用就按它的结构处理：

| 现象 | 处理 |
|---|---|
| 找不到 `venv` | 整合包一般自带 `python_embeded/`，用 `python_embeded\python.exe -m pip install xxx` |
| 页面没有 Manager | 手工 `git clone` Manager 到 `ComfyUI\custom_nodes\`，用整合包自带的启动器重启 |
| 更新报错 / 无法 git pull | 整合包常非 git 仓库或版本被锁定；建议备份 `models/` 与 `workflows/` 后改用官方 git 安装，再用 `extra_model_paths.yaml` 指向原模型目录 |
| 路径带中文或空格 | 移动/重命名到纯英文无空格路径，否则部分节点加载失败 |

## 7. ffmpeg 缺失

**症状**：`ffmpeg` / `Unable to find ffmpeg` / 视频相关节点输出失败、预览不动。

```powershell
# Windows（三选一）
winget install ffmpeg
choco install ffmpeg
# 手工：gyan.dev 的 essentials 构建解压后把 bin 目录加入 PATH
ffmpeg -version
```
```bash
# Linux / WSL2
sudo apt install -y ffmpeg && ffmpeg -version
```
装完**必须重开终端**（PATH 才生效），必要时重启 ComfyUI。

## 8. 代理与网络问题

Hugging Face / GitHub / PyPI 拉不动时：

```powershell
# Windows PowerShell（当前会话生效）
$env:HTTPS_PROXY="http://127.0.0.1:7890"
$env:HTTP_PROXY="http://127.0.0.1:7890"
```
```bash
# Linux / WSL2
export HTTPS_PROXY=http://127.0.0.1:7890
export HTTP_PROXY=http://127.0.0.1:7890
```

- pip 单独指定源：`pip install -r requirements.txt -i https://pypi.tuna.tsinghua.edu.cn/simple`
- pip 走代理装 torch：`pip install torch --extra-index-url https://download.pytorch.org/whl/cu126 --proxy http://127.0.0.1:7890`
- WSL2 里用 Windows 的代理：地址填 Windows 宿主 IP（`ip route show default` 看到的网关），并确认代理软件允许局域网连接。
- ComfyUI-Manager 打不开节点列表：它是直接访问 GitHub 的，网络不通就只能手工 `git clone` 节点仓库。

## 9. 启动参数速查（以 `python main.py --help` 为最终准绳）

| 参数 | 作用 | 适用 |
|---|---|---|
| `--listen 0.0.0.0` | 允许非本机访问 | WSL2 / 局域网 / 远程 |
| `--port 8188` | 指定端口 | 端口冲突 |
| `--lowvram` | 激进省显存，速度最慢 | ≤6GB |
| `--medvram` | 中等省显存 | 6-10GB |
| `--normalvram` | 默认策略 | ≥12GB |
| `--highvram` / `--gpu-only` | 模型常驻显存，最快 | ≥24GB |
| `--cpu` | 纯 CPU 跑（极慢） | 无 N 卡兜底 |
| `--cuda-device 0` | 指定显卡 | 多卡 |
| `--bf16-vae` / `--fp16-vae` | VAE 半精度，省显存 | 30 系及以上优先 bf16 |
| `--force-upcast-attention` | 强制 upcast，修复部分黑图 | 出黑图/NaN |
| `--disable-smart-memory` | 关掉智能内存调度 | 特定 OOM 场景 |
| `--enable-cors-header` | 允许跨域 | 前端分离部署 |
| `--extra-model-paths-config <yaml>` | 指定共享模型配置 | 多前端共用模型 |
| `--disable-auto-launch` | 不自动开浏览器 | 服务器 |

## 10. 启动脚本模板

见 `templates/run-comfyui-windows.bat` 与 `templates/run-comfyui-linux.sh`，把里面的路径与显存参数改成用户实际情况再交付。
