#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""video2ppt.py —— 把录制的PPT演示视频导出成图片型PPT + 便携HTML（含图形界面/命令行/自测）。"""

import argparse
import os
import sys
import time
import base64
import io
import threading

import cv2
import numpy as np
from PIL import Image
from pptx import Presentation
from pptx.util import Emu


def crop_black_borders(img, threshold=12):
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    mask = gray > threshold
    if not mask.any():
        return img
    ys, xs = np.where(mask)
    x0, x1 = int(xs.min()), int(xs.max())
    y0, y1 = int(ys.min()), int(ys.max())
    x0 = max(0, x0 - 2); y0 = max(0, y0 - 2)
    x1 = min(img.shape[1] - 1, x1 + 2); y1 = min(img.shape[0] - 1, y1 + 2)
    cropped = img[y0:y1 + 1, x0:x1 + 1]
    if (x1 - x0 + 1) * (y1 - y0 + 1) < 0.97 * img.shape[0] * img.shape[1]:
        return cropped
    return img


def auto_white_balance(img):
    try:
        return cv2.xphoto.createGrayworldWB().balanceWhite(img)
    except Exception:
        b, g, r = cv2.split(img)
        m_b, m_g, m_r = b.mean(), g.mean(), r.mean()
        mg = (m_b + m_g + m_r) / 3.0
        b = cv2.convertScaleAbs(b, alpha=mg / (m_b + 1e-6))
        g = cv2.convertScaleAbs(g, alpha=mg / (m_g + 1e-6))
        r = cv2.convertScaleAbs(r, alpha=mg / (m_r + 1e-6))
        return cv2.merge([b, g, r])


def enhance_contrast(img):
    lab = cv2.cvtColor(img, cv2.COLOR_BGR2LAB)
    l, a, b = cv2.split(lab)
    clahe = cv2.createCLAHE(clipLimit=2.2, tileGridSize=(8, 8))
    l = clahe.apply(l)
    lab = cv2.merge([l, a, b])
    return cv2.cvtColor(lab, cv2.COLOR_LAB2BGR)


def sharpen(img, amount=0.3, radius=0.5):
    """极轻微 USM 锐化，避免文字边缘出现 halo/白边。"""
    blurred = cv2.GaussianBlur(img, (0, 0), sigmaX=radius)
    return cv2.addWeighted(img, 1.0 + amount, blurred, -amount, 0)


def denoise(img):
    h, w = img.shape[:2]
    if h * w > 1920 * 1080:
        return cv2.GaussianBlur(img, (3, 3), 0.4)
    return cv2.bilateralFilter(img, d=5, sigmaColor=18, sigmaSpace=18)


def upscale_if_small(img, min_side=1280):
    h, w = img.shape[:2]
    short = min(h, w)
    if short < min_side:
        scale = min(min_side / float(short), 2.0)
        nh, nw = int(round(h * scale)), int(round(w * scale))
        return cv2.resize(img, (nw, nh), interpolation=cv2.INTER_CUBIC)
    return img


def clean_white_bg(img, thr=248):
    """把接近纯白的背景像素拉回纯白，去掉压缩噪点/灰底带来的脏色（保留浅色图表）。"""
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    mask = gray > thr
    if mask.any():
        img = img.copy()
        img[mask] = (255, 255, 255)
    return img


def enhance_contrast_soft(img):
    """轻量对比度增强，仅让文字更清晰，低 clip 避免引入噪点/色块。"""
    lab = cv2.cvtColor(img, cv2.COLOR_BGR2LAB)
    l, a, b = cv2.split(lab)
    clahe = cv2.createCLAHE(clipLimit=1.3, tileGridSize=(8, 8))
    l = clahe.apply(l)
    lab = cv2.merge([l, a, b])
    return cv2.cvtColor(lab, cv2.COLOR_LAB2BGR)


def denoise_soft(img):
    """仅对超大图做极轻高斯，避免糊字；小图不动。"""
    h, w = img.shape[:2]
    if h * w > 1920 * 1080:
        return cv2.GaussianBlur(img, (1, 1), 0.3)
    return img


