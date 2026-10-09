#!/usr/bin/env python3
"""校验 Markdown 里引用的图片**真的在仓库里**。

    python scripts/check-doc-images.py

# 它守的是什么

README 在 2026-10-09 从 1531 行收成产品页，第一次有了配图（`docs/img/*.png`）。
配图的失败模式很阴：

* **编译、tsc、出包、其它门槛一个都不会响。** 它们都不读 Markdown。
* **本地也看不出来。** 路径写错了，本地没人会去点开 README 一张张看。
* 直到有人打开 GitHub 才看见一排碎图 —— 而那时候"产品介绍没重点"的差评已经发生过了。

这和 `check-tooltips.py` / `check-hover-silent.py` 是**同一类问题**：
截图看不见、编译看不见，只能靠一次机械的遍历去数。
所以它也在每次必跑的静态检查里（纯读文件，几毫秒）。

# 判据

* 扫描 `README.md` 与 `docs/*.md`。
* 取 Markdown 的 `![alt](path)` 与 HTML 的 `<img src="path">`。
* **先剥掉围栏代码块和行内代码** —— 文档里正经会贴 `![...](...)` 当示例，
  不剥就会把示例当成真引用。★ 两种都得剥，这不是理论：本文件第一次跑起来
  就栽在**行内代码**上 —— `判断基线.md` 里那句"`![alt](path)` 当示例"是
  单反引号包住的，只剥围栏块时它照样被当真引用，于是门槛对着
  `docs/path` 报了一条断链。**一个假阳性比没有门槛更坏**（下次真断链就没人信了）。
* `http(s)://` / `data:` 跳过；其余按**引用它的那个文件所在目录**解析。
* 路径先做 URL 解码（`%20` 之类），再判存在。
* 顺带报一遍 `docs/img/` 里的**孤儿图**（没有任何文档引用）—— 只提示，不判失败：
  图可以先入库、后引用，这不构成错误。

# 退出码

  0 = 全部引用都能落到文件上；1 = 有断链（打印 文件:行号 → 路径）。
"""

from __future__ import annotations

import os
import re
import sys
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# 扫描范围：仓库根的 README + docs 下的 Markdown（不递归进 docs/shots —— 那里只有图）
def doc_files() -> list[str]:
    out = ["README.md"]
    d = os.path.join(ROOT, "docs")
    if os.path.isdir(d):
        out += [
            os.path.join("docs", n)
            for n in sorted(os.listdir(d))
            if n.endswith(".md")
        ]
    return out

MD_IMG = re.compile(r"!\[[^\]]*\]\(\s*<?([^)>\s]+)>?\s*(?:\"[^\"]*\"|'[^']*')?\s*\)")
HTML_IMG = re.compile(r"""<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["']""", re.I)
FENCE = re.compile(r"^\s*(```|~~~)")
# 行内代码：单反引号或双反引号包住的片段。★ 必须剥：文档里用行内代码举
# 「图片语法长什么样」的例子是很常见的写法，不剥就是持续的假阳性。
INLINE_CODE = re.compile(r"``[^`]*``|`[^`]*`")


def strip_fences(text: str) -> list[str]:
    """把围栏代码块的内容换成空行 —— 行号仍然对得上。"""
    out, inside = [], False
    for line in text.splitlines():
        if FENCE.match(line):
            inside = not inside
            out.append("")
            continue
        out.append("" if inside else line)
    return out


def refs_in(path: str):
    """→ [(行号, 原始路径)]"""
    with open(path, encoding="utf-8") as f:
        lines = strip_fences(f.read())
    found = []
    for i, line in enumerate(lines, 1):
        # 行内代码先抹掉再找 —— 否则 `![alt](path)` 这种示例会被当成真引用。
        line = INLINE_CODE.sub(" ", line)
        for m in MD_IMG.finditer(line):
            found.append((i, m.group(1)))
        for m in HTML_IMG.finditer(line):
            found.append((i, m.group(1)))
    return found


def main() -> int:
    problems, scanned, used = [], 0, set()

    for rel in doc_files():
        abs_md = os.path.join(ROOT, rel)
        if not os.path.exists(abs_md):
            continue
        scanned += 1
        base = os.path.dirname(abs_md)
        for lineno, raw in refs_in(abs_md):
            if raw.startswith(("http://", "https://", "data:", "//")):
                continue
            target = urllib.parse.unquote(raw.split("#")[0])
            if not target:
                continue
            resolved = os.path.normpath(os.path.join(base, target))
            if os.path.exists(resolved):
                used.add(os.path.relpath(resolved, ROOT).replace("\\", "/"))
            else:
                problems.append(f"{rel}:{lineno} → {raw}（找不到 {resolved}）")

    print(f"扫描 {scanned} 份文档，图片引用全部落地检查完毕。")

    # 孤儿图：只提示，不判失败
    img_dir = os.path.join(ROOT, "docs", "img")
    if os.path.isdir(img_dir):
        orphans = [
            f"docs/img/{n}"
            for n in sorted(os.listdir(img_dir))
            if n.lower().endswith((".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"))
            and f"docs/img/{n}" not in used
        ]
        if orphans:
            print(f"ℹ️ docs/img/ 下 {len(orphans)} 张图没有被任何文档引用（不算错）：")
            for o in orphans:
                print("   ·", o)

    if problems:
        print(f"\n发现 {len(problems)} 处断链：")
        for p in problems:
            print("  ✗", p)
        print("\n怎么修：把路径改成相对**引用它的那份文档**的正确位置，"
              "或把图片放到那个位置。")
        return 1
    print("✅ 全部图片引用都有对应的文件。")
    return 0


if __name__ == "__main__":
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    sys.exit(main())
