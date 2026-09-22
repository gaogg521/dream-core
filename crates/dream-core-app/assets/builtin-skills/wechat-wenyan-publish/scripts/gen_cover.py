#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""编辑风封面生成器：白底 #f7f7f8 + 绿强调 #10a37f，900×383，左文右图。

规则（重要）：
  - 封面一律不加作者署名（不放"作者名·"式水印，保持版面干净）
  - 主标题用 Noto Sans CJK Black，副标题 Bold，绿色小字眉题
  - 右侧图示默认是"1 个中枢 → 3 个能力块"的 hub 模板；也可用 --right-svg 指定自定义 SVG
  - 需要 PIL + cairosvg（pip install Pillow cairosvg）

用法：
  python3 gen_cover.py --out cover.png \
      --eyebrow "栏目 · 日期" \
      --title "主标题" --subtitle "副标题" \
      --pills "卖点1,卖点2,卖点3" \
      [--right-svg assets/_right.svg]
"""
import argparse
import tempfile
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw, ImageFont, ImageFilter

GREEN = (16, 163, 127)
GREEN_D = (13, 130, 102)
DARK = (23, 23, 23)
GRAY = (113, 113, 122)
BORDER = (229, 229, 229)

_FONT_DIRS = [
    "/usr/share/fonts/opentype/noto",
    "/usr/share/fonts/truetype/noto",
    str(Path.home() / "AppData/Local/Microsoft/Windows/Fonts"),
    "C:/Windows/Fonts",
    "/System/Library/Fonts",
    str(Path.home() / "Library/Fonts"),
]
_FONT_CANDIDATES = {
    "black": ["NotoSansCJK-Black.ttc", "NotoSansSC-Black.otf", "NotoSansSCblack.ttf",
              "msyhbd.ttc", "PingFang SC.ttc", "Hiragino Sans GB.ttc"],
    "bold": ["NotoSansCJK-Bold.ttc", "NotoSansSC-Bold.otf", "NotoSansSCbold.ttf",
             "msyhbd.ttc", "PingFang SC.ttc", "Hiragino Sans GB.ttc", "msyh.ttc"],
}


def _find_font(kind: str) -> str:
    """按候选名在各常见字体目录查找；找不到则报错并提示安装 fonts-noto-cjk。"""
    for d in _FONT_DIRS:
        for name in _FONT_CANDIDATES[kind]:
            p = Path(d) / name
            if p.is_file():
                return str(p)
    raise SystemExit(
        f"未找到 CJK 字体（{kind}）。请安装 Noto Sans CJK（Debian/Ubuntu: apt install fonts-noto-cjk）"
        "或设置 _FONT_DIRS 指向含中文字体的目录。"
    )


F_BLACK = _find_font("black")
F_BOLD = _find_font("bold")

DEFAULT_RIGHT_SVG = """<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 360 300" font-family="'Noto Sans SC','PingFang SC',sans-serif">
  <defs><marker id="ar" markerWidth="10" markerHeight="7" refX="9" refY="3.5" orient="auto">
  <polygon points="0 0, 10 3.5, 0 7" fill="#10a37f"/></marker></defs>
  <rect x="128" y="96" width="104" height="58" rx="14" fill="#10a37f"/>
  <text x="180" y="120" text-anchor="middle" font-size="15" font-weight="700" fill="#ffffff">主标题</text>
  <text x="180" y="140" text-anchor="middle" font-size="10" fill="#d1fae5">副标题</text>
  <line x1="180" y1="154" x2="180" y2="176" stroke="#10a37f" stroke-width="2.5" marker-end="url(#ar)"/>
  <line x1="180" y1="154" x2="82" y2="176" stroke="#10a37f" stroke-width="2.5" marker-end="url(#ar)"/>
  <line x1="180" y1="154" x2="278" y2="176" stroke="#10a37f" stroke-width="2.5" marker-end="url(#ar)"/>
  <rect x="52" y="180" width="116" height="30" rx="7" fill="#ffffff" stroke="#10a37f" stroke-width="1.5"/>
  <text x="110" y="199" text-anchor="middle" font-size="12" font-weight="700" fill="#111827">能力一</text>
  <rect x="122" y="180" width="116" height="30" rx="7" fill="#ffffff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="180" y="199" text-anchor="middle" font-size="12" font-weight="700" fill="#111827">能力二</text>
  <rect x="192" y="180" width="116" height="30" rx="7" fill="#ffffff" stroke="#8b5cf6" stroke-width="1.5"/>
  <text x="250" y="199" text-anchor="middle" font-size="12" font-weight="700" fill="#111827">能力三</text>
  <text x="180" y="240" text-anchor="middle" font-size="10.5" fill="#6b7280">一句话说明</text>
</svg>"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--eyebrow", default="AI · 生态观察")
    ap.add_argument("--title", required=True)
    ap.add_argument("--subtitle", default="")
    ap.add_argument("--pills", default="")
    ap.add_argument("--right-svg")
    ap.add_argument("--title-size", type=int, default=38)
    args = ap.parse_args()

    W, H = 900, 383
    img = Image.new("RGB", (W, H))
    dr = ImageDraw.Draw(img)
    c1, c2 = (250, 250, 249), (240, 253, 250)
    for y in range(H):
        t = y / (H - 1)
        dr.line([(0, y), (W, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(c1, c2)))

    ov = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    od = ImageDraw.Draw(ov)
    od.ellipse([620, -60, 980, 200], fill=(16, 163, 127, 26))
    od.ellipse([-40, 250, 300, 430], fill=(16, 163, 127, 18))
    ov = ov.filter(ImageFilter.GaussianBlur(28))
    img = Image.alpha_composite(img.convert("RGBA"), ov).convert("RGB")
    dr = ImageDraw.Draw(img)

    def font(p, s): return ImageFont.truetype(p, s)

    # 左栏
    dr.rectangle([48, 84, 54, 244], fill=GREEN)
    dr.text((74, 92), args.eyebrow, font=font(F_BOLD, 16), fill=GREEN)
    dr.text((74, 126), args.title, font=font(F_BLACK, args.title_size), fill=DARK)
    if args.subtitle:
        dr.text((74, 184), args.subtitle, font=font(F_BOLD, 20), fill=(55, 65, 81, 255))
    x = 74
    for text in [p for p in args.pills.split(",") if p]:
        f = font(F_BOLD, 13)
        w = int(dr.textlength(text, font=f)) + 30
        dr.rounded_rectangle([x, 226, x + w, 254], radius=14, outline=GREEN, width=1, fill=(240, 253, 250, 255))
        dr.text((x + 15, 230), text, font=f, fill=GREEN_D)
        x += w + 12

    # 分隔线
    dr.line([(492, 30), (492, 353)], fill=BORDER, width=1)

    # 右栏
    svg_src = args.right_svg
    if svg_src and not svg_src.endswith(".svg"):
        svg_src = None
    if not svg_src:
        svg_src = DEFAULT_RIGHT_SVG
    elif svg_src.endswith(".svg"):
        svg_src = open(svg_src, encoding="utf-8").read()
    tmp = str(Path(tempfile.gettempdir()) / "_cover_right.png")
    cairosvg.svg2png(bytestring=svg_src.encode(), write_to=tmp, scale=2)
    right = Image.open(tmp).convert("RGBA")
    tw = 372
    right = right.resize((tw, int(right.height * tw / right.width)), Image.LANCZOS)
    mask = Image.new("L", right.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, right.width, right.height], radius=16, fill=255)
    img.paste(right, (508, 16), mask)

    img.save(args.out)
    print("cover ok ->", args.out, img.size)


if __name__ == "__main__":
    main()
