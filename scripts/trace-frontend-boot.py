"""A/B 追踪：`click_through` 开关打开时，前端启动链路断在哪一步。

背景（本轮实测）：改完"上报名单只有两条栏"之后，穿透在真机上仍**时好时坏**。
继续量下去发现它其实**不随机**，而是和配置里 `click_through` 的初值绑死：

  * `click_through = false` ⇒ 日志里能看到 `[click-through] 可交互区已更新 → 2 块 …`（一切正常）
  * `click_through = true`  ⇒ bootstrap 之后**什么都没有**（连 `开关 → 开` 都没有）

而 `click_through` 在前端只有一处被读：`App.tsx` 里 `if (b.config.click_through) …`。
release 产物没有 devtools，前端"停在哪一步"从外面看不见 ——
所以给后端加了一个 `frontend_beat` 打点命令，本脚本就是拿它做对照实验的。

★ 这个脚本只读不写业务数据：它改 `click_through` 跑两次对照，最后**还原成跑之前的值**。
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = sys.executable

spec = importlib.util.spec_from_file_location(
    "vct", ROOT / "scripts" / "verify-click-through.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

CONFIG = m.CONFIG
LOG = m.LOG

# 只关心这几类行：前端的打点、穿透的关键里程碑。
KEEP = ("[frontend]", "[bootstrap]", "可交互区", "开关", "[script]", "FRONTEND")


def read_log() -> str:
    try:
        return LOG.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def set_flag(value: bool) -> None:
    cfg = json.loads(CONFIG.read_text(encoding="utf-8"))
    cfg["click_through"] = value
    CONFIG.write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")


def one_case(value: bool, wait: float = 18.0) -> list[str]:
    """把 click_through 设成 value，拉起一次，返回这次新产生的关键日志行。"""
    set_flag(value)
    before = len(read_log())
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.pop("NODE_OPTIONS", None)
    try:
        subprocess.run([PY, str(ROOT / "scripts" / "probe-webview-udd.py"),
                        "--real", "--wait", str(wait)],
                       capture_output=True, text=True, timeout=wait + 60, env=env)
    except subprocess.TimeoutExpired:
        subprocess.run(["taskkill", "/F", "/T", "/IM", "prompter.exe"],
                       capture_output=True)
    new = read_log()[before:]
    return [ln for ln in new.splitlines() if any(k in ln for k in KEEP)]


def main() -> int:
    original = CONFIG.read_text(encoding="utf-8")
    orig_flag = json.loads(original).get("click_through")
    print(f"跑之前 click_through = {orig_flag}\n")

    results: dict[bool, list[str]] = {}
    try:
        for value in (False, True):
            print(f"################ click_through = {value} ################")
            lines = one_case(value)
            results[value] = lines
            for ln in lines:
                # 时间戳只留时分秒，便于对照
                print("   " + ln)
            print()
    finally:
        # 必须还原：两份现场都不是老大原来的状态。
        CONFIG.write_text(original, encoding="utf-8")
        print(f"已还原 click_through = "
              f"{json.loads(CONFIG.read_text(encoding='utf-8')).get('click_through')}")

    def has(lines: list[str], key: str) -> bool:
        return any(key in ln for ln in lines)

    print()
    print("=" * 60)
    print("对照结论")
    print("=" * 60)
    checks = [
        ("bootstrap 返回", "[frontend] bootstrap 返回"),
        ("BOOTSTRAP 已 dispatch", "BOOTSTRAP 已 dispatch"),
        ("隐身已开启 / 失败", "隐身"),
        ("穿透已开启 / 失败", "穿透已"),
        ("首次上报可交互区", "首次上报可交互区"),
        ("后端收到可交互区", "可交互区已更新"),
        ("开关 → 开", "开关 → 开"),
    ]
    print(f"  {'检查点':<24} {'false':<8} {'true':<8}")
    for name, key in checks:
        print(f"  {name:<24} {'有' if has(results[False], key) else '—':<8} "
              f"{'有' if has(results[True], key) else '—':<8}")

    # 只要 true 那一列少东西，就说明"开关初值为真"会让链路断在某一环。
    missing = [k for _, k in checks if has(results[False], k) and not has(results[True], k)]
    print()
    if missing:
        print("★ click_through=true 时缺失的环节：" + "、".join(missing))
        print("  （这就是「开关开着时穿透没反应」的断点）")
    else:
        print("两边的链路一样 —— 那 `click_through` 初值不是原因，得换个方向查。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
