#!/usr/bin/env python3
"""按平台校验版面：**Windows 一个字节都不能变，macOS 少了「任务栏」那条路**。

    python scripts/check-platform-layout.py

# 它守的是什么

2026-09-27（macOS 移植轮）之后，顶栏那颗「收进任务栏」在非 Windows 平台上
**整个消失**（后端 `win::platform` 给 `has_taskbar=false`，理由见
`src-tauri/src/commands.rs` 里 `minimize_to_taskbar` 的文档）。

这让"顶栏图标数"从一个**定数**（曾经写死为 5，见 `TopBar.tsx` 的注释）
变成了**随平台而变**。而定数变活值，正是最容易悄悄出问题的地方：

  * Windows 上少了一颗 ⇒ 标题被压缩、或某个功能入口没了，**没人会发现**；
  * macOS 上多了一颗 ⇒ 「收进任务栏」在那边按下等于收起，出现就是**两颗按钮
    干同一件事**。

两种错在截图里都"看着正常"，只能靠数字判。所以这个脚本把两边都钉住：

| 平台 | 断言 |
|---|---|
| Windows | `topIcons=5`、`titleW=116`、`progW=61`、`topH=46` —— **与移植前逐字节相同** |
| macOS | `topIcons=4`、`titleClip=no`（标题**不再被截**）、且顶栏**不含**那颗图标 |

# ★★ `titleShort` 的符号别搞反

桩里它是 **`文字宽度 − 盒子宽度`**，所以：

* `titleShort > 0` ⇒ **放不下**，浏览器挂上了省略号（**这才是被截**）；
* `titleShort < 0` ⇒ 有余量，文字完整。

移植前 Windows 是 `-0.6`（刚刚够，余量 0.6px），macOS 是 `-16.6`（余 16.6px）。
⇒ 判定要用 `titleClip`（桩直接给的 yes/no），不要自己对着 `titleShort`
的符号下结论 —— 我第一次就把它读反了，差点去"修"一个本来就对的数字。

# 为什么 macOS 上"标题变宽"是**通过**而不是**回归**

因为图标数是"最多 5"，不是"必须凑满 5"。少一颗图标省下的 30px 全部归标题，
而**移植前标题在 560px 下本来就是被省略号截断的**（余量仅 0.6px）。
⇒ macOS 上余量明显变大才是对的。这条写进断言，
是为了防止后人"看到数不对就补一颗图标把位置填满" —— 那会把标题又压回去。

# 依赖

`ui-preview.py --snapshot`（无头 Edge + 桩）。跑之前先 `npm run build`，
因为预览读的是 `dist/`。

# ★★ 为什么不给 `--out` 传 `os.devnull`

第一版写的是 `"--out", os.devnull`（就是"我不要那张图"），
结果 Edge 直接拒绝：`Unsupported screenshot image file type:` ——
`--screenshot` 是**按扩展名**判格式的，`nul` / `/dev/null` 都没有扩展名。
更阴的是它的失败链条：Edge 不写图 ⇒ 脚本末段 `if not out.exists(): SCREENSHOT_FAILED`
⇒ `probe()` 只见 "SCREENSHOT_FAILED" ⇒ 抛 "拿不到测量结果 —— 预览没跑起来
（先 npm run build 试试）"。

⇒ 真正的病因（出图路径不合法）和它给的提示（"先 build 试试"）
**完全对不上**，人会去反复重 build 一个本来没问题的前端。

现在改成写一个**真的 `.png` 临时文件**再删掉 —— 我们只要 DOM，图是副作用。
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

# 真实窗口尺寸（默认窗口就是 560×360）
W, H = 560, 360

# 移植前的 Windows 基线。**这是回归线，不是"抄当前值"** ——
# 改动让这些数变了，要么是漏改期望，要么是动了不该动的东西。
WIN_BASELINE = {
    "topIcons": 5,
    "titleW": 116,
    "progW": 61,
    "topH": 46,
}


def clean_env():
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    return env


def probe(query: str) -> dict:
    """跑一次 ui-preview --snapshot，取回桩写进 stdout 的量化结果。

    ★ `--out` 必须是一个**带 .png 后缀的合法路径**：Edge 按扩展名判格式，
      给 `nul`（Windows 的 os.devnull）会直接被拒。我们不要那张图，
      所以写进临时目录、用完即删。见文件头那段注释。
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
        # ★ 放 finally：预览崩了也别留垃圾文件。
        try:
            os.unlink(tmp.name)
        except OSError:
            pass

    out = (p.stdout or "") + (p.stderr or "")
    # 脚本会打印 `   key = value` 那一段（人读的），以及一整行 `measure: ...`。
    # 取前者：它已经做过一次格式化，且不依赖空格分隔的解析。
    vals: dict[str, str] = {}
    for line in out.splitlines():
        m = re.match(r"\s+([A-Za-z][A-Za-z0-9_]*) = (.*)$", line)
        if m:
            vals[m.group(1)] = m.group(2).strip()
    if not vals:
        print(out[-3000:])
        raise SystemExit("拿不到测量结果 —— 预览没跑起来（先 npm run build 试试）")
    return vals


