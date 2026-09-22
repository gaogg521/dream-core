#!/usr/bin/env bash
# 重建 supertonic-tts skill 的 Python 3.10 venv
# 用法：在 skill 根目录执行 ./scripts/setup.sh
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PY310="${PY310:-/opt/homebrew/bin/python3.10}"

if [[ ! -x "$PY310" ]]; then
  echo "未找到 $PY310，请先：brew install python@3.10" >&2
  exit 1
fi

"$PY310" -m venv "$SKILL_DIR/venv"
"$SKILL_DIR/venv/bin/pip" install --quiet --upgrade pip
"$SKILL_DIR/venv/bin/pip" install --quiet supertonic-mnn

echo "OK -> $SKILL_DIR/venv/bin/supertonic-mnn"