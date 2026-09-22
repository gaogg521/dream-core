#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成视频号 + 抖音 带中文配音+字幕 mp4（通用 canonical）。
复用已有竖版封面 + 小红书3图当画面；高斯模糊背景 + contain 居中贴图，底部独立字幕带，
徽章仅封面镜显示（避免压卡），主题色随 --deep。边缘加噪点消除 banding。

用法：
  python3 build_videos.py --date 20260809 --theme "主题串" --deep "#9E2B25" \
      --scenes_vx '[{"bg":"cover","subs":[...],"narr":"..."}, ...]' \
      --scenes_dy '[{"bg":"x1","subs":[...],"narr":"..."}]'
"""
import argparse, os, asyncio, subprocess, json, tempfile
from PIL import Image, ImageDraw, ImageFont, ImageFilter

import os
SHARED = os.environ.get("BID_SHARED_DIR", os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# 注：原系统硬编码 /home/laoty/.openclaw/workspace-bid-shared，已参数化为 BID_SHARED_DIR（未设置时回退到本脚本所在目录）
FONT = os.path.join(SHARED, "fonts/LXGWWenKai.ttf")
if not os.path.exists(FONT):
    FONT = "/tmp/LXGWWenKai.ttf"
if not os.path.exists(FONT):
    FONT = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"

W, H = 1080, 1920
FPS = 25
import edge_tts


def hex2rgb(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))


def fsize(s):
    return ImageFont.truetype(FONT, s)


def wrap(d, text, f, maxw):
    lines = []
    cur = ""
    for ch in text:
        if d.textbbox((0, 0), cur + ch, font=f)[2] <= maxw:
            cur += ch
        else:
            lines.append(cur)
            cur = ch
    if cur:
        lines.append(cur)
    return lines


def get_avatar():
    for p in [os.path.join(SHARED, "assets/AI形象.png"),
              "/home/laoty/Downloads/AI形象.png"]:
        if os.path.exists(p):
            return p
    return None


def round_avatar(avatar_path, size=260, seal_rgb=(158, 43, 37)):
    a = Image.open(avatar_path).convert("RGBA")
    px = a.load()
    w, h = a.size
    for y in range(h):
        for x in range(w):
            r, g, b, al = px[x, y]
            if r > 235 and g > 235 and b > 235:
                px[x, y] = (r, g, b, 0)
    a = a.crop(a.getbbox()).resize((size, size))
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, size, size], fill=255)
    circ = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    circ.paste(a, (0, 0), mask)
    edge = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(edge).ellipse([0, 0, size - 1, size - 1], outline=seal_rgb, width=8)
    return Image.alpha_composite(circ, edge)


def add_dither(img, amount=10):
    """全图随机噪点打散 8bit 渐变 banding（画字前调用，文字本身不受影响）。"""
    w, h = img.size
    noise = Image.effect_noise((w, h), amount).convert("L")
    px = img.load()
    nx = noise.load()
    for y in range(h):
        for x in range(w):
            d = nx[x, y] - 128
            r, g, b = px[x, y][:3]
            px[x, y] = (max(0, min(255, r + d)), max(0, min(255, g + d)), max(0, min(255, b + d)))
    return img


def make_scene(bg_path, sub_lines, out_png, big=False, avatar=None, seal_rgb=(158, 43, 37),
              is_cover=False):
    """图片按比例适配进上方区域（contain，绝不拉伸），底部预留独立字幕带。
    is_cover=True 时整屏背景、显示 AI 形象徽章；配图卡镜 is_cover=False 不显示徽章避免压卡。"""
    src = Image.open(bg_path).convert("RGB")
    back = src.resize((W // 12, H // 12)).resize((W, H)).filter(ImageFilter.GaussianBlur(60))
    dark = Image.new("RGB", (W, H), (18, 16, 16))
    back = Image.blend(back, dark, 0.45)
    BAND = 560
    top_h = H - BAND
    sw, sh = src.size
    scale = min(W / sw, top_h / sh)
    nw, nh = max(1, int(sw * scale)), max(1, int(sh * scale))
    img_f = src.resize((nw, nh))
    x = (W - nw) // 2
    back.paste(img_f, (x, 0))
    # 底部字幕带
    BAND = 560
    cv = back.convert("RGBA")
    d = ImageDraw.Draw(cv)
    band = Image.new("RGBA", (W, BAND), (0, 0, 0, 0))
    ImageDraw.Draw(band).rectangle([0, 0, W, BAND], fill=(0, 0, 0, 175))
    cv.alpha_composite(band, (0, H - BAND))
    # 字幕加噪点打散 banding（字在噪点之后绘制，保持锐利）
    cv = add_dither(cv.convert("RGB")).convert("RGBA")
    d = ImageDraw.Draw(cv)
    f = fsize(56 if big else 52)
    y = H - BAND + 60
    for ln in sub_lines:
        for line in wrap(d, ln, f, W - 120):
            d.text((62, y + 2), line, font=f, fill=(0, 0, 0, 220), anchor="la")
            d.text((60, y), line, font=f, fill=(255, 255, 255, 255), anchor="la")
            y += 66
    if avatar is not None and is_cover:
        size = 200
        av = round_avatar(avatar, size, seal_rgb=seal_rgb)
        pad = 14
        bx, by = W - size - 50 - pad, 50 - pad
        plate = Image.new("RGBA", (size + 2 * pad, size + 2 * pad), (0, 0, 0, 0))
        ImageDraw.Draw(plate).rounded_rectangle([0, 0, size + 2 * pad - 1, size + 2 * pad - 1],
                                                radius=size // 2 + pad, fill=seal_rgb + (235,))
        cv.alpha_composite(plate, (bx, by))
        cv.alpha_composite(av, (bx + pad, by + pad))
    cv.convert("RGB").save(out_png)


async def tts(text, voice, out_mp3, retries=6):
    """edge-tts 带重试 + 零字节校验（2026-08-10 加固）。
    背景：canonical 原本无重试，edge-tts 服务端偋发 NoAudioReceived / 空文件，
    导致整个 Phase5.5 中断。此处指数退避重试，并在每次写盘后断言文件非空。"""
    last = None
    for attempt in range(retries):
        try:
            if os.path.exists(out_mp3):
                os.remove(out_mp3)
            await edge_tts.Communicate(text, voice).save(out_mp3)
            if os.path.exists(out_mp3) and os.path.getsize(out_mp3) > 1024:
                return
            last = RuntimeError(f"empty audio ({out_mp3})")
        except Exception as e:
            last = e
        await asyncio.sleep(min(2.0 * (attempt + 1), 8.0))
    raise RuntimeError(f"TTS failed after {retries} tries: {last}")


def dur(mp3):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                        "-of", "default=noprint_wrappers=1:nokey=1", mp3],
                       capture_output=True, text=True)
    return float(r.stdout.strip())


def build_video(name, scenes, voice, out_mp4, seal_rgb=(158, 43, 37)):
    avatar = get_avatar()
    TMP = tempfile.mkdtemp(prefix="vid_")
    manifests = []
    for i, sc in enumerate(scenes):
        mp3 = os.path.join(TMP, f"{name}_{i}.mp3")
        png = os.path.join(TMP, f"{name}_{i}.png")
        asyncio.run(tts(sc["narr"], voice, mp3))
        print(f"  [tts] {name}[{i}] {os.path.getsize(mp3)}B", flush=True)
        is_cover = (sc.get("bg") == "cover")
        make_scene(sc["bg"], sc["subs"], png, big=(name == "douyin"), avatar=avatar,
                   seal_rgb=seal_rgb, is_cover=is_cover)
        d = dur(mp3)
        seg = os.path.join(TMP, f"{name}_{i}.mp4")
        subprocess.run(["ffmpeg", "-y", "-loop", "1", "-i", png, "-i", mp3,
                        "-c:v", "libx264", "-tune", "stillimage", "-pix_fmt", "yuv420p",
                        "-c:a", "aac", "-b:a", "192k", "-shortest", "-t", str(d), "-r", str(FPS), seg],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        manifests.append(seg)
    listf = os.path.join(TMP, f"{name}_list.txt")
    with open(listf, "w") as f:
        for s in manifests:
            f.write(f"file '{s}'\n")
    subprocess.run(["ffmpeg", "-y", "-f", "concat", "-safe", "0", "-i", listf,
                    "-c", "copy", out_mp4], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"{name} -> {out_mp4}")


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--date", required=True)
    p.add_argument("--theme", required=True)
    p.add_argument("--deep", default="#9E2B25")
    p.add_argument("--scenes_vx", default="[]")
    p.add_argument("--scenes_dy", default="[]")
    p.add_argument("--scenes_xhs", default="[]")
    return p.parse_args()


# 片尾「AI 辅助创作」声明 scene（对齐《推荐运营规范》7.4 + 公众号 HTML 版 render_ai_note）
# 复用行动清单图 x3 作底，屏显两行 + 口播一行；仅通用已核实表述，不暴露不确定性
AI_NOTE_SCENE = [{
    "bg": "x3",
    "subs": ["本文由 AI 辅助创作", "政策以各地官方公告原文为准"],
    "narr": "本文由 AI 辅助创作，政策要点以各地官方公告原文为准。",
}]

# 片尾「转发给同行专家」提示 scene（2026-08-18 增补 · 对齐视频号小助手诊断「互动近零/缺社交传播动力」）
# 纯提示、不利益诱导（对齐 AGENTS.md 文末互动规则）；复用 x3 底，屏显一行 + 口播一句。
SHARE_HINT_SCENE = [{
    "bg": "x3",
    "subs": ["觉得有用？转发给同行专家 ↓"],
    "narr": "觉得有用，转发给身边同行的评标专家，一起把政策吃透。",
}]



if __name__ == "__main__":
    C = parse_args()
    D = os.path.join(SHARED, C.date)
    os.makedirs(D, exist_ok=True)
    prefix = f"{C.date}-"
    def _path(key):
        return os.path.join(D, f"{prefix}{key}-{C.theme}.png")
    # 素材映射:cover/x1/x2/x3
    cover = _path("竖版封面")
    x1 = _path("小红书配图1-首图")
    x2 = _path("小红书配图2-对比自查")
    x3 = _path("小红书配图3-行动清单")
    mapping = {"cover": cover, "x1": x1, "x2": x2, "x3": x3}
    seal_rgb = hex2rgb(C.deep)
    def _resolve(scenes):
        out = []
        for sc in scenes:
            bg = mapping.get(sc["bg"], sc["bg"])
            out.append({"bg": bg, "subs": sc["subs"], "narr": sc["narr"]})
        return out
    def _load(arg):
        if arg.startswith('@'):
            import os as _os
            with open(_os.path.join(SHARED, arg[1:]) if not _os.path.isabs(arg[1:]) else arg[1:], encoding='utf-8') as _f:
                return json.load(_f)
        return json.loads(arg)
    vx = _load(C.scenes_vx)
    dy = _load(C.scenes_dy)
    if vx:
        build_video("weishipin", _resolve(vx + AI_NOTE_SCENE + SHARE_HINT_SCENE), "zh-CN-YunxiNeural",
                    os.path.join(D, f"{prefix}微信视频号-视频-{C.theme}.mp4"), seal_rgb=seal_rgb)
    if dy:
        build_video("douyin", _resolve(dy + AI_NOTE_SCENE + SHARE_HINT_SCENE), "zh-CN-YunyangNeural",
                    os.path.join(D, f"{prefix}抖音-视频-{C.theme}.mp4"), seal_rgb=seal_rgb)
    xhs = _load(C.scenes_xhs)
    if xhs:
        # 小红书视频：复用竖版渲染（1080×1920 即小红书兼容尺寸），女声 Xiaoxiao 更贴合小红书受众
        build_video("xiaohongshu", _resolve(xhs + AI_NOTE_SCENE + SHARE_HINT_SCENE), "zh-CN-XiaoxiaoNeural",
                    os.path.join(D, f"{prefix}小红书-视频-{C.theme}.mp4"), seal_rgb=seal_rgb)
    if not vx and not dy and not xhs:
        print("无分镜 JSON,跳过视频合成(仅验证脚本可加载)")
    print("DONE")
