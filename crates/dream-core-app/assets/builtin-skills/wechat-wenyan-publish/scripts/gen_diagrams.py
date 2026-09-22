#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""SVG → PNG 配图工具（白底、中文标签、统一配色，微信正文只收 PNG/JPG）。

用法：
  python3 gen_diagrams.py a.svg b.svg ... [--scale 2] [--outdir assets]

规范（见 references/cover-and-image-style.md）：
  - 画布 <svg viewBox="0 0 780 H">，白色背景 + 1px 边框圆角
  - 中文标签 → SVG 内 font-family="'Noto Sans SC','PingFang SC','Microsoft YaHei',sans-serif"
  - 统一用 #10a37f 绿为主色、#3b82f6 蓝 / #8b5cf6 紫做区分、#e2e8f0 描边
  - 需要 cairosvg（pip install cairosvg）
"""
import argparse
import cairosvg
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("svgs", nargs="+", type=Path)
    ap.add_argument("--scale", type=int, default=2)
    ap.add_argument("--outdir", type=Path, default=Path("assets"))
    args = ap.parse_args()
    args.outdir.mkdir(parents=True, exist_ok=True)
    for s in args.svgs:
        out = args.outdir / (s.stem + ".png")
        cairosvg.svg2png(url=str(s), write_to=str(out), scale=args.scale)
        print("ok ->", out)


if __name__ == "__main__":
    main()
