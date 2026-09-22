#!/usr/bin/env bash
# supertonic-tts skill wrapper
# 用法：
#   speak.sh "Hello world."                       -> /tmp/supertonic_out.wav, M1, int8
#   speak.sh "Hello world." /tmp/out.wav F1 int8
#   cat long.txt | speak.sh -                     -> stdin
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON_BIN="${SKILL_DIR}/venv/bin/supertonic-mnn"
CACHE_DIR="${HOME}/.cache/supertonic-mnn"
mkdir -p "$CACHE_DIR"

# 默认走镜像，避免 huggingface.co 502
export HF_ENDPOINT="${HF_ENDPOINT:-https://hf-mirror.com}"

OUT="${2:-/tmp/supertonic_out.wav}"
VOICE="${3:-M1}"
PRECISION="${4:-int8}"

if [[ "${1:-}" == "-" ]]; then
  exec "$PYTHON_BIN" -o "$OUT" --voice "$VOICE" --precision "$PRECISION"
fi

TEXT="${1:-}"
if [[ -z "$TEXT" ]]; then
  echo "usage: speak.sh <text|-> [output.wav] [voice M1-M5/F1-F5] [precision fp32|fp16|int8]" >&2
  exit 2
fi

printf '%s\n' "$TEXT" | "$PYTHON_BIN" -o "$OUT" --voice "$VOICE" --precision "$PRECISION"