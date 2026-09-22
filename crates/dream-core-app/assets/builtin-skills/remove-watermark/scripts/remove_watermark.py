#!/usr/bin/env python3
"""去除图片水印 / logo / 角标 / 文字。

提供两条路线：
  - 传统路线（默认，快）：OpenCV inpaint（Telea / Navier-Stokes），秒级出结果。
  - AI 路线（--ai，质量高）：LaMa inpainting 模型智能补全，适合大面积、复杂背景。

定位水印区域三选一：
  - --region fx,fy,fw,fh   归一化坐标（相对宽高 0~1 比例），Claude 驱动的主路径
  - --box x,y,w,h          绝对像素坐标
  - --gui                  交互式框选（OpenCV selectROI）

示例：
  python remove_watermark.py photo.png --info
  python remove_watermark.py photo.png --region 0.7,0.8,0.2,0.15 -o out.png
  python remove_watermark.py photo.png --box 1200,1800,400,250 --ai
"""

import argparse
import os
import sys

# 规避 Windows 下 torch 的 libiomp5md.dll 冲突（仅在加载 torch 前设置，无副作用）
os.environ.setdefault("KMP_DUPLICATE_LIB_OK", "TRUE")

# Windows 控制台默认 GBK，强制 UTF-8 输出避免中文乱码（失败则忽略）
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        try:
            _stream.reconfigure(encoding="utf-8")
        except Exception:
            pass

import numpy as np
from PIL import Image, ImageFilter


def load_pil(path):
    """读取图片并加载像素，返回 PIL Image。"""
    if not os.path.isfile(path):
        raise SystemExit(f"文件不存在: {path}")
    img = Image.open(path)
    img.load()
    return img


def print_info(path):
    img = load_pil(path)
    print(f"path:   {path}")
    print(f"width:  {img.width}")
    print(f"height: {img.height}")
    print(f"mode:   {img.mode}")
    print(f"format: {img.format}")


def resolve_box(img_w, img_h, region, box):
    """把归一化 / 像素坐标统一换算成像素 box (x, y, w, h)，并夹紧到图像范围内。"""
    if region:
        parts = region.split(",")
        if len(parts) != 4:
            raise SystemExit("--region 需要 4 个值: fx,fy,fw,fh")
        fx, fy, fw, fh = [float(p) for p in parts]
        x, y = int(fx * img_w), int(fy * img_h)
        w, h = int(fw * img_w), int(fh * img_h)
    elif box:
        parts = box.split(",")
        if len(parts) != 4:
            raise SystemExit("--box 需要 4 个值: x,y,w,h")
        x, y, w, h = [int(p) for p in parts]
    else:
        raise SystemExit("必须用 --region 或 --box 指定水印区域（或加 --gui 交互框选）")

    x = max(0, min(x, img_w - 1))
    y = max(0, min(y, img_h - 1))
    w = max(1, min(w, img_w - x))
    h = max(1, min(h, img_h - y))
    return x, y, w, h


def make_mask(img_w, img_h, x, y, w, h, pad):
    """生成 inpaint 掩码：水印区域（含外扩 pad）为 255，其余为 0。"""
    mask = np.zeros((img_h, img_w), dtype=np.uint8)
    x0 = max(0, x - pad)
    y0 = max(0, y - pad)
    x1 = min(img_w, x + w + pad)
    y1 = min(img_h, y + h + pad)
    mask[y0:y1, x0:x1] = 255
    return mask


def pil_to_bgr(img):
    """PIL -> numpy BGR 三通道（灰度/RGBA 都归一为 BGR，alpha 另存）。"""
    mode = img.mode
    arr = np.asarray(img)
    alpha = None
    if mode in ("RGBA", "LA", "P") or (mode == "RGB" and arr.shape[-1] == 4):
        rgba = img.convert("RGBA")
        alpha = np.asarray(rgba)[:, :, 3]
        rgb = rgba.convert("RGB")
    elif mode == "L":
        rgb = img.convert("RGB")
    else:
        rgb = img.convert("RGB")
    bgr = np.asarray(rgb)[:, :, ::-1].copy()
    return bgr, alpha


def bgr_to_pil(bgr, alpha, out_path):
    """numpy BGR (+可选 alpha) -> PIL Image，按输出后缀决定是否保留透明。"""
    rgb = bgr[:, :, ::-1]
    if alpha is not None:
        out = Image.fromarray(np.dstack([rgb, alpha]), "RGBA")
    else:
        out = Image.fromarray(rgb, "RGB")
    suffix = (os.path.splitext(out_path)[1] or "").lower()
    if suffix in (".jpg", ".jpeg") and out.mode == "RGBA":
        out = out.convert("RGB")
    return out


