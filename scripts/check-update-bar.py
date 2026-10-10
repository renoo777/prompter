#!/usr/bin/env python3
"""守住「更新提示条 + 更新入口」这几处的版面与存在性。

    python scripts/check-update-bar.py

# 它守的是什么（都是这一轮真踩到的，不是推演）

| 坑 | 症状 | 是什么让它没被发现的 |
|---|---|---|
| 提示条被顶栏盖住 | 屏幕上什么都没有 | DOM 里一切都好：`updateBar=yes`、高度 28 —— **只有几何能戳穿** |
| 齿轮上的圆点占了宽度 | 顶栏预算被吃、标题被压 | 圆点走 `::after` 伪元素，**DOM 里查不到节点**，截图看着也只是"有个点" |
| 提示条在没更新时也渲染 | 悄悄占掉 28px | 一条空壳在截图里和"设计如此"长得一样 |
| 收起态下正文贴着顶栏 | 收起又展开后第一行被盖住 | 只在"收起 + 有更新"同时成立时出现 |

# 三条判据（一一对上面的坑）

1. **默认必须是 `updateBar=no`** —— 没有更新时这条**整个节点都不该存在**。
   判据是"在不在"，不是"高不高"：`display:none` 的 28px 空壳照样在文档流里。
2. **有更新时 `ubTop >= topH` 且 `vpTop >= ubBottom`** ——
   提示条要真的在顶栏**下面**，正文要真的排在提示条**下面**。
   `.topbar` 是 `position:absolute`，流内的提示条不给 margin 就从 y=0 开始。
3. **圆点不吃布局** —— 有更新时齿轮宽度必须仍是 30。
   这是"选点不选图标"那条决定的验收依据：顶栏能分给标题的宽度是定数。

# 前提

先 `npm run build`（预览读 `dist/`）。

# ★ 为什么必须有 `--hold`

前端的启动检查**刻意延后 3 秒**（`App.tsx` 里那个 `setTimeout`，产品决定：
不与 bootstrap 抢网络）。所以带 `update=` 的那几次必须把 `hold` 加到
4 秒以上，否则量到的是"提示条还没长出来" —— 那看起来和"功能没做"一模一样。
这是本脚本唯一需要人工照顾的时序，写在参数里而不是靠记。
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

# 顶栏基线（与 check-platform-layout.py 同一份口径）。
# ★ 提示条在场与否都**不许动这些数** —— 圆点选了"点"而不是"图标"，
#   争的就是这几个数。
TOPBAR_BASELINE = {"topIcons": 5, "titleW": 116, "progW": 61, "topH": 46}

# 启动检查延后 3s + 桩自己的 900ms 收尾 ⇒ 4.5s 起跳，给到 6s 稳一点。
UPDATE_HOLD = 6500


def clean_env():
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    return env


def probe(query: str, hold: int = 3200) -> dict:
    """跑一次 ui-preview，取回桩写进 stdout 的量化结果。

    ★ `--out` 必须是**带 .png 后缀**的合法路径（Edge 按扩展名判格式，
      给 `nul` 会直接被拒 —— 同 check-platform-layout.py 文件头那段）。
    """
    tmp = tempfile.NamedTemporaryFile(suffix=".png", delete=False)
    tmp.close()
    try:
        args = [
            sys.executable, str(PREVIEW),
            "--out", tmp.name,
            "--w", str(W), "--h", str(H),
            "--hold", str(hold),
            # ★ 必须带 `--measure`（或 `--snapshot`）—— 只有这两条路会让
            #   ui-preview.py 去 dump-dom 并把桩量好的 `key = value` 打出来。
            #   少写这个，脚本会在"拿不到测量结果"处报一个**指错方向**的提示
            #   （"先 npm run build 试试"），而真正的原因只是少了个参数。
            "--measure",
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

    problems: list[str] = []

    # ================= A) 默认：这条**整个不存在** =================
    print("--- A) 默认（没有更新）" + "-" * 40)
    base = probe("")
    for k, yes_no in (("updateBar", "no"), ("gearDot", "no")):
        got = base.get(k)
        mark = "✓" if got == yes_no else "✗"
        print(f"  {mark} {k} = {got}  (要 {yes_no})")
        if got != yes_no:
            problems.append(
                f"[默认] {k} = {got}，要 {yes_no} —— 没有更新时它不该存在/出现。"
                "★ 别用 `display:none` 糊：那还占着文档流，而且和'设计如此'长得一样"
            )
    print(f"    vpTop={base.get('vpTop')}  vpPadTop={base.get('vpPadTop')}")
    if num(base, "vpPadTop") != 60:
        problems.append(
            f"[默认] 正文 vpPadTop = {base.get('vpPadTop')}，期望 60 —— "
            "没有提示条时正文的让位不该被改动（它要替绝对定位的顶栏让道）"
        )
    for k, want in TOPBAR_BASELINE.items():
        got = num(base, k)
        if got != want:
            problems.append(f"[默认] 顶栏 {k} = {got}，基线 {want}")

    # ================= B) 有更新：位置、宽度、不重叠 =================
    print("--- B) 有更新（update=0.1.1）" + "-" * 33)
    upd = probe("update=0.1.1&settle=3200", hold=UPDATE_HOLD)

    checks = [
        ("updateBar", "yes", "有更新时提示条必须出现"),
        ("gearDot", "yes", "有更新时齿轮上必须有点"),
    ]
    for k, want, why in checks:
        got = upd.get(k)
        mark = "✓" if got == want else "✗"
        print(f"  {mark} {k} = {got}  (要 {want})")
        if got != want:
            problems.append(f"[有更新] {k} = {got}，要 {want} —— {why}")

    barH = num(upd, "updateBarH")
    topH = num(upd, "topH")
    ubTop = num(upd, "ubTop")
    ubBottom = num(upd, "ubBottom")
    vpTop = num(upd, "vpTop")
    gearW = num(upd, "gearW")

    print(f"    updateBarH={barH}  ubTop={ubTop}  ubBottom={ubBottom} "
          f"vpTop={vpTop}  vpPadTop={upd.get('vpPadTop')}")
    print(f"    ubMsg={upd.get('ubMsg')}  ubBtns={upd.get('ubBtns')}  gearW={gearW}")

    # ① 提示条必须真的在顶栏**下面**（不是被 absolute 的顶栏盖在 y=0）
    if ubTop is None or topH is None or ubTop < topH:
        problems.append(
            f"[有更新] 提示条 ubTop={ubTop} 没有落在顶栏（topH={topH}）之下 —— "
            "`.topbar` 是 position:absolute，流内的提示条必须自己 "
            "`margin-top: var(--h-top)`。★ 这个坑在 DOM 上看不出来，只有几何能戳穿"
        )
    # ② 正文必须排在提示条**下面**（不重叠）
    if vpTop is None or ubBottom is None or vpTop < ubBottom:
        problems.append(
            f"[有更新] 正文 vpTop={vpTop} 压到了提示条底部 {ubBottom} —— 两者重叠了"
        )
    # ③ 正文的让位必须**缩回去**（提示条已经替它让过一次）
    if num(upd, "vpPadTop") != 14:
        problems.append(
            f"[有更新] 正文 vpPadTop = {upd.get('vpPadTop')}，期望 14 —— "
            "提示条已经让过顶栏那道位，正文再让一遍会多出 60px 空白"
        )
    # ④ 圆点不许吃宽度
    if gearW != 30:
        problems.append(
            f"[有更新] 齿轮宽度 = {gearW}，期望 30 —— "
            "圆点必须走绝对定位（`::after`），一点布局宽度都不能占："
            "顶栏能分给「标题 + 进度条」的总宽是定数，多 6px 就从标题嘴里出"
        )
    # ⑤ 按钮数与文案
    if num(upd, "ubBtns") != 2:
        problems.append(
            f"[有更新] 提示条按钮数 = {upd.get('ubBtns')}，期望 2（更新 / 稍后）"
        )
    msg = upd.get("ubMsg", "")
    if "0.1.1" not in msg:
        problems.append(f"[有更新] 提示条没说出新版本号：ubMsg={msg!r}")
    # ⑥ 顶栏基线不许因为多了个点而变
    for k, want in TOPBAR_BASELINE.items():
        got = num(upd, k)
        mark = "✓" if got == want else "✗"
        print(f"  {mark} 顶栏 {k} = {got}  (基线 {want})")
        if got != want:
            problems.append(
                f"[有更新] 顶栏 {k} 从 {want} 变成 {got} —— "
                "提示条与圆点都不该动到顶栏的宽度分配"
            )

    # ================= C) 收起 + 有更新：不许把正文顶上去 =================
    #
    # ★ 这一条守的是 `:not(.collapsed)` 那个限定。少了它：
    #   收起态下提示条虽然 `display:none`（节点还在），`:has()` 照样命中，
    #   正文的让位被缩成 14px ⇒ 收起又展开之后第一行被顶栏盖住。
    #   而那要"收起 + 有更新"同时成立才出现，手工点很难碰上。
    print("--- C) 收起态 + 有更新" + "-" * 38)
    col = probe("collapsed=1&update=0.1.1&settle=3200", hold=UPDATE_HOLD)
    ch, cpad = num(col, "updateBarH"), num(col, "vpPadTop")
    print(f"    updateBarH={ch}  vpPadTop={cpad}")
    if ch != 0:
        problems.append(
            f"[收起] 提示条高度 = {ch}，期望 0 —— 收起态只有 46px，它必须让位"
        )
    # ★ 收起态 `.viewport` 是 `height:0; padding:0`（那是设计如此：它已经被收成
    #   0 高，没有"正文让它"这回事），所以正确值是 **0**。
    #   如果那条 `:has(.update-bar)` 漏了 `:not(.collapsed)` 限定，它的特异性
    #   （.app + :not(.collapsed) + :has(...) + .viewport = 4 类）会压过
    #   `.app.collapsed .viewport`（3 类），把收起态的 `padding-top` 顶成 14 ——
    #   于是"展开回来后第一行被浮层顶栏盖住"。所以**非零即失败**。
    if cpad != 0:
        problems.append(
            f"[收起] 正文 vpPadTop = {cpad}，期望 0 —— "
            "收起态 `.viewport` 是 `padding:0`，不该被那条 `:has(.update-bar)` 改到。"
            "多半是它少了 `:not(.collapsed)` 限定（特异性 4 > 3，会盖掉收起态规则）"
        )

    print("=" * 72)
    if problems:
        print("更新版面检查未过：")
        for p in problems:
            print("  ✗", p)
        return 1
    print("更新版面检查通过 ✅  默认整条不存在；有更新时在顶栏之下、正文之上；"
          "圆点不吃宽度；收起态不顶正文。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