def enhance(img, quality="compress"):
    img = crop_black_borders(img)
    # PPT 录屏自带标准色彩，不做全局白平衡（否则背景/图表偏色）
    img = clean_white_bg(img)
    img = enhance_contrast_soft(img)
    img = denoise_soft(img)
    img = sharpen(img, amount=0.8)
    if quality == "compress":
        h, w = img.shape[:2]
        if w > 1920:
            scale = 1920.0 / w
            img = cv2.resize(img, (1920, int(round(h * scale))), interpolation=cv2.INTER_AREA)
    return img


def dhash_gray(gray, hash_size=8):
    small = cv2.resize(gray, (hash_size + 1, hash_size), interpolation=cv2.INTER_AREA)
    return small[:, 1:] > small[:, :-1]


def scene_detect(video_path, sample_sec=0.3, thresh=12, pix_thresh=8.0, min_dur=2.5):
    """切页检测。
    用 cap.grab() 跳帧解码（只对每 sample_sec 的采样帧做 decode），速度提升数倍；
    过滤/合并 <min_dur 的超短场景，去除动画过渡、鼠标高亮等瞬时帧造成的误切。
    """
    cap = cv2.VideoCapture(video_path)
    if not cap.isOpened():
        raise RuntimeError("无法打开视频：" + video_path)
    fps = cap.get(cv2.CAP_PROP_FPS) or 25.0
    total = int(cap.get(cv2.CAP_PROP_FRAME_COUNT) or 0)
    print("[info] fps=%.2f 总帧数约=%d" % (fps, total), flush=True)

    sample_interval = max(1, int(round(fps * sample_sec)))
    hash_size = 8
    small_size = (64, 36)
    scenes = []
    prev_hash = None
    prev_small = None
    cur_scene = None
    frame_idx = 0
    t0 = time.time()

    while True:
        if not cap.grab():
            break
        if frame_idx % sample_interval == 0:
            ret, frame = cap.retrieve()
            if not ret:
                break
            gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)
            h = dhash_gray(gray, hash_size)
            small = cv2.resize(frame, small_size).astype(np.float32)
            t = frame_idx / fps
            if cur_scene is None:
                # frames 只存轻量 (frame_idx, t)，不存全尺寸图，避免大视频 OOM
                cur_scene = {"start_frame": frame_idx, "start_time": t, "frames": [(frame_idx, t)]}
            else:
                dist = int(np.sum(prev_hash != h)) if prev_hash is not None else 0
                pdiff = float(np.mean(np.abs(small - prev_small))) if prev_small is not None else 999.0
                if pdiff > pix_thresh or dist > thresh:
                    cur_scene["end_time"] = t
                    scenes.append(cur_scene)
                    cur_scene = {"start_frame": frame_idx, "start_time": t, "frames": [(frame_idx, t)]}
                else:
                    cur_scene["frames"].append((frame_idx, t))
            prev_hash = h
            prev_small = small
        frame_idx += 1
        if frame_idx % 3000 == 0:
            el = time.time() - t0
            print("  ..解码进度 %d/%d  %.0f fps" % (frame_idx, total, frame_idx / el), flush=True)

    if cur_scene is not None:
        cur_scene["end_time"] = frame_idx / fps
        scenes.append(cur_scene)
    cap.release()

    # 过滤极短场景（闪烁/瞬时）
    scenes = [s for s in scenes if (s["end_time"] - s["start_time"]) >= 0.4]
    # 合并超短场景到前一个（动画过渡/鼠标高亮瞬态误判）
    merged = []
    for s in scenes:
        if (s["end_time"] - s["start_time"]) < min_dur and merged:
            merged[-1]["end_time"] = s["end_time"]
            merged[-1]["frames"].extend(s["frames"])
        else:
            merged.append(s)
    scenes = merged

    print("[info] 检测到 %d 页" % len(scenes), flush=True)
    return scenes, fps


def pick_representative(scene):
    frames = scene["frames"]
    idx_pos = min(len(frames) - 1, max(0, int(len(frames) * 0.66)))
    return frames[idx_pos]  # (frame_idx, t)


