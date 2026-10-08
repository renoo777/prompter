"""自动验证「投屏隐身」是否真的生效（不需要人眼）。

# 原理

改进了原来的做法。原版只对比「隐身开 / 隐身关」两张图，于是**必须先知道
当前处在哪个状态**，否则两张图的身份会反，结论也就反了 —— 这个坑实际踩过两次
（一次因为程序启动时隐身默认开着，一次因为前一步的手动切换）。

新版加了一张**锚点图**：用 `Ctrl+Alt+H` 把窗口整个藏起来，此时屏幕捕获到的
必然是纯背景。有了它：

  * 两张候选图里，跟锚点几乎完全一致的那张 = 窗口被排除出捕获的那张；
  * 另一张 = 合成结果。

方向不需要假设，而且验证的结论更强 —— 原来只能证明「切换前后有变化」，
现在能证明「隐身开启时，窗口对捕获画面的贡献**完全为零**」
（像素级等同于把窗口藏起来），这才是"投屏隐身"真正的含义。

# 步骤

    1) 记录窗口矩形
    2) Ctrl+Alt+H 隐藏 → 截锚点 REF（纯背景）
    3) Ctrl+Alt+H 恢复
    4) Ctrl+Alt+S → 截 X
    5) Ctrl+Alt+S → 截 Y（状态复位）
    6) 判定 X / Y 谁是"被排除"的那张，并算出不透明度

# 输出

| 文件 | 含义 |
|---|---|
| `stealth_ref.png` | 锚点：窗口隐藏时的纯背景 |
| `stealth_excluded.png` | 隐身开启时捕获到的画面（应与 REF 几乎一致） |
| `stealth_composite.png` | 隐身关闭时捕获到的合成画面（能看到界面） |
| `stealth_A.png` / `stealth_B.png` | 同上，按 `measure-alpha.py` 约定的命名重排 —— **A=合成、B=纯背景** |

用法：
    python scripts/verify-stealth.py --outdir shots/mic

# 两个必须记住的前提

1. **应用默认自己隐身**（`AppConfig::default().stealth_on_start == true`）。
   所以刚启动的应用在本脚本的截图里本来就不该出现 —— 那是设计，不是故障。
   判断"隐身有没有生效"必须靠**切换前后对比**，不能靠单张图。
2. **期望的不透明度要从应用自己的 config.json 读**（本脚本已自动读）。
   设置抽屉里的滑杆一改就落盘成 `"opacity"`，脚本若硬编码默认的 0.94，
   就会把"应用忠实地按 0.3 渲染"误报成"透明度算了两遍"。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import json
import os
import statistics
import time

from PIL import Image, ImageChops, ImageGrab, ImageStat

VK_CONTROL = 0x11
VK_MENU = 0x12  # Alt
KEYEVENTF_KEYUP = 0x0002

LETTER_VK = {c: 0x41 + i for i, c in enumerate("ABCDEFGHIJKLMNOPQRSTUVWXYZ")}

# 判定「两张图几乎一致」的阈值（平均灰阶差）。同一画面重复截图的噪声远小于此值。
SAME_THRESHOLD = 6.0
# 判定「两张图明显不同」的阈值。
DIFF_THRESHOLD = 25.0

APP_BG = (14, 17, 22)  # 与 styles.css 的 --bg 一致

# 期望的不透明度必须**从应用自己的配置里读**，不能硬编码。
# 滑杆一改就写进 config.json，硬编码 0.94 会把「忠实地按 0.3 渲染」误报成异常。
CONFIG_PATH = os.path.join(os.environ.get("APPDATA", ""), "cn.bobby.prompter", "config.json")
DEFAULT_ALPHA = 0.94


def configured_alpha(path: str | None = None) -> tuple[float, str]:
    p = path or CONFIG_PATH
    try:
        with open(p, encoding="utf-8") as f:
            return float(json.load(f).get("opacity", DEFAULT_ALPHA)), p
    except FileNotFoundError:
        return DEFAULT_ALPHA, f"默认值（没有 {p}）"
    except Exception as e:
        return DEFAULT_ALPHA, f"默认值（读配置失败：{e}）"


CONFIG_ALPHA = DEFAULT_ALPHA  # 仅供类型/向后引用；实际取值在 main() 里读配置


def set_dpi_aware() -> None:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:
            pass


def find_window(prefix: str = "Prompter"):
    """找标题以 prefix 开头的可见顶层窗口，返回 (hwnd, title, rect)。"""
    user32 = ctypes.windll.user32
    found = []

    CB = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _lp):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n <= 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        title = buf.value.strip()
        if not title.lower().startswith(prefix.lower()):
            return True
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value in ("CabinetWClass", "ExploreWClass", "Progman", "WorkerW"):
            return True
        r = wt.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(r))
        if r.right - r.left > 0 and r.bottom - r.top > 0:
            found.append((hwnd, title, r))
        return True

    user32.EnumWindows(CB(cb), 0)
    return found


def send_hotkey(vk: int) -> None:
    """按下并释放 Ctrl+Alt+<vk>。"""
    u = ctypes.windll.user32
    u.keybd_event(VK_CONTROL, 0, 0, 0)
    u.keybd_event(VK_MENU, 0, 0, 0)
    u.keybd_event(vk, 0, 0, 0)
    time.sleep(0.06)
    u.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)
    u.keybd_event(VK_MENU, 0, KEYEVENTF_KEYUP, 0)
    u.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)


def brightness(img: Image.Image) -> float:
    return ImageStat.Stat(img.convert("L")).mean[0]


def mean_abs_diff(a: Image.Image, b: Image.Image) -> float:
    stat = ImageStat.Stat(ImageChops.difference(a.convert("RGB"), b.convert("RGB")))
    return sum(stat.mean) / 3.0


def changed_ratio(a: Image.Image, b: Image.Image, thresh: int = 24) -> float:
    """逐像素差超过 thresh 灰阶的比例（%，对整体亮度漂移不敏感）。"""
    gray = ImageChops.difference(a.convert("RGB"), b.convert("RGB")).convert("L")
    hist = gray.histogram()
    total = sum(hist)
    return sum(hist[thresh:]) / total * 100.0 if total else 0.0


def solve_alpha(composite: Image.Image, background: Image.Image) -> float | None:
    """由 `comp = α·C + (1-α)·bg` 逐像素反解 α。

    只在「背景接近纯白」的像素上取中位数 —— 那里 C 必然落应用的深色平铺背景，
    而文字像素只占少数，中位数就稳定落在背景团簇上。
    """
    cp, bp = composite.convert("RGB").load(), background.convert("RGB").load()
    w, h = composite.size
    samples = []
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            cr, br = cp[x, y][0], bp[x, y][0]
            if 225 <= br <= 255:
                samples.append((cr, br))
    if len(samples) < 500:
        return None
    c_med = statistics.median(s[0] for s in samples)
    b_med = statistics.median(s[1] for s in samples)
    denom = APP_BG[0] - b_med
    if denom == 0:
        return None
    return (c_med - b_med) / denom


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--key", default="s", help="切换隐身的字母键（默认 s）")
    ap.add_argument("--hide-key", default="h", help="切换窗口显示的字母键（默认 h）")
    ap.add_argument("--wait", type=float, default=1.2, help="每次操作后等待秒数")
    ap.add_argument("--outdir", default=".", help="截图输出目录")
    ap.add_argument("--opacity", type=float, default=None,
                    help="手动指定期望的不透明度；默认从应用的 config.json 读")
    args = ap.parse_args()

    set_dpi_aware()
    os.makedirs(args.outdir, exist_ok=True)

    vk_stealth = LETTER_VK.get(args.key.upper())
    vk_hide = LETTER_VK.get(args.hide_key.upper())
    if vk_stealth is None or vk_hide is None:
        print(f"[错误] 不支持的键：--key {args.key} / --hide-key {args.hide_key}")
        return 2

    wins = find_window("Prompter")
    if not wins:
        print("[失败] 找不到 Prompter 窗口")
        return 1
    hwnd, title, r = wins[0]
    print(f"窗口：'{title}' hwnd={hwnd} rect=({r.left},{r.top},{r.right},{r.bottom})")

    # 矩形先记下来 —— 窗口一旦藏起来就找不到了，只能靠这个坐标继续截
    pad = 24
    box = (max(0, r.left - pad), max(0, r.top - pad), r.right + pad, r.bottom + pad)

    def grab(name: str) -> Image.Image:
        img = ImageGrab.grab(bbox=box, all_screens=True)
        img.save(os.path.join(args.outdir, name))
        print(f"  {name:<24} 平均亮度={brightness(img):.1f}")
        return img

    print("\n步骤 1/4：Ctrl+Alt+%s 隐藏窗口 → 截锚点（纯背景）" % args.hide_key.upper())
    send_hotkey(vk_hide)
    time.sleep(args.wait)
    ref = grab("stealth_ref.png")

    print("步骤 2/4：Ctrl+Alt+%s 恢复窗口" % args.hide_key.upper())
    send_hotkey(vk_hide)
    time.sleep(args.wait)

    print("步骤 3/4：Ctrl+Alt+%s → 截 X" % args.key.upper())
    send_hotkey(vk_stealth)
    time.sleep(args.wait)
    x = grab("stealth_X.png")

    print("步骤 4/4：Ctrl+Alt+%s → 截 Y（状态复位）" % args.key.upper())
    send_hotkey(vk_stealth)
    time.sleep(args.wait)
    y = grab("stealth_Y.png")

    if not (ref.size == x.size == y.size):
        print(f"\n[结论] 尺寸不一致（{ref.size} / {x.size} / {y.size}），窗口可能被移动。")
        return 1

    # ---- 用锚点判定 X / Y 谁是"被排除"的那张 ----
    d_x = mean_abs_diff(ref, x)
    d_y = mean_abs_diff(ref, y)
    r_x = changed_ratio(ref, x)
    r_y = changed_ratio(ref, y)
    print(f"\n与锚点(纯背景)的平均差：X = {d_x:.1f} 灰阶（显著变化像素 {r_x:.2f}%），"
          f"Y = {d_y:.1f} 灰阶（{r_y:.2f}%）")

    if d_x <= d_y:
        excluded, composite, d_exc, d_comp = x, y, d_x, d_y
        name_exc, name_comp = "X", "Y"
    else:
        excluded, composite, d_exc, d_comp = y, x, d_y, d_x
        name_exc, name_comp = "Y", "X"

    excluded.save(os.path.join(args.outdir, "stealth_excluded.png"))
    composite.save(os.path.join(args.outdir, "stealth_composite.png"))
    # 按 measure-alpha.py 的约定重排：A = 合成，B = 纯背景
    composite.save(os.path.join(args.outdir, "stealth_A.png"))
    excluded.save(os.path.join(args.outdir, "stealth_B.png"))

    print(f"判定：{name_exc} 是隐身生效时的捕获画面，{name_comp} 是能看到界面的合成画面")

    cross = mean_abs_diff(x, y)
    cross_ratio = changed_ratio(x, y)
    print(f"\n两张候选图之间：平均差 {cross:.1f} 灰阶，显著变化像素 {cross_ratio:.1f}%")

    print("\n" + "=" * 64)
    ok = True

    # 判据 1：隐身开启时，捕获到的必须与「窗口藏起来」几乎一致
    if d_exc <= SAME_THRESHOLD:
        print(f"✅ 隐身生效：捕获画面与「窗口隐藏」几乎完全一致（差 {d_exc:.1f} 灰阶）")
        print("   —— 窗口对捕获结果的贡献为零，而不只是「有变化」。")
    else:
        ok = False
        print(f"❌ 隐身未完全生效：捕获画面与「窗口隐藏」差了 {d_exc:.1f} 灰阶")
        print("   —— 窗口仍有部分内容进入了捕获。")

    # 判据 2：不隐身时必须能拍到界面
    if d_comp >= DIFF_THRESHOLD:
        print(f"✅ 对照组有效：不隐身时明显拍到了界面（与纯背景差 {d_comp:.1f} 灰阶）")
    else:
        ok = False
        print(f"❌ 对照组无效：不隐身时也只差 {d_comp:.1f} 灰阶 ——")
        print("   要么窗口被别的窗口完全遮住，要么它其实一直是隐身的（热键没生效）。")

    # 判据 3（参考信息）：不透明度
    config_alpha, alpha_src = configured_alpha(args.config)
    if args.opacity is not None:
        config_alpha, alpha_src = args.opacity, "--opacity 命令行指定"
    alpha = solve_alpha(composite, excluded)
    if alpha is None:
        print("ℹ️ 不透明度反解样本不足，跳过（不影响上面两条主结论）")
        print("   想要可信的不透明度数字，用 scripts/alpha-probe.py 铺纯色底测。")
    else:
        note = ""
        if abs(alpha - config_alpha) > 0.08:
            note = "  ⚠ 与配置差得较多 —— 见下方提示"
        print(f"ℹ️ 反解出的不透明度 α ≈ {alpha:.3f}（配置 {config_alpha}，来自 {alpha_src}）{note}")
        if note:
            print("      背景若是**另一个带透明图层的窗口**（WorkBuddy 等），多层叠加会让反解失准；")
            print("      要一个可信数字，改跑 scripts/alpha-probe.py（铺纯色底）。")

    print("=" * 64)
    if ok:
        print("[结论] 通过。")
        print(f"       对照图：{args.outdir}\\stealth_composite.png（能看到界面）")
        print(f"               {args.outdir}\\stealth_excluded.png（与 stealth_ref.png 一致）")
        return 0

    print("[结论] 未通过，见上面的 ❌ 项。")
    print("       常见原因：① 隐身 API 未生效；② 窗口被遮挡；")
    print("       ③ 热键没被应用收到（看 run.log 是否显示 5 个快捷键均已注册）。")
    return 3


if __name__ == "__main__":
    raise SystemExit(main())
