"""量 Windows 任务栏上的按钮 —— 回答"提词器到底在不在任务栏上"。

用法：
    python scripts/check-taskbar-button.py                 # 只报告窗口侧状态，不动任何东西
    python scripts/check-taskbar-button.py --selftest      # 调 AddTab/DeleteTab，看返回码
    python scripts/check-taskbar-button.py --pixel         # 截图对比：按钮真的出现了吗

# 本机（Win11 + 2520×1680，缩放 1.75）实测结论，2026-09-27

| 手段 | 结果 |
|---|---|
| `MSTaskListWClass` 数按钮 | ❌ **数不到**。Win11 任务栏是 XAML 渲染（任务栏下有 `DesktopWindowContentBridge`），这条老 Toolbar 是 `visible=False` 的空壳，`TB_BUTTONCOUNT` 恒为 0 |
| 查 `WS_EX_TOOLWINDOW` | ❌ **会得到假结论**。tao 的 `set_skip_taskbar` 用的是 COM `ITaskbarList::AddTab/DeleteTab`，**没有动那个样式位** |
| `AddTab` / `DeleteTab` 返回码 | ✅ 都是 `S_OK` |
| **截图对比（`--pixel`）** | ✅ **AddTab 后任务栏确实多出一枚按钮**（34163 像素差异，落在按钮区），DeleteTab 后回到原样（残留 2554 像素＝重绘噪声） |

# 为什么需要它

「收进任务栏」这一段的风险**不在我们的代码里**，而在一个 Windows 事实：
`set_skip_taskbar(false)` 走的是 tao 的 `ITaskbarList::AddTab`
（`tao/src/platform_impl/windows/window.rs`），能不能让按钮真的出现，
是微软的行为，不是我们能断言的 —— 只能量。

# 两个坑（都真踩过）

1. **坐标口径**：本进程必须 DPI 感知（见 `_make_dpi_aware`），否则
   `GetWindowRect` 给虚拟化坐标、`ImageGrab` 按物理像素截，截到的是一条
   屏幕中部的无关内容 —— 而差异恰好是 0，看着特别像"按钮没出现"。
2. **重绘滞后**：explorer 处理 Add/DeleteTab 是异步的。截得太快会拿到上一帧，
   现象是"AddTab 那张没差异、DeleteTab 那张反而有差异"。每次调用后要等约 0.5s。

★ 脚本默认只读；`--selftest` / `--pixel` 会去动，但动的是**提词器自己**的按钮，
  且结尾一定 `DeleteTab` 还原。
"""

import ctypes
import time
import sys
from ctypes import wintypes
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
ole32 = ctypes.WinDLL("ole32", use_last_error=True)


def _make_dpi_aware() -> None:
    """让本进程按**物理像素**工作。

    ★★ 不设它的后果是"两个口径打架"：Windows 会把坐标**虚拟化**给一个
      非 DPI 感知的进程（按缩放比缩小），而 `ImageGrab.grab(bbox=…)`
      是按**物理像素**截的。于是 `GetWindowRect` 说"任务栏在 y=912"、
      实际在 y=1596（本机缩放 1.75），截出来的是屏幕中部一条无关内容 ——
      **而差异会是 0 像素**，看着像"按钮没出现"，其实是根本没截到任务栏。
      （本项目在离线预览那侧踩过同一个坑的另一半：Edge 两种模式对视口口径的解释
        不一样，`--window-size` 一个当外框一个当视口。）
    """
    try:
        # 2 = PROCESS_PER_MONITOR_DPI_AWARE
        ctypes.WinDLL("shcore").SetProcessDpiAwareness(2)
        return
    except Exception:  # noqa: BLE001 - 老系统没有 shcore
        pass
    try:
        user32.SetProcessDPIAware()
    except Exception:  # noqa: BLE001
        pass


_make_dpi_aware()

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
STR_OFF = 0x800

WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

