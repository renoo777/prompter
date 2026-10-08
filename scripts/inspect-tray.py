"""枚举 Windows 通知区域（托盘）里的图标 —— 回答"提词器的托盘图标到底在不在、在哪一格"。

用法：  python scripts/inspect-tray.py

原理：托盘不是一个 API，它是 explorer.exe 里的一个 **Toolbar 控件**。
  * 可见的那一段：  Shell_TrayWnd > TrayNotifyWnd > SysPager > ToolbarWindow32
  * 被折进 `^` 的那一段：顶层窗口 NotifyIconOverflowWindow > ToolbarWindow32
每个按钮的 `dwData` 指向 explorer 进程内的一个 UTF-16 提示串 —— 把它读回来就是图标名。
（TB_GETBUTTONTEXT 对跨进程工具栏要求把缓冲区放在**目标进程**里，所以先用
VirtualAllocEx 在 explorer 里开一块，再 ReadProcessMemory 取回。）

★ 这一步只读，不点击、不改变任何状态。
★ 结论怎么读：名字出现在"可见区"= 用户一眼能看见；只在"溢出区"= 默认被 Win11 折进 `^`，
  用户得先点 `^` 才看得到 —— 这正是"我点收起了，但它去哪了"的成因。
"""

import ctypes
import sys
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

PROCESS_VM_OPERATION = 0x0008
PROCESS_VM_READ = 0x0010
PROCESS_VM_WRITE = 0x0020
PROCESS_QUERY_INFORMATION = 0x0400
MEM_COMMIT = 0x1000
MEM_RESERVE = 0x2000
MEM_RELEASE = 0x8000
PAGE_READWRITE = 0x04

WM_USER = 0x0400
TB_BUTTONCOUNT = WM_USER + 24
TB_GETBUTTON = WM_USER + 23
TB_GETBUTTONTEXTW = WM_USER + 75

BUFSIZE = 0x1000
STR_OFF = 0x800  # 在远程缓冲里给字符串留的偏移

user32.FindWindowW.restype = wintypes.HWND
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.FindWindowExW.argtypes = [
    wintypes.HWND,
    wintypes.HWND,
    wintypes.LPCWSTR,
    wintypes.LPCWSTR,
]
user32.SendMessageW.restype = ctypes.c_ssize_t
user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, ctypes.c_size_t, ctypes.c_ssize_t]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]

kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.VirtualAllocEx.restype = ctypes.c_void_p
kernel32.VirtualAllocEx.argtypes = [
    wintypes.HANDLE,
    ctypes.c_void_p,
    ctypes.c_size_t,
    wintypes.DWORD,
    wintypes.DWORD,
]
kernel32.VirtualFreeEx.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD]
kernel32.ReadProcessMemory.argtypes = [
    wintypes.HANDLE,
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_size_t),
]


def read_remote(hproc, addr, size):
    buf = ctypes.create_string_buffer(size)
    got = ctypes.c_size_t(0)
    ok = kernel32.ReadProcessMemory(hproc, ctypes.c_void_p(addr), buf, size, ctypes.byref(got))
    if not ok or got.value == 0:
        return None
    return buf.raw[: got.value]


def read_remote_wstr(hproc, addr, max_chars=512):
    raw = read_remote(hproc, addr, max_chars * 2)
    if not raw:
        return None
    text = raw.decode("utf-16-le", errors="ignore")
    return text.split("\x00", 1)[0]


def buttons_of(toolbar_hwnd):
    """返回 [(序号, 控件id, 提示文本)]。失败抛异常，由调用方记账。"""
    pid = wintypes.DWORD(0)
    user32.GetWindowThreadProcessId(toolbar_hwnd, ctypes.byref(pid))
    if not pid.value:
        raise RuntimeError("拿不到工具栏所属进程")

    access = PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_QUERY_INFORMATION
    hproc = kernel32.OpenProcess(access, False, pid.value)
    if not hproc:
        raise RuntimeError(
            "OpenProcess 失败（err=%d）—— explorer 以更高完整性级别运行时读不了"
            % ctypes.get_last_error()
        )

    remote = kernel32.VirtualAllocEx(hproc, None, BUFSIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
    if not remote:
        kernel32.CloseHandle(hproc)
        raise RuntimeError("VirtualAllocEx 失败")

    out = []
    try:
        count = user32.SendMessageW(toolbar_hwnd, TB_BUTTONCOUNT, 0, 0)
        for i in range(int(count)):
            if not user32.SendMessageW(toolbar_hwnd, TB_GETBUTTON, i, remote):
                continue
            # TBBUTTON（x64，共 32 字节）：iBitmap(4) idCommand(4) fsState(1) fsStyle(1)
            #   bReserved[6] dwData(8) iString(8)
            raw = read_remote(hproc, remote, 32)
            if not raw or len(raw) < 32:
                continue
            ctrl_id = int.from_bytes(raw[4:8], "little")
            dw_data = int.from_bytes(raw[16:24], "little")
            i_string = int.from_bytes(raw[24:32], "little")

            text = None
            n = user32.SendMessageW(toolbar_hwnd, TB_GETBUTTONTEXTW, ctrl_id, remote + STR_OFF)
            if n and n > 0:
                text = read_remote_wstr(hproc, remote + STR_OFF, min(int(n) + 2, 512))
            if not text and dw_data:
                text = read_remote_wstr(hproc, dw_data)
            if not text and i_string:
                text = read_remote_wstr(hproc, i_string)
            out.append((i, ctrl_id, (text or "(读不到文本)").strip()))
    finally:
        kernel32.VirtualFreeEx(hproc, remote, 0, MEM_RELEASE)
        kernel32.CloseHandle(hproc)
    return out


WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.EnumWindows.argtypes = [WNDENUMPROC, wintypes.LPARAM]
user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]


