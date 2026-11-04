#!/usr/bin/env python3
"""段落样式的预设数量：Rust 常量 ↔ 两套主题的 CSS 变量，必须一一对上。

    python scripts/check-style-presets.py

# 它守的是什么

「标重点」的颜色是**预设索引**（`ParaStyle.color: Option<u8>`），不是色值 ——
后端只存"第 3 号色"，真正长什么样由 `styles.css` 的主题块定义：

    :root                       { --mark-0 … --mark-3 }   ← 深色
    :root[data-theme="light"]   { --mark-0 … --mark-3 }   ← 浅色

于是**同一个数字有两处实现**：`script::COLOR_PRESETS` 说"有 4 种"，
`styles.css` 负责真的给出这 4 个颜色。哪一边多、哪一边少，都不会报错：

* 后端多一个（5）而 CSS 只有 4 个 ⇒ 用户选了第 5 号，`var(--mark-4)`
  求值为空 ⇒ 那一段字变成**继承色**，看着像"这个颜色没生效"；
* CSS 多一个而常量还是 4 ⇒ 那颗色块根本不会被画出来（前端按后端的数量渲染），
  等于白写一条没人用的样式；
* 浅色主题漏一条 ⇒ **只在浅色下**出错。这类"只在某个主题下坏"的问题
  截图很难覆盖全，往往要等用户抱怨"浅色模式下这个颜色没了"。

# 为什么是静态检查而不是出图量测

两件事：① 出图只能验证"我截的那一档主题"，而这里要的是**两档都对**；
② 这是一条纯文本对应关系，读两个文件几毫秒就完，没有理由驱动一次浏览器。
（准入条件是"截图/编译发现不了 + 几毫秒"，见 `gates.py` 顶部的注释。）

# ★ 改完必须做变异测试

  · 把 `script.rs` 的 `COLOR_PRESETS` 改成 5 ⇒ 必须报红（缺 `--mark-4`）；
  · 删掉浅色块里的一条 `--mark-3` ⇒ 必须报红（浅色缺一档）；
  · 把某条 `--markbg-1` 写成 `--mark-2` ⇒ 必须报红（重号 ⇒ 少一个）。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT_RS = ROOT / "src-tauri" / "src" / "script.rs"
CSS = ROOT / "src" / "styles.css"

# `:root` / `:root[data-theme="light"]` 这种"主题块开头"。
# ★ 不能只判 `startswith(":root")` —— `:root.no-glass .stylebar {` 也满足，
#   而它不是主题块（那是玻璃兜底的一条规则），混进来会把检查范围搞乱。
ROOT_BLOCK = re.compile(r'^(:root(?:\[[^\]]+\])?)\s*\{\s*$')
DEF = re.compile(r"^--(mark|markbg)-(\d+)\s*:")


def rust_const(name: str) -> int:
    src = SCRIPT_RS.read_text(encoding="utf-8")
    m = re.search(rf"pub const {name}: u8 = (\d+);", src)
    if not m:
        raise SystemExit(f"在 {SCRIPT_RS.name} 里找不到 `{name}` —— 常量改名了？本门槛需要跟着改。")
    return int(m.group(1))


def theme_blocks() -> dict[str, dict[str, list[int]]]:
    """`{主题块选择器: {"mark": [...], "markbg": [...]}}` —— 按出现顺序。"""
    out: dict[str, dict[str, list[int]]] = {}
    cur: str | None = None
    for raw in CSS.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if cur is None:
            m = ROOT_BLOCK.match(line)
            if m:
                cur = m.group(1)
                out.setdefault(cur, {"mark": [], "markbg": []})
            continue
        # 主题块以顶格的 `}` 结束（块内的规则都有缩进）
        if line == "}":
            cur = None
            continue
        d = DEF.match(line)
        if d:
            out[cur][d.group(1)].append(int(d.group(2)))
    return out


def main() -> int:
    if sys.platform == "win32":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    colors = rust_const("COLOR_PRESETS")
    bgs = rust_const("BG_PRESETS")
    blocks = theme_blocks()

    print("--- 段落样式预设：Rust 常量 ↔ 主题里的 CSS 变量 " + "-" * 18)
    print(f"  Rust：字色 {colors} 档 / 底色 {bgs} 档")

    problems: list[str] = []
    if not blocks:
        problems.append("styles.css 里一个主题块都没解析到 —— 选择器的写法变了？本门槛需要跟着改")
    elif len(blocks) < 2:
        problems.append(
            f"只找到 {len(blocks)} 个主题块（{'、'.join(blocks)}）—— "
            "浅色主题的样式也要定义，否则浅色下这几个颜色会求值成空"
        )

    for name, defs in blocks.items():
        for kind, want in (("mark", colors), ("markbg", bgs)):
            got = defs[kind]
            uniq = sorted(set(got))
            if uniq != list(range(want)):
                # 说得具体一点：缺哪个、重哪个，改的人才知道补什么
                missing = [i for i in range(want) if i not in uniq]
                extra = [i for i in uniq if i >= want]
                dup = sorted({i for i in got if got.count(i) > 1})
                bits = []
                if missing:
                    bits.append("缺 " + "、".join(f"--{kind}-{i}" for i in missing))
                if extra:
                    bits.append("多 " + "、".join(f"--{kind}-{i}" for i in extra))
                if dup:
                    bits.append("重号 " + "、".join(f"--{kind}-{i}" for i in dup))
                problems.append(f"{name} 的 {kind}：" + "；".join(bits))
            else:
                print(f"  {name:<28} --{kind}-0..{want - 1} ✅")

    print("=" * 72)
    if problems:
        print("样式预设检查未过：")
        for p in problems:
            print("  ✗", p)
        print()
        print(
            "两处必须同时改：`src-tauri/src/script.rs` 的 COLOR_PRESETS / BG_PRESETS，"
            "以及 `src/styles.css` **两个**主题块里的 --mark-N / --markbg-N。"
        )
        return 1

    print(f"样式预设检查通过 ✅  两套主题都完整给出 {colors} 个字色 / {bgs} 个底色。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
