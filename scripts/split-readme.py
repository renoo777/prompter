#!/usr/bin/env python3
"""把 README 里的工程内容按「标题边界」切到 docs/ 下 —— **一次性迁移脚本，已经跑过了**。

    python scripts/split-readme.py --dry-run   # 只看会搬什么（当前是空跑：README 里已经没有那些章节）
    python scripts/split-readme.py             # 真正落盘

# 为什么留着它

2026-10-09 把 README 从 1531 行收成产品页（≈210 行）时用它做的切片。
留在这里是为了让「当时到底怎么搬的」可复现、可复核 —— 而不是只留一句
「我搬过了」。**它不会再有第二次用途**：README 里那三个章节已经不存在了，
现在跑它只会打印「找不到章节」然后什么都不做（幂等、安全）。

按项目习惯，一次性脚本也留档：`probe-webview-udd.py` 同理
（那条路走不通，留着是为了别再走一遍）。

# 原则：**只搬运，不重写**

被搬走的都是工程判断的原始记录 —— 改一个字都是信息损失。
所以本脚本按标题边界**原样切片**，不做任何措辞修改。

# 三个目标文档（标题必须与 README 里的原文一字不差）

| 输出 | 收哪些章节 |
|---|---|
| `docs/技术决策与架构.md` | 架构 / 几个关键技术决策 / 目录结构 |
| `docs/能力与验证状态.md` | 现状一览 / 已验证-未验证 / 下一步 |
| `docs/隐身验证与排查.md` | 自动化验证「隐身」是否真的生效 / 排查 |

★ 切完必须**手工**把其它文档里指向「`README.md` 的某某一节」的引用改到新文档 ——
本脚本**不管跨文档引用**。那一次改掉了 5 处（`UI改版-PM拆解.md` ×2、
`判断基线.md` ×2、`磨砂玻璃-调研与方案.md` ×1）。
★★ 而且**不要**去改分析类文档（竞品调研 / 功能缺口分析）里描述「当时 README 怎样」
的句子 —— 那是历史记录，改了才是造假。
"""
from __future__ import annotations

import argparse
import os
import re
import sys

# 本脚本在 `<repo>/scripts/` 下 ⇒ 仓库根是上一层。
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
README = os.path.join(ROOT, "README.md")

# 目标文档：(输出路径, 标题, [要收录的章节标题（按 ## / ### 原文照抄）])
PLAN = [
    ("docs/技术决策与架构.md", "技术决策与架构",
     ["架构", "几个关键技术决策", "目录结构"]),
    ("docs/能力与验证状态.md", "能力清单与验证状态",
     ["现状一览", "已验证 / 未验证", "下一步"]),
    ("docs/隐身验证与排查.md", "隐身验证与排查",
     ["4. 自动化验证「隐身」是否真的生效", "排查"]),
]


def parse_sections(lines):
    """→ [(level, title, start_idx, end_idx_exclusive)]

    章节范围 = 从本行到「下一个 level <= 本 level 的标题」之前。
    """
    heads = []
    for i, ln in enumerate(lines):
        m = re.match(r"^(#{1,6})\s+(.*?)\s*$", ln)
        if m:
            heads.append((len(m.group(1)), m.group(2), i))
    out = []
    for idx, (lvl, title, start) in enumerate(heads):
        end = len(lines)
        for lvl2, _t2, start2 in heads[idx + 1:]:
            if lvl2 <= lvl:
                end = start2
                break
        out.append((lvl, title, start, end))
    return out


def find(sections, title):
    for lvl, t, s, e in sections:
        if t == title:
            return s, e
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    text = open(README, encoding="utf-8").read()
    lines = text.splitlines()
    sections = parse_sections(lines)
    print("README 现有 %d 行，识别出 %d 个标题\n" % (len(lines), len(sections)))

    for rel, docname, titles in PLAN:
        chunks = []
        missing = []
        for t in titles:
            r = find(sections, t)
            if not r:
                missing.append(t)
                continue
            s, e = r
            body = "\n".join(lines[s:e]).rstrip()
            chunks.append(body)
            print("  [%s] ← 「%s」 第 %d-%d 行（%d 行）"
                  % (docname, t, s + 1, e, e - s))
        if missing:
            print("  ✗ 找不到章节：%s" % missing)
            continue
        header = (
            "# %s\n\n"
            "> 本文由 `README.md` 拆出（原样搬运，未改措辞）。\n"
            "> 背景：README 原先 1500+ 行、把设计日志和产品介绍混在一起，对外是「大堆文字没重点」。\n"
            "> 现在 README 只留产品页该有的内容，工程判断与验证记录按主题收到 docs 里。\n\n"
            "---\n\n" % docname
        )
        content = header + "\n\n---\n\n".join(chunks).lstrip() + "\n"
        out = os.path.join(ROOT, rel)
        print("  → %s（%d 字符）" % (rel, len(content)))
        if not args.dry_run:
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8", newline="\n") as f:
                f.write(content)
        print()

    if args.dry_run:
        print("(--dry-run，未写文件)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
