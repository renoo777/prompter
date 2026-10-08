"""自测：`heal_leftover_config()` 能不能把"被 TerminateProcess 打断"留下的
脏配置改回去。

为什么值得单独测：Windows 上 `SIGTERM` 不可捕获 ⇒ `finally` 不保证执行 ⇒
脚本可能把老大的 `config.json` 永久留在 `click_through: true`（一个"窗口点不动"的状态）。
自愈是这条链上唯一可靠的兜底，所以它自己必须被测过。

做法：直接造出"被打断后该有的现场"（配置脏 + 副本在），调一次 heal，看是否复原。
"""

from __future__ import annotations

import importlib.util
import io
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location(
    "vct", ROOT / "scripts" / "verify-click-through.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

CONFIG = m.CONFIG
BACKUP = m.BACKUP

# 记下真实现场，测完必须原样放回去 —— 这个自测本身也不许改动老大的配置。
before = CONFIG.read_text(encoding="utf-8")
cfg = json.loads(before)
print(f"当前 click_through = {cfg.get('click_through')}")

fails = []
try:
    # ---- 造现场：配置脏（true），副本里是干净的（false）----
    clean = dict(cfg)
    clean["click_through"] = False
    dirty = dict(cfg)
    dirty["click_through"] = True

    BACKUP.write_text(json.dumps(clean, ensure_ascii=False, indent=2), encoding="utf-8")
    CONFIG.write_text(json.dumps(dirty, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"造现场：配置 = {json.loads(CONFIG.read_text(encoding='utf-8'))['click_through']}，"
          f"副本 = {json.loads(BACKUP.read_text(encoding='utf-8'))['click_through']}")

    healed = m.heal_leftover_config()

    got = json.loads(CONFIG.read_text(encoding="utf-8"))["click_through"]
    print(f"自愈后：返回值={healed} 配置={got} 副本还在吗={BACKUP.exists()}")

    if not healed:
        fails.append("该报「修过」却返回了 False")
    if got is not False:
        fails.append(f"配置没被改回 false（实际 {got}）")
    if BACKUP.exists():
        fails.append("副本没被删掉 —— 它长期存在就说明还有泄露")

    # ---- 没有副本时不该误报 ----
    if m.heal_leftover_config():
        fails.append("没有副本时不该报「修过」")

finally:
    CONFIG.write_text(before, encoding="utf-8")
    if BACKUP.exists():
        BACKUP.unlink()
    print(f"已把真实现场放回：click_through = "
          f"{json.loads(CONFIG.read_text(encoding='utf-8')).get('click_through')}")

print()
if fails:
    for f in fails:
        print("  ❌ " + f)
    sys.exit(1)
print("  ✅ 自愈逻辑正确")
