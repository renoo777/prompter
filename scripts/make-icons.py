#!/usr/bin/env python3
"""从 `icon.ico` 生成 macOS / Linux 打包需要的 PNG 图标集。

    python scripts/make-icons.py

# 为什么非要有这个脚本

`src-tauri/icons/` 里**只有 `icon.ico`**（Windows 用的那份）。
Tauri 在 macOS 上打包时要求一组 PNG（`icon.icns` 由它自己从这些 PNG 合成），
缺了就是打包期报错 —— 而本项目的规矩是**永不产安装包**
（`tauri:build` = `tauri build --no-bundle`），所以这个错在开发机上
从来不会自己冒出来，只有真到 Mac 上出包那一刻才炸。

⇒ 用脚本生成，而不是手拖几个 PNG 进去：手拖的那份**没人知道它是从哪来的**，
下次换了品牌图标就对不上；脚本里写清了"源是 `icon.ico`"，重跑即可。

# 为什么尺寸是这几个

Tauri 官方模板用的就是这套（`32x32` 与 `128x128@2x` 覆盖 Dock 与 Finder，
`icon.png` 是 Linux / 通用入口，`StoreLogo` 与 `Square*Logo` 是 Windows Store）。
多出的尺寸不会被 macOS 用到，但**留着**是因为删掉它们会让
`tauri.conf.json` 的 `icon` 数组和官方模板分叉 —— 分叉本身比多几个文件更贵。

⚠️ 生成的 PNG 是**用现有 PNG 编码器**存的，不涉及重采样质量调优；
    `icon.ico` 里那一张是 256×256，所有尺寸都是它的等比缩放。
"""
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"
SRC = ICONS / "icon.ico"

# (文件名, 边长)。与 Tauri 官方 `tauri icon` 生成的清单一致。
TARGETS = [
    ("32x32.png", 32),
    ("128x128.png", 128),
    ("128x128@2x.png", 256),
    ("icon.png", 512),
    ("StoreLogo.png", 50),
    ("Square30x30Logo.png", 30),
    ("Square44x44Logo.png", 44),
    ("Square71x71Logo.png", 71),
    ("Square89x89Logo.png", 89),
    ("Square107x107Logo.png", 107),
    ("Square142x142Logo.png", 142),
    ("Square150x150Logo.png", 150),
    ("Square284x284Logo.png", 284),
    ("Square310x310Logo.png", 310),
]


def main() -> int:
    if os_name_nt():
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    try:
        from PIL import Image
    except ImportError:
        print("缺 Pillow：pip install pillow（或直接用已装好的那个解释器跑）")
        return 1

    if not SRC.exists():
        print(f"找不到源图标：{SRC}")
        return 1

    # ★ `.ico` 里通常有多张（16/32/48/256…）。`Image.open` 拿到的是**第一张**，
    #   而我们要的是**最大那张** —— 拿 16×16 去放大成 512 会糊成一团，
    #   而且这一点在缩略图上看不出来（小尺寸看不出，Dock 上一眼就是渣）。
    src = Image.open(SRC)
    best = src
    for i in range(getattr(src, "n_frames", 1)):
        src.seek(i)
        if src.size[0] > best.size[0]:
            best = src.copy() if i else src
    # `ico` 的 info 里有各尺寸清单，按它挑最大的那张更稳。
    sizes = src.info.get("sizes")
    if sizes:
        biggest = max(sizes)
        try:
            src.size = biggest
            best = src.copy()
        except Exception:
            pass

    print(f"源：{SRC.name} → 取到 {best.size[0]}×{best.size[1]}")
    if best.size[0] < 256:
        print("  ⚠️ 源图标最大只有 "
              f"{best.size[0]}px，放大会糊。建议换一份 ≥256 的源图。")

    written = 0
    for name, px in TARGETS:
        out = ICONS / name
        im = best.convert("RGBA").resize((px, px), Image.LANCZOS)
        im.save(out, "PNG")
        written += 1
    print(f"已写出 {written} 个 PNG → {ICONS}")
    print("  ★ Windows 的 `icon.ico` 原样保留（不要删，出包还在用）。")
    return 0


def os_name_nt() -> bool:
    import os
    return os.name == "nt"


if __name__ == "__main__":
    sys.exit(main())
