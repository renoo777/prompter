"""把用户配置里的磨砂档位改成指定值（带备份）。

用法：python scripts/set-glass-mode.py system|off|css
只改 config.json 的 glass_mode 键；先备份成 config.json.bak。
"""
from __future__ import annotations

import json
import os
import shutil
import sys
from pathlib import Path

CFG = Path(os.environ["APPDATA"]) / "cn.bobby.prompter" / "config.json"

mode = sys.argv[1] if len(sys.argv) > 1 else "system"
if mode not in ("off", "css", "system"):
    raise SystemExit("档位只能是 off / css / system")

if CFG.is_file():
    shutil.copy2(CFG, CFG.with_suffix(".json.bak"))
    data = json.loads(CFG.read_text(encoding="utf-8"))
else:
    data = {}

old = data.get("glass_mode", "(没有这一项)")
data["glass_mode"] = mode
CFG.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
print(f"glass_mode: {old} → {mode}   ({CFG})")
