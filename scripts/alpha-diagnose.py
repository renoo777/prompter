"""诊断不透明度反解异常：把每个像素簇拆开看，找出 α 偏差来自哪里。

不透明度反解 `α = (comp - bg) / (C - bg)` 的前提是：样本像素上的
「应用色 C」确实是我们以为的那个平铺背景。

一旦窗口里出现**大面积浅色内容**（比如空态提示、大字号标语、浅色卡片），
或者捕获到的区域里有**别的窗口压在提词器上面**，中位数就会离开背景团簇，
反解出来的 α 自然对不上 —— 这是**测量方法**的问题，不是应用透明度的问题。

本脚本对同一组截图做三件事：
  1. 打印每张图整体与分区域的亮度分布（直方图分位）；
  2. 在「背景近白」的像素上，打印合成图的**颜色分布**（不是只看中位数）；
  3. 用「中位数 / 众数 / 最低密度峰」三种估计分别反解 α，看它们是否收敛。

用法：
    python scripts/alpha-diagnose.py shots/mic
    python scripts/alpha-diagnose.py shots/release
"""

from __future__ import annotations

import collections
import os
import statistics
import sys

from PIL import Image, ImageChops, ImageStat

APP_BG_R = 14
CONFIG_ALPHA = 0.94


def gray(img: Image.Image) -> Image.Image:
    return img.convert("L")


def pct(img: Image.Image, ps=(5, 25, 50, 75, 95)) -> str:
    g = gray(img)
    hist = g.histogram()
    total = sum(hist)
    out = []
    acc = 0
    idx = 0
    for p in ps:
        target = total * p / 100.0
        while idx < 256 and acc + hist[idx] < target:
            acc += hist[idx]
            idx += 1
        out.append(f"p{p}={idx}")
    return "  ".join(out)


def main() -> int:
    d = sys.argv[1] if len(sys.argv) > 1 else "shots/mic"
    names = {
        "ref": "stealth_ref.png",
        "excluded": "stealth_excluded.png",
        "composite": "stealth_composite.png",
        "A": "stealth_A.png",
        "B": "stealth_B.png",
        "X": "stealth_X.png",
        "Y": "stealth_Y.png",
    }
    imgs = {}
    for k, n in names.items():
        p = os.path.join(d, n)
        if os.path.exists(p):
            imgs[k] = Image.open(p).convert("RGB")

    print(f"目录：{d}")
    print("-" * 72)
    for k, im in imgs.items():
        print(f"{k:<9} size={im.size}  亮度均值={ImageStat.Stat(gray(im)).mean[0]:6.1f}  {pct(im)}")

    # 两张候选图之间到底差多少（逐像素）
    if "X" in imgs and "Y" in imgs:
        diff = gray(ImageChops.difference(imgs["X"], imgs["Y"]))
        hist = diff.histogram()
        total = sum(hist)
        print("-" * 72)
        print("X vs Y 逐像素差分布：")
        for t in (2, 5, 10, 24, 48, 96):
            print(f"  差 >{t:>3}: {sum(hist[t:]) / total * 100:6.2f}%")
        print(f"  最大差 = {max(i for i, c in enumerate(hist) if c)}")

    if "ref" in imgs and "excluded" in imgs:
        diff = gray(ImageChops.difference(imgs["ref"], imgs["excluded"]))
        hist = diff.histogram()
        total = sum(hist)
        print("-" * 72)
        print("锚点(ref) vs 判定的被排除图 逐像素差分布：")
        for t in (2, 5, 10, 24):
            print(f"  差 >{t:>3}: {sum(hist[t:]) / total * 100:6.2f}%")
        print(f"  最大差 = {max(i for i, c in enumerate(hist) if c)}")

    if "composite" not in imgs or "excluded" not in imgs:
        print("\n缺少 composite / excluded，无法反解 α")
        return 1

    comp = imgs["composite"]
    bg = imgs["excluded"]

    cp = comp.load()
    bp = bg.load()
    w, h = comp.size
    samples = []
    for y in range(0, h):
        for x in range(0, w):
            br = bp[x, y][0]
            if 225 <= br <= 255:
                samples.append(cp[x, y][0])

    print("-" * 72)
    print(f"「背景近白」像素数：{len(samples)} / {w * h}（{len(samples) / (w * h) * 100:.1f}%）")
    if len(samples) < 500:
        print("样本太少，无法反解")
        return 1

    b_med = statistics.median(bp[x, y][0] for y in range(0, h) for x in range(0, w) if 225 <= bp[x, y][0] <= 255)

    # 合成图在这些像素上的红通道分布
    hist = collections.Counter(samples)
    print(f"\n合成图在该区域的 R 通道分布（前 12 个高频值）：")
    for v, c in hist.most_common(12):
        bar = "#" * max(1, int(c / len(samples) * 60))
        print(f"  R={v:>3}: {c / len(samples) * 100:6.2f}%  {bar}")

    # 只有「深色」样本才是真正的应用背景；浅色样本混进来会把中位数抬高
    dark = [s for s in samples if s <= 60]
    print(f"\n其中 R<=60 的深色样本占比：{len(dark) / len(samples) * 100:.1f}%")

    def solve(vals, b):
        med = statistics.median(vals)
        denom = APP_BG_R - b
        return (med - b) / denom if denom else None

    print("-" * 72)
    print(f"背景(bg) 中位数 = {b_med:.0f}；分母 = {APP_BG_R} - {b_med:.0f} = {APP_BG_R - b_med:.0f}")
    a_all = solve(samples, b_med)
    print(f"① 全部样本中位数      → R_med={statistics.median(samples):6.1f}  α ≈ {a_all:.3f}")
    if dark:
        a_dark = solve(dark, b_med)
        print(f"② 仅深色样本(R<=60)中位 → R_med={statistics.median(dark):6.1f}  α ≈ {a_dark:.3f}")
    mode = hist.most_common(1)[0][0]
    print(f"③ 众数(最高频值)      → R={mode:>3}      α ≈ {(mode - b_med) / (APP_BG_R - b_med):.3f}")
    print(f"\n配置 α = {CONFIG_ALPHA}")
    print("[判读] ② 与 ③ 若接近配置值，说明 ① 的偏差只是浅色像素混入样本造成的；")
    print("       若 ② ③ 也偏离配置值，才需要怀疑透明度真的算了不止一次。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
