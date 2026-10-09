#!/usr/bin/env python3
"""校验 Markdown 里引用的东西**真的在仓库里** —— 图片与文档链接。

    python scripts/check-doc-refs.py

# 它守的是什么

README 在 2026-10-09 从 1531 行收成产品页，第一次有了配图（`docs/img/*.png`），
同时把工程内容拆成三份新文档 ⇒ 大量新增的**相对引用**。这类引用的失败模式很阴：

* **编译、tsc、出包、其它门槛一个都不会响。** 它们都不读 Markdown。
* **本地也看不出来。** 路径写错了，没人会去把 README 里的链接一个个点开。
* 直到有人打开 GitHub 才看见一排碎图和一堆死链 —— 那时候"产品介绍没重点"
  的印象已经发生了。

这和 `check-tooltips.py` / `check-hover-silent.py` 是**同一类问题**：
截图看不见、编译看不见，只能靠一次机械遍历去数。所以它也在每次必跑的静态检查里
（纯读文件，几毫秒）。

# 为什么把"链接"也一起查了（原本只管图片）

2026-10-09 那次重构里，**悬空引用手工修了 6 处**（拆分产生的 4 处 + 我自己写的 2 处）——
同一天、同一个人、同一个错误重复 6 次，说明它不该靠"记得检查"。
⇒ 一次遍历能同时查的东西，就别留给下一个人用脑子记。

★ 加完链接检查后**第一次跑就抓到 2 处真实断链**，且都是之前所有手段都发现不了的：

* `docs/能力与验证状态.md` 里的 `src-tauri/src/stealth/macos.rs` —— 它在 README 里
  是对的（相对仓库根），**被拆进 `docs/` 之后就断了**。这是拆分本身引入的回归，
  靠肉眼 diff 基本不可能发现（路径长得完全正常）。
* `docs/上传GitHub操作指引.md` 里写 `docs/macOS上机测试清单.md` —— 在 `docs/` 里
  就成了 `docs/docs/…`。

⇒ 这两条正好把"为什么必须机械遍历"讲完了：**相对路径的正确性取决于文件在哪，
文件一搬家，所有相对引用都可能悄悄失效。**

# 判据

* 扫描 `README.md` 与 `docs/*.md`（不递归进 `docs/shots/` —— 那里只有图）。
* 取三类引用：Markdown 图片 `![alt](path)`、HTML `<img src="...">`、
  Markdown 链接 `[text](path)`（`<a href>` 也收）。
* **先剥掉围栏代码块和行内代码** —— 文档里正经会贴这些语法当示例，不剥就是假阳性。
  ★★ 两种都得剥。只剥围栏块时，`` `![alt](path)` `` 这种**行内代码**里的例子
  会被当成真引用 ⇒ 门槛对着不存在的 `docs/path` 报假断链。
  **一个假阳性比没有门槛更坏**（真断链出现时没人再信它）。
* 跳过 `http(s)://` / `data:` / `mailto:` / `//`；**纯锚点（`#xxx`）跳过**。
* 其余按**引用它的那份文档所在目录**解析；路径先 URL 解码（`%20` 之类）；
  带 `#锚点` 的只校验文件那一段（★ **故意不校验锚点** —— 忠实复现 GitHub 对中文
  标题的 slug 规则做不到，做不到就会制造假阳性，那比不查更坏）。
* 目录也算落地（`docs/` 这种链接合法）。
* 顺带报 `docs/img/` 里的**孤儿图**（没有任何文档引用）—— 只提示，不判失败：
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
# (?<!!) —— 别把图片当成链接再报一遍
MD_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(\s*<?([^)>\s]+)>?\s*(?:\"[^\"]*\"|'[^']*')?\s*\)")
IMG_TAG = re.compile(r"""<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["']""", re.I)
A_TAG = re.compile(r"""<a\b[^>]*?\bhref\s*=\s*["']([^"']+)["']""", re.I)

FENCE = re.compile(r"^\s*(```|~~~)")
# 行内代码：单反引号或双反引号包住的片段。★ 必须剥：文档里用行内代码举
# 「这些语法长什么样」的例子是很常见的写法，不剥就是持续的假阳性。
INLINE_CODE = re.compile(r"``[^`]*``|`[^`]*`")

SKIP_PREFIX = ("http://", "https://", "data:", "mailto:", "tel:", "//")
IMG_EXT = (".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".avif")


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
    """→ [(行号, 原始路径, 是图片)]"""
    with open(path, encoding="utf-8") as f:
        lines = strip_fences(f.read())
    found = []
    for i, line in enumerate(lines, 1):
        # 行内代码先抹掉再找 —— 否则 `![alt](path)` 这种示例会被当成真引用。
        line = INLINE_CODE.sub(" ", line)
        for m in MD_IMG.finditer(line):
            found.append((i, m.group(1), True))
        for m in IMG_TAG.finditer(line):
            found.append((i, m.group(1), True))
        for m in MD_LINK.finditer(line):
            found.append((i, m.group(1), False))
        for m in A_TAG.finditer(line):
            found.append((i, m.group(1), False))
    return found


def main() -> int:
    problems, scanned, used = [], 0, set()

    for rel in doc_files():
        abs_md = os.path.join(ROOT, rel)
        if not os.path.exists(abs_md):
            continue
        scanned += 1
        base = os.path.dirname(abs_md)
        for lineno, raw, is_img in refs_in(abs_md):
            if raw.startswith(SKIP_PREFIX):
                continue
            # 只看文件那一段：`foo.md#锚点` / `#纯锚点`
            target = urllib.parse.unquote(raw.split("#", 1)[0])
            if not target:
                continue  # 纯锚点，跳过（锚点不校验，见文件头）
            resolved = os.path.normpath(os.path.join(base, target))
            if os.path.exists(resolved):
                used.add(os.path.relpath(resolved, ROOT).replace("\\", "/"))
            else:
                kind = "图片" if is_img or target.lower().endswith(IMG_EXT) else "链接"
                problems.append(f"{rel}:{lineno} [{kind}] {raw}（找不到 {resolved}）")

    print(f"扫描 {scanned} 份文档：图片与链接的落地检查完毕。")

    # 孤儿图：只提示，不判失败
    img_dir = os.path.join(ROOT, "docs", "img")
    if os.path.isdir(img_dir):
        orphans = [
            f"docs/img/{n}"
            for n in sorted(os.listdir(img_dir))
            if n.lower().endswith(IMG_EXT)
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
              "或把被引用的文件放到那个位置。")
        return 1
    print("✅ 全部引用都有对应的文件。")
    return 0


if __name__ == "__main__":
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    sys.exit(main())