HTML_TEMPLATE = """<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__</title>
<style>
  html,body{margin:0;height:100%;background:#000;overflow:hidden;
    font-family:-apple-system,"PingFang SC","Microsoft YaHei",sans-serif;}
  #stage{position:fixed;inset:0;display:flex;align-items:center;justify-content:center;background:#000;}
  #img{width:100%;height:100%;object-fit:contain;display:none;}
  #bar{position:fixed;left:0;right:0;bottom:0;display:flex;gap:10px;align-items:center;
    justify-content:center;padding:10px;background:rgba(0,0,0,.45);color:#fff;font-size:14px;
    opacity:.12;transition:opacity .25s;}
  #bar:hover{opacity:1;}
  #bar button{background:#222;color:#fff;border:1px solid #555;border-radius:6px;
    padding:6px 12px;cursor:pointer;font-size:14px;}
  #bar button:hover{background:#3a3a3a;}
  #page{padding:0 8px;min-width:70px;text-align:center;}
  #hint{position:fixed;top:10px;right:12px;color:#fff;opacity:.35;font-size:12px;user-select:none;}
  #title{position:fixed;top:10px;left:12px;color:#fff;opacity:.5;font-size:13px;
    max-width:60%;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;}
  .prog{position:fixed;top:0;left:0;height:3px;background:#3b82f6;width:0;}
</style>
</head>
<body>
<div class="prog" id="prog"></div>
<div id="title">__TITLE__</div>
<div id="stage"><img id="img" alt="slide"></div>
<div id="hint"><- -> 翻页 . 空格 播放/暂停 . F 全屏 . 双击全屏</div>
<div id="bar">
  <button id="prev"><- 上一页</button>
  <button id="play">|| 暂停</button>
  <span id="page">1 / 1</span>
  <button id="next">下一页 -></button>
</div>
<script>
const DATA = __DATA__;
const imgs = DATA.imgs, dur = DATA.dur, title = DATA.title;
let i = 0, playing = true, timer = null, startTs = 0;
const el = document.getElementById('img');
const pageEl = document.getElementById('page');
const playBtn = document.getElementById('play');
const prog = document.getElementById('prog');
function show(n){
  i = Math.max(0, Math.min(imgs.length-1, n));
  el.src = imgs[i]; el.style.display='block';
  pageEl.textContent = (i+1)+' / '+imgs.length;
  resetTimer();
}
function resetTimer(){
  if(timer){clearTimeout(timer);timer=null;}
  prog.style.width='0%';
  if(playing){
    const ms = Math.max(800, dur[i]*1000);
    startTs = performance.now();
    timer = setTimeout(function(){show(i+1);}, ms);
    requestAnimationFrame(tick);
  }
}
function tick(){
  if(!playing) return;
  const ms = Math.max(800, dur[i]*1000);
  const e = performance.now()-startTs;
  prog.style.width = Math.min(100,(e/ms)*100)+'%';
  if(playing) requestAnimationFrame(tick);
}
function play(){ playing=true; playBtn.textContent='|| 暂停'; resetTimer(); }
function pause(){ playing=false; playBtn.textContent='> 播放'; if(timer){clearTimeout(timer);timer=null;} }
function toggle(){ playing?pause():play(); }
document.getElementById('prev').onclick=function(){pause();show(i-1);};
document.getElementById('next').onclick=function(){pause();show(i+1);};
playBtn.onclick=toggle;
document.addEventListener('keydown',function(e){
  if(e.key==='ArrowRight'){pause();show(i+1);}
  else if(e.key==='ArrowLeft'){pause();show(i-1);}
  else if(e.key===' '){e.preventDefault();toggle();}
  else if(e.key==='Home'){pause();show(0);}
  else if(e.key==='End'){pause();show(imgs.length-1);}
  else if(e.key.toLowerCase()==='f'){toggleFull();}
});
function toggleFull(){ if(!document.fullscreenElement) document.documentElement.requestFullscreen(); else document.exitFullscreen(); }
document.getElementById('stage').ondblclick=toggleFull;
show(0);
play();
</script>
</body>
</html>"""


def build_html(slides_b64, durations, title):
    import json
    data = {"imgs": slides_b64, "dur": [round(float(d), 1) for d in durations], "title": title}
    js = json.dumps(data, ensure_ascii=False)
    safe_title = title.replace('"', "&quot;")
    return HTML_TEMPLATE.replace("__DATA__", js).replace("__TITLE__", safe_title)


