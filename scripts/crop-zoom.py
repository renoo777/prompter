"""把一张截图裁一块、放大若干倍，方便看清某条栏的细节。

为什么需要它：无头截图的输出尺寸是整窗的（例如 880x300），
而顶栏只有 38px 高 —— 直接看图时它会被缩放到看不清字形与间距。
放大只影响观感判断，**量化结论仍以 ui-preview.py --measure 的数字为准**。

用法：
  python scripts/crop-zoom.py in.png out.png --box 0,90,880,150 --zoom 3
"""

from __future__ import annotations

import argparse

from PIL import Image


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--box", required=True, help="left,top,right,bottom（原图像素）")
    ap.add_argument("--zoom", type=float, default=3.0)
    args = ap.parse_args()

    box = tuple(int(v) for v in args.box.split(","))
    if len(box) != 4:
        raise SystemExit("--box 需要 4 个数：left,top,right,bottom")

    im = Image.open(args.src).convert("RGB").crop(box)
    w, h = im.size
    im = im.resize((int(w * args.zoom), int(h * args.zoom)), Image.NEAREST)
    im.save(args.dst)
    print(f"crop {box} -> {args.dst} ({im.size[0]}x{im.size[1]})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
