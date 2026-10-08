"""抓取 Prompter 窗口区域的屏幕截图（用于无人值守验证渲染效果）。

用法：
    python scripts/shot.py out.png [--full]

说明：
- 默认按窗口定位，截取窗口矩形 + 外扩 24px 边框。
- 若开启「投屏隐身」(WDA_EXCLUDEFROMCAPTURE)，窗口区域会呈现纯黑，
  这本身就是隐身生效的客观证据。
- 用 --full 截取整块虚拟屏幕。
"""

import ctypes
import ctypes.wintypes as wt
import sys

from PIL import ImageGrab


def set_dpi_aware() -> None:
    """让进程感知物理像素，避免高 DPI 下截图与窗口坐标错位。"""
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)  # PROCESS_PER_MONITOR_DPI_AWARE
    except Exception:
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:
            pass


def find_window(title_part: str = "Prompter"):
    """按「标题前缀」查找可见顶层窗口，返回 (hwnd, title, rect) 列表。

    必须是「标题以前缀开头」，否则会误命中资源管理器里名字里带
    prompter 的文件夹窗口（形如 "stealth-prompter - 文件资源管理器"）。
    同时排除 shell 类窗口，双保险。
    """
    user32 = ctypes.windll.user32
    found = []
    prefix = title_part.lower()

    WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _lparam):
        if not user32.IsWindowVisible(hwnd):
            return True
        length = user32.GetWindowTextLengthW(hwnd)
        if length <= 0:
            return True
        buf = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(hwnd, buf, length + 1)
        title = buf.value.strip()
        if not title.lower().startswith(prefix):
            return True

        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value in ("CabinetWClass", "ExploreWClass", "Progman", "WorkerW"):
            return True

        rect = wt.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        w = rect.right - rect.left
        h = rect.bottom - rect.top
        if w > 0 and h > 0:
            found.append((hwnd, title, rect))
        return True

    user32.EnumWindows(WNDENUMPROC(cb), 0)
    return found


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: shot.py <out.png> [--full]")
        return 2

    out = sys.argv[1]
    full = "--full" in sys.argv

    set_dpi_aware()

    if full:
        img = ImageGrab.grab(all_screens=True)
        img.save(out)
        print(f"full-screen -> {out} ({img.width}x{img.height})")
        return 0

    wins = find_window("Prompter")
    if not wins:
        print("NO_WINDOW: 未找到标题含 Prompter 的可见窗口")
        # 兜底：还是存一张全屏，方便排查
        img = ImageGrab.grab(all_screens=True)
        img.save(out)
        print(f"fallback full-screen -> {out} ({img.width}x{img.height})")
        return 1

    hwnd, title, rect = wins[0]
    pad = 24
    box = (
        max(0, rect.left - pad),
        max(0, rect.top - pad),
        rect.right + pad,
        rect.bottom + pad,
    )
    img = ImageGrab.grab(bbox=box, all_screens=True)
    img.save(out)
    print(f"window '{title}' hwnd={hwnd} rect=({rect.left},{rect.top},{rect.right},{rect.bottom})")
    print(f"saved -> {out} ({img.width}x{img.height})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
