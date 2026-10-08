"""启动 exe，判断「前端到底跑没跑」——把**环境问题**和**代码问题**分开。

# 为什么需要它

真机验证穿透（`verify-click-through.py`）时踩到过一个很难分辨的症状：
**窗口出来了、托盘也有了、日志在滚，但页面是空白的。**
穿透开关是前端调的，前端没跑 ⇒ `ct_enabled` 永远是 false ⇒
"点穿透毫无反应"，看起来完全像"功能没实现"。

而"前端没跑"有**两个完全不同的原因**，处置方式相反：

  * **代码问题**：打包产物有语法/运行时报错 ⇒ 必须修代码。
  * **环境问题**：宿主不允许 WebView2 创建子进程（脚本被沙箱/受限作业对象拉起），
    或 `EBWebView` 这个 user-data-dir 被别的进程占着 ⇒ 代码没问题，
    换个方式启动（用户在桌面双击 / 关掉占用者）就好。

判据就一条：**看 `msedgewebview2.exe` 有没有被拉起来**。

  | webview 子进程数 | 日志里有 `[bootstrap]` | 结论 |
  |---|---|---|
  | 0 | — | **环境挡住了**：webview 根本没起来，代码无从谈起 |
  | >0 | 没有 | webview 起来了但页面没执行 ⇒ 多半是资源加载/打包问题 |
  | >0 | 有 | 前端正常，可以放心跑真机验证 |

`[bootstrap]` 是前端挂载后调的第一个命令（见 `commands::bootstrap` 里的路标日志），
所以它出现 = 页面真的跑起来了。

@return 打印结论行，退出码 0=前端正常 / 3=前端没跑。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_EXE = ROOT / "src-tauri" / "target" / "release" / "prompter.exe"
LOG = (Path(os.path.expandvars("%LOCALAPPDATA%")) / "cn.bobby.prompter"
       / "logs" / "Prompter.log")

kernel32 = ctypes.windll.kernel32
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


def snapshot() -> list[tuple[int, int, str]]:
    k = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if k == -1:
        return []
    out: list[tuple[int, int, str]] = []
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


def read_log() -> str:
    try:
        return LOG.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def subtree(procs, root_pid: int) -> list[tuple[int, int, str]]:
    """root_pid 的整棵进程树（只往下走一层就够了：webview 直接挂在宿主下）。"""
    kids = [p for p in procs if p[1] == root_pid]
    out = list(kids)
    for pid, _, _ in kids:
        out += subtree(procs, pid)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(DEFAULT_EXE))
    ap.add_argument("--wait", type=float, default=9.0, help="等前端就位的秒数")
    args = ap.parse_args()

    exe = Path(args.exe)
    if not exe.exists():
        raise SystemExit(f"找不到 exe：{exe}")

    # ★ debug 产物会去连 devUrl，必然白页 —— 先提醒一句，省得白查一轮
    is_debug = exe.parent.name.lower() == "debug"
    if is_debug:
        print("⚠️  这是 debug 产物：它加载 devUrl（http://localhost:5273）而不是内嵌资源。")
        print("   没有 vite 在跑时必然是白页 —— 真机验证请改用 release 产物。")

    before = len(read_log())

    # ★ 尽量还原"用户双击"的启动条件，否则会拿一个**假失败**去怀疑代码：
    #   * cwd —— 双击时是 exe 所在目录；从脚本拉起时会继承脚本的 cwd。
    #     WebView2 / 资源相对路径一旦与 cwd 有关，两者就不同。
    #   * 环境变量 —— 脚本的宿主会往环境里塞垫片（`PYTHONPATH` 的 safe-delete shim、
    #     `NODE_OPTIONS` 等）。这些是**宿主的东西**，不该继承给被测应用。
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)

    proc = subprocess.Popen(
        [str(exe)], close_fds=True, cwd=str(exe.parent), env=env,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    print(f"已拉起 {exe.name} (pid={proc.pid})，cwd={exe.parent}，等 {args.wait:g} 秒…")

    boot = ""
    deadline = time.time() + args.wait
    while time.time() < deadline:
        for line in read_log()[before:].splitlines():
            if "[bootstrap]" in line:
                boot = line.strip()
                break
        if boot:
            break
        time.sleep(0.4)

    procs = snapshot()
    kids = subtree(procs, proc.pid)
    wv = [p for p in kids if p[2].lower() == "msedgewebview2.exe"]
    print()
    print(f"  进程树（{len(kids)} 个子孙进程）：")
    for pid, ppid, name in kids[:14]:
        print(f"    pid={pid:<7} ppid={ppid:<7} {name}")
    if len(kids) > 14:
        print(f"    …还有 {len(kids) - 14} 个")
    print(f"  msedgewebview2.exe 子进程数 = {len(wv)}")
    print(f"  日志里的 [bootstrap] = {boot or '（无）'}")

    subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                   capture_output=True, env=dict(os.environ))
    time.sleep(0.6)
    # 清掉可能留下的、已经没有宿主的 WebView2 进程
    alive = {p for p, _, _ in snapshot()}
    for pid, ppid, name in snapshot():
        if name.lower() == "msedgewebview2.exe" and ppid not in alive:
            subprocess.run(["taskkill", "/F", "/PID", str(pid)],
                           capture_output=True, env=dict(os.environ))

    print()
    if boot:
        print("✅ 前端正常：页面跑起来了。（webview 子进程 %d 个）" % len(wv))
        return 0
    if not wv:
        print("❌ 结论：**环境挡住了** —— WebView2 连子进程都没拉起来，")
        print("   所以这不是代码问题。宿主可能不允许创建子进程（受限作业对象/沙箱），")
        print("   或者 EBWebView 这个 user-data-dir 被别的进程占着。")
        print("   换一个启动方式（桌面上双击 / 关掉占用者）再试。")
        return 3
    print("❌ 结论：**像是代码/资源问题** —— webview 起来了（%d 个子进程）" % len(wv))
    print("   但页面从没执行。检查打包产物能否加载（release 应内嵌 dist）。")
    return 3


if __name__ == "__main__":
    sys.exit(main())
