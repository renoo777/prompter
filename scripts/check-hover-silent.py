#!/usr/bin/env python3
"""把鼠标事件扫过**每一个**可交互控件，数界面上弹出了几个提示。

    python scripts/check-hover-silent.py

# 它守的是什么

第十一轮用户的投诉原话是：

> 「鼠标移到所有按钮的时候都会弹出文字提示」

提示（原生 `title` + 自绘气泡）已经被整个删掉。删完之后有两个问题：

1. **静态检查证明不了"扫过去不弹东西"。** `check-tooltips.py` 查的是源码里
   有没有那几个名字 —— 但如果有人用**别的名字**重新实现了一套悬停提示
   （比如 `Tooltip.tsx` / `popover` / 自己写一个 `mouseover` 就 append 一个
   div），静态查名字是拦不住的。
2. **截图更证明不了。** 截图里本来就没有悬停态，一张"干净的界面图"在
   有提示的时候同样长这样。

⇒ 所以必须**真的把事件发出去**，然后看 DOM 里多了什么。

# 怎么做的

`ui-preview` 的桩里有一个 `hoverall=1` 动作：它按真实顺序
（`pointerover` → `mouseover` → `mouseenter` → `mousemove`）把事件打到
页面上**每一个**可交互元素（button / select / input / [tabindex] /
顶栏图标 / 状态灯 / 大圆按钮 / 设置里的行 …），并顺带走一遍 `focus()`
（提示也可以挂在焦点上）。扫完立刻报三个数：

    hoverSwept=<扫了几个元素>
    hoverBubbles=<扫完之后文档里有几个 .tip-bubble>
    hoverTitles=<扫完之后文档里有几个带 title 属性的元素>

期望：`hoverBubbles=0` 且 `hoverTitles=0`。

★ `hoverSwept` 不是为了断言通过 —— 它**本身就是一个防伪指标**：
  如果哪天改成 `hoverSwept=0`（一个元素都没扫到，例如选择器全写错了），
  那 `hoverBubbles=0` 就毫无意义（没扫过当然没弹）。所以这里要求
  `hoverSwept` 至少 20 —— **扫不到东西 = 这道门槛自己瞎了**。

# ★★ 改完必须做变异测试

  · 往任意一个按钮塞一个 `title="x"` → `hoverTitles` 必须 > 0
  · 往任意组件挂一个 `.tip-bubble` 的 div → `hoverBubbles` 必须 > 0

只验"绿"是没用的 —— 一道**永远绿**的门槛比没有门槛更坏，它让人放心地
不去看。这个项目已经栽过两次（`check-tooltips.py` 第一版逐行正则、
桩的 `byTitle()` 静默失效）。

# ★★★ 它读的是 `dist/`，不是源码 —— 这是它最大的一个坑

本脚本驱动 `ui-preview.py`，而后者加载的是**打包产物** `dist/`。
所以：**改完源码必须先 `npm run build`**，否则它验的是上一版。

做变异测试时真踩了一次，而且形态很值得记：
  我往 `App.tsx` 塞了 `.tip-bubble`，但塞的位置让 JSX 语法错了
  ⇒ `npm run build` 报 `TS1382` 失败 ⇒ `dist/` **还是旧的**（没有那个 div）
  ⇒ 本脚本照跑、照报绿。
  也就是说：**一次语法错误，可以让这道门槛悄悄验一个不存在的界面。**

⇒ 处置：`gates.py` 里它排在 `npm run build` **之后**，所以正常流程里
  build 一失败就轮不到它，这个坑进不了 CI。但**单独手工跑**（做变异测试时）
  必须自己先确认 build 真的成功了 —— 别只看它报绿。
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

# 至少要扫到这么多元素，否则说明选择器失效、这道门槛自己瞎了。
MIN_SWEPT = 20


def clean_env():
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    return env


def probe(query: str) -> dict:
    """跑一次 ui-preview --snapshot，取回桩写进 title 的量化结果。

    ★ `--out` 必须是带 `.png` 后缀的合法路径（Edge 按扩展名判格式），
      写完即删 —— 见 check-platform-layout.py 文件头那个坑。
    """
    tmp = tempfile.NamedTemporaryFile(suffix=".png", delete=False)
    tmp.close()
    try:
        args = [
            sys.executable, str(PREVIEW),
            "--out", tmp.name,
            "--w", str(W), "--h", str(H),
            "--snapshot",
        ]
        if query:
            args += ["--query", query]
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
    # 取脚本打印的 `   key = value` 那一段（已格式化，不依赖空格切分）。
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
        return int(float(vals[key]))
    except (KeyError, ValueError):
        return None


def main() -> int:
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    print("--- 鼠标扫过所有控件：界面上会不会弹出提示 " + "-" * 20)

    problems: list[str] = []

    # 两个场景都要扫：主界面 + 打开的设置抽屉。
    #   ★ 只在主界面扫是不够的 —— 设置里控件最多（滑杆、开关、下拉、色点、
    #     按钮），历史上"提示最密"的地方就是它。漏了它等于漏了大头。
    for tag, query in (("主界面", "hoverall=1"), ("设置抽屉", "hoverall=1&open=settings")):
        vals = probe(query)
        swept = num(vals, "hoverSwept")
        bubbles = num(vals, "hoverBubbles")
        titles = num(vals, "hoverTitles")

        print(f"  [{tag}] 扫过 {swept} 个控件 → 气泡 {bubbles} 个 / 原生 title {titles} 个")

        # 防伪：扫不到东西的话后面两个 0 毫无意义
        if swept is None or swept < MIN_SWEPT:
            problems.append(
                f"[{tag}] 只扫到 {swept} 个控件（< {MIN_SWEPT}）—— "
                "很可能选择器写错了。先修桩，再谈通不通过。"
            )
            continue

        if bubbles:
            problems.append(
                f"[{tag}] 扫过控件后出现了 {bubbles} 个 `.tip-bubble` —— "
                "悬停气泡被加回来了（或换了名字实现了一套新的）"
            )
        if titles:
            problems.append(
                f"[{tag}] 扫过控件后出现了 {titles} 个带 `title` 的元素 —— "
                "原生系统 tooltip 又回来了"
            )

    print("=" * 72)
    if problems:
        print("悬停静默检查未过：")
        for p in problems:
            print("  ✗", p)
        print()
        print("第十一轮用户原话：「不行，还是删掉全部的提示吧」。")
        print("根因不是气泡好不好看，是「鼠标扫过按钮就弹出东西」这件事本身 ——")
        print("它会遮住稿子，而提词器上盖住正文是最大的一宗罪。")
        print("理由与全过程见 src/components/Hint.tsx 顶部注释。")
        return 1

    print("悬停静默检查通过 ✅  鼠标扫过所有控件，界面上什么都没弹出来。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
