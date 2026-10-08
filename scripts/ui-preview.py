"""用无头 Edge 渲染 dist/ 的前端产物，给 UI 改动做离线截图与量化验证。

# 为什么需要它（而不是直接跑应用 + scripts/shot.py）

  1. 应用的窗口默认开「投屏隐身」（WDA_EXCLUDEFROMCAPTURE），屏幕抓取拿到的是
     一片纯黑 —— 那是设计如此，不是 bug。
  2. release exe 经常被正在运行的实例锁住（LNK1104），改完界面想看效果
     不必等出包，也不打扰正在用的人。
  3. 它能在页面里自动点开设置抽屉、点开某个提示气泡，于是
     "只有交互时才出现"的东西也截得到、也量得到。

# 时序是怎么解决的（踩过坑，记在这）

无头截图默认在 `load` 之后发生。两个方案：

  * `--virtual-time-budget=N`：快，但**没用** —— 虚拟时间下定时器照常执行，
    可 React 的状态更新不 flush（点了按钮 DOM 里不出现新节点，渲染结果与
    未交互时逐字节相同）。合成器阶段的标志位全拧过一遍，无效。
  * ★ **推迟 load**（本脚本采用）：在 body 末尾挂一个 `<script async>`，
    让服务器故意晚几秒才回它。async 不阻塞解析，却会把 `load` 推后 ——
    这段时间正好让页面用**真实时间**跑完交互，React 一切正常，
    然后浏览器才截图。

`--hold` 就是那个"晚几秒"。它同时也是页面能用来做交互的时间窗口。

★★ **`--w` / `--h` 就是真实窗口尺寸**（2026-09-27 改）。
   在此之前这里有个"要传 W+32 / H+96"的口诀，因为它靠的是
   「dump 模式的视口比 `--window-size` 小一圈、截图模式相等」这个**不一致**。
   那套口诀的直接后果是：**截出来的图比真实窗口宽 32px** ——
   而 32px 刚好够标题把话说完，于是"截图里标题完好、真机上已被省略号截掉"。
   现在换算收敛在 `run_browser` 里，两种模式渲染出的视口都等于 `--w × --h`。
"""

from __future__ import annotations

import argparse
import http.server
import os
import re
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
STUB_SRC = ROOT / "scripts" / "ui-preview" / "tauri-stub.js"

EDGE_CANDIDATES = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
]


def find_browser() -> str:
    for p in EDGE_CANDIDATES:
        if os.path.exists(p):
            return p
    raise SystemExit("找不到 Edge/Chrome 可执行文件")


class Handler(http.server.SimpleHTTPRequestHandler):
    """静态服务 + 一个故意慢的 `/__hold`。

    `/__hold?ms=N` 睡 N 毫秒再回一个空脚本。它存在的唯一目的就是把 `load`
    事件推后，给页面留出用真实时间跑交互的窗口（见模块头部说明）。
    """

    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(DIST), **kw)

    def log_message(self, *a):  # noqa: D102
        pass

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):  # noqa: N802
        if self.path.startswith("/__hold"):
            ms = int(
                urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get(
                    "ms", ["2000"]
                )[0]
            )
            time.sleep(ms / 1000.0)
            body = b"// hold\n"
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()


def serve(port: int):
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def build_preview(port: int, hold_ms: int) -> str:
    """把 dist/index.html 变成 __preview.html 并注入桩脚本，返回访问 URL。"""
    index = DIST / "index.html"
    if not index.exists():
        raise SystemExit("dist/index.html 不存在 —— 先跑一次 vite build")

    html = index.read_text(encoding="utf-8")
    # 桩脚本必须是**普通 script**（不是 module）并排在 module 之前：
    # 打包产物是 <script type="module">，它天然 defer，普通脚本会先执行。
    if "</head>" not in html:
        raise SystemExit("index.html 结构变了，找不到 </head>")
    html = html.replace(
        "</head>", '<script src="/__tauri-stub.js"></script>\n  </head>', 1
    )
    if hold_ms > 0:
        if "</body>" not in html:
            raise SystemExit("index.html 结构变了，找不到 </body>")
        html = html.replace(
            "</body>",
            f'<script async src="/__hold?ms={hold_ms}"></script>\n  </body>',
            1,
        )

    (DIST / "__preview.html").write_text(html, encoding="utf-8")
    shutil.copyfile(STUB_SRC, DIST / "__tauri-stub.js")
    return f"http://127.0.0.1:{port}/__preview.html"


