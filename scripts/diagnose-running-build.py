"""判断「现在跑着的到底是哪一版」，以及「能不能重新构建」。

# 为什么需要这个脚本

这不是一个抽象的好习惯，它是踩出来的：

上一轮加了八个新功能，交付后用户回来说「XX 根本没做」。查下去发现
**他跑的是改动之前那个 exe** —— 磁盘上确实有新版代码，但没人重启过应用。
那次是靠翻数据目录（新手版本会写 `scripts.json`、`config.json` 里多出新字段）
才反推出结论的，花了十几分钟。

同一轮还撞上第二件事：想重新构建时链接器报
`LNK1104: 无法打开文件 …\\release\\prompter.exe` —— 旧实例还开着，文件被锁死。
第一反应是"把 exe 改个名字绕过去"，**没用**：
`release\\prompter.exe` 与 `release\\deps\\prompter.exe` 是同一个文件的硬链接
（`st_nlink == 2`、`st_ino` 相同），改掉一个目录项，文件本身照样锁着。

所以这个脚本要一次回答两个问题：

1. **磁盘上那个 exe 是什么时候构建的？现在有几个实例在跑？**
2. **跑着的实例是新版还是旧版？**（看数据目录里的痕迹）

用法：

    python scripts\\diagnose-running-build.py

不改动任何东西 —— 只读文件、只列进程。要结束旧实例请自己动手
（或按脚本末尾给出的命令），因为它可能是你有意开着的。
"""

from __future__ import annotations

import ctypes
import ctypes.wintypes as wt
import json
import os
import sys
import time

# ---------------------------------------------------------------------------
# 路径
# ---------------------------------------------------------------------------

# 与 tauri.conf.json 的 identifier 一致。改标识符的话两处都要改。
APP_ID = "cn.bobby.prompter"

HERE = os.path.dirname(os.path.abspath(__file__))
RELEASE_EXE = os.path.normpath(
    os.path.join(HERE, "..", "src-tauri", "target", "release", "prompter.exe")
)
DEPS_EXE = os.path.normpath(
    os.path.join(HERE, "..", "src-tauri", "target", "release", "deps", "prompter.exe")
)

# 各轮新增的 config.json 字段。用来判断"跑过的版本有多新"。
# 加新字段时往这里追加，旧的那批别删 —— 它们正是判断"落后几版"的依据。
FIELD_TIERS: list[tuple[str, list[str]]] = [
    ("R1-R5（界面升级 / 排版 / 历史 / 穿透 / 折叠）", [
        "window", "padding_x", "letter_spacing", "anchor", "align",
        "theme", "glass", "click_through",
    ]),
    ("本轮（固定控制栏 / 灵敏度 / 语速标尺）", [
        "bar_pinned", "match_floor", "wpm_ref",
    ]),
    # ★ 第十二轮把 `glass` 布尔换成 `glass_mode` 三档 —— 这一层**同时**判
    #   "跑的是新版本"：新版本写出的 config.json 一定有 `glass_mode`，
    #   老版本一定没有（老用户升级后由 load_config 迁移补上）。
    ("第十二轮（磨砂三档 glass_mode）", [
        "glass_mode",
    ]),
]


def human_time(ts: float) -> str:
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ts))


def section(title: str) -> None:
    print()
    print(f"=== {title} ===")


# ---------------------------------------------------------------------------
# ① 产物与进程
# ---------------------------------------------------------------------------


def list_prompter_processes() -> list[tuple[int, str]]:
    """枚举进程。不用 wmic —— 本机 Win11 已经把它移除了。"""
    TH32CS_SNAPPROCESS = 0x2

    class PROCESSENTRY32(ctypes.Structure):
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
            ("szExeFile", ctypes.c_char * 260),
        ]

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    pe = PROCESSENTRY32()
    pe.dwSize = ctypes.sizeof(PROCESSENTRY32)

    found: list[tuple[int, str]] = []
    if k32.Process32First(snap, ctypes.byref(pe)):
        while True:
            name = pe.szExeFile.decode("mbcs", "replace")
            if "prompter" in name.lower():
                found.append((pe.th32ProcessID, name))
            if not k32.Process32Next(snap, ctypes.byref(pe)):
                break
    k32.CloseHandle(snap)
    return found


