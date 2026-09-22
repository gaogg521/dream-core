#!/usr/bin/env bash
# 一次性安装 expense-merger 所需的 Python 环境。幂等：已存在直接退出。
set -e

VENV="$HOME/.cache/expense-merger-venv"

if [ -f "$VENV/bin/python3" ]; then
    # 已安装：快速验证关键包还在
    if "$VENV/bin/python3" -c "import easyofd, pypdf, reportlab" 2>/dev/null; then
        echo "[expense-merger] venv 已就绪：$VENV"
        exit 0
    fi
fi

echo "[expense-merger] 创建 venv 于 $VENV"
mkdir -p "$(dirname "$VENV")"
python3 -m venv "$VENV"

echo "[expense-merger] 安装 easyofd / pypdf / reportlab"
"$VENV/bin/pip" install --quiet --upgrade pip
"$VENV/bin/pip" install --quiet easyofd pypdf reportlab

echo "[expense-merger] 完成"