def run_browser(browser: str, url: str, w: int, h: int, out: Path,
                dump_dom: bool = False, profile: str = "", bg: str = ""):
    """跑一次无头浏览器。`w, h` 是**要渲染出来的视口尺寸**，即真实窗口大小。

    ★ 路径一律转成**绝对路径 + 正斜杠**再交给 Edge：
      相对路径会相对 Edge 自己的工作目录落盘（于是脚本这边找不到文件，
      报成"截图失败"）；反斜杠则可能被命令行解析吃掉。
      这两个坑叠在一起时的症状就是"命令跑完了但什么都没生成"。

    ★★ `--dump-dom` 与 `--screenshot` 对 `--window-size` 的解释**不一样**
       （2026-09-27 实测，这就是本轮差点据一张假图改版面的原因）：

         截图模式：视口 = W × H           （给多少就是多少）
         dump 模式：视口 = (W−32) × (H−96)（固定多扣一圈）

       于是同一对参数，**截出来的图和量出来的数出自两个不同大小的视口** ——
       实测 `--w 592 --h 370` 时，dump 报 `vw=560`，而 PNG 里那条栏横跨满 592。
       后果非常隐蔽：32px 的富余正好够标题把话说完，于是
       **截图上标题完好，真实窗口里它已经被省略号截掉了**。
       这种"图说没事、数说有事"的分歧比任何一方单独出错都难查。

       ⇒ 统一在这里换算：让两种模式都渲染出调用方要的 `(w, h)`。
         调用方从此只需给**真实窗口尺寸**，不用再记"要传 W+32 / H+96"那套口诀。
    """
    win_w, win_h = (w + 32, h + 96) if dump_dom else (w, h)
    args = [
        browser,
        "--headless=new",
        "--disable-gpu",
        "--hide-scrollbars",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-features=Translate",
        f"--window-size={win_w},{win_h}",
    ]
    if profile:
        args.append(f"--user-data-dir={Path(profile).as_posix()}")
    if bg:
        args.append(f"--default-background-color={bg}")
    if dump_dom:
        args.append("--dump-dom")
    else:
        args.append(f"--screenshot={Path(out).resolve().as_posix()}")
    args.append(url)

    proc = subprocess.run(
        args, capture_output=True, timeout=180,
        encoding="utf-8", errors="replace",
    )
    # ★★ 2026-09-27：Edge 的 stderr 以前是被直接丢掉的。它丢掉的恰恰是
    #    "页面为什么是红的/桩为什么没写 title"这类问题的**唯一线索** ——
    #    本轮就踩了：`--measure` 回的是光秃秃的 `Prompter`，工具提示
    #    "可能是 --hold 太小"，加大 hold 也没用；真因在页面抛了异常，
    #    而异常只在浏览器 stderr / console 里。⇒ 加开关把它露出来。
    if os.environ.get("UI_PREVIEW_DEBUG"):
        err = (proc.stderr or "").strip()
        if err:
            print("--- browser stderr ---")
            print(err[-2000:])
            print("--- /browser stderr ---")
    return proc.stdout or ""


def measure(dom: str) -> dict:
    """从 --dump-dom 的输出里取回桩脚本量好的数字。

    桩脚本把结果写进 <title>，格式是若干组 key=value（空格分隔）。
    这条路径比"看截图像不像"可靠 —— 越界这种问题必须靠数字判。
    """
    m = re.search(r"<title>([^<]*)</title>", dom, re.I)
    if not m:
        return {}
    raw = m.group(1).strip()
    if not raw:
        return {"raw": ""}
    out = {"raw": raw}
    for tok in raw.split():
        if "=" in tok:
            k, v = tok.split("=", 1)
            out[k] = v
    return out


