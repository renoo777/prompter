#!/usr/bin/env python3
"""在 Windows（或任何非 mac）上，把 macOS 专属后端**编译一遍**。

    python scripts/check-macos-backend.py

# 为什么需要它

提词器在两个平台上各有一份"碰系统 API"的实现，它们被 `#[cfg(target_os = ...)]`
互相隔离：

| 文件 | 圈在哪个 cfg 里 | 在 Windows 上 `cargo test` 会看它吗 |
|---|---|---|
| `stealth/macos.rs` | `target_os = "macos"` | **不会** |
| `stealth/windows.rs` | `target_os = "windows"` | 会 |

而 `stealth/macos.rs` 恰恰是整个项目里最不可能靠肉眼看出错的一段 ——
全文是 `objc_msgSend` 的签名强转、`sel_registerName` 的选择子字符串、
`dlsym` 的私有符号查找、`sysctlbyname` 的缓冲区长度。任何一处写错
都不会在 Windows 上产生任何反馈，只有到 Mac 上第一次 `cargo build` 才炸 ——
而那一刻还同时有 SDK 版本、链接、Tauri 打包一堆报错挤在一起。

这个脚本用 `--cfg prompter_check_macos` 把它强制编译进来
（机制见 `src-tauri/src/stealth/mod.rs` 的 `macos_check` 模块），
于是"这段代码到底能不能编过"这件事，在开发机上就能被回答。

# 它**不能**回答什么

⚠️ 它**不产生任何可执行的 macOS 产物**，也**验证不了运行行为**：
`NSWindow.sharingType` 到底有没有让窗口从投屏里消失、`setIgnoresMouseEvents:`
的方向对不对，都只能在真 Mac 上试。这个脚本能给的只有"编得过来"这一条。

⚠️ 它也不检查 macOS 专属的**链接**：`#[link(name = "objc")]` 这种属性在
编译阶段不解析符号，真正的"符号存不存在"要等到链接，而链接只在 Mac 上发生。
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TAURI_DIR = os.path.join(ROOT, "src-tauri")

# 打开检查用的自定义 cfg（在 Cargo.toml 的 [lints.rust] 里已声明，不会触发 unexpected_cfgs）
CHECK_CFG = "prompter_check_macos"

# ★ 复现真实使用：本机的 `cargo test` 是带 default features 跑的，
#   所以这里也带。sherpa-onnx 已编译过，增量下这一遍很快。
CARGO_ARGS = ["cargo", "check", "--lib"]


def clean_env():
    """去掉会干扰子进程的注入变量（本项目踩过多次）。"""
    env = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        env.pop(k, None)
    prev = env.get("RUSTFLAGS", "")
    env["RUSTFLAGS"] = (prev + f" --cfg {CHECK_CFG}").strip()
    return env


def warning_lines(out):
    return [l for l in out.splitlines()
            if l.strip().startswith("warning:") and "generated" not in l]


def main():
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    print(f"$ RUSTFLAGS='--cfg {CHECK_CFG}' {' '.join(CARGO_ARGS)}   (cwd=src-tauri)")
    p = subprocess.run(
        CARGO_ARGS, cwd=TAURI_DIR, env=clean_env(),
        capture_output=True, encoding="utf-8", errors="replace",
    )
    out = (p.stdout or "") + (p.stderr or "")
    print(out)

    problems = []
    if p.returncode != 0:
        # 把 `error[E..]` / `error:` 那几行单独拎出来 —— 满屏里就这几行有用
        errs = [l for l in out.splitlines() if re.match(r"\s*error(\[|:)", l)]
        problems.append("macOS 后端编译失败：")
        problems.extend("    " + e.strip() for e in errs[:12])
    warns = warning_lines(out)
    if warns:
        problems.append(f"有 {len(warns)} 条警告（门槛要求零）：")
        problems.extend("    " + w.strip() for w in warns[:5])

    print("=" * 72)
    if problems:
        print("macOS 后端离线检查未过：")
        for x in problems:
            print("  ✗", x)
        return 1
    print("macOS 后端离线检查通过 ✅ —— 编译无误、零警告。")
    print("  ⚠️ 注意：这只说明「编得过来」。隐身/穿透的**实际行为**仍需真 Mac 验证。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
