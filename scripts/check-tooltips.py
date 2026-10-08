#!/usr/bin/env python3
"""禁止界面上出现**任何**提示 —— 原生 `title` 与自绘气泡都不许有。

    python scripts/check-tooltips.py

# 它守的是什么（这一段是本项目最贵的一条教训，别删）

这个门槛换过三代：

**第一代（第四轮之前）**：界面里每一个按钮、每一行都挂着原生 `title`。
鼠标扫过去系统就弹一个白底黑框小条。用户：「**特别特别 low，而且交互
体验会特别差**」。→ 于是做了自绘气泡。

**第二代（第十轮）**：全部换成自绘气泡（`useHint` + `Hint.tsx`）：
跟随主题、能换行、会避让窗口边界。**视觉问题解决了，问题本身没解决。**

**第三代（第十一轮，现在）**：用户看完截图说「**不行，还是删掉全部的
提示吧**」。对。根因不在"气泡好不好看"，在于**「鼠标扫过按钮就弹出东西」
这件事本身**：

  1. 它**遮住稿子** —— 在提词器上这是最大的一宗罪。触发它只需要鼠标**路过**。
  2. 它**不受控** —— 提词时鼠标往往就停在窗口上，气泡反复弹出又收回。
  3. 里面装的**不是用户当时需要的东西** —— 那是给开发看的解释，
     不是给正在念稿的人看的。

⇒ 结论：**界面上所有说明文字一律不留。**

# 判据：查**名字**，不查意图

本文件拒绝下面这些词出现在 `src/` 的任何源码或样式里：

    title=          （且标签名小写 ⇒ 是 DOM 属性）
    useHint  Hint  （那个 hook 与那个组件）
    tip-bubble  hint-ico  （气泡与感叹号按钮的样式类）

★ 为什么"查名字"够用：这个项目里没有别的用途会用到这几个名字。
  只要它们出现，就说明有人把提示加回来了 —— **宁可误报让人来删这行
  门槛，也不要安静地漏掉**。这是用户明确点名过的硬要求，不是风格偏好。

★ 什么**不算**违规（都是刻意放行的）：
  * 组件 prop 上的 `title=`（`<Group title="提词">`）—— 它只传文字，
    不落 DOM、不产生系统 tooltip。判据是**标签名小写开头**。
  * `aria-label` / `aria-describedby` —— 那是**读屏**通道，不是视觉提示。
    无障碍不受这条禁令约束，本项目一直都有。
  * `scriptExtsHint` / `fallback_hint` / `collapse_hint` 这类**后端字段名** ——
    它们不是那几个词，也早已由常驻文字消费（不是悬停）。
  * 注释里的说明文字 —— 注释不是界面。

# ★★ 为什么不能一行一行地正则（第一版的致命缺陷）

第一版是逐行 `re.search` 扫 `<tag ... title=`，它**漏掉了一整个类别**：

    <input                ← 标签名在这行，title 在三行之后
      ref={inputRef}
      className="fb-input"
      title="关闭"        ← 逐行扫描看不见，因为本行没有 `<`
    />

JSX 里**绝大多数**标签都是这么写的（一个属性一行）。
⇒ 逐行扫描的实际命中率取决于"作者有没有恰好把 title 写在开标签那一行"，
  这跟被守的东西毫无关系。第一次验证时它"通过"了，**是因为它也瞎**。

这个教训比它守的规则更重要，所以写在这里：
**一道门槛"通过"了什么也证明不了，要先证明它"能失败"。**
本次用变异测试（往 `FindBar` 塞一个 title）才发现 —— 逐行版照样报绿。
⇒ 任何改动这个脚本的人，改完**必须**做一次变异测试（塞 → 报红 → 还原）。
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

# 只看这些源码类型
EXTS = {".tsx", ".ts", ".jsx", ".js", ".css"}

# ---------------------------------------------------------------------------
# 一、原生 title
# ---------------------------------------------------------------------------
#
# 一个"开标签单元"：从 `<tag` 一直到它自己的 `>` 或 `/>` 收尾。
# ★ [^<>] 的含义就是"不许跨过另一个标签的尖括号" —— 这样它天然停在开标签末尾，
#   不会一口气吃到后面几十行。跨行靠 re.S 让字符类吃掉换行。
TAG_UNIT = re.compile(r"<([a-z][a-zA-Z0-9]*)\b[^<>]*>", re.S)
TITLE_ATTR = re.compile(r"\btitle\s*=")

# ---------------------------------------------------------------------------
# 二、自绘气泡的"复活词"
# ---------------------------------------------------------------------------
#
# ★ 用 \b 锚定整词，避免误伤 `scriptExtsHint` / `collapse_hint` 这类
#   后端字段名（它们的 `Hint` / `hint` 前面是字母，\b 不成立）。
FORBIDDEN_WORDS = [
    (re.compile(r"\buseHint\b"), "useHint（气泡 hook）"),
    (re.compile(r"\bHint\b"), "Hint（气泡组件）"),
    (re.compile(r"\btip-bubble\b"), "tip-bubble（气泡样式类）"),
    (re.compile(r"\bhint-ico\b"), "hint-ico（感叹号按钮样式类）"),
]

# 整行都是注释 ⇒ 跳过。
LINE_COMMENT = re.compile(r"^\s*(//|\*|/\*|#)")

# 块注释（{/* ... */}、/* ... */）：扫描前整体剔除。
# ★ 必须剔除，否则文件头那段"反例"文档注释会把脚本自己判红。
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)


def line_of(text: str, index: int) -> int:
    """字符下标 → 行号（从 1 起）。"""
    return text.count("\n", 0, index) + 1


def blank_comments(text: str) -> str:
    """把块注释换成等长空格（保住行号），行注释行整行清掉。

    ★ 用**等长**替换：直接删掉注释会让后面所有行号整体前移，
      报出来的位置和真实文件对不上，人还得自己数 —— 那就白报了。
    """
    text = BLOCK_COMMENT.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)
    kept = []
    for line in text.split("\n"):
        kept.append(" " * len(line) if LINE_COMMENT.match(line) else line)
    return "\n".join(kept)


def check_title(path: Path) -> list[tuple[int, str, str]]:
    """返回 [(行号, 标签名, 该标签的开标签原文)] 形式的原生 title 命中。"""
    text = blank_comments(path.read_text(encoding="utf-8"))
    hits: list[tuple[int, str, str]] = []
    for m in TAG_UNIT.finditer(text):
        unit = m.group(0)
        if not TITLE_ATTR.search(unit):
            continue
        compact = " ".join(unit.split())
        hits.append((line_of(text, m.start()), m.group(1), compact[:110]))
    return hits


def check_forbidden(path: Path) -> list[tuple[int, str]]:
    """返回 [(行号, 命中的词)] —— 任何"把提示加回来"的写法。"""
    # ★ 这里**不能**用 blank_comments：注释里提到这些名字是**允许**的
    #   （它们正是本脚本的守则本身）。所以按行判：整行是注释就跳过，
    #   行尾注释里的命中也不报（代码里出现才算违规）。
    out: list[tuple[int, str]] = []
    for i, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
        if LINE_COMMENT.match(line):
            continue
        # 去掉行尾注释再判
        code = line.split("//", 1)[0]
        for rx, label in FORBIDDEN_WORDS:
            if rx.search(code):
                out.append((i, label))
    return out


def main() -> int:
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    title_hits: list[tuple[Path, int, str, str]] = []
    word_hits: list[tuple[Path, int, str]] = []

    for path in sorted(SRC.rglob("*")):
        if path.suffix not in EXTS or not path.is_file():
            continue
        rel = path.relative_to(ROOT)
        for n, tag, unit in check_title(path):
            title_hits.append((rel, n, tag, unit))
        for n, label in check_forbidden(path):
            word_hits.append((rel, n, label))

    print("--- 检查界面上有没有提示（原生 title / 自绘气泡）" + " " + "-" * 16)

    if title_hits:
        for rel, n, tag, unit in title_hits:
            print(f"  ✗ {rel}:{n}  <{tag} …>  {unit}")
        print()
        print("=" * 72)
        print(f"发现 {len(title_hits)} 处原生 `title` —— 界面上不该出现系统 tooltip。")
        print()
        print("这个项目**不留任何悬停提示**（判据与全过程见本文件顶部）。")
        print("怎么改：把那行 `title=\"...\"` 整个删掉。")
        print("  · 若它说的是**运行状态**（「现在是什么」）或**警告**（不做会出事），")
        print("    那它不该是提示，应当**就地常驻显示**（.grp-s 那种小字）。")
        print("  · 若它只是**解释原理**，删掉不要 —— 那些属于注释/README。")
        print("★ 全写一遍太啰嗦就直接删；缺了它用户真会做错事的那半句，改常驻。")
        return 1

    if word_hits:
        for rel, n, label in word_hits:
            print(f"  ✗ {rel}:{n}  {label}")
        print()
        print("=" * 72)
        print(f"发现 {len(word_hits)} 处气泡机制的残留 —— 悬停提示已经被整个删掉。")
        print()
        print("第十一轮用户的原话：「不行，还是删掉全部的提示吧」。")
        print("整个自绘气泡（useHint / Hint / .tip-bubble / .hint-ico）")
        print("已经删除，包括它的样式。**不要把它加回来**：")
        print("  · 根因不是气泡好不好看，而是「鼠标扫过就弹东西」这件事本身 ——")
        print("    它会遮住稿子，而提词器上盖住正文是最大的一宗罪。")
        print("  · 界面上所有说明文字一律不留。")
        print()
        print("怎么改：把这一行整句删掉。若它承载的是状态或警告，")
        print("        改成就地常驻的小字（.grp-s），而不是悬停可见。")
        print("★ 理由与全过程见 src/components/Hint.tsx 顶部那段注释。")
        return 1

    print("  未发现原生 DOM `title` 属性 ✅")
    print("  未发现自绘气泡机制的残留 ✅")
    print("=" * 72)
    print("提示检查通过 ✅  界面上没有任何悬停提示。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
