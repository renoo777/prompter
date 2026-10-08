"""给 Prompter 的构建产物在桌面建一个快捷方式（.lnk）。

为什么需要它：`npm run tauri:build` 带的是 `--no-bundle`（见 package.json），
所以只产一个裸的 `prompter.exe`，不出安装包 ⇒ 桌面、开始菜单里**没有任何入口**，
每次都得手工爬到 `src-tauri/target/release/` 里去双击。

依赖 pywin32。本机没装的话用隔离 venv：
    python -m venv %TEMP%\\prompter-venv
    %TEMP%\\prompter-venv\\Scripts\\python.exe -m pip install pywin32

用法：
    python scripts/make-shortcut.py                    # 桌面 → 提词器.lnk
    python scripts/make-shortcut.py --name Prompter    # 换个显示名
    python scripts/make-shortcut.py --remove           # 删掉它
"""
from __future__ import annotations

import argparse
import ctypes
import pathlib

REPO = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO / "src-tauri" / "target" / "release" / "prompter.exe"

# CSIDL_DESKTOP：走系统 API 取桌面，而不是拼 "~/Desktop" ——
# 桌面可能被 OneDrive 重定向到别的盘，拼路径会写到用户看不见的地方。
CSIDL_DESKTOP = 0


def desktop() -> pathlib.Path:
    buf = ctypes.create_unicode_buffer(260)
    ctypes.windll.shell32.SHGetFolderPathW(None, CSIDL_DESKTOP, None, 0, buf)
    return pathlib.Path(buf.value)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", default="提词器", help="快捷方式显示名（默认：提词器）")
    ap.add_argument("--exe", default=str(DEFAULT_EXE), help="目标可执行文件")
    ap.add_argument("--remove", action="store_true", help="删掉这个快捷方式")
    args = ap.parse_args()

    lnk = desktop() / f"{args.name}.lnk"

    if args.remove:
        if lnk.exists():
            lnk.unlink()
            print(f"已删除 {lnk}")
        else:
            print(f"没有这个快捷方式：{lnk}")
        return 0

    exe = pathlib.Path(args.exe)
    if not exe.exists():
        print(f"找不到可执行文件：{exe}")
        print("先构建：npm run tauri:build")
        return 1

    try:
        from win32com.client import Dispatch
    except ImportError:
        print("缺 pywin32 —— 装它：python -m pip install pywin32")
        return 2

    shell = Dispatch("WScript.Shell")
    sc = shell.CreateShortcut(str(lnk))
    sc.TargetPath = str(exe)
    # 工作目录指到 exe 旁边：应用若要在同目录找资源，双击才与命令行启动一致。
    sc.WorkingDirectory = str(exe.parent)
    sc.IconLocation = f"{exe},0"
    sc.Description = "Prompter 提词器"
    sc.Save()

    # 写完立刻读回。`Save()` 不抛异常**不等于**写对了 ——
    # 要验的是"双击它会开什么"，而不是"文件在不在"（后者在桌面重定向/写错盘时也为真）。
    back = shell.CreateShortcut(str(lnk))
    ok = (
        lnk.exists()
        and str(pathlib.Path(back.TargetPath)).lower() == str(exe).lower()
        and str(pathlib.Path(back.WorkingDirectory)).lower() == str(exe.parent).lower()
    )
    print(f"{'OK ' if ok else 'FAIL '}{lnk}")
    print(f"   target  = {back.TargetPath}")
    print(f"   workdir = {back.WorkingDirectory}")
    return 0 if ok else 3


if __name__ == "__main__":
    raise SystemExit(main())