user32.FindWindowW.restype = wintypes.HWND
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.SendMessageW.restype = ctypes.c_ssize_t
user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, ctypes.c_size_t, ctypes.c_ssize_t]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindow.argtypes = [wintypes.HWND]
user32.IsWindow.restype = wintypes.BOOL
user32.IsIconic.argtypes = [wintypes.HWND]
user32.IsIconic.restype = wintypes.BOOL
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.IsWindowVisible.restype = wintypes.BOOL
user32.EnumWindows.argtypes = [WNDENUMPROC, wintypes.LPARAM]
user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]

kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.VirtualAllocEx.restype = ctypes.c_void_p
kernel32.VirtualAllocEx.argtypes = [
    wintypes.HANDLE, ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD, wintypes.DWORD
]
kernel32.VirtualFreeEx.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD]
kernel32.ReadProcessMemory.argtypes = [
    wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)
]


# --------------------------------------------------------------------------
# 小工具
# --------------------------------------------------------------------------


def proc_name(pid: int) -> str:
    """进程 exe 文件名。拿不到就回空串 —— 诊断脚本不该因为一个名字而挂掉。"""
    h = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return ""
    try:
        buf = ctypes.create_unicode_buffer(1024)
        size = wintypes.DWORD(1024)
        # QueryFullProcessImageNameW 没声明 argtypes 也能调，够用
        if not kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)):
            return ""
        return buf.value.rsplit("\\", 1)[-1]
    finally:
        kernel32.CloseHandle(h)


def window_pid(hwnd: int) -> int:
    pid = wintypes.DWORD(0)
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def prompter_windows():
    """所有属于 prompter.exe 的顶层窗口（含标题/类名/最小化状态）。"""
    found = []

    def cb(hwnd, _lp):
        if proc_name(window_pid(hwnd)).lower() != "prompter.exe":
            return True
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        title = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, title, 512)
        found.append(
            {
                "hwnd": hwnd,
                "class": cls.value,
                "title": title.value,
                "visible": bool(user32.IsWindowVisible(hwnd)),
                "iconic": bool(user32.IsIconic(hwnd)),
            }
        )
        return True

    user32.EnumWindows(WNDENUMPROC(cb), 0)
    return found


# --------------------------------------------------------------------------
# 任务栏按钮列表
# --------------------------------------------------------------------------


def _read_remote(hproc, addr, size):
    buf = ctypes.create_string_buffer(size)
    got = ctypes.c_size_t(0)
    if not kernel32.ReadProcessMemory(hproc, ctypes.c_void_p(addr), buf, size, ctypes.byref(got)):
        return None
    return buf.raw[: got.value] or None


def taskbar_list_diag() -> dict:
    """任务栏按钮列表那个窗口自己的状态。

    ★ 需要它是因为 Win11 把任务栏也重写成了 XAML（`DesktopWindowXamlSource`），
      `MSTaskListWClass` 很可能只是一个**空壳** —— 它还在窗口树里，
      但按钮其实是 XAML 元素、不在它里面。
      于是"TB_BUTTONCOUNT 回 0"有两种完全不同的解释：
        (a) 任务栏真的没有按钮；(b) 我们数错了地方。
      尺寸 / 可见性能把这两者分开 —— 空壳的矩形是 0×0 或者根本不可见。
      不把这个前缀量出来，就会拿 (b) 去证明 (a)。
    """
    tray = user32.FindWindowW("Shell_TrayWnd", None)
    rebar = user32.FindWindowExW(tray, None, "ReBarWindow32", None) if tray else None
    switcher = user32.FindWindowExW(rebar, None, "MSTaskSwWClass", None) if rebar else None
    toolbar = user32.FindWindowExW(switcher, None, "MSTaskListWClass", None) if switcher else None

    info = {"tray": tray, "rebar": rebar, "switcher": switcher, "toolbar": toolbar}
    if toolbar:
        r = wintypes.RECT()
        user32.GetWindowRect(toolbar, ctypes.byref(r))
        info["rect"] = (r.left, r.top, r.right - r.left, r.bottom - r.top)
        info["visible"] = bool(user32.IsWindowVisible(toolbar))
        info["count"] = int(user32.SendMessageW(toolbar, TB_BUTTONCOUNT, 0, 0))
    # Win11 的任务栏正文在 XAML bridge 里
    if tray:
        info["xaml"] = bool(
            user32.FindWindowExW(tray, None, "Windows.UI.Composition.DesktopWindowContentBridge", None)
        )
    return info


