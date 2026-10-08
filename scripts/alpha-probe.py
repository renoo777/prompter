"""在受控纯色背景上，判定提词器窗口的**实际**不透明度。

# 为什么要这个脚本

`measure-alpha.py` 只有两张图、且假设「窗口在该像素上画的是 rgb(14,17,22)」。
一旦背后是 WorkBuddy 这种自身也带图层的窗口，假设就崩，反解出的 α 毫无意义
（实测反解出 0.303 —— 一个既不是 0.94 也不是任何配置值的数）。

这里换掉那条脆弱假设：**把桌面铺成一整块已知纯色**，所有量就都已知了。

    CSS 里 .app 画的是   C = 0.94·(14,17,22) + 0.06·背景
    屏幕上该出现       P = β·C + (1-β)·背景          β 是窗口级不透明度

背景取洋红 (255,0,255)，因为它在 UI 里绝不会自然出现，两类结论读数差得极远：

| 情形 | 屏幕上的样子 |
|---|---|
| 正常（β=1） | 深色 ~(28, 16, 36)，几乎不洋红 |
| 窗口被多乘一层 β≈0.3 | 明显发亮发洋红 ~(183, 5, 186) |
| α 被乘了两遍（β=0.94） | 深色偏洋红 ~(42, 15, 40) |

# 两个必须记住的坑（都实际踩过）

1. **应用默认自己隐身**（`AppConfig::default().stealth_on_start == true`）。
   直接截屏只会得到纯背景，看起来像「窗口什么都没画」。所以本脚本会先发一次
   `Ctrl+Alt+S` 把隐身关掉，再截。
2. **切换隐身之后不要立刻截**。DWM 重新合成需要时间，太早截会拍到一张
   「半透明鬼影」，从而误判成「窗口真的被降了透明度」。
   本脚本会在切换后**连拍三张**（+0.8s / +2.5s / +5.5s），若第一张淡、后面变实，
   那就只是过渡假象。

用法：
    python scripts/alpha-probe.py                # 铺洋红底 → 起应用 → 截三张 → 收摊
    python scripts/alpha-probe.py --no-bg        # 不铺底，直接在现有桌面上量
    python scripts/alpha-probe.py --keep         # 保留进程，方便人眼看
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time

from PIL import Image, ImageGrab

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PROMPTER_EXE = os.path.join(ROOT, "src-tauri", "target", "release", "prompter.exe")

APP_BG = (14, 17, 22)
CSS_ALPHA = 0.94
BG_MAGENTA = (255, 0, 255)

VK_CONTROL, VK_MENU, KEYEVENTF_KEYUP = 0x11, 0x12, 0x0002
LETTER_VK = {c: 0x41 + i for i, c in enumerate("ABCDEFGHIJKLMNOPQRSTUVWXYZ")}


def set_dpi_aware() -> None:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:
            pass


def send_hotkey(vk: int) -> None:
    u = ctypes.windll.user32
    u.keybd_event(VK_CONTROL, 0, 0, 0)
    u.keybd_event(VK_MENU, 0, 0, 0)
    u.keybd_event(vk, 0, 0, 0)
    time.sleep(0.06)
    u.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)
    u.keybd_event(VK_MENU, 0, KEYEVENTF_KEYUP, 0)
    u.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)


def find_window(prefix: str = "Prompter"):
    u = ctypes.windll.user32
    found = []
    CB = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _lp):
        if not u.IsWindowVisible(hwnd):
            return True
        n = u.GetWindowTextLengthW(hwnd)
        if n <= 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(hwnd, buf, n + 1)
        if not buf.value.strip().lower().startswith(prefix.lower()):
            return True
        r = wt.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(r))
        if r.right - r.left > 0 and r.bottom - r.top > 0:
            found.append((hwnd, buf.value.strip(), r))
        return True

    u.EnumWindows(CB(cb), 0)
    return found


def dominant(img: Image.Image, quant: int = 8, top: int = 5):
    """最高频的若干颜色（量化合并噪声）→ [((r,g,b), 占比%)]。"""
    small = img.convert("RGB")
    if small.width > 700:
        small = small.resize((small.width // 2, small.height // 2))
    px = small.load()
    counter: dict[tuple[int, int, int], int] = {}
    for y in range(0, small.height, 2):
        for x in range(0, small.width, 2):
            r, g, b = px[x, y]
            k = (r // quant * quant, g // quant * quant, b // quant * quant)
            counter[k] = counter.get(k, 0) + 1
    total = sum(counter.values()) or 1
    return sorted(((k, v / total * 100) for k, v in counter.items()), key=lambda t: -t[1])[:top]


def expected_over(bg):
    a = CSS_ALPHA
    return tuple(round(a * c + (1 - a) * d) for c, d in zip(APP_BG, bg))


def solve_beta(screen, bg):
    c = expected_over(bg)
    num = den = 0.0
    for s, cc, bb in zip(screen, c, bg):
        if abs(cc - bb) > 3:
            num += s - bb
            den += cc - bb
    return num / den if den else None


def report(label: str, img: Image.Image, bg, outdir: str, fname: str) -> None:
    img.save(os.path.join(outdir, fname))
    mean = tuple(round(v) for v in img.convert("RGB").resize((1, 1)).getpixel((0, 0)))
    dom = dominant(img)
    top, pct = dom[0]
    print(f"  [{label}] 均值={mean}  最高频=rgb{top} ({pct:.1f}%)")
    tail = "  ".join(f"{c}:{p:.0f}%" for c, p in dom[1:4])
    if tail:
        print(f"          次高：{tail}")
    if bg is not None:
        beta = solve_beta(top, bg)
        if beta is not None:
            verdict = "正常" if beta >= 0.9 else ("⚠ 偏低" if beta >= 0.8 else "❌ 被大幅降透明")
            print(f"          → 反解窗口级 β ≈ {beta:.3f}  {verdict}")
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=PROMPTER_EXE)
    ap.add_argument("--outdir", default=os.path.join(ROOT, "shots", "alpha"))
    ap.add_argument("--wait", type=float, default=10.0)
    ap.add_argument("--no-bg", action="store_true")
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()

    set_dpi_aware()
    os.makedirs(args.outdir, exist_ok=True)
    sys.stdout.reconfigure(line_buffering=True)  # 免得被管道缓冲住看不到进度

    if not os.path.exists(args.exe):
        print(f"[错误] 找不到 {args.exe}")
        return 2

    procs: list[subprocess.Popen] = []
    bg = None

    if not args.no_bg:
        bg = BG_MAGENTA
        print("步骤 1/6：铺满屏纯色背景 #ff00ff")
        # 子进程的 stdout 必须丢掉 —— 否则它会继承管道，让上层 tail 永远等不到 EOF
        procs.append(subprocess.Popen(
            [sys.executable, os.path.join(HERE, "solid-bg.py"), *map(str, BG_MAGENTA)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        time.sleep(2.5)
    else:
        print("步骤 1/6：不铺底，直接在现有桌面上量")

    print("步骤 2/6：启动应用")
    procs.append(subprocess.Popen([args.exe], cwd=ROOT,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    print(f"          等待 {args.wait:.0f}s（WebView2 冷启动偏慢）")
    time.sleep(args.wait)

    wins = find_window("Prompter")
    if not wins:
        print("[失败] 找不到 Prompter 窗口")
        _cleanup(procs, args.keep)
        return 1
    hwnd, title, r = wins[0]
    box = (r.left, r.top, r.right, r.bottom)
    print(f"步骤 3/6：窗口 '{title}' hwnd={hwnd} rect={box} ({r.right - r.left}x{r.bottom - r.top})")

    full = ImageGrab.grab(all_screens=True)
    print(f"          屏幕 {full.width}x{full.height}")

    print("步骤 4/6：先截一张「启动态」——应用默认自己隐身，这里应该几乎看不到它")
    report("启动态(隐身 ON)", ImageGrab.grab(bbox=box, all_screens=True), bg, args.outdir,
           "s1_after_launch.png")

    print("步骤 5/6：发 Ctrl+Alt+S 关掉隐身，然后连拍三张")
    send_hotkey(LETTER_VK["S"])
    marks = [(0.8, "s2_plus0.8s.png"), (2.5, "s3_plus2.5s.png"), (5.5, "s4_plus5.5s.png")]
    t0 = time.time()
    for due, fname in marks:
        time.sleep(max(0.0, due - (time.time() - t0)))
        report(f"+{due:.1f}s", ImageGrab.grab(bbox=box, all_screens=True), bg, args.outdir, fname)

    ImageGrab.grab(all_screens=True).save(os.path.join(args.outdir, "s0_fullscreen.png"))

    print("步骤 6/6：" + ("保留进程（--keep）" if args.keep else "收摊"))
    print("=" * 70)
    print(f"CSS 声明：.app 背景 alpha={CSS_ALPHA}、色 rgb{APP_BG}")
    if bg:
        print(f"铺底 {bg} 时，正常渲染应得到 {expected_over(bg)}（深色）")
    print("判读：三张里若第一张淡、后两张实 —— 只是 DWM 过渡假象，窗口本身没问题；")
    print("      三张都一样淡 —— 才是窗口真的被多乘了一层透明度。")
    print("=" * 70)

    _cleanup(procs, args.keep)
    return 0


def _cleanup(procs, keep: bool) -> None:
    if keep:
        print("--keep：进程保留，请手动结束 prompter.exe 与 python solid-bg.py")
        return
    for p in procs:
        if p.poll() is None:
            subprocess.run(["taskkill", "-F", "-PID", str(p.pid)],
                           capture_output=True)
    subprocess.run(["taskkill", "-F", "-IM", "prompter.exe"], capture_output=True)
    print("已结束应用与背景窗口")


if __name__ == "__main__":
    raise SystemExit(main())
