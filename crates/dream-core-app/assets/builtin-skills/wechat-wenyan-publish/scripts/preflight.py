#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""发布前自检：frontmatter + 微信 API 硬约束 + 图片引用/文件校验。

用法：python3 preflight.py article.md [--min-images 2]
规则（微信 API）：
  - title ≤ 64 字节（中文字符≈3B）
  - summary(description) ≤ 120 字节
  - author ≤ 8 字节（两个汉字=6B）
  - 封面必须在 frontmatter 的 cover 字段且文件存在
  - 正文图必须 PNG/JPG（SVG 会被拒），引用路径必须对应真实文件
"""
import argparse
import re
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("md", type=Path)
    ap.add_argument("--min-images", type=int, default=2)
    args = ap.parse_args()
    md = args.md.resolve()
    text = md.read_text(encoding="utf-8")

    problems, notes = [], []
    m = re.match(r"^---\s*\n(.*?)\n---", text, re.DOTALL)
    if not m:
        problems.append("缺少 frontmatter")
        return
    meta = {}
    for line in m.group(1).splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            meta[k.strip().lower()] = v.strip().strip('"').strip("'")

    def chk(key, limit, name):
        v = meta.get(key, "")
        b = len(v.encode("utf-8"))
        status = "OK" if b <= limit else "超限!"
        (problems if b > limit else notes).append(f"{name}: {b}B/{limit}B {status} | {v[:24]}")
        return b

    chk("title", 64, "title")
    chk("description", 120, "summary(description)")
    chk("author", 8, "author")

    cover = meta.get("cover", "")
    cover_ok = cover and Path(cover).is_file()
    (problems if not cover_ok else notes).append(
        f"cover: {'OK' if cover_ok else '缺失或文件不存在'} | {cover}")

    body_imgs = re.findall(r"!\[[^\]]*\]\(([^)]+)\)", text)
    missing = [p for p in body_imgs if not (md.parent / p).is_file()]
    img_types = {p.lower().split(".")[-1] for p in body_imgs}
    bad_type = img_types - {"png", "jpg", "jpeg", "webp", "gif"}
    notes.append(f"正文图: {len(body_imgs)} 张 (引用 {body_imgs})")
    if len(body_imgs) < args.min_images:
        problems.append(f"正文图不足 {args.min_images} 张（建议至少 2-3 张，否则画面单薄）")
    if missing:
        problems.append(f"图片文件缺失: {missing}")
    if bad_type:
        problems.append(f"不支持的图片类型（SVG 会被微信拒）: {bad_type}")
    for p in body_imgs:
        if p.lower().endswith(".svg"):
            problems.append("正文含 SVG 图片，微信不接受，请转 PNG: " + p)

    print("\n".join(notes))
    if problems:
        print("\n❌ 需修复：")
        print("\n".join("  - " + p for p in problems))
        return 1
    print("✅ 全部通过，可以发布")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
