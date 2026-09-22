#!/usr/bin/env python3
"""
裁剪掉 ImageGen 生成的旧照片修复结果右下角的默认水印。

用法：
    python remove_watermark_crop.py <input.png/jpg> <output.png/jpg> [crop_height]

crop_height 默认 80 像素（1536x1024 输出时足够覆盖右下角水印区域）。
如果水印更大或更小，可按实际调整，例如 60 / 100。
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ModuleNotFoundError:
    print("Error: Pillow not installed. Run: pip install Pillow")
    raise SystemExit(1)


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        raise SystemExit(1)

    input_path = Path(sys.argv[1])
    output_path = Path(sys.argv[2])
    crop_height = int(sys.argv[3]) if len(sys.argv) > 3 else 80

    if not input_path.exists():
        print(f"Error: input file not found: {input_path}")
        raise SystemExit(1)

    with Image.open(input_path) as img:
        w, h = img.size
        if crop_height >= h:
            print(f"Error: crop_height ({crop_height}) must be smaller than image height ({h})")
            raise SystemExit(1)

        cropped = img.crop((0, 0, w, h - crop_height))
        output_path.parent.mkdir(parents=True, exist_ok=True)
        cropped.save(output_path)
        print(f"Cropped {w}x{h} -> {w}x{h - crop_height}, saved to {output_path}")


if __name__ == "__main__":
    main()