def snapshot(dom: str, name: str = "__snap.html") -> str:
    """把「交互之后的 DOM」存成一个静态页，返回它的 URL。

    ★ 为什么需要这一步：`--screenshot` **不等 `load`**（用 /__hold 推迟 load
      也拦不住它，实测截图时机早于交互完成），所以凡是"点了之后才出现"的
      东西（气泡、抽屉、浮层）都截不到 —— 但 `--dump-dom` 能等到，并且拿到
      的是**渲染完成的完整 HTML**，连 React 写在元素上的内联定位都在。

      于是：先 dump 再静态重放。截到的是真实组件的渲染结果，
      不是手搓一个长得像的元素。

    同时把**所有** script 标签摘掉：
      * 预览桩本身（`/__tauri-stub.js`、`/__hold`）—— 留着会再执行一遍，
        把刚打开的气泡又点关上；
      * 打包产物 `<script type="module" src="/assets/index-*.js">` ——
        ★ 这条最隐蔽：留着它会重新挂载 React，而此刻桩已经没了，
        bootstrap 必然抛错，页面直接白屏（症状是截图只有 1.4KB 的空白）。
        快照要的是"已经渲染好的那份 DOM"，不需要也不应该再跑一遍脚本。

    ★ 快照会失真的地方（别照着截图判 bug）：
      * **`scrollTop`** —— 运行时状态，不写进 HTML。所以"滚动到某处"的
        截图会退回顶部，而气泡的坐标（内联 style）又是按滚动后的位置算的，
        两者对不上。要验证滚动后的样子，改看 `--measure` 的数字。
      * **`<select>` 的选中项** —— 同一个道理：HTML 里靠 `<option selected>`
        表达，运行时改的 `selectedIndex` 不落进源码。曾据此误判"模型没选中"，
        查了 `selectedIndex` 才确认运行时是对的（0,1 → 第二个下拉选中了第 1 项）。
    """
    html = re.sub(r"<script\b[^>]*>.*?</script>", "", dom, flags=re.S | re.I)

    # 冻住淡入动画。
    # ★ 为什么必须：快照页一 load 完就截图，而**任何在 load 时启动的动画都会
    #   被抓在起始帧**。带淡入的浮层（toast 的 `pop-in`）于是显示成半透明，
    #   与背后的稿件文字糊在一起 —— 看起来像"这个提示做得看不清"，
    #   其实它只是被定格在动画的第一帧。
    #   "动画在不在动"这件事故意**不靠截图**回答：桩会把 `::after` 的
    #   `animation-name` 写进 measure（见 `pillState`），那才是它的判据。
    #   ⇒ 以后每加一个"淡入才看得见"的浮层，都要把它的选择器加进这条规则。
    #
    # ★ 名单变化（第四轮）：`.tray-hint` 已经不存在了 —— 那条提示
    #   跟着按钮从顶栏搬进设置时，从"固定居中的浮层"改成了全局 toast。
    #   规则跟着改成 `.toast`，否则 `click=taskbar` 那条验收会截到半透明的提示。
    #   （键名 2026-09-27 从 `tray` 改成 `taskbar`：动作的落点变了，名字得跟着走。）
    freeze = "<style>.toast{animation:none !important}</style>"
    if "</head>" in html:
        html = html.replace("</head>", freeze + "</head>", 1)

    (DIST / name).write_text(html, encoding="utf-8")
    return name


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="输出 png 路径")
    ap.add_argument("--w", type=int, default=560,
                    help="真实窗口宽度（截图与测量**都是**这个视口，见 run_browser）")
    ap.add_argument("--h", type=int, default=360,
                    help="真实窗口高度（同上）。本机存的窗口是 560×274")
    ap.add_argument("--query", default="", help="传给 __preview.html 的查询串")
    ap.add_argument(
        "--hold", type=int, default=3200,
        help=(
            "把 load 推后多少毫秒（同时就是留给页面做交互 + 测量的时间窗口）。"
            "★ 必须大于 `900 + 步数*400 + settle`，否则 measure 会在桩写 title "
            "*之前* 就 dump，回一个光秃秃的 'Prompter'（看着像桩挂了，其实是 hold 太小）。"
            "默认 3200 已经覆盖「无交互、无 settle」；带 settle 的必须自己加够。"
        ),
    )
    ap.add_argument("--port", type=int, default=5199)
    ap.add_argument("--measure", action="store_true", help="额外跑一次 dump-dom 取量化结果")
    ap.add_argument(
        "--snapshot", action="store_true",
        help="先把交互后的 DOM 存成静态页再截图（交互态必需，见 snapshot()）",
    )
    ap.add_argument("--bg", default="FF2B3038", help="页面底色（模拟桌面背景）")
    args = ap.parse_args()

    browser = find_browser()
    url = build_preview(args.port, args.hold)
    if args.query:
        url += "?" + args.query

    httpd = serve(args.port)
    profile = tempfile.mkdtemp(prefix="edge-preview-")
    try:
        time.sleep(0.4)
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)

        dom = ""
        if args.snapshot or args.measure:
            dom = run_browser(
                browser, url, args.w, args.h, out,
                dump_dom=True, profile=profile, bg=args.bg,
            )

        if args.snapshot:
            name = snapshot(dom)
            run_browser(
                browser, f"http://127.0.0.1:{args.port}/{name}", args.w, args.h, out,
                profile=profile, bg=args.bg,
            )
        else:
            run_browser(browser, url, args.w, args.h, out, profile=profile, bg=args.bg)

        if not out.exists():
            print("SCREENSHOT_FAILED")
            return 1
        print(f"shot -> {out} ({out.stat().st_size} bytes)  {args.w}x{args.h}")

        if dom:
            info = measure(dom)
            print("measure:", info.get("raw") or "(空 —— 桩脚本没写 title)")
            # ★ 把最容易被误读的一种失败**说出来**：title 还是页面默认那个
            #   （一个 token、不含 '='），说明桩根本没来得及写 —— 99% 是
            #   `--hold` 小于 `900 + 步数*400 + settle`。不点破的话它会显示成
            #   "measure: Prompter"，和"桩抛异常"长得一模一样，而两者处置完全不同。
            raw = info.get("raw", "")
            if raw and "=" not in raw:
                print(
                    f"  ⚠️ 桩没来得及写 title（title 仍是页面默认值 {raw!r}）。"
                    f"最可能是 --hold={args.hold} 太小："
                    f"需要 > 900 + 步数*400 + settle。"
                    f"也可能是桩在写 title 之前抛了异常。"
                )
            for k, v in info.items():
                if k != "raw":
                    print(f"   {k} = {v}")
    finally:
        httpd.shutdown()
        shutil.rmtree(profile, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
