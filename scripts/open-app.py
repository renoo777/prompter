"""打开 Prompter（用最新的 release 产物），并确认**界面真的跑起来了**。

为什么不能双击 exe 了事 —— 三件事都是实测踩出来的：

1. 「窗口出现」不等于「界面跑起来」。本项目的失败形态是**窗口在、托盘在、页面空白**
   （日志里没有 `[bootstrap]`）：这种实例能看能点，但里面什么都没有。
   所以脚本把「`[bootstrap]` + 可交互区已上报」两条日志当作就位判据，
   **两条都到才算打开成功** —— 只看前者会把"页面执行了但停在半路"放进来。
2. 启动期有 flaky：同一个 exe 实测有「能起 / 不能起」两种结果（WebView2 启动期一次性失败）。
   → 脚本自动重试 3 次，最后一次把 profile 换成全新的（同 `verify-click-through.py`）。
3. 应用**没有单实例守卫**：已经开着一个时再双击会起第二个。→ 在跑就只把它唤到前台，
   不重复启动；真要换成新产物用 `--restart`（**优雅关闭**，不用 taskkill 硬杀 —— 硬杀会让
   下一次启动更容易 flaky）。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import importlib.util
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

user32 = ctypes.windll.user32

WM_CLOSE = 0x0010
SW_RESTORE = 9
DETACHED_PROCESS = 0x00000008
CREATE_NEW_PROCESS_GROUP = 0x00000200
# ★ 本轮实测踩出来的：只给 DETACHED_PROCESS 不够 —— 脚本退出、命令结束后，
#   宿主（沙箱）把子进程一起收走了，日志停在「隐身已开启」之后没有任何退出记录
#   （= 被 TerminateProcess，不是自己退出的）。要显式要求脱离宿主的 job object。
CREATE_BREAKAWAY_FROM_JOB = 0x01000000


def _load_verify():
    """加载 `verify-click-through.py`（文件名带连字符，只能这样 import）。

    它模块级只有常量定义，没有副作用（main 在 `__main__` 守卫里），
    所以这里只为复用它的日志/进程/窗口辅助函数，**不会**动老大的配置。
    """
    spec = importlib.util.spec_from_file_location("verify_ct", HERE / "verify-click-through.py")
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


v = _load_verify()


def running_pids() -> list[int]:
    return [p for p, _, n in v.snapshot_processes() if n.lower() == "prompter.exe"]


def focus_existing() -> str:
    """把已经在跑的实例唤到前台。返回描述。

    为什么用 ShowWindow+SetForegroundWindow 而不是重启：重启会丢掉老大当前的状态，
    而"打开软件"这个诉求本身就只是"我要看到它"。
    """
    for pid in running_pids():
        hwnd = v.find_main_window(pid)
        if hwnd:
            user32.ShowWindow(hwnd, SW_RESTORE)
            # SetForegroundWindow 对非前台进程有限制，失败不致命（窗口至少被唤起）
            user32.SetForegroundWindow(hwnd)
            return f"已经在跑（pid={pid}），已把它唤到前台，没重复启动"
    return ""


def close_gracefully(timeout: float = 8.0) -> int:
    """向每个实例的主窗发 WM_CLOSE 让它自己走完退出流程。返回送出的窗数。

    ★ 不用 `taskkill /F`：硬杀会让 WebView2 下次启动更容易 flaky（实测）。
      关闭请求发不动（窗口没响应）时才退到 taskkill，并**明说**做了这件事。
    """
    sent = 0
    targets: list[int] = []
    for pid in running_pids():
        hwnd = v.find_main_window(pid)
        if hwnd:
            user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
            targets.append(pid)
            sent += 1

    deadline = time.time() + timeout
    while time.time() < deadline:
        if not running_pids():
            break
        time.sleep(0.3)

    left = running_pids()
    if left:
        print(f"  窗口没有响应关闭请求，退到强制结束（pid={left}）")
        for pid in left:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)],
                           capture_output=True)
        time.sleep(0.8)
        v.kill_orphan_webviews()
    return sent


def _popen(exe: Path, flags: int) -> subprocess.Popen:
    return subprocess.Popen(
        [str(exe)], cwd=str(exe.parent), close_fds=True, creationflags=flags,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL)


def spawn_via_wmi(exe: Path) -> int:
    """让 **WMI 服务**（WmiPrvSE.exe）替我们把进程拉起来，返回 pid。

    ★★ 为什么非绕这一道（本轮实测）：
      从助手的 shell 里直接 `Popen`，哪怕带 `DETACHED_PROCESS`，命令一结束应用就没了 ——
      日志停在启动成功那一刻、连一句退出记录都没有，这是**被宿主 job 回收**的指纹。
      试过 `CREATE_BREAKAWAY_FROM_JOB`：宿主 job 没开 breakaway 开关，直接 `WinError 5`。
      WMI 服务不在宿主的 job 里，它创建的进程就是真正的独立进程。

    Win32_Process.Create 顺带还避开了宿主注入的环境变量（`PYTHONPATH`/`NODE_OPTIONS`），
    对被测程序反而更接近老大自己双击的效果。
    """
    esc = str(exe).replace("'", "''")
    cwd = str(exe.parent).replace("'", "''")
    ps = (
        "$ErrorActionPreference='Stop';"
        "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create "
        f"-Arguments @{{CommandLine='\"{esc}\"'; CurrentDirectory='{cwd}'}};"
        "Write-Output (\"$($r.ReturnValue)|$($r.ProcessId)\")"
    )
    out = subprocess.run(
        ["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
        capture_output=True, text=True, timeout=90,
        env={k: val for k, val in os.environ.items()
             if k not in ("PYTHONPATH", "NODE_OPTIONS")})
    line = (out.stdout or "").strip().splitlines()
    if not line or "|" not in line[-1]:
        raise RuntimeError(f"WMI 启动没有返回结果：{(out.stdout or '').strip()} "
                           f"{(out.stderr or '').strip()[:200]}")
    rc, pid = line[-1].split("|", 1)
    if rc.strip() != "0":
        raise RuntimeError(f"Win32_Process.Create 失败，ReturnValue={rc.strip()}")
    return int(pid)


def launch(exe: Path, force_wmi: bool = False) -> tuple[int, str]:
    """拉起应用，返回 (pid, 用了哪种方式)。"""
    if not force_wmi:
        try:
            proc = _popen(exe, DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
                          | CREATE_BREAKAWAY_FROM_JOB)
            time.sleep(1.5)
            if proc.poll() is None:
                return proc.pid, "breakaway"
        except OSError:
            pass
        # 宿主 job 不允许 breakaway ⇒ 走 WMI。
    pid = spawn_via_wmi(exe)
    return pid, "wmi"


def wait_ready(pid: int, log_before: int,
               timeout: float = 28.0) -> tuple[int | None, str, bool]:
    """等三件事：窗口出现 → `[bootstrap]` → 可交互区上报。返回 (hwnd, bootstrap行, 区域到了吗)。"""
    hwnd = None
    deadline = time.time() + timeout
    while time.time() < deadline:
        hwnd = v.find_main_window(pid)
        if hwnd:
            break
        if pid not in running_pids():   # 进程自己退了（崩了）
            return None, "", False
        time.sleep(0.3)
    if not hwnd:
        return None, "", False

    boot = v.wait_frontend(log_before)
    if not boot:
        return hwnd, "", False
    zones = v.wait_zones(log_before)
    return hwnd, boot, zones


def main() -> int:
    ap = argparse.ArgumentParser(description="打开 Prompter 并确认界面就位")
    ap.add_argument("--restart", action="store_true",
                    help="已经在跑时也换成当前产物（先优雅关闭）")
    ap.add_argument("--exe", default=str(v.DEFAULT_EXE), help="要启动的产物")
    ap.add_argument("--tries", type=int, default=3, help="启动尝试次数")
    ap.add_argument("--force-wmi", action="store_true",
                    help="强制走 WMI 启动（诊断用；默认先试 breakaway）")
    args = ap.parse_args()

    exe = Path(args.exe)
    if not exe.exists():
        print(f"找不到产物：{exe}")
        print("  先构建：npm run tauri:build（★ 原始 cargo build 不嵌前端，会白屏）")
        return 2

    existing = running_pids()
    if existing and not args.restart:
        msg = focus_existing()
        print(msg or f"已经在跑（pid={existing}）")
        return 0

    if existing:
        print(f"关闭在跑的实例（pid={existing}）…")
        close_gracefully()
        time.sleep(1.0)

    print(f"启动 {exe.name}（{time.strftime('%H:%M:%S')}，产物 {time.strftime('%m-%d %H:%M', time.localtime(exe.stat().st_mtime))}）")

    hwnd = None
    boot = ""
    zones = False
    pid = None
    how = ""
    for attempt in range(1, args.tries + 1):
        log_before = len(v.read_log())
        pid, how = launch(exe, force_wmi=args.force_wmi)
        hwnd, boot, zones = wait_ready(pid, log_before)

        if boot and zones:
            break

        why = ("窗口都没出来" if not hwnd
               else "窗口在，但页面没执行（空白）" if not boot
               else "页面执行了，但停在半路（可交互区没上报）")
        print(f"  第 {attempt}/{args.tries} 次失败：{why}")
        if pid in running_pids():
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)],
                           capture_output=True)
            time.sleep(0.8)
            v.kill_orphan_webviews()
        if attempt < args.tries:
            if attempt == args.tries - 1:
                moved = v.reset_profile()
                print("  最后一次换成全新 profile 再试"
                      + (f"（旧的挪到 {moved}）" if moved else ""))
            else:
                print("  重试…")

    print()
    if not (boot and zones):
        print("❌ 没能打开：连试 %d 次界面都没跑起来。" % args.tries)
        print("   先别怀疑业务代码 —— 这是 WebView2 启动期的问题（见 scripts/README 的脚本表）。")
        print(f"   日志：{v.LOG}")
        return 3

    print("✅ 已打开，界面跑起来了")
    print(f"   进程 pid = {pid}（{how} 方式启动 = 独立于本会话，不会随命令结束消失）")
    print(f"   主窗句柄 = {hwnd}")
    print(f"   {boot}")
    print("   可交互区已上报 ✓（穿透判定要用它，没有它穿透会一直不触发）")

    # 隐身只是对屏幕捕获隐身，老大本人看得见 —— 但他要给我截图时得知道这一条。
    import json
    try:
        stealth = json.loads(v.CONFIG.read_text(encoding="utf-8")).get("stealth_on_start")
    except Exception:
        stealth = None
    if stealth:
        print("   注：开了开机隐身（防截屏），你自己看得见；要我帮你截图看界面的话先关掉它。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
