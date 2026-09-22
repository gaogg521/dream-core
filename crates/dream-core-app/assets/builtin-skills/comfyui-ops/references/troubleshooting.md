# 报错关键字 → 根因 → 修复速查表

拿到日志：**只抓关键行**（报错类型 + Traceback 最后一行），不要复述全量日志。按下面关键字定位。

回答结构固定为：**根因 → ① 临时修复 → ② 根治方案 → 验证方式**。

---

## A. 启动即失败 / 环境类

### A1. `Torch not compiled with CUDA enabled` / `torch.cuda.is_available()` 返回 False
**根因**：装到了 CPU 版 torch（常见于先跑 `pip install -r requirements.txt` 再装 torch，或用了国内源镜像无 CUDA 包）。

① 临时：先 `--cpu` 启动确认本体没别的问题（极慢，仅验证）。
② 根治：先卸载再按 index 重装。
```bash
pip uninstall -y torch torchvision torchaudio
pip install torch torchvision torchaudio --extra-index-url https://download.pytorch.org/whl/cu126
```
**验证**：`python -c "import torch; print(torch.version.cuda, torch.cuda.is_available())"` → 输出 CUDA 版本 + `True`。

### A2. `ModuleNotFoundError: No module named 'xxx'`
**根因**：依赖没装全，或装到了别的 Python（系统 Python vs venv）。

① 临时：`pip install xxx`（缺啥装啥）。
② 根治：确认用的是 venv 里的 pip，再整体重装。
```bash
# Windows：确认提示符前有 (venv)，且
where python
where pip
# Linux/WSL2
which python && which pip

pip install -r requirements.txt
```
**验证**：重启后不再报同一个模块。

### A3. `ImportError: DLL load failed while importing ...`（Windows）
**根因**：VC++ 运行库缺失，或 torch 与 Python/CUDA 版本不匹配。
① 临时：装 Microsoft Visual C++ Redistributable（2015-2022 x64）重启。
② 根治：Python 降到 3.11，重建 venv，按 A1 重装 torch。
**验证**：`python -c "import torch"` 无报错。

### A4. `ERROR: Could not find a version that satisfies the requirement xxx`
**根因**：Python 版本过新（3.13）或过旧（3.8），无对应 wheel。
① 临时：换源重试 `pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -r requirements.txt`（有时只是源没同步）。
② 根治：装 Python 3.11，删掉旧 venv 重建。
**验证**：`python -V` → 3.11.x。

### A5. `NVIDIA driver ... is too old` / `CUDA driver version is insufficient`
**根因**：驱动版本低于 torch 需要的 CUDA 运行时。
① 临时：装低一档 CUDA 的 torch（如 `cu121`）。
② 根治：升级 NVIDIA 驱动到最新（`nvidia-smi` 显示的 CUDA Version 是驱动支持上限）。
**验证**：`nvidia-smi` 右上角 CUDA Version ≥ 12.1。

---

## B. 自定义节点 / 变红类

### B1. 节点变红 + `Failed to import custom node` / `IMPORT FAILED: <节点目录>`
**根因**：节点依赖没装、节点与本体版本不兼容、git clone 不完整。

① 临时：禁用该节点（ComfyUI-Manager → 该节点 Disable），先恢复可用。
② 根治：
```bash
cd <ComfyUI目录>/custom_nodes/<出问题的节点目录>
git pull
# 若节点带 requirements.txt
..\..\venv\Scripts\python.exe -m pip install -r requirements.txt    # Windows
# ../../venv/bin/python -m pip install -r requirements.txt          # Linux/WSL2
```
仍失败：删掉该目录，用 Manager 重装；或记录节点名让用户确认是否必需。
**验证**：启动日志不再出现 `IMPORT FAILED`。

### B2. ComfyUI-Manager 打不开 / 节点列表空白 / 404
**根因**：网络不通 GitHub、Manager 版本过旧、或仓库已迁移到 Comfy-Org。
① 临时：手工 `git clone` 需要的节点仓库到 `custom_nodes/`。
② 根治：
```bash
cd <ComfyUI目录>/custom_nodes/ComfyUI-Manager && git pull
```
配合代理环境变量（见 `install-playbooks.md` 第 8 节）重启。
**验证**：页面 Manager 按钮可打开且列表非空。

---

## C. 模型类