def run_pipeline(video_path, out_dir=None, sample=0.3, thresh=12,
                 keep_png=False, keep_html=False, quality="compress",
                 dedup=True, dedup_thresh=5.0):
    if not os.path.isfile(video_path):
        raise RuntimeError("找不到视频文件：" + video_path)

    out_dir = out_dir or (os.path.splitext(video_path)[0] + "_ppt")
    os.makedirs(out_dir, exist_ok=True)
    slides_dir = os.path.join(out_dir, "slides")
    if keep_png:
        os.makedirs(slides_dir, exist_ok=True)

    scenes, fps = scene_detect(video_path, sample, thresh)

    cap = cv2.VideoCapture(video_path)
    frames = []
    durations = []
    aspect_w, aspect_h = None, None
    for i, scene in enumerate(scenes):
        fidx, t = pick_representative(scene)
        cap.set(cv2.CAP_PROP_POS_FRAMES, fidx)
        ret, frame = cap.read()
        if not ret:
            continue
        frame = enhance(frame, quality)
        rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
        if keep_png:
            p = os.path.join(slides_dir, "slide_%03d.png" % (i + 1))
            Image.fromarray(rgb).save(p, "PNG")
        frames.append(rgb)
        durations.append(scene["end_time"] - scene["start_time"])
        if aspect_w is None:
            aspect_h, aspect_w = frame.shape[:2]
        print("  页 %3d: t=%7.2f s  时长=%5.1f s  尺寸=%dx%d" % (i + 1, t, durations[-1], frame.shape[1], frame.shape[0]))
    cap.release()

    if not frames:
        raise RuntimeError("没有提取到任何页，请调小 --thresh 重试。")

    # 合并相邻高度相似页（演讲者回翻、动画抖动等造成的重复帧）
    if dedup and len(frames) > 1:
        final_frames = [frames[0]]
        final_durations = [durations[0]]
        merged_count = 0
        for rgb, dur in zip(frames[1:], durations[1:]):
            prev = cv2.resize(cv2.cvtColor(final_frames[-1], cv2.COLOR_RGB2GRAY), (32, 18)).astype(float)
            cur = cv2.resize(cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY), (32, 18)).astype(float)
            if np.mean(np.abs(prev - cur)) < dedup_thresh:
                final_durations[-1] += dur
                merged_count += 1
                continue
            final_frames.append(rgb)
            final_durations.append(dur)
        if merged_count:
            print("[info] 相邻相似页合并 %d 页，去重后剩余 %d 页" % (merged_count, len(final_frames)))
        frames = final_frames
        durations = final_durations

    prs = Presentation()
    sw, sh = aspect_w, aspect_h
    base_w_in = 13.333
    ratio = sw / sh
    if ratio >= 1:
        slide_w_in, slide_h_in = base_w_in, base_w_in / ratio
    else:
        slide_h_in, slide_w_in = base_w_in, base_w_in * ratio
    prs.slide_width = Emu(int(slide_w_in * 914400))
    prs.slide_height = Emu(int(slide_h_in * 914400))
    quality_cfg = {
        "compress": ("JPEG", {"quality": 92}),
        "high": ("JPEG", {"quality": 98}),
        "full": ("PNG", {}),
        "lossless": ("PNG", {}),
    }
    fmt, kw = quality_cfg.get(quality, ("JPEG", {"quality": 92}))

    blank = prs.slide_layouts[6]
    for rgb in frames:
        s = prs.slides.add_slide(blank)
        buf = io.BytesIO()
        Image.fromarray(rgb).save(buf, fmt, **kw)
        buf.seek(0)
        s.shapes.add_picture(buf, 0, 0, width=prs.slide_width, height=prs.slide_height)

    stem = os.path.splitext(os.path.basename(video_path))[0]
    out_pptx = os.path.join(out_dir, stem + "_导出.pptx")
    prs.save(out_pptx)

    out_html = ""
    if keep_html:
        out_html = os.path.join(out_dir, stem + "_便携放映.html")
        slides_b64 = []
        for rgb in frames:
            b = io.BytesIO()
            Image.fromarray(rgb).save(b, "PNG")
            slides_b64.append("data:image/png;base64," + base64.b64encode(b.getvalue()).decode())
        with open(out_html, "w", encoding="utf-8") as f:
            f.write(build_html(slides_b64, durations, stem))

    print("[完成] 共 %d 页" % len(frames))
    print("  PPTX(可编辑): %s" % out_pptx)
    if keep_png:
        print("  图片目录: %s" % slides_dir)
    if keep_html:
        print("  HTML(便携零安装): %s" % out_html)
    return {"n": len(frames), "pptx": out_pptx, "html": out_html,
            "slides": (slides_dir if keep_png else "")}


