#!/usr/bin/env python3
"""禁止在**提前返回之后**调用 hook（React error #310 ⇒ 整页白屏）。

    python scripts/check-hook-order.py

# 它守的是什么

React 要求同一个组件在每次渲染里调用**数量与顺序都一样**的 hook。
一旦某个 hook 写在组件体的提前返回之后：

  首帧条件不成立 → 直接 return，后面的 hook 一个都不跑；
  条件成立那一帧 → 突然多出 N 个 hook ⇒ **Minified React error #310**。

★ 症状的恶劣之处：压缩产物里 `#310` 就是个数字，
  `https://reactjs.org/docs/error-decoder.html?invariant=310` 是唯一线索 ——
  **界面直接白屏**（本项目里是错误边界的红底），而报错信息指不到任何一行自己的代码。
  ⇒ "是不是 hook 写错位置了"这个假设，正常情况下**根本想不到**。

# 为什么必须用脚本守（本轮的血证）

`SettingsDrawer.tsx` 里**早就写着**一段注释，标题就是
「这两个 hook 必须在所有提前返回之前调用（React 规则 #310）」，
还完整记着上一轮踩坑的症状。然后本轮往另外**四个**组件里加 `useHint` 时，
四个全都写在了提前返回**之后** —— 包括那份写着规则的文件的自己。

⇒ 结论只能有一个：**这条规则靠"读注释记住"是不成立的**，必须机器判。

# ★★ 判据的关键：`return` 在哪一层（第一版就栽在这）

第一版把「提前返回」判成"**任何** `if (…) return;`"，于是报出 20 处 —— 其中
绝大多数是**回调内部的正常退出**：

    useEffect(() => {
      if (!pasteOpen) return;     ← 这是 effect 的守卫，不是组件的提前返回
      …
    }, [pasteOpen]);

这类 `return` 跟 hook 计数毫无关系（每次渲染 effect 都照样调、hook 数不变）。
把它们算进去，等于让门槛**永远红着**，而"永远红"的门槛会被直接忽略掉。

所以现在按**花括号深度**判：只认「组件函数体那一层」的 `return`。

  * 组件体深度 = 函数开头 `{` 之后的深度 D；
  * 只有缩进层级**恰好等于 D** 的 `return` 才算提前返回；
  * `useEffect(() => { … })` 里的 `return` 深度是 D+2 往上 ⇒ 自动排除；
  * IIFE 内部的 `return` 同理排除。

深度用"扫整份文件的 `{}` 计数"来算 —— 对 TSX 的 `{expr}` 属性包、
对象字面量也成立（它们都成对且不跨出函数）。
★ 唯一的已知弱点：字符串/注释里的裸花括号会误计。注释已先清空，
  字符串里出现不平衡 `{` 的写法在本项目里没有（有的话这里会报出可疑行，
  人一眼可辨 —— 宁可让人来判，也不要静默漏过）。
"""
from __future__ import annotations

import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

EXTS = {".tsx", ".ts", ".jsx", ".js"}

# 组件开头：export function X( / function X( / const X = (…) => / const X = () => {
# ★ 只看大写开头 —— React 组件必须大写，小写的是普通工具函数。
COMPONENT = re.compile(
    r"^(?:export\s+)?(?:function\s+([A-Z]\w*)\s*\(|const\s+([A-Z]\w*)\s*(?::[^=]+)?=\s*(?:\([^)]*\)|\w+)\s*=>)",
    re.M,
)

HOOK_CALL = re.compile(r"\buse[A-Z]\w*\s*\(")

# 组件体那一层的 return（行首缩进之后直接 return，或 if (…) return）
RETURN_STMT = re.compile(r"^\s*(?:if\s*\(.*?\)\s*(?:\{\s*)?)?return\b")

LINE_COMMENT = re.compile(r"^\s*(//|\*|/\*)")
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)


def blank_comments(text: str) -> str:
    """块注释换成等长空格（保住行号），行注释行整行清掉。"""
    text = BLOCK_COMMENT.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)
    kept = []
    for line in text.split("\n"):
        kept.append(" " * len(line) if LINE_COMMENT.match(line) else line)
    return "\n".join(kept)