def num(vals: dict, key: str) -> float | None:
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

    # ---------------- Windows：一个字节都不许变 ----------------
    win = probe("")
    print("--- Windows " + "-" * 50)
    for k, want in WIN_BASELINE.items():
        got = num(win, k)
        mark = "✓" if got == want else "✗"
        print(f"  {mark} {k} = {got}  (基线 {want})")
        if got != want:
            problems.append(f"[windows] {k} 变了：{got} ≠ {want}")

    # 那颗图标必须在
    if num(win, "topIcons") != 5:
        problems.append(
            "[windows] 顶栏图标数不是 5 —— 「收进任务栏」那颗在 Windows 上不该消失"
        )

    # ---------------- macOS：少一条路，标题变宽 ----------------
    mac = probe("plat=macos")
    print("--- macOS " + "-" * 52)
    mac_icons = num(mac, "topIcons")
    mac_short = num(mac, "titleShort")
    mac_clip = mac.get("titleClip")

    print(f"  topIcons = {mac_icons}  (期望 4：少了「收进任务栏」那颗)")
    if mac_icons != 4:
        problems.append(
            f"[macos] 顶栏图标数 = {mac_icons}，期望 4 —— "
            "多一颗说明「收进任务栏」在 macOS 上没被藏掉（那边按下它等于收起）"
        )

    # ★ 用桩直接给的 titleClip，不自己对 titleShort 的符号下结论（符号容易读反）
    print(f"  titleClip = {mac_clip}  (期望 no：标题不该被省略号截)")
    if mac_clip != "no":
        problems.append(
            f"[macos] 标题被截了（titleClip={mac_clip}, titleShort={mac_short}）。"
            "少一颗图标省下的宽度**必须**归标题 —— "
            "★ 不要靠加一颗图标来'修正'这个数（见本脚本文件头）"
        )

    # 余量必须比 Windows 大：这正是"省下来的宽度归了标题"的量化证据。
    win_short = num(win, "titleShort")
    print(f"  titleShort = {mac_short}  (Windows 基线 {win_short}；macOS 应更宽)")
    if mac_short is not None and win_short is not None and mac_short >= win_short:
        problems.append(
            f"[macos] 标题余量 {mac_short} 没有比 Windows 的 {win_short} 更宽松 —— "
            "说明少掉那颗图标省下的宽度没有落到标题身上"
        )

    # ---------------- 两个平台的高度必须一致 ----------------
    wh, mh = num(win, "topH"), num(mac, "topH")
    print(f"--- 顶栏高度  win={wh}  mac={mh}")
    if wh != mh:
        problems.append(f"顶栏高度两平台不一致：win={wh} mac={mh}")

    # ---------------- 设置里「收起」那一行的用语与按钮数 ----------------
    #
    # ★★ 这一组是 2026-09-27 补的。上一组只验了**顶栏**，
    #    而"收进任务栏"在设置里还有第二个入口 —— 那边同样必须按平台分岔：
    #      * macOS 标签是「收起窗口」（不能说一件用户做不到的事）
    #      * macOS 的 sub 里**不能出现「任务栏」三个字**
    #      * macOS 右边**只有 1 颗按钮**（「收进任务栏」整个不渲染）
    #    这三条都属于"写错了界面照样渲染"的文案/数量错误 —— 截图看不出来。
    #    所以这一组**必须显式打开抽屉**（`open=settings`）才量得到。
    print("--- 设置里「收起」那一行 " + "-" * 34)
    mac_set = probe("plat=macos&open=settings")
    win_set = probe("open=settings")

    # 抽屉没开的话下面全是 none，先把这个失败模式单独报出来
    for tag, vals in (("macos", mac_set), ("windows", win_set)):
        if vals.get("collapseRowLabel") in (None, "none"):
            problems.append(
                f"[{tag}] 没找到「收起」那一行 —— 抽屉可能没打开"
                "（检查 ui-preview.py 是否收到 open=settings 参数）"
            )

    for tag, vals, want_btns, want_label in (
        # ★ 期望值里的空格被桩的 `nowrap()` 压掉了（它同时负责把长文本折成一行）。
        #   所以这里写的是**压缩后**的样子 —— 以实际量到的稳定值写死，
        #   而不是照抄 JSX 里的字面量，否则每次都会误报。
        ("windows", win_set, 2, "收起/收进任务栏"),
        ("macos", mac_set, 1, "收起窗口"),
    ):
        label = vals.get("collapseRowLabel", "none")
        btns = num(vals, "collapseRowBtns")
        ok_b = btns == want_btns
        print(f"  {tag}: label={label!r}  btns={btns}")
        if label != want_label:
            problems.append(
                f"[{tag}] 收起那一行标签是 {label!r}，期望 {want_label!r}"
            )
        if not ok_b:
            problems.append(
                f"[{tag}] 收起那一行按钮数 {btns}，期望 {want_btns} —— "
                + (
                    "macOS 上「收进任务栏」必须消失（按下它等于收起，留着就是两颗按钮干同一件事）"
                    if tag == "macos"
                    else "Windows 上两颗都该在"
                )
            )

    # ---------------- 抽屉里不许再出现行内小字 ----------------
    #
    # ★★ 2026-09-28：原来这里有一条断言「macOS 的收起提示里不许出现『任务栏』」，
    #   它读的是那一行的 sub。设置面板的行内小字本轮**全部删除**之后，
    #   `collapseRowSub` 恒为 `none`，那条断言变成**永真** ——
    #   它再也不会失败，于是给人一个"平台用语有人守着"的错觉。
    #   失效的门槛比没有门槛更危险，所以换成一条真有判别力的：
    #
    #   **抽屉里 `.grp-l .grp-s`（标签下面那行小字）的数量必须为 0。**
    #
    # ★ 为什么量 `.grp-l .grp-s` 而不是 `.grp-s`：
    #   块级说明（自检结论、模型缺失警告、"最近听到"、诊断块）**仍然是**
    #   `.grp-s`，它们不是"功能介绍小字"，也不该被这条断言误伤。
    #   `.grp-l .grp-s` 精确命中"行内副标题"那一种形态 ——
    #   与 `styles.css` 里那条选择器是同一个判据。
    #
    # ★ 这条同时守住本轮的成果：以后谁想再加一行小字回来，
    #   这道门槛会报红（`sub` prop 已从 `Row`/`SliderRow` 上拆掉，
    #   想加回来得先把 prop 装回去 —— 那是第二次改动，该被讨论一次）。
    print("--- 设置里不许有行内小字 " + "-" * 36)
    for tag, vals in (("windows", win_set), ("macos", mac_set)):
        n = num(vals, "inlineSubs")
        print(f"  {tag}: 行内小字 {n} 处")
        if n != 0:
            problems.append(
                f"[{tag}] 设置抽屉里还有 {n} 处行内小字（.grp-l .grp-s）—— "
                "2026-09-28 起这一页只留标签与控件，说明统一收进「更多」里的功能介绍"
            )

    print("=" * 72)
    if problems:
        print("平台版面检查未过：")
        for p in problems:
            print("  ✗", p)
        return 1
    print(
        "平台版面检查通过 ✅  Windows 逐字节未变；macOS 少了那颗、宽度归了标题；"
        "两侧都没有行内小字。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
