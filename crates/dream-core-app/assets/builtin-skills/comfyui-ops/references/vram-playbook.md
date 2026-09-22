# 显存溢出（CUDA OOM）分层处置

原则：**从代价最小到最大逐层加码**，每加一层就跑一次原工作流验证，不要一口气全开。

---

## 第 0 层：先量化现状

```bash
nvidia-smi                      # 看总显存、已用、是否有别的进程占着
nvidia-smi --query-gpu=memory.total,memory.used --format=csv
python -c "import torch;print(torch.cuda.get_device_name(0), torch.cuda.get_device_properties(0).total_memory/1024**3)"
```

**先把别人占了显存的进程关掉**（浏览器硬件加速、其它 AI 工具、游戏）。这一步经常直接解决问题。

---

## 第 1 层：降低单次计算量（代价最小）

| 动作 | 显存收益 |
|---|---|
| 分辨率 1024×1024 → 768×768 | 显著（显存与像素数近似线性） |
| batch size 4 → 1 | 显著 |
| 关掉多余预览 / 一次只跑一张 | 中等 |
| 减少同时加载的 LoRA 数量 | 中等 |

## 第 2 层：启动参数（按显存档位选）

| 显存 | 推荐参数 |
|---|---|
| ≤ 4 GB | `--lowvram --fp16-vae`（基本只能跑 SD1.5） |
| 6 GB | `--lowvram --bf16-vae` |
| 8 GB | `--medvram --bf16-vae`（SDXL 可跑，FLUX 吃力） |
| 12 GB | 默认 `--normalvram --bf16-vae` |
| ≥ 24 GB | `--highvram`（模型常驻，速度最快） |

```bash
python main.py --medvram --bf16-vae
```

## 第 3 层：VAE 阶段单独优化

- **VAE 半精度**：`--bf16-vae`（30 系及以上）或 `--fp16-vae`（老卡）。
- **VAE 分块解码**：把工作流里的 `VAE Decode` 换成 `VAE Decode (Tiled)`，或降低 tile_size。这一步对"模型能加载、但解码时 OOM"特别有效。
- 单独加载官方 VAE 文件（放 `models/vae/`），不要依赖 checkpoint 内置 VAE。

## 第 4 层：模型加载策略

| 策略 | 说明 |
|---|---|
| 用 fp16 / bf16 权重 | 别下 fp32 版本 |
| GGUF 量化 | 装 `https://github.com/city96/ComfyUI-GGUF`，把量化模型放 `models/unet/`，Q5_K_M 通常是性价比拐点 |
| 减少常驻模型 | 不要同时加载多个 checkpoint |
| 拆分加载 | 用独立的 CLIP / UNET 加载器，按需加载 |

## 第 5 层：系统层 swap / 分页文件

**Windows**：高级系统设置 → 性能 → 虚拟内存 → 自定义大小，初始/最大都设 32768 MB 以上，重启生效。
报错 `OSError: [WinError 1455] 页面文件太小` 必做这一步。

**Linux / WSL2**：
```bash
sudo fallocate -l 32G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile
# 永久生效：在 /etc/fstab 追加
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
free -h
```
WSL2 还需在 `C:\Users\<用户名>\.wslconfig` 设置：
```
[wsl2]
memory=32GB
swap=32GB
```

## 第 6 层：CPU fallback（最后兜底）

```bash
python main.py --cpu
```
极慢（SD1.5 一张可能数分钟），仅用于验证工作流本身是否正确。需要时配合 `--cpu-vae`。

---

## 快速决策表

| 症状 | 直接跳到 |
|---|---|
| `nvidia-smi` 显示显存已被别的进程占满 | 第 0 层 |
| 加载模型时就 OOM | 第 2 层 + 第 4 层 |
| 采样中途 OOM | 第 1 层 + 第 2 层 |
| VAE 解码时 OOM | 第 3 层（Tiled VAE） |
| Windows 报 `[WinError 1455]` | 第 5 层 |
| 没有 N 卡 / 驱动装不上 | 第 6 层 |

## 验证方式

每加一层后跑同一工作流，用下面命令观察峰值：
```bash
nvidia-smi --query-gpu=memory.used --format=csv -l 1
```
峰值留出 10-20% 余量才算稳定，否则下一轮还会 OOM。