def inpaint_traditional(bgr, mask, method, radius):
    import cv2
    flag = cv2.INPAINT_TELEA if method == "telea" else cv2.INPAINT_NS
    return cv2.inpaint(bgr, mask, radius, flag)


def inpaint_ai(img_pil, mask):
    try:
        import torch
        from simple_lama_inpainting import SimpleLama
    except ImportError as e:
        raise SystemExit(
            "AI 路线需要 simple-lama-inpainting，请先安装（注意用 --no-deps 避免依赖冲突）:\n"
            "  python -m pip install simple-lama-inpainting --no-deps\n"
            f"（原始错误: {e}）"
        )
    # 强制 CPU：避免机器 GPU 架构与 torch 的 CUDA 版本不匹配时报 CUDA kernel 错误
    model = SimpleLama(device=torch.device("cpu"))  # 首次运行会自动下载 LaMa 模型（约 200MB）
    rgb_img = img_pil.convert("RGB")
    result = model(rgb_img, mask)  # 返回 PIL RGB Image
    return np.asarray(result)[:, :, ::-1].copy()  # RGB -> BGR


def fallback_pillow(img, x, y, w, h, pad):
    """cv2 缺失时的轻量兜底：高斯模糊覆盖水印区域（质量较低，仅应急）。"""
    x0 = max(0, x - pad)
    y0 = max(0, y - pad)
    x1 = min(img.width, x + w + pad)
    y1 = min(img.height, y + h + pad)
    crop = img.crop((x0, y0, x1, y1)).filter(ImageFilter.GaussianBlur(radius=max(6, w // 8)))
    out = img.copy()
    out.paste(crop, (x0, y0))
    return out


def select_roi_gui(bgr):
    import cv2
    roi = cv2.selectROI("框选水印区域后按 Enter（按 c 取消）", bgr, showCrosshair=True, fromCenter=False)
    cv2.destroyAllWindows()
    x, y, w, h = roi
    if w <= 0 or h <= 0:
        raise SystemExit("未选择有效区域，已取消")
    return x, y, w, h


def main(argv=None):
    ap = argparse.ArgumentParser(description="去除图片水印（传统 inpaint + AI LaMa）")
    ap.add_argument("image", help="输入图片路径")
    ap.add_argument("-o", "--output", help="输出图片路径（默认 <原名>_clean.<原后缀>）")
    ap.add_argument("--info", action="store_true", help="只打印图片信息（宽高/模式）后退出")
    ap.add_argument("--region", help="归一化区域 fx,fy,fw,fh，如 0.7,0.8,0.2,0.15")
    ap.add_argument("--box", help="像素区域 x,y,w,h")
    ap.add_argument("--gui", action="store_true", help="交互式框选水印区域")
    ap.add_argument("--method", choices=["telea", "ns", "ai"], default="telea",
                    help="抹除方法：telea（默认）/ ns / ai（LaMa）")
    ap.add_argument("--ai", action="store_true", help="使用 AI（LaMa）高质量修复，等价于 --method ai")
    ap.add_argument("--radius", type=int, default=3, help="传统 inpaint 半径（默认 3）")
    ap.add_argument("--pad", type=int, default=8, help="水印区域四周外扩像素（默认 8）")
    args = ap.parse_args(argv)

    if args.info:
        print_info(args.image)
        return 0

    img = load_pil(args.image)

    if args.gui:
        bgr, _ = pil_to_bgr(img)
        x, y, w, h = select_roi_gui(bgr)
    else:
        x, y, w, h = resolve_box(img.width, img.height, args.region, args.box)

    out_path = args.output
    if not out_path:
        base, ext = os.path.splitext(args.image)
        out_path = f"{base}_clean{ext or '.png'}"

    method = "ai" if args.ai else args.method

    # AI 路线
    if method == "ai":
        _, alpha = pil_to_bgr(img)
        mask = make_mask(img.width, img.height, x, y, w, h, args.pad)
        result = inpaint_ai(img, mask)
        out = bgr_to_pil(result, alpha, out_path)
        out.save(out_path)
        print(f"[AI/LaMa] 已去除水印 -> {out_path}")
        return 0

    # 传统路线
    try:
        import cv2  # noqa: F401
    except ImportError:
        print("警告: 未安装 opencv-python，使用 Pillow 高斯模糊兜底（质量较低）。", file=sys.stderr)
        out = fallback_pillow(img, x, y, w, h, args.pad)
        out.save(out_path)
        print(f"[fallback] 已处理（模糊覆盖）-> {out_path}")
        return 0

    bgr, alpha = pil_to_bgr(img)
    mask = make_mask(img.width, img.height, x, y, w, h, args.pad)
    result = inpaint_traditional(bgr, mask, method, args.radius)
    out = bgr_to_pil(result, alpha, out_path)
    out.save(out_path)
    print(f"[{method}] 已去除水印 -> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
