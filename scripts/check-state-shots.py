"""按颜色校验各状态截图**真的拍到了那个状态**。

🚧 **本脚本已过期，先别信它的 ✅**（2026-09-27 记）
    它读的是 `shots/` 下那批 `s-sp-*.png` / `s-demo-*.png`，而**那批图是
    UI 改版前拍的**：截的是旧版的两条栏布局（旧的 `mini-pill.demo.*`、
    y 47..77 那条进度栏），断言里的期望也都对着旧 DOM 写的。
    改版之后跑它，验的是旧图 —— **拿到 ✅ 也不代表新界面验过**。
    要重新用它：先按新布局重拍那批图（`make-state-sheet.py` 同样要先改），
    再把下面 `CASES` 里的选择器/期望值对齐。在那之前，
    新界面的状态验收看 `ui-preview.py --measure`（桩里有 `collapsed=`/`slotText=`
    /`barParts=`/`titleShort=` 这些键）。

为什么需要它（本轮真踩过）：
    我把 592x456 的截图按"窗口居中"猜了个裁切偏移 `(16, 48)`，裁出来十张图
    张张一样（全是正文），然后直接拼成对照表 —— 单看每一张都像"界面正常"。
    真正暴露它的是"拿调色板去数强调色像素"：绿和琥珀一个都没有。

    ⇒ "裁哪里"不能靠猜，而且**裁完必须验证**。这个脚本负责两件事：
        ① **两两不同** —— 直接对应"裁错了，拍到的是同一块地方"这个错误；
        ② **该有的强调色在、不该有的不在** —— 对应状态机本身。

为什么只看**顶栏**（y 0..47）：
    进度栏（y 47..77）有一条蓝色的进度填充，颜色与 `--accent` 几乎一样。
    圈进去的话，"已暂停必须出现蓝"这条**永远**成立 —— 一条不会失败的检查
    比没有检查更坏，因为它给人已经验过了的错觉。
    ★ 实测：s-off 的蓝像素 244 个全在 y=60..62 的进度填充上。

为什么用**相对基线**而不是绝对值：
    「▶ 演示」那枚胶囊本来就是绿的，所以"绿色出现"在任何状态下都成立。
    只有与 s-off 基线的**差值**才是状态变化的信号。

用法：
    python scripts/check-state-shots.py
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SHOTS = ROOT / "shots"

# 只看顶栏那一条（见文件头说明）。46 是顶栏高度（--h-top），+1 让边框也进来。
BAR_H = 47
BASELINE = "s-off.png"

PALETTE = {
    "green": (52, 211, 153),
    "amber": (245, 158, 11),
    "blue": (59, 130, 246),
}
TOL = 40

# 文件名 -> (相对基线至少要增加的色, 一个都不能有的色, 说明)
#
# ★ 说明里带「(只查琥珀)」的两条，`need` 是空的 —— **它们在这项检查里是弱项**：
#   「就位中」的变化主要是扫光（颜色相近、像素差只 +7），靠调色板数不出来。
#   那两条的精确断言在 `ui-preview.py --measure` 里：
#   `dpill=mini-pill.demo.starting` + `dpillAnim=pill-scan` + 文字「就位中」。
#   这里只负责"没误亮琥珀"这一半，写清楚免得被当成"验过了"。
EXPECT: dict[str, tuple[dict[str, int], tuple[str, ...], str]] = {
    "s-off.png": ({}, (), "未隐身（基线，不查）"),
    "s-sp-start.png": ({"green": 25}, ("amber",), "隐身·开启中"),
    "s-sp-on.png": ({"green": 25}, ("amber",), "投屏隐身"),
    "s-sp-paused.png": ({"blue": 25}, ("amber",), "已暂停：必须用蓝，不能用琥珀"),
    "s-sp-warn.png": ({"amber": 25}, (), "隐身·未生效"),
    "s-demo-start.png": ({}, ("amber",), "演示·就位中（只查琥珀，扫光归 --measure）"),
    "s-demo-on.png": ({"green": 200}, ("amber",), "演示中（实心绿，绿量应大幅上升）"),
    "s-ct-start.png": ({"green": 25}, ("amber",), "穿透·开启中（宽限期内，不许亮琥珀）"),
    "s-ct-on.png": ({"green": 25}, ("amber",), "穿透中"),
    "s-ct-warn.png": ({"amber": 25}, (), "穿透·未生效（宽限期过后必须亮琥珀）"),
}


def count(path: Path) -> dict[str, int]:
    a = np.asarray(Image.open(path).convert("RGB")).astype(int)[0:BAR_H]
    return {
        name: int((np.abs(a - np.array(rgb)).sum(2) < TOL).sum())
        for name, rgb in PALETTE.items()
    }


def main() -> int:
    missing = [n for n in EXPECT if not (SHOTS / n).exists()]
    if missing:
        print("缺截图（先跑 ui-preview.py）：", ", ".join(missing))
        return 1

    counts = {n: count(SHOTS / n) for n in EXPECT}
    base = counts[BASELINE]

    bad = 0

    # ① 两两不同 —— 对应"裁切位置错了"这个具体错误
    print("① 截图两两不同？")
    dup = []
    names = list(EXPECT)
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            ia = Image.open(SHOTS / a).convert("RGB").crop((0, 0, 592, BAR_H))
            ib = Image.open(SHOTS / b).convert("RGB").crop((0, 0, 592, BAR_H))
            if np.array_equal(np.asarray(ia), np.asarray(ib)):
                dup.append(f"{a} == {b}")
    if dup:
        bad += 1
        print(f"   ❌ 有完全相同的截图：{', '.join(dup)}")
        print("      ★ 这几乎一定是**裁切位置错了**（拍的是同一块地方），不是状态机没变。")
    else:
        print(f"   ✅ {len(names)} 张互不相同")

    # ② 强调色
    print()
    print("② 强调色（只看顶栏 y 0..47；括号里是与 s-off 的差）")
    print(f"{'截图':<18}{'绿':>14}{'琥珀':>14}{'蓝':>14}   判定")
    for name, (need, forbid, why) in EXPECT.items():
        c = counts[name]
        cell = lambda k: f"{c[k]}{'' if name == BASELINE else f'({c[k] - base[k]:+d})'}"
        problems = []
        for k, min_delta in need.items():
            if c[k] - base[k] < min_delta:
                problems.append(f"{k}只多 {c[k] - base[k]}（要 ≥{min_delta}）")
        for k in forbid:
            if c[k] > 0:
                problems.append(f"不该有{k}({c[k]}px)")
        ok = not problems
        bad += 0 if ok else 1
        mark = "✅" if ok else "❌ " + "、".join(problems)
        print(f"{name:<18}{cell('green'):>14}{cell('amber'):>14}{cell('blue'):>14}   {mark}  {why}")

    print()
    if bad:
        print("❌ 有检查没过。先怀疑**裁切位置**，再怀疑状态机。")
        return 2
    print(f"✅ {len(EXPECT)} 张全部符合预期。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
