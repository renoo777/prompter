#!/usr/bin/env python3
"""一条命令跑完项目的四道门槛（改完必过）。

    python scripts/gates.py            # 只跑测试与前端构建
    python scripts/gates.py --build    # 再出一次 release 产物（= npm run tauri:build）

四道门槛（见 docs/判断基线.md）：
  1. cargo test --lib                     期望 112 passed / 6 ignored
  2. cargo test --lib --no-default-features 期望 96 passed
  3. 上面两次构建零警告
  4. tsc --noEmit 零错 + vite build 通过 + 静态前端检查（STATIC_CHECKS，
     目前是 check-tooltips.py / check-hook-order.py / check-hover-silent.py /
     check-glass-mode.py / check-doc-images.py）

★ 期望值只认下面那个 `EXPECT` 字典 —— 这份文件头只是说明，数字以 `EXPECT` 为准。
  ★ 2026-10-09 真踩：这两行一直停在 107 / 91，而 `EXPECT` 早已是 112 / 96
    （第十轮 107→112、第十二轮 91→96 都只改了字典）。**两处都在同一个文件里，
    却没有任何机制保证它们一致** —— 所以上面这句"以 EXPECT 为准"必须留着。

★ 为什么不写成一串 shell：
  * Git Bash 的 PATH 被截断（没有 /usr/bin），`grep`/`tail` 都不可用；
  * 沙箱会按命令行文本拦 `cmd /c` 之类；
  * NODE_OPTIONS / PYTHONPATH 被注入时 npm / python 会崩。
  用 Python 直接 subprocess（显式 env、显式 cwd）最省事，也不受上面的影响。

★ 期望数写死在这里是**故意的**：它是"断言"，不是"抄结果"。
  某天数字变了而没人改这里，就该有人来解释为什么。

★ 2026-09-27 起另有两道**平台专项**门槛（跑在最后，见 `PLATFORM_CHECKS`）：
  * `check-macos-backend.py` —— 把 macOS 专属后端在 Windows 上强制编译一遍。
    没有它，"mac 那段到底能不能编过"只有到真 Mac 上才知道。
  * `check-platform-layout.py` —— 两个平台的顶栏版面逐字节校验。
    它守的是"Windows 一个字节都不能变、macOS 少了「任务栏」那条路"。
  ★ 这两道**不放进**上面那个"四道门槛"的计数里，因为它们是平台专项、
    不是每次改 UI 都需要跑；但 `--build` 时会一起跑（出包前值得多花几秒）。

★ 2026-09-27（第十轮）另加一道**每次都要跑**的静态前端门槛（见 STATIC_CHECKS）：
  * `check-tooltips.py` —— 禁止界面里出现原生 `title=`（系统白框 tooltip）。
    ★★ 它之所以必须**每次**跑、而不是只在 `--build` 时跑：
    这个问题**截图看不见**（不悬停就不出现）⇒ 人手 review 一定漏。
    成本几毫秒，收益是拦住下一次"顺手加个 title"。
  * `check-hook-order.py` —— 禁止 hook 写在提前返回之后（React error #310）。
    ★★ 同样是**截图看不见**：白屏之前，编译、tsc、打包**全都过**。
    加它的直接原因：本轮往四个组件里加 `useHint`，四个都写在了提前返回之后
    （其中一份文件里还写着"必须在提前返回之前"的注释)，
    而这个 bug 一路躲过 tsc 和出包，直到预览里 React 整页白屏才暴露。
    ⇒ 又一次证明：**能靠读注释记住的规则，在"刚写完一大段代码"时就是不成立**。
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TAURI_DIR = os.path.join(ROOT, "src-tauri")
IS_WIN = os.name == "nt"

# 平台专项检查（都在 scripts/ 下，用同一个解释器跑）。
# 放在 `--build` 那一步之前：它们比出包快得多，先跑能早点拦住问题。
PLATFORM_CHECKS = ["check-macos-backend.py", "check-platform-layout.py"]

# 静态前端检查：**每次**都跑（不依赖编译、不依赖出包，纯读源码）。
# 加进来的条件：① 守的是截图/编译都发现不了的问题；② 跑起来只要几毫秒。
#
# ★ 2026-09-28（第十一轮）加了第三道 `check-hover-silent.py`。
#   它**不读源码**，而是真的驱动一次预览、把鼠标事件扫过每个控件再数残留 ——
#   所以严格说它不是"静态"检查，只是同样"截图看不见"。
#   为什么非它不可：`check-tooltips.py` 查的是**名字**（title/Hint/tip-bubble）。
#   有人要是换个名字重新实现一套悬停提示（Tooltip.tsx / popover / 自己 append），
#   静态查名字拦不住；而截图里本来就没有悬停态，同样拦不住。
#   ★ 代价：它要跑一次 headless Edge（约 3 秒），比前两道贵得多 ——
#   但它是这一轮用户投诉的**唯一直接验收手段**，值得。
STATIC_CHECKS = [
    "check-tooltips.py",
    "check-hook-order.py",
    "check-hover-silent.py",
    "check-glass-mode.py",
    "check-doc-images.py",
]

# ★ 2026-10-09（第十四轮·README 收成产品页）加了第四道 `check-doc-images.py`。
#   README 这一轮第一次有配图（docs/img/），而**图片路径写错没有任何东西会响** ——
#   编译 / tsc / 出包 / 其它门槛全都不读 Markdown，本地也没人会一张张点开看。
#   它纯读文件、几毫秒，所以按"每次必跑"挂在这里。
#   ★ 已做变异测试：塞一条断链 → 报红 rc=1（且**只报那一条**）；
#     塞进围栏代码块 / **行内代码**里的示例引用 → 都不报
#     （`strip_fences()` + `INLINE_CODE` 负责）。
#     ★ 行内代码那一条是它上线后**第一次真跑就暴露**的漏洞：只剥围栏块时，
#       `判断基线.md` 里用单反引号举的"图片语法长什么样"的例子被当真引用，
#       门槛对着 `docs/path` 报了一条假断链。**假阳性比没有门槛更坏**。
#     修完还原复跑 ✅。

# ★ Windows 上必须写全名：subprocess 走 CreateProcess，会自动补 `.exe`，
#   但**不会**补 `.cmd` ⇒ 写裸 `npm` 会 FileNotFoundError。
NPM = "npm.cmd" if IS_WIN else "npm"

# 门槛期望值：改了代码就该改它，改不动就说明门槛没过
#
# ★ 2026-09-27（macOS 移植轮）103→107 / 87→91：两处都 +4，
#   来自新增的 `win::platform` 那四条测试（收进任务栏的跨平台落点）。
#   两个 feature 组合下都是 +4，说明这四条测试**不依赖 local-asr**，
#   与它们要守的东西（窗口层判定）一致。
#
# ★ 2026-10-04（第十二轮·磨砂玻璃）107→112 / 91→96：两处都 +5，
#   来自 `win::effects`（材质方案的平台映射 3 条）与 `state`（老配置 `glass` ⇒
#   `glass_mode` 迁移 2 条）。仍然不依赖 local-asr —— 与被守的东西一致。
#   ★ 真踩：先写成了 +3（只数了 effects 那三条），门槛直接报"数字变了"——
#     这道断言的价值就在这儿，它不许你凭印象改数字。
EXPECT = {
    "default": {"passed": 112, "ignored": 6},
    "no-default-features": {"passed": 96},
}


def clean_env():
    """去掉会干扰子进程的注入变量（本项目踩过多次）。"""
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    return env


def run(args, cwd, label):
    print(f"\n{'=' * 72}\n$ {' '.join(args)}   (cwd={os.path.relpath(cwd, ROOT)})\n{'=' * 72}", flush=True)
    p = subprocess.run(
        args, cwd=cwd, env=clean_env(),
        capture_output=True, encoding="utf-8", errors="replace",
    )
    out = (p.stdout or "") + (p.stderr or "")
    print(out, flush=True)
    return p.returncode, out


def parse_result(out):
    """从 `test result: ok. 103 passed; 0 failed; 6 ignored; ...` 取数。"""
    m = re.search(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", out)
    if not m:
        return None
    return {"status": m.group(1), "passed": int(m.group(2)),
            "failed": int(m.group(3)), "ignored": int(m.group(4))}


def warning_lines(out):
    """Rust 警告行（`warning: ...`），排除摘要行。"""
    return [l for l in out.splitlines()
            if l.strip().startswith("warning:") and "generated" not in l]


def main():
    do_build = "--build" in sys.argv
    problems = []

    # --- 门槛 1 / 2：两种特性的测试 ---
    for key, extra in (("default", []), ("no-default-features", ["--no-default-features"])):
        rc, out = run(["cargo", "test", "--lib"] + extra, TAURI_DIR, key)
        res = parse_result(out)
        warns = warning_lines(out)

        if rc != 0 or res is None or res["failed"] != 0 or res["status"] != "ok":
            problems.append(f"[{key}] 测试未通过（rc={rc}, result={res}）")
        else:
            exp = EXPECT[key]
            if res["passed"] != exp["passed"]:
                problems.append(
                    f"[{key}] passed={res['passed']} ≠ 期望 {exp['passed']} —— 数字变了要么是漏改期望、要么是漏测")
            if "ignored" in exp and res["ignored"] != exp["ignored"]:
                problems.append(
                    f"[{key}] ignored={res['ignored']} ≠ 期望 {exp['ignored']}")

        if warns:
            problems.append(f"[{key}] 有 {len(warns)} 条警告：{warns[:3]}")
        print(f"--> [{key}] {res}  警告 {len(warns)} 条", flush=True)

    # --- 门槛 3：前端（tsc --noEmit 已含在 npm run build 里） ---
    rc, out = run([NPM, "run", "build"], ROOT, "frontend")
    if rc != 0:
        problems.append("[frontend] tsc/vite 构建失败")
    if "error TS" in out:
        problems.append("[frontend] tsc 报错")

    # --- 门槛 3b：静态前端检查（每次必跑，几毫秒） ---
    # ★ 与平台专项不同：这些检查纯读源码，既不需要编译也不需要出包，
    #   而它们守的问题（如"原生 title 白框"）**截图看不见** ⇒ 不能等 --build。
    for name in STATIC_CHECKS:
        script = os.path.join(ROOT, "scripts", name)
        if not os.path.exists(script):
            problems.append(f"[static] 找不到 {name}")
            continue
        rc, out = run([sys.executable, script], ROOT, name)
        if rc != 0:
            problems.append(f"[static] {name} 未通过")

    # --- 门槛 4（可选）：出包 ---
    if do_build:
        # 平台专项检查放在出包之前 —— 它们只要几秒，却能在构建十几分钟之前
        # 就把"mac 后端编不过" / "顶栏版面跑了"这类问题拦住。
        for name in PLATFORM_CHECKS:
            script = os.path.join(ROOT, "scripts", name)
            if not os.path.exists(script):
                # 缺文件不该静默跳过（那就是"检查还在、其实没跑"）。
                problems.append(f"[platform] 找不到 {name}")
                continue
            # ★ 用同一个解释器跑：`sys.executable` 就是现在这个。
            #   写裸 `python` 会依赖 PATH，而 Git Bash 的 PATH 是被截断的。
            rc, out = run([sys.executable, script], ROOT, name)
            if rc != 0:
                problems.append(f"[platform] {name} 未通过")

        rc, out = run([NPM, "run", "tauri:build"], ROOT, "release")
        if rc != 0 or "Built application at:" not in out:
            problems.append("[release] 出包失败")
        else:
            exe = next((l for l in out.splitlines() if "Built application at:" in l), "")
            print(f"--> 产物：{exe.split('Built application at:')[-1].strip()}", flush=True)

    print("\n" + "=" * 72)
    if problems:
        print("门槛未全过：")
        for p in problems:
            print("  ✗", p)
        return 1
    print("门槛全过 ✅" + ("（含出包）" if do_build else ""))
    return 0


if __name__ == "__main__":
    if IS_WIN:
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    sys.exit(main())
