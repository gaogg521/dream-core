#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
ComfyUI 环境一键体检脚本（纯标准库，无第三方依赖）

用法：
    python diagnose.py                     # 扫描当前目录
    python diagnose.py --comfyui-dir D:\ComfyUI
    python diagnose.py --comfyui-dir /home/user/ComfyUI

输出：系统信息 / Python / torch+CUDA / 显卡 / ComfyUI 版本 / 自定义节点 / 模型统计
可安全只读运行，不做任何修改。
"""

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

MODEL_DIRS = {
    "checkpoints": "Checkpoint(大模型)",
    "loras": "LoRA",
    "vae": "VAE",
    "controlnet": "ControlNet",
    "embeddings": "Embedding",
    "upscale_models": "放大模型",
    "clip": "CLIP",
    "unet": "UNET(含GGUF)",
    "clip_vision": "CLIP Vision",
    "style_models": "Style Model",
}

MODEL_EXT = {".safetensors", ".ckpt", ".pt", ".pth", ".bin", ".gguf"}
SKIP_DIR_NAMES = {".git", "__pycache__", ".ipynb_checkpoints"}


def run(cmd, timeout=25):
    """执行命令，返回 (ok, stdout+stderr)。失败不抛异常。"""
    try:
        p = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout,
            errors="replace", shell=False,
        )
        out = (p.stdout or "") + (p.stderr or "")
        return p.returncode == 0, out.strip()
    except FileNotFoundError:
        return False, "command not found"
    except subprocess.TimeoutExpired:
        return False, "timeout"
    except Exception as e:  # noqa: BLE001
        return False, f"error: {e}"


def line(title):
    print(f"\n{'=' * 60}\n{title}\n{'=' * 60}")


def guess_comfyui_dir(start: Path) -> Path:
    """从起始目录向上找包含 main.py + models 的目录。"""
    for p in [start] + list(start.parents):
        if (p / "main.py").exists() and (p / "models").exists():
            return p
    return start


def section_system():
    line("1. 系统与显卡")
    print(f"操作系统      : {platform.system()} {platform.release()} ({platform.version()})")
    print(f"架构          : {platform.machine()}")
    in_wsl = "microsoft" in platform.release().lower() or os.environ.get("WSL_DISTRO_NAME")
    print(f"WSL2          : {'是' if in_wsl else '否'}")

    ok, out = run(["nvidia-smi", "--query-gpu=name,memory.total,memory.used,driver_version",
                   "--format=csv,noheader"])
    if ok:
        print(f"nvidia-smi    : {out}")
    else:
        print("nvidia-smi    : 不可用（未装 NVIDIA 驱动 / 不在 PATH / 无 N 卡）")


def section_python():
    line("2. Python 环境")
    print(f"解释器        : {sys.executable}")
    print(f"版本          : {platform.python_version()}")
    print(f"虚拟环境      : {'是 (' + sys.prefix + ')' if sys.prefix != sys.base_prefix else '否（系统 Python，建议用 venv）'}")
    print(f"pip           : {shutil.which('pip') or shutil.which('pip3') or '未找到'}")


def section_torch():
    line("3. PyTorch / CUDA")
    code = (
        "import torch;"
        "print('torch版本', torch.__version__);"
        "print('编译CUDA', torch.version.cuda);"
        "print('可用', torch.cuda.is_available());"
        "print('设备数', torch.cuda.device_count());"
        "print('设备名', torch.cuda.get_device_name(0) if torch.cuda.is_available() else 'N/A')"
    )
    ok, out = run([sys.executable, "-c", code], timeout=90)
    if ok and "torch版本" in out:
        for ln in out.splitlines():
            print("  " + ln)
        if "可用 False" in out or "可用 False" in out.replace("可用 ", "可用 "):
            print("\n  ⚠ torch 不是 CUDA 版本，或驱动/CUDA 不匹配。")
            print("    修复：pip uninstall -y torch torchvision torchaudio")
            print("          pip install torch torchvision torchaudio "
                  "--extra-index-url https://download.pytorch.org/whl/cu126")
    else:
        print("  torch 未安装或导入失败：")
        print("   " + (out[:500] if out else "无输出"))


def section_comfyui(cdir: Path):
    line("4. ComfyUI 本体")
    print(f"目录          : {cdir}")
    print(f"目录存在      : {cdir.exists()}")
    if not cdir.exists():
        return

    print(f"main.py       : {'有' if (cdir / 'main.py').exists() else '无 ⚠ 不是 ComfyUI 根目录'}")
    print(f"requirements  : {'有' if (cdir / 'requirements.txt').exists() else '无 ⚠'}")
    print(f"venv          : {'有' if (cdir / 'venv').exists() else '无'}")

    if (cdir / ".git").exists():
        ok, out = run(["git", "-C", str(cdir), "log", "-1", "--format=%H %ci %s"])
        print(f"git 版本      : {out if ok else 'git 不可用'}")
        ok2, out2 = run(["git", "-C", str(cdir), "status", "-sb"])
        if ok2:
            first = out2.splitlines()[0] if out2 else ""
            dirty = any(l.strip().startswith(("M ", "?? ", " M", "UU")) for l in out2.splitlines()[1:])
            print(f"git 状态      : {first}{'（有本地改动，git pull 可能冲突）' if dirty else ''}")
    else:
        print("git 版本      : 非 git 仓库（整合包常见），无法 git pull 更新")


def section_nodes(cdir: Path):
    line("5. 自定义节点")
    cn = cdir / "custom_nodes"
    if not cn.exists():
        print("custom_nodes 目录不存在")
        return
    items = sorted([d for d in cn.iterdir() if d.is_dir() and d.name not in SKIP_DIR_NAMES])
    print(f"数量          : {len(items)}")
    for d in items:
        is_git = (d / ".git").exists()
        req = (d / "requirements.txt").exists()
        flags = []
        if is_git:
            flags.append("git")
        if req:
            flags.append("有requirements")
        print(f"  - {d.name} [{'/'.join(flags) or '无git无requirements'}]")
    print("\n提示：启动日志里出现 'IMPORT FAILED: <目录名>' 即为该节点加载失败。")


def section_models(cdir: Path):
    line("6. 模型统计")
    models = cdir / "models"
    if not models.exists():
        print("models 目录不存在")
        return
    total = 0
    for sub, label in MODEL_DIRS.items():
        d = models / sub
        if not d.exists():
            continue
        files = [f for f in d.rglob("*") if f.is_file() and f.suffix.lower() in MODEL_EXT]
        if files:
            size = sum(f.stat().st_size for f in files) / 1024 ** 3
            print(f"  {label:<18} {len(files):>3} 个  {size:>7.2f} GB   -> models/{sub}/")
            total += len(files)
    print(f"\n合计          : {total} 个模型文件")

    emp = (cdir / "extra_model_paths.yaml").exists()
    print(f"extra_model_paths.yaml : {'已配置（共享外部模型）' if emp else '未配置'}")
    if not emp and (cdir / "extra_model_paths.yaml.example").exists():
        print("  （存在 .example 模板，复制改名即可启用）")


def section_disk(cdir: Path):
    line("7. 磁盘空间")
    try:
        usage = shutil.disk_usage(str(cdir))
        print(f"总容量        : {usage.total / 1024 ** 3:.1f} GB")
        print(f"已用          : {usage.used / 1024 ** 3:.1f} GB")
        print(f"可用          : {usage.free / 1024 ** 3:.1f} GB")
        if usage.free / 1024 ** 3 < 20:
            print("  ⚠ 可用空间不足 20GB，下载大模型前请先扩容")
    except Exception as e:  # noqa: BLE001
        print(f"无法读取：{e}")


def main():
    ap = argparse.ArgumentParser(description="ComfyUI 环境体检（只读）")
    ap.add_argument("--comfyui-dir", default=".", help="ComfyUI 根目录，默认当前目录")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出")
    args = ap.parse_args()

    start = Path(args.comfyui_dir).expanduser().resolve()
    cdir = guess_comfyui_dir(start) if not (start / "main.py").exists() else start

    if args.json:
        data = {"comfyui_dir": str(cdir), "os": platform.platform(),
                "python": platform.python_version(), "venv": sys.prefix != sys.base_prefix}
        print(json.dumps(data, ensure_ascii=False, indent=2))
        return

    print("ComfyUI 环境体检报告（只读，不修改任何文件）")
    section_system()
    section_python()
    section_torch()
    section_comfyui(cdir)
    section_nodes(cdir)
    section_models(cdir)
    section_disk(cdir)

    line("下一步")
    print("把以上输出整段发给运维助手，即可直接定位问题。")
    print("若 torch 可用=False：优先修复 CUDA 版 torch。")
    print("若某个节点 IMPORT FAILED：只针对该节点目录 git pull + 装 requirements。")


if __name__ == "__main__":
    main()
