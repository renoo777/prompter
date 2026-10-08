"""从未隐身(A) / 隐身(B) 两张同位置截图反解「窗口实际不透明度」。

原理：
    隐身关闭时，屏幕捕获到的是合成结果  A = α·C + (1-α)·B
    隐身开启时，窗口被排除，捕获到的是纯背景  B
    于是逐像素可解出  α = (A - B) / (C - B)

    其中 C 是应用在该像素上的颜色。取 B 为接近纯白（背后是白底窗口）的像素，
    再取 A 的**中位数**：这些像素里绝大多数是应用的平铺背景（深色 rgb(14,17,22)），
    文字像素只占少数，中位数就能稳定落在「背景团簇」上。

用法：
    python scripts/measure-alpha.py shots/release
"""

from __future__ import annotations

import json
import os
import statistics
import sys

from PIL import Image

APP_BG_R, APP_BG_G, APP_BG_B = 14, 17, 22  # 与 styles.css 的 --bg 一致

# 应用把窗口不透明度持久化在这里（Tauri identifier = cn.bobby.prompter）。
CONFIG_PATH = os.path.join(os.environ.get("APPDATA", ""), "cn.bobby.prompter", "config.json")
DEFAULT_ALPHA = 0.94  # 与 AppConfig::default().opacity 一致


def configured_alpha(path: str | None = None) -> tuple[float, str]:
    """读**持久化配置**里的真实不透明度，而不是拿默认值硬比。

    这一步非有不可：设置抽屉里的滑杆会把值写进 config.json，应用每次启动都按
    那个值渲染。脚本若仍拿默认的 0.94 去比，就会把「应用忠实地按 0.3 渲染」
    误判成「透明度算了两遍」。这个坑真实踩过 —— 反解出 0.303，查了半天代码，
    最后一读 config.json 才发现里面写着 "opacity": 0.3。
    """
    p = path or CONFIG_PATH
    try:
        with open(p, encoding="utf-8") as f:
            v = float(json.load(f).get("opacity", DEFAULT_ALPHA))
        return v, p
    except FileNotFoundError:
        return DEFAULT_ALPHA, f"默认值（没有 {p}）"
    except Exception as e:  # 配置写坏也不该让脚本崩
        return DEFAULT_ALPHA, f"默认值（读配置失败：{e}）"



def main() -> int:
    argv = sys.argv[1:]
    cfg = None
    if "--config" in argv:
        i = argv.index("--config")
        cfg = argv[i + 1] if i + 1 < len(argv) else None
        del argv[i:i + 2]
    d = argv[0] if argv else "shots/release"
    pa = os.path.join(d, "stealth_A.png")
    pb = os.path.join(d, "stealth_B.png")

    if not (os.path.exists(pa) and os.path.exists(pb)):
        print(f"找不到 {pa} 或 {pb}")
        return 2

    a = Image.open(pa).convert("RGB")
    b = Image.open(pb).convert("RGB")
    if a.size != b.size:
        print(f"尺寸不一致：{a.size} vs {b.size}")
        return 1

    ap = a.load()
    bp = b.load()
    w, h = a.size

    samples = []  # (A_R, B_R)
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            ar, _, _ = ap[x, y]
            br, _, _ = bp[x, y]
            # 只取「背后接近纯白」的像素，保证 C 是应用的深色背景
            if 225 <= br <= 255:
                samples.append((ar, br))

    if len(samples) < 500:
        print(f"可用样本太少（{len(samples)}），换一组截图或手动指定区域")
        return 1

    a_med = statistics.median(s[0] for s in samples)
    b_med = statistics.median(s[1] for s in samples)

    denom = APP_BG_R - b_med
    if denom == 0:
        print("分母为 0，无法反解")
        return 1
    alpha = (a_med - b_med) / denom
    config_alpha, alpha_src = configured_alpha(cfg)

    print(f"样本数            : {len(samples)}")
    print(f"A（合成）通道中位 : {a_med:.1f}")
    print(f"B（纯背景）中位   : {b_med:.1f}")
    print(f"应用背景假定      : rgb({APP_BG_R},{APP_BG_G},{APP_BG_B})")
    print()
    print(f"反解出的实际不透明度 α ≈ {alpha:.3f}")
    print(f"配置里的期望值     : {config_alpha}   ← 来自 {alpha_src}")

    gap = abs(alpha - config_alpha)
    if gap <= 0.05:
        print("[结论] 一致：透明度只生效了一次，配置值与实际观感对得上。")
        return 0
    if alpha < config_alpha - 0.05:
        print(f"[结论] 实际比配置更透（差 {gap:.3f}）。")
        print("       先看两件事，再怀疑代码：")
        print("       ① config.json 里的 opacity 是不是被滑杆改过（本脚本已自动读取）；")
        print("       ② 背景是不是**另一个带透明图层的窗口**（WorkBuddy 之类）——")
        print("          多层透明叠加会让反解失准，改用 scripts/alpha-probe.py 铺纯色底重测。")
        return 0
    print(f"[结论] 实际比配置更实（差 {gap:.3f}），可能是样本混入了文字像素。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
