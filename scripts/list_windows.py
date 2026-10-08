"""列出当前所有可见顶层窗口（标题 + 类名 + 矩形 + 进程名）。

用途：排查「进程在跑但找不到窗口」——确认窗口到底叫什么、在哪。
"""

import ctypes
import ctypes.wintypes as wt

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
psapi = ctypes.windll.psapi

try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    pass


def proc_name(pid: int) -> str:
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    h = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not h:
        return "?"
    buf = ctypes.create_unicode_buffer(512)
    size = wt.DWORD(512)
    ok = kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size))
    kernel32.CloseHandle(h)
    if not ok:
        return "?"
    return buf.value.rsplit("\\", 1)[-1]


rows = []

WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)


def cb(hwnd, _lparam):
    if not user32.IsWindowVisible(hwnd):
        return True
    length = user32.GetWindowTextLengthW(hwnd)
    buf = ctypes.create_unicode_buffer(max(length + 1, 2))
    user32.GetWindowTextW(hwnd, buf, length + 1)
    title = buf.value.strip()

    cls = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, cls, 256)

    rect = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w = rect.right - rect.left
    h = rect.bottom - rect.top

    pid = wt.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))

    rows.append(
        (
            hwnd,
            title or "(无标题)",
            cls.value,
            w,
            h,
            rect.left,
            rect.top,
            pid.value,
            proc_name(pid.value),
        )
    )
    return True


user32.EnumWindows(WNDENUMPROC(cb), 0)

# 先打印进程名含 prompter 的
print("=== 进程名含 prompter 的窗口 ===")
hit = False
for r in rows:
    if "prompter" in r[8].lower():
        print(f"  hwnd={r[0]} title='{r[1]}' class={r[2]} {r[3]}x{r[4]} @({r[5]},{r[6]}) pid={r[7]} exe={r[8]}")
        hit = True
if not hit:
    print("  (无)")

print()
print("=== 全部可见顶层窗口（前 40 个）===")
for r in rows[:40]:
    print(f"  hwnd={r[0]} title='{r[1]}' class={r[2]} {r[3]}x{r[4]} @({r[5]},{r[6]}) pid={r[7]} exe={r[8]}")
print(f"总计 {len(rows)} 个")