### C1. 模型放进目录但 ComfyUI 里不显示
**根因**：放错子目录 / 需要刷新 / 文件损坏 / 扩展名不支持。
① 临时：网页点 Refresh，或重启 ComfyUI。
② 根治：按 `models-and-paths.md` 核对子目录；检查文件大小是否与下载源一致（`safetensors` 下载中断很常见）。
**验证**：`python main.py` 启动日志里能看到 `CheckpointLoader` 列表包含该模型。

### C2. `safetensors_rust.SafetensorError: Error while deserializing header`
**根因**：模型文件下载不完整或被损坏（常见于网盘/断线下载）。
① 临时：换一个模型先用。
② 根治：重新下载，优先用支持断点续传的方式；下完比对文件大小/哈希。
**验证**：加载该 checkpoint 不再报 header 错误。

### C3. `ValueError: ... checkpoint not found` / 工作流报 `Value not in list`
**根因**：工作流引用的模型名与实际文件名不一致（含子目录前缀也算不一致）。
① 临时：改工作流里的模型下拉选项为现有文件。
② 根治：把文件放到正确目录并同名，或用 `check_workflow.py` 列出引用清单逐个补。
**验证**：工作流不再报红。

---

## D. 显存 / 性能类

### D1. `CUDA out of memory` / `torch.cuda.OutOfMemoryError` / `RuntimeError: CUDA error: out of memory`
**根因**：模型 + 分辨率 + 批次超出显存。
① 临时：降分辨率/批次 → 加 `--lowvram` 或 `--medvram` → `--bf16-vae`。
② 根治：按 `vram-playbook.md` 的分层方案（量化 GGUF、VAE 分块、swap、CPU fallback）。
**验证**：同一工作流能跑完，`nvidia-smi` 峰值显存有富余。

### D2. Windows 报 `Page file too small` / `OSError: [WinError 1455]`
**根因**：系统分页文件不足。
① 临时：关闭其它占内存的程序。
② 根治：增大虚拟内存（高级系统设置 → 性能 → 虚拟内存 → 自定义 32GB+），或加物理内存。
**验证**：不再出现 1455。

### D3. 出图极慢 / 每步好几秒
**根因**：模型被反复在内存与显存间搬运（显存不足触发），或跑到了 CPU。
① 临时：`nvidia-smi` 看 GPU 利用率是否接近 0（接近 0 = 在 CPU 跑）。
② 根治：确认 torch 是 CUDA 版（A1）；显存够就去掉 `--lowvram`；把模型放到 SSD。
**验证**：`nvidia-smi` 出图时 GPU 利用率显著上升。

### D4. 出黑图 / NaN / 花屏
**根因**：半精度溢出或 attention 精度问题。
① 临时：加 `--force-upcast-attention`。
② 根治：VAE 改 fp32（`--fp32-vae`），或换用官方推荐的 VAE 文件单独加载。
**验证**：同一 seed 出图正常。

---

## E. 网络 / 访问类

### E1. `Address already in use` / 端口占用
① 临时：`python main.py --port 8189`。
② 根治：找出并结束占用进程。
```powershell
# Windows
netstat -ano | findstr :8188
taskkill /PID <PID> /F
```
```bash
# Linux / WSL2
sudo lsof -i :8188
sudo kill -9 <PID>
```
**验证**：`python main.py` 正常绑定 8188。

### E2. `http://127.0.0.1:8188` 打不开
**根因**：服务没起来 / 绑错地址 / 防火墙。
① 临时：看终端最后几行是否有 `Starting server`，有则换 `localhost` 或加 `--listen 0.0.0.0`。
② 根治：WSL2 必须用 `--listen 0.0.0.0`；远程访问检查防火墙入站规则；确认没有被代理软件劫持（浏览器关掉系统代理再试）。
**验证**：`curl http://127.0.0.1:8188` 返回 HTML。

### E3. ffmpeg 相关报错
见 `install-playbooks.md` 第 7 节。

### E4. 图片找不到 / 输出目录异常
**根因**：输出默认在 `<ComfyUI目录>/output`。
① 临时：ComfyUI 右键节点 → "Open Image" 或看网页 History。
② 根治：用 `--output-directory <路径>` 显式指定；确认该目录有写权限。
**验证**：出图后目录下出现新文件。

---

## F. 工作流类

见 `workflow-json.md`，典型关键字：`Invalid node type`、`Node not found`、`Prompt outputs failed validation`、`JSON parse error`。
