"""真机验证「鼠标穿透」到底有没有生效 —— 读窗口标志位 + hit-test，不靠截图。

# 为什么不能靠截图

  1. 本应用默认开投屏隐身（`WDA_EXCLUDEFROMCAPTURE`），屏幕抓取拿到的是
     一片背景；"窗口在不在"这一条就判不了。
  2. 更重要的是：穿透的语义是**鼠标事件落到谁身上**，这从来不是像素问题，
     是 hit-test 问题。

# 判定原理

  Tauri 的 `set_ignore_cursor_events(true)` 在 Windows 上给窗口加
  `WS_EX_TRANSPARENT | WS_EX_LAYERED`（见 tao `window_state.rs:285`）。
  所以最硬的信号是**直接读主窗自己的 ex-style**：
  有 `WS_EX_TRANSPARENT` ⇒ 现在这一刻是穿透的。

  再配一条 hit-test 作交叉验证：`WindowFromPoint` 会跳过带
  `WS_EX_TRANSPARENT` 的窗口，于是"这个点最上面是哪个窗口"也能读出穿透状态。

  ★★ 但 hit-test 有个必踩的坑：**必须回溯到顶层窗口再判归属**。
     WebView2 的渲染子窗口属于 `msedgewebview2.exe` 进程，不是宿主进程 ——
     直接拿 `WindowFromPoint` 返回值的 pid 去比"是不是我"，永远比不中，
     会把"应用窗口好好地在那儿"整片误判成"穿透了"。
     而且这些子窗口自己就带 `WS_EX_TRANSPARENT`，读它的 ex-style 也没意义。
     所以：`GetAncestor(hit, GA_ROOT)` 拿到顶层，再读顶层。

# 为什么脚本自己拉起应用

  穿透状态只在运行时存在，配置里那个 `click_through` 只是"上次的意图"。
  流程：备份配置 → 置 `click_through=true` → 拉起 exe → 扫竖向剖面 →
  杀掉 → **还原配置**（不改用户的偏好）→ **把光标挪回原处**（不改用户的鼠标）。

# DPI

  开头就把本进程设成 per-monitor-v2，否则 `GetCursorPos` / `GetWindowRect`
  会被系统做 DPI 虚拟化，和 Rust 侧（物理像素）对不上，
  表现为"扫出来的分界线和界面看到的差一截"。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

# ---- DPI：必须在任何窗口 API 之前 ----
try:
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        pass

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32

user32.WindowFromPoint.restype = ctypes.c_void_p
user32.WindowFromPoint.argtypes = [wt.POINT]

GWL_EXSTYLE = -20
WS_EX_TRANSPARENT = 0x00000020
GA_ROOT = 2

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_EXE = ROOT / "src-tauri" / "target" / "release" / "prompter.exe"
CONFIG = Path(os.path.expandvars("%APPDATA%")) / "cn.bobby.prompter" / "config.json"
LOG = (Path(os.path.expandvars("%LOCALAPPDATA%")) / "cn.bobby.prompter"
       / "logs" / "Prompter.log")
PROFILE = (Path(os.path.expandvars("%LOCALAPPDATA%")) / "cn.bobby.prompter"
           / "EBWebView")
STYLES = ROOT / "src" / "styles.css"
CT_RS = ROOT / "src-tauri" / "src" / "win" / "click_through.rs"


# ---------------------------------------------------------------------------
# 版面常量：从**唯一源**里读，不在这个脚本里抄一份
# ---------------------------------------------------------------------------
#
# ★★ 为什么必须这样（本轮真踩）：可点区从"顶上一条"变成"上下两块"之后，
#    验收区间就**跟着窗口高度变**了。而窗口高度是用户拖出来的
#    （实测本机配置里存的是 274，不是默认的 360）。
#    于是写死的 `--expect-css "0-88"` 或 `"0-61,263-360"` 都会"红得没道理"：
#    失败原因跟穿透无关，纯粹是窗口高度/版面变了 —— 那种假警报最费时间，
#    而且很容易被当成"功能坏了"去查一个不存在的问题。
#
#    ⇒ 期望值由 `--expect-bars` 从源码现算：
#       每条栏的高 ← styles.css 的 `--h-top` `--h-cb`（前端唯一源）
#       接近环     ← click_through.rs 的 `APPROACH_PAD`（后端唯一源）
#    这样改版面时**不需要改这个脚本**，也就不会漏改。
#    ★★ 栏的**条数**同样现读：`--h-cb: 0px` ⇒ 底部没有栏（2026-09-26 起就是这个形态），
#       此时不会凭空造出一条底部期望带。写死"上下两条"会让脚本在底栏取消后
#       把"底部能穿透"报成失败 —— 一个与穿透无关的假警报。

def _read_css_px(name: str) -> float | None:
    """从 styles.css 里取一个 `--x: NNpx` 的值。读不到就返回 None（不猜）。"""
    if not STYLES.exists():
        return None
    for line in STYLES.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if line.startswith(f"{name}:"):
            raw = line.split(":", 1)[1].strip().rstrip(";").strip()
            if raw.endswith("px"):
                try:
                    return float(raw[:-2])
                except ValueError:
                    return None
    return None


def _read_approach_pad() -> float | None:
    """从 click_through.rs 里取 APPROACH_PAD（屏幕物理像素）。"""
    if not CT_RS.exists():
        return None
    for line in CT_RS.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if s.startswith("pub const APPROACH_PAD"):
            try:
                return float(s.split("=", 1)[1].strip().rstrip(";"))
            except (IndexError, ValueError):
                return None
    return None


def bars_expectation(css_h: float, scale: float) -> list[tuple[float, float]]:
    """按当前版面算出「可点」应在的各段区间（CSS px，距窗口顶）。

    每一条栏各往外扩一个接近环 —— 稿件区（栏之间的那段）应当是穿透的。
    ★★ 栏有**几条**必须从版面常量现读，不能写死"上下两条"：
       `--h-cb: 0px` 就是"窗口底部没有栏"的意思（2026-09-26 两条栏并成一条，
       底栏整个取消）。此时若还算出一条底部的期望带，脚本就会把"底部能穿透"
       报成失败 —— **一个跟穿透毫无关系的假警报**，
       而这正是本脚本的注释反反复复在防的那类错。
       所以 `--h-cb` 为 0 时直接跳过底部那一段。

    取不到常量时**返回空表并说明原因**，绝不退化成"猜一个"——
    猜出来的期望值只会制造假警报。
    """
    h_top = _read_css_px("--h-top")
    h_cb = _read_css_px("--h-cb")
    pad = _read_approach_pad()
    missing = [n for n, v in (("--h-top", h_top), ("--h-cb", h_cb),
                              ("APPROACH_PAD", pad)) if v is None]
    if missing:
        print(f"  ⚠️ 读不到版面常量 {missing}，无法用 --expect-bars 判定。"
              f"\n     顶栏/控制台的高度在 {STYLES}，接近环在 {CT_RS}。")
        return []

    pad_css = pad / scale if scale > 0 else pad
    bands = [(0.0, h_top + pad_css)]
    if h_cb > 0:
        bands.append((css_h - h_cb - pad_css, css_h))
    bars = "顶栏" + (" + 底部控制台" if h_cb > 0 else "（底部无栏）")
    print(f"  版面常量：--h-top={h_top:.0f} --h-cb={h_cb:.0f} "
          f"APPROACH_PAD={pad:.0f}物理px ÷ {scale:.2f} = {pad_css:.1f}css ⇒ {bars}")
    return bands



# ---------------------------------------------------------------------------
# 现场还原（`finally` 与信号处理共用，必须幂等）
# ---------------------------------------------------------------------------

# 改动前的配置副本。★★ 它存在的**唯一**理由是：Windows 上 `SIGTERM` 是
# `TerminateProcess`，**根本不可捕获** —— 脚本被外部打断时 `finally` 和信号处理
# 都不会跑，配置就永久停在改动后的状态。所以把原文件另存一份，
# 下一次运行开头先自愈（见 heal_leftover_config）。
BACKUP = CONFIG.with_name("config.json.verify-backup")

# 还原所需的东西。放模块级是因为信号处理函数拿不到 main() 的局部变量。
_RESTORE: dict = {"original": None, "cursor": None, "proc": None, "done": False}


def heal_leftover_config() -> bool:
    """上一次运行有没有留下没还原的配置？有就修回去，返回是否修过。

    ★★ 这就是"被打断"的正解（本轮真踩过一次，代价是**改动了老大的配置**）：
       Windows 上 `SIGTERM` 走的是 `TerminateProcess`，**不可捕获、不给清理机会**，
       所以任何"靠 finally 保证还原"的写法在这台机器上都是不成立的。
       ⇒ 改成**把原文件另存一份**，下次运行开头自愈。
         副作用是"泄露最长持续到下一次运行" —— 所以运行结束必须把这份副本删掉，
         它一旦长期存在就说明有泄露，这本身也是一个可观测信号。
    """
    if not BACKUP.exists():
        return False
    CONFIG.write_text(BACKUP.read_text(encoding="utf-8"), encoding="utf-8")
    BACKUP.unlink()
    print("⚠️ 发现上一次运行留下的未还原配置，已自动改回："
          f"click_through = {json.loads(CONFIG.read_text(encoding='utf-8')).get('click_through')}")
    return True


def restore_now() -> None:
    """把老大的配置、光标、以及脚本拉起的实例恢复到跑之前的样子。幂等。

    ★ 注意：Windows 上 `SIGTERM` 不可捕获，所以这里**不是**最后一道保险 ——
      最后一道是 `heal_leftover_config()`（靠另存的副本自愈）。
    """
    if _RESTORE["done"]:
        return
    _RESTORE["done"] = True

    cursor = _RESTORE["cursor"]
    if cursor is not None:
        user32.SetCursorPos(cursor.x, cursor.y)

    proc = _RESTORE["proc"]
    if proc is not None:
        # 用 /T：只杀主进程会留下 WebView2 孤儿，把自己的下一次启动弄坏
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                       capture_output=True, env=dict(os.environ))
        time.sleep(0.8)
        kill_orphan_webviews()

    original = _RESTORE["original"]
    if original is not None:
        CONFIG.write_text(original, encoding="utf-8")
        # 还原成功 ⇒ 副本没用了。它长期存在 = 有泄露，见 heal_leftover_config。
        if BACKUP.exists():
            BACKUP.unlink()
        print()
        print(f"已还原配置 click_through = {json.loads(original).get('click_through')}，"
              f"光标归位，脚本拉起的实例已关闭。")


def on_signal(signum, _frame):
    """被打断时尽量还原。★ 只在**真的能收到信号**的场合有效（POSIX / 控制台 Ctrl-C）。

    Windows 上被外部 `TerminateProcess` 时这个函数不会被执行 ——
    那种情况由 `heal_leftover_config()` 兜底。
    """
    print(f"\n收到信号 {signum}，先还原现场再退出…")
    restore_now()
    sys.exit(128 + signum)


def reset_profile() -> str | None:
    """把 WebView2 的 profile 挪走，让应用下次重建一个全新的。返回挪去哪了。

    ★★ 为什么需要这个兜底（本项目实测，重要）：
       同一个**二进制**，有时拉起后页面正常执行、有时窗口出来了但页面一片空白
       （`msedgewebview2.exe` 子进程 4 个都在，只是从没执行过页面 → 日志里没有 `[bootstrap]`）。
       这不是我们的代码问题 —— 已用同一个 exe 反复实测复现"能起 / 不能起"两种结果；
       挪走 profile 后立刻恢复，说明是 WebView2 启动期的一次性失败。

       ⇒ 所以脚本**先重试**（别把 flaky 当结论），重试都失败才清 profile。
         这也解释了另一个坑：连续硬杀实例（`taskkill /F`）会让这个概率变高，
         因为浏览器进程可能没走完自己的退出流程。

    ★ 代价：丢掉 webview 的本地状态（cache / 登录态）。本应用的配置在 `%APPDATA%`，
      不在这里，所以损失可以忽略。挪走而不是删掉，出问题还能翻旧账。
    """
    if not PROFILE.exists():
        return None
    dest = PROFILE.with_name(f"EBWebView.bak-{time.strftime('%Y%m%d-%H%M%S')}")
    try:
        PROFILE.rename(dest)
    except OSError as e:
        print(f"  挪 profile 失败（{e}），继续用现有的")
        return None
    return str(dest)


def wait_zones(log_before: int, timeout: float = 12.0) -> bool:
    """等前端把「可交互区」上报上来。返回有没有等到。

    ★★ 为什么光有 `[bootstrap]` 还不够（本轮实测到的第二种失败形态）：
       `bootstrap` 正常返回了，但前端之后**停在原地** —— 连可交互区都不上报。
       此时后端的兜底是"整窗按可交互处理"，于是**整窗都可点**（剖面 `0-356` 全可点），
       看起来和"穿透完全没实现"一模一样。我被这个形态骗过一次，
       在真机上量出一整列"可点"然后去找后端的 bug。

       ⇒ "这个实例能不能测"的判据必须是**两条日志都到齐**：
         `[bootstrap]`（页面跑起来了）+ `[click-through] 可交互区已更新`（链路通了）。
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        if "可交互区已更新" in read_log()[log_before:]:
            return True
        time.sleep(0.4)
    return False