def _no_gui_fallback(video=None):
    """本机无 tkinter 时兜底：用 ctypes 弹原生对话框反馈（--windowed 下 print 不可见）。"""
    import ctypes
    MB_OK = 0x40
    if video:
        try:
            info = run_pipeline(video, None, 0.3, 12, quality="lossless")
            msg = f"完成！共 {info['n']} 页\n\nPPTX: {info['pptx']}\n"
            if info.get("html"):
                msg += f"HTML: {info['html']}\n"
            msg += "\n输出目录已生成。"
            ctypes.windll.user32.MessageBoxW(0, msg, "视频转PPT", MB_OK)
        except Exception as e:
            ctypes.windll.user32.MessageBoxW(0,
                f"出错：{e}", "视频转PPT", MB_OK)
    else:
        ctypes.windll.user32.MessageBoxW(0,
            "图形界面不可用（本机 Python 未带 tkinter）。\n\n"
            "两种用法（任选其一）：\n"
            "1. 把视频文件直接拖到这个 exe 上即可\n"
            "2. 命令行：  视频转PPT.exe  视频路径\n\n"
            "产物在视频同级的「视频名_ppt」文件夹里：可编辑 PPTX（默认无损画质）。\n"
            "想更小体积可在命令行加 --quality compress（JPEG限宽1920）。",
            "视频转PPT", MB_OK)


def main_cli():
    ap = argparse.ArgumentParser(description="视频转PPT（图片型PPT，默认压缩档）")
    ap.add_argument("video", nargs="?", help="视频文件路径")
    ap.add_argument("--out", default=None, help="输出目录")
    ap.add_argument("--sample", type=float, default=0.3, help="采样间隔(秒)")
    ap.add_argument("--thresh", type=int, default=12, help="切页灵敏度")
    ap.add_argument("--quality", default="compress", choices=["compress", "high", "full", "lossless"],
                    help="compress=JPEG92限宽1920(默认,体积小); high=JPEG98限宽1920(更清晰); full=原分辨率PNG; lossless=PNG限宽1920(推荐,清晰度最高)")
    ap.add_argument("--keep-png", action="store_true", help="同时导出每页PNG图片")
    ap.add_argument("--keep-html", action="store_true", help="同时导出便携HTML放映件")
    ap.add_argument("--dedup", action=argparse.BooleanOptionalAction, default=True,
                    help="合并相邻高度相似页(默认开启); --no-dedup 关闭")
    args = ap.parse_args()
    if not args.video:
        ap.print_help()
        sys.exit(1)
    info = run_pipeline(args.video, args.out, args.sample, args.thresh,
                        keep_png=args.keep_png, keep_html=args.keep_html, quality=args.quality,
                        dedup=args.dedup)
    print("输出目录:", os.path.dirname(info["pptx"]))