def _class_of(hwnd):
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def _title_of(hwnd):
    buf = ctypes.create_unicode_buffer(512)
    user32.GetWindowTextW(hwnd, buf, 512)
    return buf.value


def _descend(hwnd, depth=0, maxdepth=4):
    out = []
    child = user32.FindWindowExW(hwnd, None, None, None)
    while child:
        out.append((depth, child, _class_of(child), _title_of(child)))
        if depth < maxdepth:
            out.extend(_descend(child, depth + 1, maxdepth))
        child = user32.FindWindowExW(hwnd, child, None, None)
    return out


def dump_windows():
    """把跟托盘/任务栏有关的顶层窗口及其子窗口层级打出来 —— 用于校准类名。

    Win11 把任务栏重写成了 XAML，`TrayNotifyWnd > SysPager > ToolbarWindow32`
    这套 Win10 的路径不一定还在。不猜，直接看。
    """
    hits = []

    def cb(hwnd, _lp):
        cls = _class_of(hwnd)
        if any(k in cls for k in ("Tray", "Notify", "Shell", "Task")):
            hits.append(hwnd)
        return True

    user32.EnumWindows(WNDENUMPROC(cb), 0)
    print("=== 与任务栏/托盘相关的顶层窗口 ===")
    for hwnd in hits:
        print("\n[%s]  %r  (hwnd=%d)" % (_class_of(hwnd), _title_of(hwnd), hwnd))
        for depth, child, cls, title in _descend(hwnd):
            print("   %s%-28s %r" % ("  " * depth, cls, title))
    return 0


def dump_notify_settings():
    """Win11 把每个托盘图标"显示在主区还是折进 `^`"记在注册表里：

        HKCU\\Control Panel\\NotifyIconSettings\\<hash>
            ExecutablePath   这个图标属于哪个 exe
            IsPromoted       1 = 显示在通知区域主区；0 = 折进 `^` 溢出区

    —— 这是"它到底在哪一格"的权威答案，比对着截图猜靠谱。
    """
    import winreg

    base = r"Control Panel\NotifyIconSettings"
    rows = []
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, base) as k:
            for i in range(winreg.QueryInfoKey(k)[0]):
                name = winreg.EnumKey(k, i)
                with winreg.OpenKey(k, name) as sk:

                    def gv(field):
                        try:
                            return winreg.QueryValueEx(sk, field)[0]
                        except OSError:
                            return None

                    rows.append((gv("ExecutablePath") or "?", gv("IsPromoted"), gv("InitialTooltip")))
    except OSError as e:
        print("读不到 %s：%s" % (base, e))
        return 1

    print("=== HKCU\\Control Panel\\NotifyIconSettings（Win11 托盘显示状态）===")
    print("%-6s %s" % ("主区?", "exe"))
    hit = []
    for exe, promoted, tip in sorted(rows, key=lambda r: (r[0] or "").lower()):
        mark = "★" if "prompter" in exe.lower() else " "
        print("%s %-4s %s" % (mark, "是" if promoted else "否", exe))
        if "prompter" in exe.lower():
            hit.append((exe, promoted, tip))

    print("\n=== 结论 ===")
    if not hit:
        print("★ 注册表里**没有** prompter 的条目 ⇒ 这个图标从来没有被注册进通知区域过。")
    else:
        for exe, promoted, tip in hit:
            where = "通知区域主区（一眼看得见）" if promoted else "折进了 `^` 溢出区（要先点 ^）"
            print("提词器的托盘图标：%s；提示文本 %r" % (where, tip))
    return 0


def main():
    targets = []

    tray = user32.FindWindowW("Shell_TrayWnd", None)
    if tray:
        notify = user32.FindWindowExW(tray, None, "TrayNotifyWnd", None)
        pager = user32.FindWindowExW(notify, None, "SysPager", None) if notify else None
        tb = user32.FindWindowExW(pager, None, "ToolbarWindow32", None) if pager else None
        if tb:
            targets.append(("可见区（任务栏上直接看得见的那一格）", tb))
        else:
            targets.append(("可见区", None))

    overflow = user32.FindWindowW("NotifyIconOverflowWindow", None)
    tb2 = user32.FindWindowExW(overflow, None, "ToolbarWindow32", None) if overflow else None
    targets.append(("溢出区（要点 `^` 才展开的那一块）", tb2))

    print("=== Windows 通知区域枚举 ===")
    found_keywords = []
    for label, hwnd in targets:
        print("\n--- %s ---" % label)
        if not hwnd:
            print("   （找不到这个工具栏窗口）")
            continue
        try:
            rows = buttons_of(hwnd)
        except Exception as e:  # noqa: BLE001 - 诊断脚本，任何失败都要打出来而不是吞掉
            print("   枚举失败：%s" % e)
            continue
        if not rows:
            print("   （空的）")
        for i, cid, text in rows:
            print("   [%2d] id=%-6d %s" % (i, cid, text))
            if "prompter" in text.lower() or "提词" in text:
                found_keywords.append((label, text))

    print("\n=== 结论 ===")
    if found_keywords:
        for label, text in found_keywords:
            print("提词器的托盘图标在：%s —— %r" % (label, text))
    else:
        print("★ 通知区域里**没有**找到提词器的图标（可见区与溢出区都没有）。")
    return 0


if __name__ == "__main__":
    if "--dump" in sys.argv:
        sys.exit(dump_windows())
    if "--reg" in sys.argv:
        sys.exit(dump_notify_settings())
    sys.exit(main())