def launch_and_wait(exe: Path) -> tuple[subprocess.Popen | None, int | None, str, bool]:
    """拉起一次并等就位。

    返回 `(进程, 窗口句柄, bootstrap 那一行, 可交互区是否已上报)`。
    调用方要**两个都齐**才认为这次拉起可用（理由见 `wait_zones`）。
    """
    log_before = len(read_log())
    proc = subprocess.Popen(
        [str(exe)], close_fds=True,
        # 隐藏控制台：debug 产物是控制台程序，不隐藏会弹一个黑框出来。
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    print(f"  已拉起 {exe.name} (pid={proc.pid})，等窗口与界面就位…")

    hwnd = None
    deadline = time.time() + 25
    while time.time() < deadline:
        hwnd = find_main_window(proc.pid)
        if hwnd:
            break
        time.sleep(0.3)
    if not hwnd:
        return proc, None, "", False

    boot = wait_frontend(log_before)
    if not boot:
        return proc, hwnd, "", False
    return proc, hwnd, boot, wait_zones(log_before)


def read_log() -> str:
    try:
        return LOG.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def wait_frontend(before_len: int, timeout: float = 22.0) -> str:
    """等日志里出现 `[bootstrap]`，返回它那一行。

    ★★ 这是本脚本最重要的一个断言，比剖面本身还重要。
       `bootstrap` 是前端挂载后调的第一个命令（见 `commands::bootstrap` 的日志），
       所以它出现 = 页面真的跑起来了。
       没有它就意味着**窗口在、可点、但里面是空白的** ——
       这种情况下穿透当然"完全没反应"（开关是前端调的，前端没跑，
       `ct_enabled` 永远是 false）。
       第一版脚本就是在这里栽的：它把"空白实例"的症状当成了
       "穿透功能坏了"的证据，一路查到后端去，方向全错。
       所以这里宁可**直接失败**，也不要在这种实例上出数字。
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        txt = read_log()
        new = txt[before_len:]
        for line in new.splitlines():
            if "[bootstrap]" in line:
                return line.strip()
        time.sleep(0.4)
    return ""


def pid_of(hwnd) -> int:
    pid = wt.DWORD()
    user32.GetWindowThreadProcessId(ctypes.c_void_p(hwnd), ctypes.byref(pid))
    return pid.value


def rect_of(hwnd):
    r = wt.RECT()
    user32.GetWindowRect(ctypes.c_void_p(hwnd), ctypes.byref(r))
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def ex_of(hwnd) -> int:
    return user32.GetWindowLongW(ctypes.c_void_p(hwnd), GWL_EXSTYLE) & 0xFFFFFFFF


def is_through(hwnd) -> bool:
    return bool(ex_of(hwnd) & WS_EX_TRANSPARENT)


def root_at(x: int, y: int):
    """该点上最上面的顶层窗口（跳过带 WS_EX_TRANSPARENT 的窗口后的结果）。"""
    h = user32.WindowFromPoint(wt.POINT(x, y)) or 0
    if not h:
        return 0, 0
    root = user32.GetAncestor(ctypes.c_void_p(h), GA_ROOT) or h
    return root, pid_of(root)


def find_main_window(target_pid: int):
    """目标进程里面积最大的那块可见顶层窗口 —— 就是主窗。"""
    found: list[tuple[int, int]] = []
    WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _lp):
        if user32.IsWindowVisible(hwnd) and pid_of(hwnd) == target_pid:
            _, _, w, h = rect_of(hwnd)
            if w > 200 and h > 60:
                found.append((w * h, hwnd))
        return True

    user32.EnumWindows(WNDENUMPROC(cb), 0)
    return max(found)[1] if found else None


TH32CS_SNAPPROCESS = 0x00000002


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD),
        ("cntUsage", wt.DWORD),
        ("th32ProcessID", wt.DWORD),
        ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
        ("th32ModuleID", wt.DWORD),
        ("cntThreads", wt.DWORD),
        ("th32ParentProcessID", wt.DWORD),
        ("pcPriClassBase", ctypes.c_long),
        ("dwFlags", wt.DWORD),
        ("szExeFile", ctypes.c_wchar * 260),
    ]


def snapshot_processes() -> list[tuple[int, int, str]]:
    """(pid, ppid, exe 名) 全表。用 Toolhelp 快照 —— wmic 在本机已被移除。"""
    k = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if k == -1:
        return []
    out = []
    try:
        e = PROCESSENTRY32W()
        e.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        ok = kernel32.Process32FirstW(k, ctypes.byref(e))
        while ok:
            out.append((e.th32ProcessID, e.th32ParentProcessID, e.szExeFile))
            ok = kernel32.Process32NextW(k, ctypes.byref(e))
    finally:
        kernel32.CloseHandle(k)
    return out


def kill_orphan_webviews() -> int:
    """清掉**已经没有宿主**的 WebView2 浏览器进程。

    ★★ 这个坑很值得记：`taskkill /F /IM prompter.exe` 只杀主进程，
       WebView2 的浏览器进程（`msedgewebview2.exe`）会活下来，
       并且继续占着 `EBWebView` 这个 user-data-dir。
       之后每起一次新实例，它要么连到这个"行尸走肉"上、要么连不上 ——
       症状是**窗口出来了但页面是空白**：窗口在、可点、但不执行任何前端代码。
       最坑的是它看起来像"功能没实现"：点穿透毫无反应，
       因为 ct_enabled 从来没被打开过（那是前端调的）。

    只杀**父进程已经不在了**的那些：还活着的宿主说明它的 WebView 正在用，
    杀了会把别的应用界面打白。
    """
    procs = snapshot_processes()
    alive = {p for p, _, _ in procs}
    victims = [
        pid
        for pid, ppid, name in procs
        if name.lower() == "msedgewebview2.exe" and ppid not in alive
    ]
    for pid in victims:
        subprocess.run(["taskkill", "/F", "/PID", str(pid)],
                       capture_output=True, env=dict(os.environ))
    if victims:
        time.sleep(0.8)
    return len(victims)


def kill_prompter() -> None:
    # `/T` 连子进程一起杀 —— 少留孤儿（残留的孤儿由下面那步兜底清掉）
    subprocess.run(["taskkill", "/F", "/T", "/IM", "prompter.exe"],
                   capture_output=True, env=dict(os.environ))
    time.sleep(1.0)
    n = kill_orphan_webviews()
    if n:
        print(f"  （清掉 {n} 个没有宿主的 WebView2 孤儿进程 —— 它们会占住 user-data-dir）")


def parse_ranges(spec: str) -> list[tuple[float, float]]:
    out = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        a, b = part.split("-")
        out.append((float(a), float(b)))
    return out


def grab_window(hwnd, out: Path) -> None:
    """截窗口那一块屏幕。

    只在调试时用：**开了投屏隐身的话这里会是一张纯黑图**，
    因为窗口被排除了屏幕捕获（那是设计如此，不是 bug）。
    所以要它有用，必须先把 `stealth_on_start` 关掉（脚本会自动）。
    """
    try:
        from PIL import ImageGrab
    except ImportError:
        print("  截图跳过：系统 Python 没装 PIL")
        return
    x, y, w, h = rect_of(hwnd)
    pad = 24
    img = ImageGrab.grab(bbox=(x - pad, y - pad, x + w + pad, y + h + pad),
                         all_screens=True)
    out.parent.mkdir(parents=True, exist_ok=True)
    img.save(out)
    print(f"  截图 → {out}（{img.size[0]}x{img.size[1]}）")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(DEFAULT_EXE))
    ap.add_argument("--step", type=float, default=4.0, help="竖向扫描步长（CSS px）")
    ap.add_argument("--hold", type=float, default=0.40,
                    help="每个采样点等多久（要盖过 40ms 轮询 + 3 tick 迟滞≈120ms）")
    ap.add_argument("--settle", type=float, default=2.5,
                    help="前端就位后再等多久（等 400ms 一次的可交互区上报落一次，"
                         "并让 origin/scale 的定期刷新跟上——窗口启动后会被挪到记忆位置，"
                         "读早了量到的是空气）")
    ap.add_argument(
        "--expect-css", default="",
        help="期望「可点」的 CSS 像素区间（距窗口顶），如 '0-61,263-360'。"
             "留空则只打印、不判定。"
             "★★ **推荐改用 `--expect-bars`**：写死的区间跟着窗口高度变"
             "（栏的位置由窗口高度决定，而窗口高度是用户拖出来的），"
             "写死一个数字在换了版面/窗口尺寸之后会必然失败，"
             "而失败原因跟穿透无关 —— 那种假警报最费时间。",
    )
    ap.add_argument(
        "--expect-bars", action="store_true",
        help="期望值按**当前版面**现算：版面里有几条栏就算几段，各自外扩一个接近环。"
             "常量从唯一源读（styles.css 的 --h-top/--h-cb、click_through.rs 的 APPROACH_PAD），"
             "所以改版面时不用改这个脚本，也不会漏改；"
             "`--h-cb: 0px`（底部无栏）时不会凭空造出一条底部期望带。与 --expect-css 互斥。",
    )
    ap.add_argument("--tol", type=float, default=8.0,
                    help="边界容差（CSS px），扫描步长造成的量化误差要留出来")
    ap.add_argument("--tries", type=int, default=3,
                    help="拉起实例的最大次数。WebView2 启动期偶发一次性失败"
                         "（窗口在但页面不执行），重试可绕开；最后一次会自动换全新 profile。")
    ap.add_argument("--shot", default="",
                    help="抓一张窗口截图存到这里。会临时把 stealth_on_start 关掉 —— "
                         "否则投屏隐身会让截图变成纯黑（那是设计如此）。"
                         "排查「窗口是不是空白页」时非常有用。")
    args = ap.parse_args()

    exe = Path(args.exe)
    if not exe.exists():
        raise SystemExit(f"找不到 exe：{exe}")

    original = CONFIG.read_text(encoding="utf-8")
    # ★ 自愈：上一次要是被打断（Windows 上 TerminateProcess 不可捕获），
    #   配置会停在改动后的状态。先修回去再继续，别在"脏"的基础上叠加新的改动。
    heal_leftover_config()
    original = CONFIG.read_text(encoding="utf-8")
    cfg = json.loads(original)
    css_w = float(cfg.get("window", {}).get("width") or 560)
    print(f"配置：{CONFIG}")
    print(f"  改前 click_through = {cfg.get('click_through')}")

    # ★★ 兜住"自持"泄露：这个脚本是**唯一**会把 click_through 改成 true 的东西，
    #    而它把读到的值当成"老大的原值"再还原回去 —— 于是一旦漏过一次，
    #    后面每次运行都会忠实地把 true 还原成 true，永远回不去，而且**没有任何迹象**。
    #    （本轮真踩到：配置停在 true，脚本却每次都报告"已还原"，看起来完全正常。）
    #    ⇒ 读到 true 又没有副本可自愈时，把话说明白，让人有机会判断这是不是他的本意。
    if cfg.get("click_through") is True:
        print("  ⚠️ 读到 click_through = true，但没有未还原的副本可自愈。")
        print("     这个值可能是老大自己开的，也可能是上一次运行漏下的（见本文件顶部说明）。")
        print("     本轮把它当原值保留；要改成关，运行后手动改，或先删 config 里的这一项。")

    cursor_save = wt.POINT()
    user32.GetCursorPos(ctypes.byref(cursor_save))

    # 先把还原所需的东西交给信号处理函数 —— 被 SIGTERM 打断时 finally 不会跑。
    _RESTORE["original"] = original
    _RESTORE["cursor"] = cursor_save
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, on_signal)

    proc = None
    try:
        kill_prompter()
        cfg["click_through"] = True
        if args.shot:
            cfg["stealth_on_start"] = False
        # 先另存原文件再改动 —— 它是被打断时唯一的还原依据（见 heal_leftover_config）。
        BACKUP.write_text(original, encoding="utf-8")
        CONFIG.write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")

        hwnd = None
        boot = ""
        zones_ok = False
        for attempt in range(1, args.tries + 1):
            proc, hwnd, boot, zones_ok = launch_and_wait(exe)
            if boot and zones_ok:
                break
            # 同一个二进制有时能起、有时不能（见 reset_profile / wait_zones 的文档）——
            # 这不是"功能坏了"，所以先重试，别把 flaky 当结论。
            if not hwnd:
                why = "窗口都没出来"
            elif not boot:
                why = "窗口在，但页面没执行（连 [bootstrap] 都没有）"
            else:
                why = "页面起来了，但前端停在原地（一直没上报可交互区）"
            print(f"  第 {attempt}/{args.tries} 次：{why}")
            kill_prompter()
            if attempt < args.tries:
                if attempt == args.tries - 1:
                    moved = reset_profile()
                    print(f"  最后一次换成全新 profile 再试"
                          + (f"（旧的挪到 {moved}）" if moved else ""))
                else:
                    print("  重试…")

        if args.shot and hwnd:
            grab_window(hwnd, Path(args.shot))
        if not hwnd:
            print(f"WINDOW_NOT_FOUND —— 拉了 {args.tries} 次都没有可见窗口")
            return 2
        if not (boot and zones_ok):
            print("FRONTEND_NOT_READY —— 前端没能就位，测穿透没有意义。")
            print("  穿透开关是前端调的、可交互区也是前端上报的；前端没跑完，")
            print("  量出来的剖面只会是「整窗可点」，而那是**假象**（看着像功能没实现）。")
            print(f"  失败形态：{'' if boot else '没有 [bootstrap]；'}"
                  f"{'' if zones_ok else '没有收到可交互区上报；'}")
            print(f"  日志：{LOG}")
            print("  ★ 先别怀疑代码：同一个 exe 实测有「能起/不能起」两种结果，")
            print("    多半是 WebView2 启动期的一次性失败。再跑一次本脚本（默认已重试 3 次），")
            print(f"    或手动把 {PROFILE} 挪走后重试。")
            return 3
        print(f"  前端已就位：{boot}")

        # ★ 必须等界面就位**再读 rect**：启动头几秒窗口还停在系统的默认级联位置，
        #   之后才被 apply_saved_geometry 挪到记忆中的位置。
        #   早早读一次 rect 然后一直用，探的就是一片空气 ——
        #   会得到"整列都穿透"这种假象（第一版脚本就是这么误判的）。
        time.sleep(args.settle)
        x, y, w, h = rect_of(hwnd)

        scale = w / css_w if css_w > 0 else 1.0
        print(f"主窗 hwnd={hwnd}  {w}x{h} @({x},{y})   DPI 缩放 ≈ {scale:.2f}")
        print(f"  ex-style={ex_of(hwnd):#010x}  "
              f"TRANSPARENT={'是' if is_through(hwnd) else '否'}")

        # 先把光标甩到窗口外，让状态收敛到"穿透"这个起始态。
        user32.SetCursorPos(max(x - 80, 5), max(y - 80, 5))
        time.sleep(args.hold + 0.5)

        cx = x + w // 2
        css_h = h / scale
        n = int(css_h / args.step) + 1
        print()
        print(f"竖向剖面（x={cx}，距窗口顶 0..{css_h:.0f} CSS px，步长 {args.step:g}）")
        print(f"  判据 A = 主窗 ex-style 是否带 WS_EX_TRANSPARENT")
        print(f"  判据 B = WindowFromPoint 回溯顶层后是不是 prompter")
        print(f"  {'距顶css':>8} {'距顶px':>7}  {'A 标志位':<10} {'B 命中':<14} 结论")
        print("  " + "-" * 62)

        samples: list[tuple[float, bool]] = []
        for i in range(n):
            off_css = i * args.step
            off_px = int(round(off_css * scale))
            if off_px >= h:
                break
            user32.SetCursorPos(cx, y + off_px)
            time.sleep(args.hold)
            a = is_through(hwnd)
            _, hp = root_at(cx, y + off_px)
            b_hit_me = hp == proc.pid
            verdict = "穿透" if a else "可点"
            samples.append((off_css, not a))
            cross = "" if (not a) == b_hit_me else "  ← 两条判据打架"
            print(f"  {off_css:>8.0f} {off_px:>7}  "
                  f"{'TRANSPARENT' if a else '—':<10} "
                  f"{('prompter' if b_hit_me else '其他窗口'):<14} {verdict}{cross}")

        bands = []
        start = None
        for i, (off, clickable) in enumerate(samples):
            if clickable and start is None:
                start = off
            if not clickable and start is not None:
                bands.append((start, samples[i - 1][0]))
                start = None
        if start is not None and samples:
            bands.append((start, samples[-1][0]))
        print()
        print("  实测「可点」区间（CSS px，距窗口顶）："
              + ("、".join(f"{a:.0f}-{b:.0f}" for a, b in bands) if bands else "（无 —— 整窗都穿透！）"))

        if not args.expect_css and not args.expect_bars:
            return 0

        if args.expect_bars:
            expect = bars_expectation(css_h, scale)
            if not expect:
                return 2
        else:
            expect = parse_ranges(args.expect_css)
        print("  期望「可点」区间：" + "、".join(f"{a:.0f}-{b:.0f}" for a, b in expect))

        # 逐采样点比对。
        #
        # ★★ 判据的形状很关键（本轮改过一次，值得记）：
        #   扫出来的剖面是**离散**的（步长 4css），而"可点区边到底在哪"只能
        #   分辨到一个步长以内。所以：
        #
        #     · 落在区间**带容差的外沿**之内（距任一端 ≤ tol）⇒ **不关心**：
        #       这一点的答案完全取决于真实边界落在哪，方法本身分辨不出来。
        #       最初这里写成"外沿内 ⇒ 期望可点"，于是 1px 的期望误差被 tol 放大成
        #       8px 的假警报（实跑：期望 0-62、实测 0-60，却报了 64/68 两处不一致）。
        #     · 区间**内部**（距两端都 > tol）⇒ 必须可点。
        #     · 区间**外部** ⇒ 必须穿透。
        #
        #   这样"真不一致"剩下的才是真的不一致 —— 而不是被容差自己造出来的。
        bad = []
        holes = []
        for off, clickable in samples:
            near_edge = any(abs(off - a) <= args.tol or abs(off - b) <= args.tol
                            for a, b in expect)
            if near_edge:
                continue
            want = any(a <= off <= b for a, b in expect)
            if want != clickable:
                bad.append((off, want, clickable))
                # 区间内出现穿透 ⇒ 要么是噪声（有人动鼠标 / 窗口被挪），
                # 要么真的漏了一块。这两件事的处置完全相反，所以**分开报**。
                if want and not clickable:
                    holes.append(off)

        if holes:
            print(f"\n  ⚠️ 期望区间内出现 {len(holes)} 个「穿透」采样点"
                  f"（{', '.join(f'{o:.0f}' for o in holes[:8])} css）：")
            print("     可能是噪声（脚本运行时有人动鼠标 / 窗口被挪），也可能是真漏了一块 ——")
            print("     **同一条命令再跑一次**：复现就是真的，不重现就是噪声。别据单次下结论。")
            return 2

        if bad:
            print(f"\n  ❌ 不一致 {len(bad)} 处（前 6 个）：")
            for off, want, got in bad[:6]:
                print(f"     距顶 {off:.0f}css：期望{'可点' if want else '穿透'}，"
                      f"实测{'可点' if got else '穿透'}")
            return 1

        print("\n  ✅ 剖面与期望一致")
        return 0
    finally:
        # `_RESTORE["proc"]` 每次拉起都会更新，所以还原的一定是**最后一个**实例。
        _RESTORE["proc"] = proc
        restore_now()


if __name__ == "__main__":
    sys.exit(main())