def depth_at_line(text: str, target_line: int) -> int:
    """第 target_line 行**行首**所处（或之前的最后一个）花括号深度。"""
    lines = text.split("\n")
    d = 0
    for i in range(min(target_line - 1, len(lines))):
        line = lines[i]
        d += line.count("{") - line.count("}")
    return d


def component_spans(text: str) -> list[tuple[str, int, int, int]]:
    """返回 [(名字, 起始行, 结束行, 函数体深度)]。

    ★★ 结束行必须按**花括号配对**算，不能"下一个大写组件开头的前一行"：
       `export function useApp()`（小写、是 hook 不是组件）不会被 COMPONENT 匹配，
       于是 `AppProvider` 的区间会一路吃到文件末，把后面所有 hook 都算成它的 ——
       第一版就这么误报了一处（store.tsx:454 的 `useContext`）。

    做法：从签名所在行往下扫，用 depth 归零找到函数体的闭合 `}`。
    """
    starts = []
    for m in COMPONENT.finditer(text):
        name = m.group(1) or m.group(2)
        line = text.count("\n", 0, m.start()) + 1
        starts.append((name, line, m.start()))

    lines = text.split("\n")
    spans = []
    for name, line, _pos in starts:
        body_depth = depth_at_line(text, line) + 1
        end = len(lines)
        d = depth_at_line(text, line)
        for n in range(line, len(lines) + 1):
            d += lines[n - 1].count("{") - lines[n - 1].count("}")
            if n > line and d < body_depth:
                end = n
                break
        spans.append((name, line, end, body_depth))
    return spans


def check_file(path: Path) -> list[tuple[int, str, int, str]]:
    """返回 [(hook 行号, hook 原文, 提前返回行号, 提前返回原文)]。"""
    raw = path.read_text(encoding="utf-8")
    text = blank_comments(raw)
    lines = text.split("\n")

    problems = []
    for name, start, end, body_depth in component_spans(text):
        first_return = None
        for n in range(start, end):
            line = lines[n - 1]
            if depth_at_line(text, n) != body_depth:
                continue
            if RETURN_STMT.match(line):
                first_return = (n, line.strip())
                break
        if first_return is None:
            continue
        for n in range(first_return[0] + 1, end):
            line = lines[n - 1]
            # hook 也只看组件体那一层 + 它的直接子层（多行 hook 调用）
            if depth_at_line(text, n) < body_depth:
                continue
            if HOOK_CALL.search(line):
                problems.append((n, line.strip()[:90],
                                 first_return[0], first_return[1][:70]))
    return problems


def main() -> int:
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    hits = []
    for path in sorted(SRC.rglob("*")):
        if path.suffix not in EXTS or not path.is_file():
            continue
        for n, text, rn, rtext in check_file(path):
            hits.append((path.relative_to(ROOT), n, text, rn, rtext))

    print(f"--- 扫描 {SRC.relative_to(ROOT)} 下 hook 与提前返回的先后 " + "-" * 12)
    if hits:
        for rel, n, text, rn, rtext in hits:
            print(f"  ✗ {rel}:{n}  {text}")
            print(f"      该组件体的第一个提前返回在第 {rn} 行：{rtext}")
        print()
        print("=" * 72)
        print(f"发现 {len(hits)} 处 hook 写在提前返回之后 —— 运行时白屏。")
        print()
        print("症状：Minified React error #310（压缩产物里看不出是 hook 问题），")
        print("      界面直接白屏 / 错误边界红底，报错指不到自己的任何一行。")
        print()
        print("怎么改：把那些 hook 的**声明**整体上移到第一个提前返回之前。")
        print("  ★ 只上移声明；原来在下面的其它语句不用动。")
        print("  ★ 参考 SettingsDrawer.tsx 里 `useToggleCollapsed` 那几行。")
        return 1

    print("  未发现 hook 写在提前返回之后 ✅")
    print("=" * 72)
    print("hook 顺序检查通过 ✅  没有 #310 风险。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
