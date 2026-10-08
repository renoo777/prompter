"""出包前清理：杀掉 prompter.exe 及其 WebView2 子树。

    python scripts/kill-prompter.py            # 真杀
    python scripts/kill-prompter.py --dry-run  # 只看会杀谁

# 为什么需要它

`npm run tauri:build` 前若旧实例还在跑，cargo 收尾会报：

    error: failed to remove file `...\\target\\release\\prompter.exe`
    Caused by: 拒绝访问。 (os error 5)

★ 2026-09-27 实测：**这个错误之前的"查进程"环节本身也可能骗人。**
  同一时刻 Bash 的 `tasklist //FI ...` 与 PowerShell 工具都返回**空**，
  看着像"没有进程在跑"，实际上 prompter.exe 正从产物路径运行、锁着 exe。
  ⇒ 判断"有没有在跑"要用 here 的 `--dry-run`（ctypes 快照），别信一次空输出。

# ★★ 最危险的坑：msedgewebview2.exe 不能一锅端

机器上同时有多支 msedgewebview2.exe，父进程不同、归属完全不同：

  * 父为 **prompter.exe** 的 = 提词器的 WebView2，出包前要一起清；
  * 父为 **SearchHost.exe** 的 = **Windows 搜索界面的渲染进程**，
    杀了会掉任务栏搜索（那是系统的，不是我们的）。

⇒ 所以本脚本只按 **pid 树**杀：只清 prompter.exe 及其后代，
  并在动别人之前先算出"受保护"集合、事后复核它们还活着。
  绝不要用 `taskkill /IM msedgewebview2.exe`（那是把系统界面一起关了）。
"""
import ctypes
import subprocess
import sys
from ctypes import wintypes

TH32CS_SNAPPROCESS = 0x2


class PROCESSENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", ctypes.c_long),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", ctypes.c_char * 260),
    ]


def snapshot():
    k = ctypes.windll.kernel32
    snap = k.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    pe = PROCESSENTRY32()
    pe.dwSize = ctypes.sizeof(PROCESSENTRY32)
    out = []
    ok = k.Process32First(snap, ctypes.byref(pe))
    while ok:
        out.append((pe.th32ProcessID, pe.th32ParentProcessID, pe.szExeFile.decode("gbk", "replace")))
        ok = k.Process32Next(snap, ctypes.byref(pe))
    k.CloseHandle(snap)
    return out


def descendants(pid, procs):
    """广度优先取 pid 的全部后代（含自身）。"""
    seen, frontier = {pid}, [pid]
    changed = True
    while changed:
        changed = False
        for p, pp, _ in procs:
            if pp in seen and p not in seen:
                seen.add(p)
                changed = True
    return seen


def alive(pids):
    live = {p for p, _, _ in snapshot()}
    return sorted(pids & live)


def main():
    dry = "--dry-run" in sys.argv
    procs = snapshot()
    roots = [p for p, _, n in procs if n.lower() == "prompter.exe"]
    if not roots:
        print("没有 prompter.exe 在跑，无需清理。")
        return 0

    # 记录要保护的对象：祖先链里没有 prompter 的 webview 一律不动
    protect = set()
    by_pid = {p: (pp, n) for p, pp, n in procs}
    for p, pp, n in procs:
        if n.lower() == "msedgewebview2.exe":
            chain, cur, guard = [], pp, 0
            while cur in by_pid and guard < 20:
                chain.append(cur)
                cur = by_pid[cur][0]
                guard += 1
            if not any(p2 in roots for p2 in chain):
                protect.add(p)

    kill = set()
    for r in roots:
        kill |= descendants(r, procs)
    kill -= protect

    print("提词器根进程 pid：", sorted(roots))
    print("将清理的 pid：", sorted(kill))
    print("受保护（属其它应用）的 webview pid：", sorted(protect))
    if dry:
        for pid in sorted(kill):
            print(f"  [dry-run] 会杀 {by_pid.get(pid, (0, '?'))[1]} pid={pid}")
        print("\n(--dry-run，未执行任何操作)")
        return 0

    for pid in sorted(kill):
        name = by_pid.get(pid, (0, "?"))[1]
        r = subprocess.run(
            ["taskkill", "/F", "/PID", str(pid)],
            capture_output=True, encoding="gbk", errors="replace",
        )
        out = ((r.stdout or "") + (r.stderr or "")).strip().replace("\n", " ")
        print(f"  taskkill {name} pid={pid} rc={r.returncode} :: {out}")

    leftover = alive(kill)
    print("\n善后复核：仍在运行的 =", leftover or "无（干净）")
    print("受保护进程仍在 =", alive(protect) or "无")
    return 0 if not leftover else 1


if __name__ == "__main__":
    sys.exit(main())
