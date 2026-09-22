#!/usr/bin/env bash
# ============================================================
#  ComfyUI 启动脚本（Linux / WSL2-Ubuntu）
#  使用前修改：
#    1) COMFYUI_DIR 改成你的 ComfyUI 目录
#    2) VRAM_MODE 按显存选一个：low / med / normal / high
#    3) WSL2 务必保留 --listen 0.0.0.0，否则 Windows 浏览器可能连不上
#  用法：chmod +x run-comfyui-linux.sh && ./run-comfyui-linux.sh
# ============================================================
set -euo pipefail

COMFYUI_DIR="$HOME/ComfyUI"
VRAM_MODE="med"
PORT=8188

cd "$COMFYUI_DIR" || { echo "[ERROR] 目录不存在: $COMFYUI_DIR"; exit 1; }

# 激活虚拟环境
# shellcheck disable=SC1091
source "$COMFYUI_DIR/venv/bin/activate"

case "$VRAM_MODE" in
  low)    VRAM_FLAG="--lowvram" ;;
  med)    VRAM_FLAG="--medvram" ;;
  normal) VRAM_FLAG="--normalvram" ;;
  high)   VRAM_FLAG="--highvram" ;;
  *)      VRAM_FLAG="" ;;
esac

echo "[INFO] ComfyUI 目录 : $COMFYUI_DIR"
echo "[INFO] 显存模式     : $VRAM_MODE"
echo "[INFO] 端口         : $PORT"
echo

python main.py --listen 0.0.0.0 --port "$PORT" $VRAM_FLAG --bf16-vae