def check_artifact() -> bool:
    """返回「现在可以重新构建吗」。"""
    section("① 构建产物")
    if not os.path.isfile(RELEASE_EXE):
        print(f"  [缺少] {RELEASE_EXE}")
        print("        还没构建过？跑 build.bat。")
        return True

    st = os.stat(RELEASE_EXE)
    print(f"  路径    : {RELEASE_EXE}")
    print(f"  体积    : {st.st_size / 1e6:.1f} MB")
    print(f"  构建于  : {human_time(st.st_mtime)}")

    try:
        with open(RELEASE_EXE, "r+b"):
            pass
        locked = False
        print("  可独占打开: 是")
    except PermissionError:
        locked = True
        print("  可独占打开: 否  ← 有实例在跑，链接器写不进去")

    # 硬链接关系。这一条是上面那个坑的核心：
    # 两个路径是同一个文件时，改名字根本不解决问题。
    if os.path.isfile(DEPS_EXE):
        a, b = os.stat(RELEASE_EXE), os.stat(DEPS_EXE)
        if a.st_ino == b.st_ino:
            print(f"  与 deps\\prompter.exe 是同一文件（硬链接，nlink={a.st_nlink}）")
            print("        所以：给 exe 改个名字**不能**解锁，别再试了")

    procs = list_prompter_processes()
    if procs:
        print(f"  正在跑的实例: {len(procs)} 个 -> {procs}")
        print()
        print("  要重新构建，先结束它们：")
        print("      " + " ".join(f"taskkill -F -PID {pid}" for pid, _ in procs))
        print("  ★ 注意用**单**斜杠：Git Bash 会把 //F 当成路径转换，命令会静默失效。")
    else:
        print("  正在跑的实例: 无")

    return not locked


# ---------------------------------------------------------------------------
# ② 跑着的是哪一版
# ---------------------------------------------------------------------------


def check_data_dir() -> None:
    section("② 数据目录（判断跑过的是新版还是旧版）")
    appdata = os.environ.get("APPDATA", "")
    d = os.path.join(appdata, APP_ID)
    print(f"  目录: {d}")
    if not os.path.isdir(d):
        print("  [不存在] —— 说明这个应用**从未成功启动过**。")
        return

    print()
    for name in sorted(os.listdir(d)):
        p = os.path.join(d, name)
        mt = human_time(os.path.getmtime(p))
        kind = "DIR " if os.path.isdir(p) else f"{os.path.getsize(p):>8}B"
        print(f"  {mt}  {kind}  {name}")

    cfg_path = os.path.join(d, "config.json")
    if not os.path.isfile(cfg_path):
        print()
        print("  [无 config.json] —— 从未保存过配置，大概率从未成功启动过。")
        return

    with open(cfg_path, encoding="utf-8") as f:
        cfg = json.load(f)

    section("③ 配置里有哪些字段 ⇒ 跑过的最新版本")
    print(f"  顶层字段: {sorted(cfg.keys())}")
    print()
    for label, fields in FIELD_TIERS:
        present = [k for k in fields if k in cfg]
        if len(present) == len(fields):
            print(f"  [有] {label}")
        elif present:
            print(f"  [部分] {label} —— 缺 {sorted(set(fields) - set(present))}")
        else:
            print(f"  [无] {label}")

    # 稿件库文件是另一条独立线索：它只在"用过新版"之后才会出现
    scripts = os.path.join(d, "scripts.json")
    print()
    print(f"  scripts.json: {'存在（用过带稿件库的版本）' if os.path.isfile(scripts) else '不存在（没用过带稿件库的版本）'}")

    if "window" in cfg:
        print(f"  window: {cfg['window']}")
    if "hotkeys" in cfg:
        print(f"  hotkeys: {sorted(cfg['hotkeys'].keys())}")


def main() -> int:
    print("Prompter — 现在跑的是哪一版 / 能不能重新构建")
    can_build = check_artifact()
    check_data_dir()

    section("结论")
    if can_build:
        print("  产物没有被占用，可以直接重新构建。")
    else:
        print("  ★ 产物被正在运行的实例占用 —— 必须先结束它，才能重新构建。")
        print("    否则会看到 LNK1104: 无法打开文件 …\\release\\prompter.exe")
    print("  ★ 另外：改了代码之后，**用户必须重启应用**才会看到变化。")
    print("    上一轮「XX 功能根本没做」的误会，根因就是这个。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
