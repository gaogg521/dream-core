#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""小红书配图（3 图制，参照 qclaw_clone make_xhs_feiping 少而精）。
通用 canonical 版（克隆 8/5 精修版，去写死日期/主题/配色/数据）。
用法：
  python3 build_xhs_cards.py --date 20260809 --theme "主题串" --deep "#9E2B25" --accent "#E8A33D" \
      --card1_title "30分钟出局！" --card1_sub "迟到30分取消资格·续聘不考就出库" \
      --card2_title "三省出库口子自查" \
      --table '["省份","关键规则","状态"],[["江苏","届满前6个月申请...","已施行"],...]' \
      --card3_title "今天就做三件事" \
      --card3_items '[{"h":"① 查聘期，设提醒","b":"..."},...]' \
      --reward "报酬：300元起 · 节假日双倍" --foot "淘汰迟到、不续聘、踩红线的专家"
产物落 <SHARED>/<date>/，文件名 <date>-小红书配图N-...-<theme>.png
"""
import argparse, os, json
from PIL import Image, ImageDraw, ImageFont

import os
SHARED = os.environ.get("BID_SHARED_DIR", os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# 注：原系统硬编码 /home/laoty/.openclaw/workspace-bid-shared，已参数化为 BID_SHARED_DIR（未设置时回退到本脚本所在目录）
FONT_HEAVY = os.path.join(SHARED, "fonts/LXGWWenKai.ttf")
FONT_REG = os.path.join(SHARED, "fonts/LXGWWenKai.ttf")
for _f in (FONT_HEAVY, FONT_REG):
    if not os.path.exists(_f):
        _f = "/tmp/LXGWWenKai.ttf"
if not os.path.exists(FONT_HEAVY):
    FONT_HEAVY = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"
if not os.path.exists(FONT_REG):
    FONT_REG = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"


def hex2rgb(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))


def font(p, s):
    return ImageFont.truetype(p, s)


def new_canvas(deep):
    im = Image.new("RGB", (1080, 1440), deep)
    return im, ImageDraw.Draw(im)


def textw(d, s, f):
    b = d.textbbox((0, 0), s, font=f)
    return b[2] - b[0]


def wrap(d, text, f, maxw):
    lines = []
    cur = ""
    for ch in text:
        if textw(d, cur + ch, f) <= maxw:
            cur += ch
        else:
            lines.append(cur)
            cur = ch
    if cur:
        lines.append(cur)
    return lines


def fit_font(d, text, path, size, maxw, minsz=14):
    """2026-08-11 新增：单行自适应缩字号，防止长文案溢出容器/画布。"""
    f = font(path, size)
    if not text or maxw <= 0:
        return f
    while size > minsz and textw(d, text, f) > maxw:
        size -= 2
        f = font(path, size)
    return f


def ctext_fit(d, text, path, size, cx, cy, fill, maxw, minsz=14):
    """居中绘制 + 自适应缩字号。"""
    f = fit_font(d, text, path, size, maxw, minsz)
    d.text((cx, cy), text, font=f, fill=fill, anchor="mm")
    return f


def ctext_wrap(d, text, path, size, cx, cy, fill, maxw, lead=8, max_lines=2, minsz=14):
    """居中多行：先尝试换行，行数超限则缩字号。返回实际底部 y。"""
    f = font(path, size)
    lines = wrap(d, text, f, maxw)
    while len(lines) > max_lines and size > minsz:
        size -= 2
        f = font(path, size)
        lines = wrap(d, text, f, maxw)
    total = len(lines) * size + (len(lines) - 1) * lead
    ty = cy - total / 2
    for ln in lines:
        d.text((cx, ty + size / 2), ln, font=f, fill=fill, anchor="mm")
        ty += size + lead
    return ty


def ctext(d, t, f, cx, cy, fill, anchor="mm"):
    w = textw(d, t, f)
    d.text((cx - w / 2, cy), t, font=f, fill=fill, anchor="la")


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--date", required=True)
    p.add_argument("--theme", required=True)
    p.add_argument("--deep", default="#9E2B25")
    p.add_argument("--accent", default="#E8A33D")
    p.add_argument("--top", default="招投标专家 · 8月新规")
    p.add_argument("--card1_title", default="30分钟出局！")
    p.add_argument("--card1_sub", default="迟到30分取消资格·续聘不考就出库")
    p.add_argument("--card1_blocks", default="迟到30分+就出局,不续聘+就出库", help="首图两个痛点块，+隔词/英文逗号隔块")
    p.add_argument("--card1_cta", default="↓ 评论区扣「1」领出库自查表")
    p.add_argument("--card2_title", default="三省出库口子自查")
    p.add_argument("--table", default="[]", help='JSON: [headers, ...rows]')
    p.add_argument("--table_hl", default="[]", help="JSON list of [row,col] 高亮格")
    p.add_argument("--card2_note", default="")
    p.add_argument("--card2_bottom", default="大部分人出库，不是能力不行，是没看日历")
    p.add_argument("--card3_title", default="今天就做三件事")
    p.add_argument("--card3_items", default="[]", help='JSON list of {"h","b"}')
    p.add_argument("--reward", default="报酬：300元起 · 节假日双倍 · 异地就高发放")
    p.add_argument("--foot", default="淘汰迟到、不续聘、踩红线的专家")
    p.add_argument("--card1_mid", default="你的出库红线，碰了吗？")
    return p.parse_args()


def table_draw(d, x, y, col_w, row_h, headers, rows, fs=30, hl=None, deep=None, accent=None, light=None, line=None, max_bottom=None):
    """自适应表格：行高按换行后实际内容计算，可选 max_bottom 自动缩放字号以放进可用高度。
    修复（2026-08-10）：①行高不再固定，杜绝长单元格串行/溢出 ②斑马底色先填后描边，
    不再把边框覆盖掉 ③支持任意列数。"""
    pad_x, pad_y, lead = 12, 14, 6

    def layout(size):
        f_ = font(FONT_REG, size)
        fh_ = font(FONT_HEAVY, size)
        hh = max(row_h_min(size), _cell_h(d, headers, col_w, f_, size, pad_y, lead))
        rh = [max(row_h_min(size), _cell_h(d, r, col_w, f_, size, pad_y, lead)) for r in rows]
        return f_, fh_, hh, rh

    def row_h_min(size):
        return size + pad_y * 2 + 10

    size = fs
    f, fh, head_h, row_hs = layout(size)
    if max_bottom is not None:
        while size > 18 and y + head_h + sum(row_hs) > max_bottom:
            size -= 2
            f, fh, head_h, row_hs = layout(size)

    total_w = sum(col_w)
    cx = x
    for j, h in enumerate(headers):
        d.rectangle([cx, y, cx + col_w[j], y + head_h], fill=deep)
        for k, ln in enumerate(wrap(d, h, fh, col_w[j] - pad_x * 2)):
            d.text((cx + pad_x, y + pad_y + k * (size + lead)), ln, font=fh,
                   fill=(255, 255, 255), anchor="la")
        cx += col_w[j]

    ry = y + head_h
    for i, row in enumerate(rows):
        rh = row_hs[i]
        if i % 2 == 1:
            d.rectangle([x, ry, x + total_w, ry + rh], fill=(224, 234, 226))
        d.rectangle([x, ry, x + total_w, ry + rh], outline=line, width=2)
        cx = x
        for j, cell in enumerate(row):
            lines = wrap(d, cell, f, col_w[j] - pad_x * 2)
            ty = ry + max(pad_y, (rh - len(lines) * (size + lead)) // 2)
            for k, ln in enumerate(lines):
                color = accent if (hl and (i, j) in hl) else (40, 50, 38)
                d.text((cx + pad_x, ty + k * (size + lead)), ln, font=f, fill=color, anchor="la")
            cx += col_w[j]
        ry += rh
    return ry


def _cell_h(d, cells, col_w, f, size, pad_y, lead):
    n = 1
    for j, c in enumerate(cells):
        n = max(n, len(wrap(d, str(c), f, col_w[j] - 24)))
    return n * (size + lead) + pad_y * 2


def card1(C, D, deep, accent, light, line):
    im, d = new_canvas(deep)
    d.rectangle([0, 0, 1080, 120], fill=deep)
    ctext(d, C.top, font(FONT_HEAVY, 36), 1080 / 2, 60, accent)
    ctext_fit(d, C.card1_title, FONT_HEAVY, 92, 1080 / 2, 280, (255, 255, 255), 1080 - 120)
    ctext_wrap(d, C.card1_sub, FONT_REG, 40, 1080 / 2, 385, accent, 1080 - 120, max_lines=2)
    blocks = [tuple(b.split("+")) for b in C.card1_blocks.split(",")]
    bw, bh = 400, 220
    x0 = (1080 - (bw * 2 + 40)) // 2
    y0 = 520
    # 2026-08-11 修复：原用固定 58px 单行居中，长文案直接横向溢出方框并与邻框串行。
    # 改为框内自适应换行（上下两段各限 2 行，宽度收进内边距）。
    inner_w = bw - 44
    for i, blk in enumerate(blocks):
        a = blk[0] if len(blk) > 0 else ""
        b = blk[1] if len(blk) > 1 else ""
        x = x0 + i * (bw + 40)
        d.rounded_rectangle([x, y0, x + bw, y0 + bh], 24, fill=deep, outline=accent, width=4)
        ctext_wrap(d, a, FONT_HEAVY, 46, x + bw / 2, y0 + 68, (255, 255, 255), inner_w,
                   lead=6, max_lines=2, minsz=24)
        ctext_wrap(d, b, FONT_HEAVY, 46, x + bw / 2, y0 + 156, accent, inner_w,
                   lead=6, max_lines=2, minsz=24)
    ctext_wrap(d, C.card1_mid, FONT_REG, 38, 1080 / 2, 845, light, 1080 - 140, max_lines=2)
    d.rounded_rectangle([120, 950, 1080 - 120, 1050], 20, fill=accent)
    ctext_fit(d, C.card1_cta, FONT_HEAVY, 38, 1080 / 2, 1000, (255, 255, 255), 1080 - 300)
    d.rectangle([0, 1300, 1080, 1440], fill=deep)
    ctext(d, "招投标专家日报 · 每日政策速递", font(FONT_REG, 30), 1080 / 2, 1348, light)
    out = os.path.join(D, f"{C.date}-小红书配图1-首图-{C.theme}.png")
    im.save(out); print("1 ->", out); return out


def card2(C, D, deep, accent, light, line):
    im, d = new_canvas(deep)
    d.rectangle([0, 0, 1080, 140], fill=deep)
    d.rounded_rectangle([40, 42, 130, 132], 20, fill=accent)
    d.text((85, 87), "2", font=font(FONT_HEAVY, 50), fill=(255, 255, 255), anchor="mm")
    ctext(d, C.card2_title, font(FONT_HEAVY, 44), 330, 87, (255, 255, 255))
    data = json.loads(C.table)
    headers, rows = data[0], data[1:]
    # 2026-08-11 修复：原按列数硬分配（如 3 列固定 180/590/210），末列过窄导致
    # “8月15日 17:00”被拆出孤儿字符、词组被腰斩。改为按各列实际最长内容比例分配。
    ncol = len(headers)
    total_w = 980
    _mf = font(FONT_REG, 28)
    need = []
    for j in range(ncol):
        w = textw(d, str(headers[j]), _mf)
        for r in rows:
            if j < len(r):
                w = max(w, textw(d, str(r[j]), _mf))
        need.append(w + 28)  # + 左右内边距
    s = sum(need)
    if s <= total_w:
        extra = total_w - s
        col_w = [n + extra // ncol for n in need]
        col_w[-1] += total_w - sum(col_w)
    else:
        col_w = [max(110, int(total_w * n / s)) for n in need]
        col_w[-1] += total_w - sum(col_w)

    # 底部预留：note + bottom + 页脚
    note_lines = len(wrap(d, C.card2_note, font(FONT_REG, 30), 1080 - 100)) if C.card2_note else 0
    reserve = 40 + note_lines * 40 + 60 + 70
    table_bottom = table_draw(d, 50, 200, col_w, None, headers, rows, fs=28,
                              hl=set(tuple(x) for x in json.loads(C.table_hl)),
                              deep=deep, accent=accent, light=light, line=line,
                              max_bottom=1440 - reserve)

    ny = table_bottom + 36
    if C.card2_note:
        for i, ln in enumerate(wrap(d, C.card2_note, font(FONT_REG, 30), 1080 - 100)):
            d.text((50, ny + i * 40), ln, font=font(FONT_REG, 30), fill=accent, anchor="la")
        ny += note_lines * 40
    ctext(d, C.card2_bottom, font(FONT_HEAVY, 32), 1080 / 2, min(ny + 44, 1440 - 96), accent)
    d.text((40, 1440 - 52), "招投标专家日报 · 截图保存", font=font(FONT_REG, 28), fill=light, anchor="la")
    out = os.path.join(D, f"{C.date}-小红书配图2-对比自查-{C.theme}.png")
    im.save(out); print("2 ->", out); return out


def card3(C, D, deep, accent, light, line):
    im, d = new_canvas(deep)
    d.rectangle([0, 0, 1080, 140], fill=deep)
    d.rounded_rectangle([40, 42, 130, 132], 20, fill=accent)
    d.text((85, 87), "3", font=font(FONT_HEAVY, 50), fill=(255, 255, 255), anchor="mm")
    ctext(d, C.card3_title, font(FONT_HEAVY, 44), 330, 87, (255, 255, 255))
    items = json.loads(C.card3_items)
    y = 200
    for it in items:
        head, body = it["h"], it["b"]
        d.rounded_rectangle([50, y, 90, y + 40], 8, fill=accent)
        d.text((70, y + 20), head[0], font=font(FONT_HEAVY, 30), fill=(255, 255, 255), anchor="mm")
        d.text((110, y - 2), head[1:], font=font(FONT_HEAVY, 33), fill=(255, 255, 255), anchor="la")
        by = y + 52
        for ln in wrap(d, body, font(FONT_REG, 28), 1080 - 160):
            d.text((110, by), ln, font=font(FONT_REG, 28), fill=light, anchor="la")
            by += 42
        y = by + 36
    # 2026-08-11 修复：原 reward/foot 用固定字号单行居中，长文案左右双向溢出画布被裁。
    # 改为框内自适应换行（最多 3 行，框高随行数增长）。
    rw_x0, rw_x1 = 50, 1080 - 50
    rw_inner = (rw_x1 - rw_x0) - 56
    rf = font(FONT_HEAVY, 29)
    rlines = wrap(d, C.reward, rf, rw_inner)
    rsize = 29
    while len(rlines) > 3 and rsize > 18:
        rsize -= 2
        rf = font(FONT_HEAVY, rsize)
        rlines = wrap(d, C.reward, rf, rw_inner)
    box_h = max(80, len(rlines) * (rsize + 10) + 28)
    rw_y0 = 1050
    d.rounded_rectangle([rw_x0, rw_y0, rw_x1, rw_y0 + box_h], 18, fill=deep, outline=accent, width=3)
    ty = rw_y0 + (box_h - (len(rlines) * (rsize + 10) - 10)) / 2
    for ln in rlines:
        d.text((1080 / 2, ty + rsize / 2), ln, font=rf, fill=accent, anchor="mm")
        ty += rsize + 10
    ctext_wrap(d, C.foot, FONT_HEAVY, 28, 1080 / 2, max(1250, rw_y0 + box_h + 60),
               accent, 1080 - 120, max_lines=2)
    d.text((40, 1440 - 56), "招投标专家日报 · 截图保存", font=font(FONT_REG, 28), fill=light, anchor="la")
    out = os.path.join(D, f"{C.date}-小红书配图3-行动清单-{C.theme}.png")
    im.save(out); print("3 ->", out); return out


if __name__ == "__main__":
    C = parse_args()
    D = os.path.join(SHARED, C.date)
    os.makedirs(D, exist_ok=True)
    deep, accent = hex2rgb(C.deep), hex2rgb(C.accent)
    light, line = (247, 236, 224), (200, 160, 110)
    card1(C, D, deep, accent, light, line)
    card2(C, D, deep, accent, light, line)
    card3(C, D, deep, accent, light, line)
    print("DONE")
