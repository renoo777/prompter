#!/usr/bin/env python3
"""磨砂三档：切档之后，CSS 玻璃 / 系统材质 / 外壳透明度是否各归各位。

    python scripts/check-glass-mode.py

# 它守的是什么

第十二轮把「背景模糊」那个开关升级成三档（关 / 柔光 / 系统磨砂），
背后是**三层各自独立的机制**，它们最容易互相盖错：

| 档位 | 窗体材质（`set_effects`） | 栏的 CSS 玻璃 | 外壳 `--win-alpha` | 不透明度滑杆 |
|---|---|---|---|---|
| 关 | **卸下** | 关（实底） | 用户滑杆 | 显示 |
| 柔光 | 无 | 开（`backdrop-filter`） | 用户滑杆 | 显示 |
| 系统磨砂 | 挂 Acrylic / hudWindow | **退位**（否则两层磨） | 固定让位（0.22） | **藏起来** |

这四条断言分别对应需求 P0-2 / P0-3 / P0-6 与「一个意思一个控件」。

# 为什么截图证明不了

三件事在图上完全看不出来：

1. `set_effects` 是**窗体合成层**，截图只是"看起来更透一点"，
   而"材质还挂着"和"材质卸下了"在 CSS 玻璃关着时**几乎同图**；
2. 外壳透明度与材质同时起作用，色差极小，肉眼没有量程；
3. 滑杆消失只在设置页里看得到，而默认截图不开抽屉。

⇒ 必须让桩**回话**：`save_config` 里模拟真后端的 `apply_glass_mode`
（`commands.rs`），把结果记在 `glassFx` 上，再量 DOM 的计算值。

# ★★ 改完必须做变异测试

  · 把 `store.tsx` 里的 `config.glass_mode !== "css"` 改成恒 `true`
    ⇒ 系统磨砂档的 `topBlur` 会变回 blur(...) ⇒ 门槛必须报红；
  · 把桩 `save_config` 里的 `glassEffects = ...` 删掉
    ⇒ 切到「关」后 `glassFx` 仍是 acrylic ⇒ 门槛必须报红。

# ★★★ 与 `check-hover-silent.py` 同坑

本脚本读 `dist/`（ui-preview 加载的是打包产物）：**改完源码先 `npm run build`**。
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PREVIEW = ROOT / "scripts" / "ui-preview.py"

W, H = 560, 360

# 系统磨砂档下外壳让位给材质用的固定 alpha（store.tsx 里的 "0.22"）。
SYSTEM_ALPHA = "0.22"
# 门槛：数值相等即可，但差 0.02 以上说明"让位"这件事没做干净。
ALPHA_TOLERANCE = 0.02


def clean_env():
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    return env


def probe(query: str) -> dict:
    """跑一次 ui-preview --snapshot，取回桩写进 title 的量化结果。

    ★ `--out` 必须带 `.png` 后缀且写完即删（Edge 按扩展名判格式，见
      `check-hover-silent.py` 文件头）。
    ★ `settle` 是**查询串参数**（不是命令行开关）：量测时刻 =
      `900 + steps*400 + settle` 毫秒，档位那一步走的是 400ms 防抖落盘，
      所以要留 1600ms 之后才量 —— 否则量到的是"还没保存"的旧状态
      （那是本脚本最容易犯的假阴性）。
    """
    tmp = tempfile.NamedTemporaryFile(suffix=".png", delete=False)
    tmp.close()
    try:
        args = [
            sys.executable, str(PREVIEW),
            "--out", tmp.name,
            "--w", str(W), "--h", str(H),
            "--snapshot",
            # ★ `--hold` 必须 ≥ 量测时刻（`900 + 步数*400 + settle`），否则桩
            #   还没写 title 页面就被收了 —— 症状是"拿不到测量结果"，
            #   而 ui-preview 自己给出的提示会指向 hold 太小（这条我们照抄即可）。
            "--hold", "7000",
            "--query", f"{query}&settle=1600",
        ]
        p = subprocess.run(
            args, cwd=ROOT, env=clean_env(),
            capture_output=True, encoding="utf-8", errors="replace",
        )
    finally:
        try:
            os.unlink(tmp.name)
        except OSError:
            pass

    out = (p.stdout or "") + (p.stderr or "")
    vals: dict[str, str] = {}
    for line in out.splitlines():
        m = re.match(r"\s+([A-Za-z][A-Za-z0-9_]*) = (.*)$", line)
        if m:
            vals[m.group(1)] = m.group(2).strip()
    if not vals:
        print(out[-3000:])
        raise SystemExit("拿不到测量结果 —— 预览没跑起来（先 npm run build 试试）")
    return vals


def num(vals: dict, key: str):
    try:
        return float(vals[key])
    except (KeyError, ValueError):
        return None


def main() -> int:
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    print("--- 磨砂三档：CSS 玻璃 / 系统材质 / 外壳透明度各归各位 " + "-" * 14)

    problems: list[str] = []

    # ---------- 场景 1：出厂默认（柔光）----------
    v = probe("open=settings")
    print(
        f"  [柔光] 材质={v.get('glassFx')} 栏玻璃={v.get('topBlur')} "
        f"外壳 alpha={v.get('winAlpha')} 滑杆={v.get('opacityRow')}"
    )
    if v.get("glassFx") != "none":
        problems.append(f"[柔光] 窗体材质应是 none，实际 {v.get('glassFx')} —— 材质不该在默认档挂着")
    if v.get("topBlur") == "none":
        problems.append("[柔光] 栏的 backdrop-filter 没了 —— 这一档的定义就是它")
    if v.get("opacityRow") != "yes":
        problems.append("[柔光] 不透明度滑杆不见了 —— 这一档外壳透明度仍归它管")

    # ---------- 场景 2：切到「系统磨砂」 ----------
    v = probe("open=settings&glassclick=system")
    print(
        f"  [系统磨砂] 材质={v.get('glassFx')} 栏玻璃={v.get('topBlur')} "
        f"外壳 alpha={v.get('winAlpha')} 滑杆={v.get('opacityRow')}"
    )
    if v.get("glassTap") is None and "glassTap" not in v:
        problems.append("桩没报 glassTap —— 切档那一步可能没跑到（`check-glass-mode.py` 的前提坏了）")
    if v.get("glassFx") != "acrylic":
        problems.append(
            f"[系统磨砂] 窗体材质应是 acrylic，实际 {v.get('glassFx')} —— "
            "要么后端没调 set_effects，要么桩的 save_config 没模拟它"
        )
    if v.get("topBlur") != "none":
        problems.append(
            f"[系统磨砂] 栏仍有 CSS 玻璃（{v.get('topBlur')}）—— "
            "系统材质 + backdrop-filter 是两层磨（苹果 HIG 点名要避免的 glass on glass）"
        )
    alpha = num(v, "winAlpha")
    if alpha is None or abs(alpha - float(SYSTEM_ALPHA)) > ALPHA_TOLERANCE:
        problems.append(
            f"[系统磨砂] 外壳 alpha 应让位到 {SYSTEM_ALPHA}，实际 {v.get('winAlpha')} —— "
            "外壳不透明会把材质整个盖住，等于没挂"
        )
    if v.get("opacityRow") != "none":
        problems.append("[系统磨砂] 不透明度滑杆还在 —— 透明度这一档由材质接管了，一个意思只留一个控件")

    # ---------- 场景 3：从系统磨砂切回「关」（材质必须真卸） ----------
    v = probe("open=settings&glass=system&glassclick=off")
    print(
        f"  [关] 材质={v.get('glassFx')} 栏玻璃={v.get('topBlur')} "
        f"外壳 alpha={v.get('winAlpha')} 滑杆={v.get('opacityRow')}"
    )
    if v.get("glassFx") != "none":
        problems.append(
            f"[关] 窗体材质仍是 {v.get('glassFx')} —— "
            "「关」必须把材质真正 set_effects(None) 卸下，而不是只关 CSS 那一层（P0-6）"
        )
    if v.get("topBlur") != "none":
        problems.append(f"[关] 栏仍有 CSS 玻璃（{v.get('topBlur')}）")
    alpha = num(v, "winAlpha")
    if alpha is not None and abs(alpha - 0.94) > ALPHA_TOLERANCE:
        problems.append(
            f"[关] 外壳 alpha 应回到用户的 0.94，实际 {v.get('winAlpha')} —— "
            "「关」这一档外壳透明度仍归不透明度滑杆管"
        )
    if v.get("opacityRow") != "yes":
        problems.append("[关] 不透明度滑杆不见了 —— 这一档它仍然有用")

    print("=" * 72)
    if problems:
        print("磨砂三档检查未过：")
        for p in problems:
            print("  ✗", p)
        print()
        print("判据来自 docs/磨砂玻璃-调研与方案.md 的 P0-2 / P0-3 / P0-6。")
        return 1

    print("磨砂三档检查通过 ✅  关 / 柔光 / 系统磨砂 三档的材质、玻璃、外壳透明度全部到位。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
