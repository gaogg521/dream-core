#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Composite crisp Chinese text onto AI backgrounds for bid_cover Phase 5.
通用 canonical 封面脚本（克隆 8/5 精修版，去写死日期/主题/配色）。
用法：
  python3 build_covers.py --date 20260809 --theme "主题串" --deep "#9E2B25" --accent "#E8A33D" \
      --series "政策收紧 · 出库预警" --tagline "8月新规 · 全链条收紧" --sub "副标题" --seal "出库收紧" \
      --cards "入库,续聘,考核,解聘,终身" --info "信息条文案" --wechat "微信头图大字≤5" --toutiao_info "头条信息条"
所有产物落 <SHARED>/<date>/，文件名前缀为 <date>-。
未来 DAG 只改参数，禁止手改结构（见 AGENTS.md「克隆+参数化唯一真相源」）。
"""
import argparse, os
from PIL import Image, ImageDraw, ImageFont, ImageFilter

import os
SHARED = os.environ.get("BID_SHARED_DIR", os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# 注：原系统硬编码 /home/laoty/.openclaw/workspace-bid-shared，已参数化为 BID_SHARED_DIR；
# 未设置时回退到本包 skills/bid-pipeline/（含 fonts/ 与 assets/），保证开箱即用。
BG_H = os.path.join(SHARED, "assets", "cover_bg_h.png")
BG_V = os.path.join(SHARED, "assets", "cover_bg_v.png")

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


def font(path, size):
    return ImageFont.truetype(path, size)


def cover_image(src, tw, th):
    im = Image.open(src).convert("RGB")
    sw, sh = im.size
    scale = max(tw / sw, th / sh)
    nw, nh = int(round(sw * scale)), int(round(sh * scale))
    im = im.resize((nw, nh), Image.LANCZOS)
    left = (nw - tw) // 2
    top = (nh - th) // 2
    return im.crop((left, top, left + tw, top + th))


def tint_image(im, rgb, strength=0.55):
    """将背景图整体偏向主题色，压住旧系列残留的橙色斜条。"""
    base = Image.new("RGB", im.size, rgb)
    return Image.blend(im.convert("RGB"), base, strength).convert("RGBA")


def add_gradient(draw, w, h, vertical=True, start=0.0, end=1.0, color=(0, 0, 0), max_alpha=150):
    span = end - start
    if vertical:
        y0 = int(h * start)
        y1 = int(h * end)
        for y in range(y0, y1):
            a = int(max_alpha * (y - y0) / max(1, (y1 - y0)))
            draw.rectangle([0, y, w, y + 1], fill=color + (a,))
    else:
        x0 = int(w * start)
        x1 = int(w * end)
        for x in range(x0, x1):
            a = int(max_alpha * (x - x0) / max(1, (x1 - x0)))
            draw.rectangle([x, 0, x + 1, h], fill=color + (a,))


def round_rect(draw, box, radius, fill=None, outline=None, width=2):
    draw.rounded_rectangle(box, radius=radius, fill=fill, outline=outline, width=width)


def text_w(draw, s, f):
    b = draw.textbbox((0, 0), s, font=f)
    return b[2] - b[0]


def fit_font(draw, text, font_path, size, max_w, min_size=14):
    """自适应缩字号直到文本宽度 <= max_w（2026-08-11 修复：原脚本无宽度自适应，
    长文案直接溢出画布/压住相邻元素）。max_w<=0 时不做处理。"""
    f = font(font_path, size)
    if not text or max_w is None or max_w <= 0:
        return f
    while size > min_size and text_w(draw, text, f) > max_w:
        size -= 2
        f = font(font_path, size)
    return f


def draw_line(draw, xy, text, highlight, f, base, hi=None, anchor="la", shadow=True, shadow_off=3):
    x, y = xy
    if shadow:
        draw.text((x + shadow_off, y + shadow_off), text, font=f, fill=(0, 0, 0, 160), anchor=anchor)
    if not highlight or highlight not in text:
        draw.text((x, y), text, font=f, fill=base, anchor=anchor)
        return
    idx = text.index(highlight)
    pre = text[:idx]
    post = text[idx + len(highlight):]
    pw = text_w(draw, pre, f)
    if anchor[0] == "l":
        cx = x
    elif anchor[0] == "m":
        cx = x - text_w(draw, text, f) / 2
    else:
        cx = x - text_w(draw, text, f)
    draw.text((cx, y), text, font=f, fill=base, anchor="la")
    draw.text((cx + pw, y), highlight, font=f, fill=hi, anchor="la")


def draw_pill(draw, xy, text, f, fill, fg, pad_x=22, pad_y=10, radius=30, outline=None, width=2,
              max_w=None, font_path=None):
    x, y = xy
    if max_w and font_path:
        f = fit_font(draw, text, font_path, f.size, max_w - pad_x * 2)
    tw = text_w(draw, text, f)
    box = [x, y, x + tw + pad_x * 2, y + f.size + pad_y * 2]
    round_rect(draw, box, radius, fill=fill, outline=outline, width=width)
    draw.text((x + pad_x, y + pad_y), text, font=f, fill=fg, anchor="la")
    return box


# ============ 解析参数 ============
def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--date", required=True)
    p.add_argument("--theme", required=True, help="文件名「主题」串（兼容化，≤22字）")
    p.add_argument("--deep", default="#9E2B25", help="深主色")
    p.add_argument("--accent", default="#E8A33D", help="强调色")
    p.add_argument("--series", default="招投标专家日报")
    p.add_argument("--tagline", default="8月新规 · 全链条收紧")
    p.add_argument("--sub", default="评标出库线全面收紧")
    p.add_argument("--seal", default="出库收紧")
    p.add_argument("--cards", default="入库,续聘,考核,解聘,终身")
    p.add_argument("--info", default="8月三省份：迟到/续聘/禁评")
    p.add_argument("--wechat", default="30分钟", help="微信头图大字（≤5字）")
    p.add_argument("--toutiao_info", default="")
    p.add_argument("--toutiao_tag", default="")
    p.add_argument("--h1", default="迟到30分钟，评标资格就没了", help="横版主标题行")
    p.add_argument("--h1_hi", default="30分钟", help="横版主标题高亮词")
    p.add_argument("--big_a", default="迟到30分")
    p.add_argument("--big_b", default="就出局", help="竖版/小红书第二行大标题")
    p.add_argument("--big_b_hi", default="出库")
    p.add_argument("--big_q", default="迟到30分钟就出局？", help="竖版/小红书疑问行")
    p.add_argument("--big_q_hi", default="一辈子")
    p.add_argument("--cover_mid", default="8月新规，你的出库红线碰了吗？")
    p.add_argument("--cover_mid2", default="三条出库口子已为你列好 →")
    p.add_argument("--cta", default="关注我", help="底部行动文案")
    p.add_argument("--cta_sub", default="每天一条招投标干货")
    return p.parse_args()


def build_horizontal(C, D):
    W, H = 1125, 450
    deep, accent, light = hex2rgb(C.deep), hex2rgb(C.accent), (251, 249, 246)
    # 2026-08-12 修复：横版原不染色，所有文章共用同一张 BG_H，肉眼像"同一张图"。
    # 改为用 deep 强染背景（0.82），让每篇主色真正落地；印章改 accent 填充，避免深底上深底不可见。
    # 暗化渐变改用「本篇 deep 同色」（alpha 降档），不再用中性黑 (40,16,14) 把 deep 拖回灰。
    im = tint_image(cover_image(BG_H, W, H), deep, 0.82).convert("RGBA")
    d = ImageDraw.Draw(im)
    add_gradient(d, W, H, vertical=False, start=0.0, end=0.72, color=deep, max_alpha=110)
    add_gradient(d, W, H, vertical=True, start=0.66, end=1.0, color=deep, max_alpha=90)
    seal_reserve = 130  # 右侧竖排印章预留带
    body_max = W - 60 - seal_reserve
    draw_pill(d, (60, 40), C.tagline, font(FONT_REG, 26), fill=accent, fg=(255, 255, 255),
              max_w=body_max, font_path=FONT_REG)
    draw_line(d, (60, 150), C.h1, C.h1_hi,
              fit_font(d, C.h1, FONT_HEAVY, 54, body_max), (255, 255, 255), accent)
    draw_line(d, (62, 248), C.info, None,
              fit_font(d, C.info, FONT_REG, 30, body_max), light)
    draw_line(d, (62, 392), "招投标专家日报  ·  每日政策速递", None, font(FONT_REG, 28), (255, 255, 255))
    # 竖排印章：自适应字数（2026-08-10 修复——原硬截断 4 字，导致「倒计时22天」变「倒计时2」）
    chars = list(C.seal)
    n = len(chars)
    # 2026-08-11 修复：原公式仅约束 n*step+40，但实际盒高为 n*step+20且循环可能在 fsz 触底
    # 时提前退出，导致末字被画布底边裁切。改为用真实盒高约束 + 居中垂直安放。
    seal_top_min, seal_bottom_max = 120, H - 30
    avail = seal_bottom_max - seal_top_min
    fsz = 38
    step = fsz + 12
    while n * step + 20 > avail and fsz > 16:
        fsz -= 2
        step = fsz + 12
    seal_f = font(FONT_HEAVY, fsz)
    box_w = fsz + 40
    sx = W - box_w - 34
    box_h = n * step + 20
    seal_top = seal_top_min + max(0, (avail - box_h) // 2)
    if seal_top + box_h > seal_bottom_max:
        seal_top = max(seal_top_min, seal_bottom_max - box_h)
    round_rect(d, [sx, seal_top, sx + box_w, seal_top + box_h], 14, fill=accent)
    for i, c in enumerate(chars):
        d.text((sx + box_w / 2, seal_top + 10 + step / 2 + i * step), c,
               font=seal_f, fill=(255, 255, 255), anchor="mm")
    out = os.path.join(D, f"{C.date}-横版封面-{C.theme}.png")
    im.convert("RGB").save(out, "PNG")
    print("HORIZONTAL ->", out)
    return out


def build_wechat_cover(C, D):
    W, H = 900, 383
    deep, accent, light = hex2rgb(C.deep), hex2rgb(C.accent), (251, 249, 246)
    title_fill, sub_fill = deep, (74, 40, 32)
    pill_bg, pill_fill = accent, (255, 255, 255)
    tag_bg, tag_fill = deep, (255, 255, 255)
    card_fills = [deep, deep, deep, deep, deep]
    text_col = (255, 255, 255)
    img = Image.new("RGB", (W, H), light)
    d = ImageDraw.Draw(img)
    cx = 302
    tag_box = [64, 288, W - 360, 332]
    left_max = tag_box[2] - tag_box[0] - 24
    d.rounded_rectangle(tag_box, radius=18, fill=tag_bg)
    d.text((cx, 310), C.info, font=fit_font(d, C.info, FONT_REG, 28, left_max),
           fill=tag_fill, anchor="mm")
    pw = 248
    d.rounded_rectangle([cx - pw // 2, 52, cx + pw // 2, 92], radius=20, fill=pill_bg)
    d.text((cx, 72), C.tagline, font=fit_font(d, C.tagline, FONT_HEAVY, 26, pw - 24),
           fill=pill_fill, anchor="mm")
    d.text((cx, 145), C.wechat, font=fit_font(d, C.wechat, FONT_HEAVY, 74, left_max),
           fill=title_fill, anchor="mm")
    d.text((cx, 228), C.sub, font=fit_font(d, C.sub, FONT_REG, 26, left_max),
           fill=sub_fill, anchor="mm")
    w, h_card, r = 150, 42, 10
    positions = [(578, 84), (642, 138), (578, 192), (642, 246), (578, 300)]
    labels = C.cards.split(",")
    for (x, y), label, fill in zip(positions, labels, card_fills):
        d.rounded_rectangle([x, y, x + w, y + h_card], radius=r, fill=fill)
        d.text((x + w // 2, y + h_card // 2), label, font=font(FONT_HEAVY, 18), fill=text_col, anchor="mm")
    out = os.path.join(D, f"{C.date}-微信公众号头图-{C.theme}.png")
    img.save(out, "PNG")
    print("WECHAT_COVER ->", out)
    return out


def build_toutiao_cover(C, D):
    W, H = 900, 500
    deep, accent, light = hex2rgb(C.deep), hex2rgb(C.accent), (251, 249, 246)
    title_fill, sub_fill = deep, (74, 40, 32)
    pill_bg = accent
    pill_fill = tuple(max(0, c - 64) for c in deep)  # 深主色字 on 琥珀金（WCAG）
    tag_bg, tag_fill = deep, (255, 255, 255)
    card_fills = [deep, deep, deep, deep, deep]
    text_col = (255, 255, 255)
    img = Image.new("RGB", (W, H), light)
    d = ImageDraw.Draw(img)
    cx = 302
    tag_box = [64, 388, W - 360, 432]
    left_max = tag_box[2] - tag_box[0] - 24
    d.rounded_rectangle(tag_box, radius=18, fill=tag_bg)
    _tinfo = C.toutiao_info or C.info
    d.text((cx, 410), _tinfo, font=fit_font(d, _tinfo, FONT_REG, 28, left_max),
           fill=tag_fill, anchor="mm")
    pw = 248
    _ttag = C.toutiao_tag or C.tagline
    d.rounded_rectangle([cx - pw // 2, 52, cx + pw // 2, 92], radius=20, fill=pill_bg)
    d.text((cx, 72), _ttag, font=fit_font(d, _ttag, FONT_HEAVY, 26, pw - 24),
           fill=pill_fill, anchor="mm")
    d.text((cx, 178), C.wechat, font=fit_font(d, C.wechat, FONT_HEAVY, 84, left_max),
           fill=title_fill, anchor="mm")
    d.text((cx, 268), C.sub, font=fit_font(d, C.sub, FONT_REG, 29, left_max),
           fill=sub_fill, anchor="mm")
    if C.toutiao_info:
        d.text((cx, 338), C.toutiao_info, font=fit_font(d, C.toutiao_info, FONT_REG, 19, left_max),
               fill=sub_fill, anchor="mm")
    w, h_card, r = 150, 42, 10
    positions = [(578, 120), (642, 174), (578, 228), (642, 282), (578, 336)]
    labels = C.cards.split(",")
    for (x, y), label, fill in zip(positions, labels, card_fills):
        d.rounded_rectangle([x, y, x + w, y + h_card], radius=r, fill=fill)
        d.text((x + w // 2, y + h_card // 2), label, font=font(FONT_HEAVY, 18), fill=text_col, anchor="mm")
    out = os.path.join(D, f"{C.date}-今日头条封面-{C.theme}.png")
    img.save(out, "PNG")
    print("TOUTIAO_COVER ->", out)
    return out


def add_avatar_badge(im, seal_rgb, size=200, pad=14, margin=50):
    """右上角嵌入 AI 形象徽章（白底 chroma 抠除→圆形遮罩→橙红描边→品牌蓝圆角底板）。
    复用 build_videos.py 的 alpha_composite 写法（非 draw.bitmap，后者会丢像素只留遮罩）。
    缺失形象图时直接跳过，不报错。"""
    avatar = None
    for p in [os.path.join(SHARED, "assets/AI形象.png")]:
        if os.path.exists(p):
            avatar = p
            break
    if avatar is None:
        return im
    a = Image.open(avatar).convert("RGBA")
    apx = a.load()
    aw, ah = a.size
    for y in range(ah):
        for x in range(aw):
            r, g, b, al = apx[x, y]
            if r > 235 and g > 235 and b > 235:
                apx[x, y] = (r, g, b, 0)
    a = a.crop(a.getbbox()).resize((size, size))
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, size, size], fill=255)
    circ = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    circ.paste(a, (0, 0), mask)
    edge = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(edge).ellipse([0, 0, size - 1, size - 1], outline=seal_rgb, width=8)
    av = Image.alpha_composite(circ, edge)
    bx = im.size[0] - size - margin - pad
    by = margin - pad
    plate = Image.new("RGBA", (size + 2 * pad, size + 2 * pad), (0, 0, 0, 0))
    ImageDraw.Draw(plate).rounded_rectangle(
        [0, 0, size + 2 * pad - 1, size + 2 * pad - 1],
        radius=size // 2 + pad, fill=(26, 95, 180, 235))
    cv = im.convert("RGBA")
    cv.alpha_composite(plate, (bx, by))
    cv.alpha_composite(av, (bx + pad, by + pad))
    return cv


def build_vertical(C, D):
    W, H = 1080, 1920
    deep, accent, light = hex2rgb(C.deep), hex2rgb(C.accent), (251, 249, 246)
    # 2026-08-12 修复：原 0.55 染色过弱，深主色压不住原图紫调，各篇都读成"同款深紫"。
    # 提到 0.82，让每篇 deep 主色真正主导背景，实现"每篇独一无二色调"。
    im = tint_image(cover_image(BG_V, W, H), deep, 0.90)
    # 2026-08-12 修复：原中性黑 (20,20,20) max_alpha=200 暗化渐变盖在染色后，把 deep 色拉回近中性深灰，
    # 各篇都像同款深色图。改用「本篇 deep 同色」暗化（alpha 大幅降低），保文字可读且让 deep 主导全图。
    d = ImageDraw.Draw(im)
    add_gradient(d, W, H, vertical=True, start=0.45, end=1.0, color=deep, max_alpha=120)
    add_gradient(d, W, H, vertical=True, start=0.0, end=0.12, color=deep, max_alpha=90)
    mx = 80
    tmax = W - mx * 2
    # 2026-08-20 修复：卡片底改为纯黑不透明(alpha=255)，避免背景色透过来导致缩略图发灰
    round_rect(d, [40, 300, W - 40, 910], 28, fill=(8, 8, 12, 255))
    _tag = f"招投标专家 · {C.tagline}"
    draw_pill(d, (mx, 110), _tag, font(FONT_REG, 32), fill=accent, fg=(255, 255, 255),
              max_w=tmax, font_path=FONT_REG)
    draw_line(d, (mx, 360), C.big_a, None,
              fit_font(d, C.big_a, FONT_HEAVY, 150, tmax), (255, 255, 255), None)
    draw_line(d, (mx, 560), C.big_b, C.big_b_hi,
              fit_font(d, C.big_b, FONT_HEAVY, 150, tmax), (255, 255, 255), accent)
    draw_line(d, (mx + 4, 800), C.big_q, C.big_q_hi,
              fit_font(d, C.big_q, FONT_REG, 60, tmax - 8), (255, 255, 255), accent)
    d.line([mx, 980, W - mx, 980], fill=(160, 160, 160, 255), width=3)
    # 2026-08-20 修复：cover_mid/mid2 改为纯白加粗描边效果（通过阴影模拟）提升缩略图可读性
    for off in [(2,2),(-2,-2),(2,-2),(-2,2)]:
        draw_line(d, (mx+off[0], 1015+off[1]), C.cover_mid, None,
                  fit_font(d, C.cover_mid, FONT_REG, 38, tmax), (0,0,0), None)
    draw_line(d, (mx, 1015), C.cover_mid, None,
              fit_font(d, C.cover_mid, FONT_REG, 38, tmax), (255, 255, 255), None)
    for off in [(1,1),(-1,-1),(1,-1),(-1,1)]:
        draw_line(d, (mx+off[0], 1075+off[1]), C.cover_mid2, None,
                  fit_font(d, C.cover_mid2, FONT_REG, 34, tmax), (0,0,0), None)
    draw_line(d, (mx, 1075), C.cover_mid2, None,
              fit_font(d, C.cover_mid2, FONT_REG, 34, tmax), (255, 255, 255), None)
    _pill = C.toutiao_info or "8/1 已施行 · 3天两道新规"
    draw_pill(d, (mx, 920), _pill, font(FONT_REG, 32), fill=(20, 20, 20), fg=(255, 255, 255),
              outline=accent, width=3, max_w=tmax, font_path=FONT_REG)
    # 2026-08-20 修复：底部"关注我"+"每天一条招投标干货"改为实心黑底药丸框，提升缩略图对比度
    cta_combined = f"{C.cta} · {C.cta_sub}"
    draw_pill(d, (mx, 1320), cta_combined, font(FONT_HEAVY, 48), fill=(0,0,0), fg=(255,255,255),
              outline=None, width=0, max_w=tmax, font_path=FONT_HEAVY)
    # 删除原 C.sub 小字（缩略图下无意义，视觉噪音）
    out = os.path.join(D, f"{C.date}-竖版封面-{C.theme}.png")
    im = add_avatar_badge(im, hex2rgb(C.deep))
    im.convert("RGB").save(out, "PNG")
    print("VERTICAL ->", out)
    return out


def build_xhs(C, D):
    W, H = 1242, 1660
    deep, accent, light = hex2rgb(C.deep), hex2rgb(C.accent), (251, 249, 246)
    # 2026-08-12 修复：同竖版，暗化渐变改用 deep 同色（alpha 120/90），不再用中性黑拖灰。
    im = tint_image(cover_image(BG_V, W, H), deep, 0.90)
    d = ImageDraw.Draw(im)
    add_gradient(d, W, H, vertical=True, start=0.45, end=1.0, color=deep, max_alpha=120)
    add_gradient(d, W, H, vertical=True, start=0.0, end=0.11, color=deep, max_alpha=90)
    mx = 70
    tmax = W - mx * 2
    # 2026-08-20 修复：卡片底纯黑不透明，避免背景透色导致缩略图发灰
    round_rect(d, [40, 250, W - 40, 830], 28, fill=(8, 8, 12, 255))
    _tag = f"招投标专家 · {C.tagline}"
    draw_pill(d, (mx, 80), _tag, font(FONT_REG, 30), fill=accent, fg=(255, 255, 255),
              max_w=tmax, font_path=FONT_REG)
    draw_line(d, (mx, 300), C.big_a, None,
              fit_font(d, C.big_a, FONT_HEAVY, 132, tmax), (255, 255, 255), None)
    draw_line(d, (mx, 470), C.big_b, C.big_b_hi,
              fit_font(d, C.big_b, FONT_HEAVY, 132, tmax), (255, 255, 255), accent)
    draw_line(d, (mx + 4, 690), C.big_q, C.big_q_hi,
              fit_font(d, C.big_q, FONT_REG, 58, tmax - 8), (255, 255, 255), accent)
    _pill = C.toutiao_info or "8/1 已施行 · 3天两道新规"
    # 2026-08-20 修复：药丸底改纯白+深字，或保持深底但加粗白边；这里改深底+白字+白边提升缩略图辨识度
    draw_pill(d, (mx, 790), _pill, font(FONT_REG, 30), fill=(20, 20, 20), fg=(255, 255, 255),
              outline=(255,255,255), width=4, max_w=tmax, font_path=FONT_REG)
    d.line([mx, 900, W - mx, 900], fill=(160, 160, 160, 255), width=3)
    # 2026-08-20 修复：cover_mid/mid2 加阴影提升缩略图可读性
    for off in [(2,2),(-2,-2),(2,-2),(-2,2)]:
        draw_line(d, (mx+off[0], 940+off[1]), C.cover_mid, None,
                  fit_font(d, C.cover_mid, FONT_REG, 37, tmax), (0,0,0), None)
    draw_line(d, (mx, 940), C.cover_mid, None,
              fit_font(d, C.cover_mid, FONT_REG, 37, tmax), (255, 255, 255), None)
    for off in [(1,1),(-1,-1),(1,-1),(-1,1)]:
        draw_line(d, (mx+off[0], 1000+off[1]), C.cover_mid2, None,
                  fit_font(d, C.cover_mid2, FONT_REG, 33, tmax), (0,0,0), None)
    draw_line(d, (mx, 1000), C.cover_mid2, None,
              fit_font(d, C.cover_mid2, FONT_REG, 33, tmax), (255, 255, 255), None)
    # 2026-08-20 修复：底部"关注我"+"每天一条招投标干货"改为实心黑底药丸框，提升缩略图对比度
    cta_combined = f"{C.cta} · {C.cta_sub}"
    draw_pill(d, (mx, 1180), cta_combined, font(FONT_HEAVY, 46), fill=(0,0,0), fg=(255,255,255),
              outline=None, width=0, max_w=tmax, font_path=FONT_HEAVY)
    # 删除原 C.sub 小字（缩略图下无意义）
    out = os.path.join(D, f"{C.date}-小红书封面-{C.theme}.png")
    im = add_avatar_badge(im, hex2rgb(C.deep), size=int(200 * 1242 / 1080), pad=int(14 * 1242 / 1080), margin=int(50 * 1242 / 1080))
    im.convert("RGB").save(out, "PNG")
    print("XHS ->", out)
    return out


def wcag_check(C):
    light = (251, 249, 246)
    deep, accent = hex2rgb(C.deep), hex2rgb(C.accent)
    pill_fill = tuple(max(0, c - 64) for c in deep)
    def _lin(c):
        c = c / 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    def _L(rgb):
        return 0.2126 * _lin(rgb[0]) + 0.7152 * _lin(rgb[1]) + 0.0722 * _lin(rgb[2])
    def _cr(fg, bg):
        l1, l2 = _L(fg), _L(bg)
        hi, lo = max(l1, l2), min(l1, l2)
        return (hi + 0.05) / (lo + 0.05)
    checks = [
        ("主标题深主色 on 浅米", _cr(deep, light)),
        ("卡片白字 on 深主色", _cr((255, 255, 255), deep)),
        ("pill深主色字 on 琥珀金", _cr(pill_fill, accent)),
    ]
    for name, r in checks:
        p = r >= 3.0  # 大字号文字 WCAG 阈值放宽到 3:1
        print(f"  {name}: {r:.2f} {'OK' if p else 'WARN(<3.0)'}")
    print("COVERS WCAG check done (warn-only)")


if __name__ == "__main__":
    C = parse_args()
    D = os.path.join(SHARED, C.date)
    os.makedirs(D, exist_ok=True)
    build_horizontal(C, D)
    build_vertical(C, D)
    build_xhs(C, D)
    build_wechat_cover(C, D)
    build_toutiao_cover(C, D)
    wcag_check(C)
    print("DONE")
