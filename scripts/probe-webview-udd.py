"""探针：测试实例能不能用**独立的 user-data-dir**（从而不碰老大的真 profile）。

背景（本项目实测）：
  验证脚本用 `taskkill /F` 硬杀实例，会在 `%LOCALAPPDATA%\\cn.bobby.prompter\\EBWebView`
  留下 0 字节 `.tmp` + `Default/LOCK`。此后**每一次**启动页面都不再执行
  （窗口在、msedgewebview2 子进程 4 个、但日志里没有 `[bootstrap]`），
  看着像"代码坏了"。挪走 profile 立刻恢复。
  ⇒ **验证脚本自己制造了它下一次跑不动的原因。**

若 `WEBVIEW2_USER_DATA_FOLDER` 被 wry 尊重，测试就永远不碰真 profile，
硬杀多少次都无所谓 —— 这是唯一能根治的方案，所以要实测而不是推断。

用法：
  python scripts/probe-webview-udd.py           # 用独立临时 profile
  python scripts/probe-webview-udd.py --real    # 对照组：用真 profile
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ROOT / "src-tauri" / "target" / "release" / "prompter.exe"
LOG = Path(os.path.expandvars("%LOCALAPPDATA%")) / "cn.bobby.prompter" / "logs" / "Prompter.log"
REAL_UDD = Path(os.path.expandvars("%LOCALAPPDATA%")) / "cn.bobby.prompter" / "EBWebView"


def read_log() -> str:
    try:
        return LOG.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def kill_all() -> None:
    subprocess.run(["taskkill", "/F", "/T", "/IM", "prompter.exe"],
                   capture_output=True, env=dict(os.environ))
    time.sleep(1.0)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--real", action="store_true", help="用真 profile 作对照")
    ap.add_argument("--wait", type=float, default=30.0)
    args = ap.parse_args()

    if not EXE.exists():
        raise SystemExit(f"找不到 exe：{EXE}")

    kill_all()

    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.pop("NODE_OPTIONS", None)

    if args.real:
        print(f"【对照组】用真 profile：{REAL_UDD}")
    else:
        udd = Path(tempfile.gettempdir()) / "prompter-verify-udd"
        shutil.rmtree(udd, ignore_errors=True)
        env["WEBVIEW2_USER_DATA_FOLDER"] = str(udd)
        print(f"【实验组】独立 profile：{udd}")

    before = len(read_log())
    proc = subprocess.Popen([str(EXE)], env=env, close_fds=True,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    print(f"  已拉起 pid={proc.pid}")

    boot = ""
    deadline = time.time() + args.wait
    while time.time() < deadline:
        for ln in read_log()[before:].splitlines():
            if "[bootstrap]" in ln:
                boot = ln.strip()
                break
        if boot:
            break
        time.sleep(0.4)

    if not args.real:
        udd = Path(env["WEBVIEW2_USER_DATA_FOLDER"])
        print(f"  临时目录存在 = {udd.exists()}"
              + (f"，含 {[p.name for p in udd.iterdir()][:4]}" if udd.exists() else ""))

    elapsed = args.wait - (deadline - time.time())
    print(f"  等了 {elapsed:.1f}s，[bootstrap] = {boot or '（无）'}")

    kill_all()

    if boot:
        print("\n✅ 前端起来了。")
        return 0
    print("\n❌ 没起来。日志尾部：")
    for ln in read_log().splitlines()[-8:]:
        print("   " + ln)
    return 1


if __name__ == "__main__":
    sys.exit(main())