def gui_main(video=None):
    try:
        import tkinter as tk
        from tkinter import filedialog, messagebox, scrolledtext
    except ImportError:
        _no_gui_fallback(video)
        return

    root = tk.Tk()
    root.title("视频转PPT . 便携版")
    root.geometry("580x540")

    v_video = tk.StringVar(value=video or "")
    v_out = tk.StringVar(value="")
    v_thresh = tk.StringVar(value="12")
    v_sample = tk.StringVar(value="0.3")

    def choose_video():
        p = filedialog.askopenfilename(title="选择视频",
            filetypes=[("视频", "*.mp4 *.avi *.mov *.mkv *.wmv *.flv *.mpeg *.mpg"), ("全部", "*.*")])
        if p:
            v_video.set(p)
            v_out.set(os.path.splitext(p)[0] + "_ppt")

    def choose_out():
        d = filedialog.askdirectory(title="选择输出目录")
        if d:
            v_out.set(d)

    f = tk.Frame(root); f.pack(fill="x", padx=10, pady=10)
    tk.Label(f, text="视频文件：").grid(row=0, column=0, sticky="w")
    tk.Entry(f, textvariable=v_video, width=46).grid(row=0, column=1)
    tk.Button(f, text="浏览", command=choose_video).grid(row=0, column=2)
    tk.Label(f, text="输出目录：").grid(row=1, column=0, sticky="w")
    tk.Entry(f, textvariable=v_out, width=46).grid(row=1, column=1)
    tk.Button(f, text="浏览", command=choose_out).grid(row=1, column=2)
    tk.Label(f, text="灵敏度(--thresh)：").grid(row=2, column=0, sticky="w")
    tk.Entry(f, textvariable=v_thresh, width=10).grid(row=2, column=1, sticky="w")
    tk.Label(f, text="采样间隔(--sample秒)：").grid(row=3, column=0, sticky="w")
    tk.Entry(f, textvariable=v_sample, width=10).grid(row=3, column=1, sticky="w")
    tk.Button(f, text="开始转换", command=lambda: start()).grid(row=4, column=1, sticky="w", pady=6)

    log = scrolledtext.ScrolledText(root, height=18)
    log.pack(fill="both", expand=True, padx=10, pady=6)

    class _W:
        def write(self, t):
            log.insert(tk.END, t); log.see(tk.END)
        def flush(self):
            pass
    old_stdout = sys.stdout
    sys.stdout = _W()

    running = {"ok": False}

    def start():
        if running.get("ok"):
            return
        vp = v_video.get().strip()
        if not vp or not os.path.isfile(vp):
            messagebox.showerror("提示", "请先选择有效的视频文件")
            return
        running["ok"] = True

        def job():
            try:
                info = run_pipeline(vp, v_out.get().strip() or None,
                                    float(v_sample.get() or 0.3), int(v_thresh.get() or 12),
                                    quality="lossless")
                messagebox.showinfo("完成",
                    "共 %d 页\n\nPPTX: %s\n\n默认无损画质（PNG 限宽1920）。" % (info["n"], info["pptx"]))
            except Exception as e:
                messagebox.showerror("出错", str(e))
            finally:
                sys.stdout = old_stdout
                running["ok"] = False

        threading.Thread(target=job, daemon=True).start()

    if video:
        v_out.set(os.path.splitext(video)[0] + "_ppt")
        root.after(400, start)

    root.mainloop()


def self_test():
    wd = os.getcwd()
    vid = os.path.join(wd, "_selftest.avi")
    vw = cv2.VideoWriter(vid, cv2.VideoWriter_fourcc(*"MJPG"), 25, (320, 180))
    for c, d in [([60, 30, 200], 3.0), ([30, 160, 60], 4.0), ([200, 160, 30], 3.0)]:
        arr = np.array(c, np.uint8)
        for _ in range(int(d * 25)):
            vw.write(np.full((180, 320, 3), arr, np.uint8))
    vw.release()
    out = os.path.join(wd, "_selftest_out")
    info = run_pipeline(vid, out, 0.3, 12, keep_html=True)
    ok = (info["n"] == 3 and os.path.exists(info["pptx"]) and os.path.exists(info["html"]))
    res = "SELFTEST %s pages=%d pptx=%s html=%s" % ("OK" if ok else "FAIL", info["n"], os.path.exists(info["pptx"]), os.path.exists(info["html"]))
    with open(os.path.join(wd, "_selftest_result.txt"), "w", encoding="utf-8") as fh:
        fh.write(res + "\n")
    try:
        import shutil
        shutil.rmtree(out); os.remove(vid)
    except Exception:
        pass
    print(res)


if __name__ == "__main__":
    # PyInstaller --windowed 打包后 stdout/stderr 可能是 None 或编码非 UTF-8，
    # 统一修复：重定向到 UTF-8，避免命令行模式下中文日志乱码。
    for _s in (sys.stdout, sys.stderr):
        try:
            if _s is not None and hasattr(_s, "reconfigure"):
                _s.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    if sys.stdout is None:
        sys.stdout = open(os.devnull, "w")
    if sys.stderr is None:
        sys.stderr = open(os.devnull, "w")

    argv = sys.argv[1:]
    if "--selftest" in argv:
        self_test()
        sys.exit(0)

    video_arg = None
    for a in argv:
        if not a.startswith("-") and os.path.isfile(a):
            video_arg = a
            break

    if video_arg and len(argv) == 1:
        # 只有视频路径：拖拽/双击模式 → 走 GUI（无 GUI 时兜底弹窗）
        gui_main(video_arg)
    elif video_arg:
        # 带命令行参数（--out/--quality 等）→ 走命令行模式，参数生效
        main_cli()
    elif any(a in ("-h", "--help") for a in argv):
        main_cli()
    else:
        gui_main()
