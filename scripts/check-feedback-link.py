#!/usr/bin/env python3
"""守住「设置 → 关于 › 意见反馈」这条外链的三个不变量。

    python scripts/check-feedback-link.py

纯读源码，几毫秒 ⇒ 所以它进 `gates.py` 的 `STATIC_CHECKS`（每次门槛都跑）。

# 它守的三个不变量（每一个都是"截图与编译都发现不了"的）

| 不变量 | 坏了会怎样 | 为什么没人会发现 |
|---|---|---|
| ① 地址在全仓**只有一份** | 有人改了一处、漏了另一处 ⇒ 用户点到的还是旧表单 | 两处都长得**完全正常**；diff 里看不出"这是同一个地址的副本" |
| ② 那一份在 `FEEDBACK` 常量里 | 常量被绕开、地址被就地内联 ⇒ 下次换表单必然漏改 | 一样能跑、一样能打开，只是"唯一出处"这条纪律没了 |
| ③ 那一行还在、还接着 `open_url` | 入口被删 / onClick 被换成别的东西 ⇒ **用户提不了意见** | tsc 不会报（少一行 row 是合法的）；截图里"少了一行"和"设计如此"长得一样 |

★ 为什么它**不**判断"地址对不对"：`FEEDBACK` 那份是飞书的**分享直链**
（`share/base/webpage/<分享id>`），换表单就会变。把分享 id 抄进这里当断言，
等于换一次表单要改两个文件 —— 而"改了两处就会有第三处忘了改"正是①要防的事。
⇒ 这里只认**形状**（`https://<租户>.feishu.cn/share/base/webpage/<id>`）
与**条数**；"这个表单是不是我们要的那个"只能靠人点一次确认。
换了表单：把新地址贴进 `SettingsDrawer.tsx` 的 `FEEDBACK`，**再手工点一次**。
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
DRAWER = SRC / "components" / "SettingsDrawer.tsx"

# 地址的**形状**（不是具体值 —— 理由见文件头）。
# 强制 https：这种外链跳到 http 会被浏览器拦一截，而 `open_url` 后端
# 只校验协议是 http/https，**不校验内容** ⇒ 写成 http 它照样"成功"。
URL_RE = re.compile(
    r"https://[A-Za-z0-9.-]+\.feishu\.cn/share/base/webpage/[A-Za-z0-9_-]+"
)

# 「唯一出处」的那个常量块。用 DOTALL 抓 `const FEEDBACK = { … } as const;`。
FEEDBACK_BLOCK_RE = re.compile(r"const FEEDBACK\s*=\s*\{(.*?)\}\s*as const;", re.S)

# 那一行的标签与它的接线（**必须走常量**，不许就地写字符串）。
ROW_LABEL = 'label="意见反馈"'
WIRING = "api.openUrl(FEEDBACK.formUrl)"


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    problems: list[str] = []

    if not DRAWER.is_file():
        print(f"✗ 找不到 {DRAWER}")
        return 1
    drawer_src = DRAWER.read_text(encoding="utf-8", errors="replace")

    # ---------- ② 地址在 FEEDBACK 常量里 ----------
    block = FEEDBACK_BLOCK_RE.search(drawer_src)
    if not block:
        problems.append(
            "找不到 `const FEEDBACK = { … } as const;` —— 意见反馈的地址必须有一个"
            "**唯一出处**的常量。地址散在 JSX 里，换表单时就必然漏改。"
        )
        urls_in_const: list[str] = []
    else:
        urls_in_const = URL_RE.findall(block.group(1))
        if len(urls_in_const) != 1:
            problems.append(
                f"`FEEDBACK` 常量里的飞书分享地址有 {len(urls_in_const)} 个，期望 1 个"
                "（形状要像 https://<租户>.feishu.cn/share/base/webpage/<id>）"
            )

    # ---------- ① 全仓只有一份 ----------
    # 扫 src/ 下所有 ts/tsx（构建产物 dist/ 不算 —— 它是生成物）。
    hits: list[str] = []
    for p in sorted(SRC.rglob("*")):
        if p.suffix not in (".ts", ".tsx") or not p.is_file():
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        for _ in URL_RE.findall(text):
            hits.append(str(p.relative_to(ROOT)).replace("\\", "/"))

    if len(hits) != 1:
        problems.append(
            f"这份飞书地址在 `src/` 下出现了 {len(hits)} 次（{hits or '一次都没有'}），"
            "要求**恰好 1 次**。★ 多出来的每一份都是一个将来必漏改的副本 —— "
            "而两处都长得完全正常，diff 里看不出来。要让别处用它，"
            "只 import `FEEDBACK`，不要复制字符串。"
        )

    # ---------- ③ 那一行还在、还接着后端 ----------
    for needle, why in (
        (
            ROW_LABEL,
            "设置里找不到 `意见反馈` 那一行 —— 入口被删掉，用户就提不了意见了"
            "（tsc 不会报，少一行 row 是合法的）",
        ),
        (
            WIRING,
            "找不到 `api.openUrl(FEEDBACK.formUrl)` —— 那颗按钮没接后端，"
            "或者没走 `FEEDBACK` 常量。★ 在桩里'点了没反应'与'压根没接后端'"
            "长得一模一样，所以这里必须按结构断言",
        ),
    ):
        if needle not in drawer_src:
            problems.append(why)

    # ---------- 报告 ----------
    print("=" * 72)
    if urls_in_const:
        print(f"· 地址（唯一出处，`FEEDBACK` 常量）：{urls_in_const[0]}")
    print(f"· src/ 下出现次数：{len(hits)}（要求 1）")
    print("=" * 72)

    if problems:
        print("意见反馈外链检查未过：")
        for p in problems:
            print("  ✗", p)
        return 1
    print("意见反馈外链检查通过 ✅  地址唯一出处、形状合法（https + 飞书分享直链）、"
          "那一行与后端接线都在。")
    print("  ⚠️ 它不判断\"这个表单是不是我们要的那个\" —— 换过表单请手工点一次确认。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