def taskbar_buttons():
    """任务栏按钮列表 → [(序号, 窗口句柄或 None, 提示文本)]。

    失败时抛 RuntimeError（调用方负责把原因打出来，不要静默吞掉）。
    """
    info = taskbar_list_diag()
    toolbar = info.get("toolbar")
    if not toolbar:
        raise RuntimeError("找不到任务栏按钮列表（MSTaskListWClass）")

    pid = window_pid(toolbar)
    access = PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_QUERY_INFORMATION
    hproc = kernel32.OpenProcess(access, False, pid)
    if not hproc:
        raise RuntimeError(f"OpenProcess 失败（err={ctypes.get_last_error()}）")
    remote = kernel32.VirtualAllocEx(hproc, None, BUFSIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
    if not remote:
        kernel32.CloseHandle(hproc)
        raise RuntimeError("VirtualAllocEx 失败")

    out = []
    try:
        count = int(user32.SendMessageW(toolbar, TB_BUTTONCOUNT, 0, 0))
        for i in range(count):
            if not user32.SendMessageW(toolbar, TB_GETBUTTON, i, remote):
                continue
            raw = _read_remote(hproc, remote, 32)
            if not raw or len(raw) < 32:
                continue
            ctrl_id = int.from_bytes(raw[4:8], "little")
            dw_data = int.from_bytes(raw[16:24], "little")

            # 任务按钮的 dwData 指向 explorer 内的一个结构，头 8 字节是 HWND。
            hwnd = None
            if dw_data:
                hb = _read_remote(hproc, dw_data, 8)
                if hb and len(hb) == 8:
                    cand = int.from_bytes(hb, "little")
                    if cand and user32.IsWindow(cand):
                        hwnd = cand

            text = None
            n = user32.SendMessageW(toolbar, TB_GETBUTTONTEXTW, ctrl_id, remote + STR_OFF)
            if n and n > 0:
                raw_t = _read_remote(hproc, remote + STR_OFF, min(int(n) + 2, 512) * 2)
                if raw_t:
                    text = raw_t.decode("utf-16-le", errors="ignore").split("\x00", 1)[0]
            out.append((i, hwnd, (text or "").strip()))
    finally:
        kernel32.VirtualFreeEx(hproc, remote, 0, MEM_RELEASE)
        kernel32.CloseHandle(hproc)
    return out


def has_taskbar_button(hwnd: int) -> tuple[bool, list]:
    rows = taskbar_buttons()
    return any(r[1] == hwnd for r in rows), rows


# --------------------------------------------------------------------------
# 机制自测：直接调 ITaskbarList，看按钮真的会多一个/少一个
# --------------------------------------------------------------------------


def _taskbar_list():
    """拿到 ITaskbarList。用 vtable 直接调 —— 不引入 pywin32。"""
    ole32.CoInitializeEx(None, 0x2)  # APARTMENTTHREADED

    class GUID(ctypes.Structure):
        _fields_ = [
            ("a", wintypes.DWORD),
            ("b", wintypes.WORD),
            ("c", wintypes.WORD),
            ("d", ctypes.c_ubyte * 8),
        ]

    # CLSID_TaskbarList {56FDF344-FD6D-11d0-958A-006097C9A090}
    # IID_ITaskbarList  {56FDF342-FD6D-11d0-958A-006097C9A090}
    clsid = GUID(
        0x56FDF344,
        0xFD6D,
        0x11D0,
        (ctypes.c_ubyte * 8)(0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90),
    )
    iid = GUID(
        0x56FDF342,
        0xFD6D,
        0x11D0,
        (ctypes.c_ubyte * 8)(0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90),
    )
    ptr = ctypes.c_void_p()
    hr = ole32.CoCreateInstance(
        ctypes.byref(clsid), None, 0x1 | 0x4, ctypes.byref(iid), ctypes.byref(ptr)  # INPROC|LOCAL
    )
    if hr != 0 or not ptr:
        raise RuntimeError(f"CoCreateInstance(TaskbarList) 失败 hr=0x{hr & 0xFFFFFFFF:X}")
    return ptr


def _vt_call(ptr, index, *args):
    """按 vtable 下标调 COM 方法。ITaskbarList: 0..2 是 IUnknown，3=HrInit，4=AddTab，5=DeleteTab。"""
    vtbl = ctypes.cast(ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    fn = ctypes.WINFUNCTYPE(ctypes.c_long, ctypes.c_void_p, *[ctypes.c_void_p] * len(args))(
        vtbl[index]
    )
    return fn(ptr, *args)


def _grab_taskbar():
    """截任务栏那一条。返回 (PIL 图, bbox)。"""
    from PIL import ImageGrab

    tray = user32.FindWindowW("Shell_TrayWnd", None)
    if not tray:
        raise RuntimeError("找不到 Shell_TrayWnd")
    r = wintypes.RECT()
    user32.GetWindowRect(tray, ctypes.byref(r))
    bbox = (r.left, r.top, r.right, r.bottom)
    return ImageGrab.grab(bbox=bbox), bbox


def _diff(a, b, thresh: int = 10):
    """两张图有多少像素不一样，以及差异落在哪儿。

    ★ 卡一个阈值（默认 10/255）是有意的：截图里会有压缩/抖动噪声，
      不卡阈值的话"时钟跳了一分钟"和"多了一枚按钮"分不开。
    """
    from PIL import ImageChops

    d = ImageChops.difference(a.convert("RGB"), b.convert("RGB")).convert("L")
    mask = d.point(lambda v: 255 if v > thresh else 0)
    return mask.getbbox(), sum(1 for p in mask.getdata() if p)


def pixel_selftest() -> int:
    """截图法验证：AddTab / DeleteTab 真的让任务栏**看起来**变了吗。

    ★★ 为什么绕这一圈：Win11 的任务栏是 XAML 渲染的，按钮不在
      `MSTaskListWClass` 里，数不出来（见 `taskbar_list_diag`）。
      但"多了一个按钮"这件事**一定能被看见** —— 所以直接看像素。
      而且提词器默认开着"隐身"（`WDA_EXCLUDEFROMCAPTURE`），
      它自己的窗口不会被截进图里，任务栏反而是干净的。

    ★ 已知噪声源：任务栏右侧的时钟每分钟会跳一次。所以除了"差异像素数"，
      还要把**差异区域的位置**打出来 —— 按钮长在左边/中部，时钟在最右，
      两者一眼可分。
    """
    wins = [w for w in prompter_windows() if w["title"]]
    if not wins:
        print("★ 没找到提词器的窗口 —— 先把应用打开再跑这个自测。")
        return 1
    hwnd = wins[0]["hwnd"]

    try:
        before, bbox = _grab_taskbar()
    except Exception as e:  # noqa: BLE001 - 诊断脚本，失败要说出来
        print(f"截图失败：{e}")
        return 1
    print(f"任务栏区域 bbox={bbox} 尺寸={before.size}")

    ptr = _taskbar_list()
    _vt_call(ptr, 3)  # HrInit

    # ★★ 每次调用之后都要**等任务栏重绘**再截图。
    #   第一次跑的时候没等，结果 AddTab 那张图差异为 0、DeleteTab 那张图反而有差异 ——
    #   原因是 explorer 处理 AddTab/DeleteTab 是异步的：截图抢在重绘之前，
    #   拿到的是上一帧。这个"数字看着像反了"的现象，就是渲染滞后造成的。
    def settle(seconds: float = 0.5) -> None:
        time.sleep(seconds)

    _vt_call(ptr, 4, ctypes.c_void_p(hwnd))  # AddTab
    settle()
    after_add, _ = _grab_taskbar()
    box_add, n_add = _diff(before, after_add)

    _vt_call(ptr, 5, ctypes.c_void_p(hwnd))  # DeleteTab
    settle()
    after_del, _ = _grab_taskbar()
    box_del, n_del = _diff(before, after_del)

    # ★ 把三张图存下来。差异是 0 的时候，"机制不成立"和"我截错了地方"
    #   长得一模一样 —— 只有把图存下来看一眼才能分开。
    outdir = ROOT / "shots" / "taskbar-test"
    outdir.mkdir(parents=True, exist_ok=True)
    for tag, img in (("before", before), ("after-add", after_add), ("after-del", after_del)):
        img.save(outdir / f"{tag}.png")
    print(f"三张图已存到 {outdir}")

    print(f"AddTab  之后与之前：差异 {n_add:5d} 像素  bbox={box_add}")
    print(f"DeleteTab 之后与之前：差异 {n_del:5d} 像素  bbox={box_del}")

    print()
    ok = True
    if n_add < 50 or not box_add:
        print("★ AddTab 之后任务栏**没有可见变化** ⇒ 按钮没有出现，机制在本机不成立。")
        ok = False
    else:
        w = box_add[2] - box_add[0]
        print(f"✅ AddTab 之后任务栏多出一块可见东西（宽约 {w}px，位置 {box_add}）")
        if w > 120:
            print("   ★ 差异区域偏宽是**正常的**：多一枚按钮会让整组按钮重新居中，")
            print("     于是整排都挪了几像素 —— 判据是『有差异 + DeleteTab 能回到原样』，")
            print("     不是『差异框跟按钮一样大』。想确认是不是真的多了一个按钮，看图：")
            print(f"     {(ROOT / 'shots' / 'taskbar-test').as_posix()}/before.png 与 after-add.png 并排对比。")
    if n_del > max(200, n_add // 4):
        print("★ DeleteTab 之后没回到原样（残留或时钟刚好跳了）—— 重跑一次确认。")
        ok = False
    else:
        print(f"✅ DeleteTab 之后基本回到原样（差异 {n_del} 像素）")
    return 0 if ok else 2


def _hr(x: int) -> str:
    x &= 0xFFFFFFFF
    name = {0x00000000: "S_OK", 0x80004005: "E_FAIL", 0x80070057: "E_INVALIDARG"}.get(x, "?")
    return f"0x{x:08X} {name}"


def selftest() -> int:
    """直接调 ITaskbarList，看这两个调用**被不被接受**。

    ★★ 它能证明什么、不能证明什么，要写清楚：
      * 能证明：命令走的是正确接口、传进去的窗口句柄被接受（返回 S_OK
        而不是 E_INVALIDARG / E_FAIL）。这是这一段最容易错的地方 ——
        接口/句柄不对时 COM 是**会明确报错**的。
      * 不能证明：按钮"肉眼可见"。Win11 的任务栏是 XAML 渲染，
        按钮不在 `MSTaskListWClass` 里（见 `taskbar_list_diag` 的说明），
        所以数不出来；这一步只能落到真机看一眼。
    """
    wins = [w for w in prompter_windows() if w["title"]]
    if not wins:
        print("★ 没找到提词器的窗口 —— 先把应用打开再跑这个自测。")
        return 1
    win = wins[0]
    hwnd = win["hwnd"]
    print(f"目标窗口 hwnd={hwnd} title={win['title']!r} minimized={win['iconic']}")

    diag = taskbar_list_diag()
    countable = diag.get("count", 0) > 0
    print(
        f"任务栏按钮列表：count={diag.get('count')} xaml={diag.get('xaml')} "
        f"⇒ 计数{'可用' if countable else '不可用（Win11 XAML 任务栏，数不到）'}"
    )

    ptr = _taskbar_list()
    hr_init = _vt_call(ptr, 3)  # HrInit
    hr_add = _vt_call(ptr, 4, ctypes.c_void_p(hwnd))  # AddTab
    hr_del = _vt_call(ptr, 5, ctypes.c_void_p(hwnd))  # DeleteTab
    print(f"HrInit()   → {_hr(hr_init)}")
    print(f"AddTab()   → {_hr(hr_add)}")
    print(f"DeleteTab()→ {_hr(hr_del)}")

    print()
    ok = True
    if hr_add != 0:
        print("★ AddTab 被拒绝了 —— 机制不成立，需要换实现（不能只放 skip_taskbar）。")
        ok = False
    if hr_del != 0:
        print("★ DeleteTab 被拒绝了 —— 那「收起来之后撤不掉按钮」会变成残留。")
        ok = False
    if countable:
        print("（本机计数可用，如需断言按钮数量请自行加，见文件头的说明。）")
    if ok:
        print("✅ 两个调用都被接受（S_OK）。可见性那一步要靠真机看一眼 —— 见文件头。")
    return 0 if ok else 2


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    if "--pixel" in sys.argv:
        return pixel_selftest()

    print("=== 提词器的窗口 ===")
    wins = prompter_windows()
    if not wins:
        print("   （没有 prompter.exe 的顶层窗口 —— 应用没在跑？）")
    for w in wins:
        print(
            f"   hwnd={w['hwnd']:<10} visible={w['visible']} minimized={w['iconic']} "
            f"class={w['class']!r} title={w['title']!r}"
        )

    try:
        rows = taskbar_buttons()
    except RuntimeError as e:
        print(f"\n读任务栏失败：{e}")
        return 1

    diag = taskbar_list_diag()
    print("\n=== 任务栏按钮列表那个窗口（先看它是不是空壳）===")
    print(f"   hwnd={diag.get('toolbar')} rect={diag.get('rect')} visible={diag.get('visible')}")
    print(f"   TB_BUTTONCOUNT={diag.get('count')}  任务栏有 XAML bridge={diag.get('xaml')}")
    if diag.get("count") == 0 and diag.get("xaml"):
        print("   ★ 计数为 0 且任务栏是 XAML 渲染 ⇒ 这是**数错了地方**，")
        print("     不是「没有按钮」。Win11 的按钮是 XAML 元素，不在这条老 Toolbar 里。")
        print("     所以本脚本的计数/列表在 Win11 上**不能**用来判断某窗口在不在任务栏上；")
        print("     它仍然能用的部分是：窗口侧状态、以及 --selftest 的 AddTab/DeleteTab 返回码。")

    print(f"\n=== 任务栏按钮（共 {len(rows)} 个）===")
    for i, hwnd, text in rows:
        mark = ""
        if hwnd:
            name = proc_name(window_pid(hwnd))
            if name.lower() == "prompter.exe":
                mark = "  ★ 这是提词器"
            text = f"{text or '(无标题)'} [{name}]"
        print(f"   [{i:2d}] hwnd={str(hwnd):<10} {text}{mark}")

    print("\n=== 结论 ===")
    for w in wins:
        if not w["title"]:
            continue
        on_bar = any(r[1] == w["hwnd"] for r in rows)
        print(f"提词器窗口 hwnd={w['hwnd']}：{'在任务栏上 ✅' if on_bar else '不在任务栏上'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
