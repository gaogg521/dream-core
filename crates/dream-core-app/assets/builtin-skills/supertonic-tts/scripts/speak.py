#!/usr/bin/env python3
"""Supertonic-mnn Python wrapper.

用法:
    from supertonic_tts import speak
    speak("Hello world.", voice="F1", precision="int8", output="out.wav")
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

# 强制走镜像（国内网络 huggingface.co 会 502）
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")

SKILL_DIR = Path(__file__).resolve().parent.parent

# Windows: no bundled venv — rely on pip-installed supertonic_mnn on sys.path.
# macOS/Linux: prepend the bundled venv's site-packages.
import sys as _sys

if _sys.platform == "win32":
    try:
        import supertonic_mnn  # noqa: F401
    except ImportError:
        _sys.exit("supertonic-mnn not installed. Run: pip install supertonic-mnn "
                  "(see SKILL.md 'Windows 注意' for the voice_styles workaround).")
else:
    _sys.path.insert(0, str(SKILL_DIR / "venv" / "lib" / "python3.10" / "site-packages"))

from supertonic_mnn import SupertonicTTS  # noqa: E402


def speak(
    text: str,
    voice: str = "M1",
    precision: str = "int8",
    output: str | os.PathLike = "/tmp/supertonic_out.wav",
    speed: float = 1.0,
    steps: int = 5,
) -> tuple[bytes, int]:
    """Synthesize text to a wav file. Returns (audio_bytes, sample_rate)."""
    tts = SupertonicTTS(precision=precision)
    audio, sr = tts.synthesize(
        text,
        voice=voice,
        speed=speed,
        steps=steps,
        output_file=str(output),
    )
    return audio, sr


def main() -> None:
    import argparse

    p = argparse.ArgumentParser(description="Supertonic-mnn TTS wrapper")
    p.add_argument("text", help="要合成的文本；传 - 表示从 stdin 读")
    p.add_argument("-o", "--output", default="/tmp/supertonic_out.wav")
    p.add_argument("-v", "--voice", default="M1", help="M1-M5/F1-F5")
    p.add_argument("-p", "--precision", default="int8", choices=["fp32", "fp16", "int8"])
    p.add_argument("--speed", type=float, default=1.0)
    p.add_argument("--steps", type=int, default=5)
    args = p.parse_args()

    if args.text == "-":
        text = sys.stdin.read().strip()
    else:
        text = args.text

    speak(
        text,
        voice=args.voice,
        precision=args.precision,
        output=args.output,
        speed=args.speed,
        steps=args.steps,
    )
    print(f"saved -> {args.output}")


if __name__ == "__main__":
    main()