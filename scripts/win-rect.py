"""打印 Prompter 主窗口的物理矩形、DPI 缩放与换算后的 CSS 尺寸。

# 为什么需要它

  * `ui-preview.py` 渲染的是 dist 前端产物，**它的视口是我们指定的**，
    所以它只能回答"在这个宽度下会不会溢出"，回答不了"真实窗口有多宽"。
  * 而无头 Edge 的 `--window-size=W,H` 并不等于页面视口，**而且两种模式还不一样**：
    截图模式视口 = `W × H`，`--dump-dom`（测量）模式视口 = `W-32 × H-96`。
    ⇒ 想渲染真实的 560×360：截图直接传 `--w 560 --h 360`，测量传 `--w 592 --h 456`。
    ★ 2026-09-27 起 `scripts/ui-preview.py` 的 `run_browser` 已经把这层换算吃掉了，
      调用方**只给真实尺寸**即可（传 592 反而会渲染出 592×456 的假窗口）。
      这里保留这条记录，是因为"两种模式口径不同"这件事值得知道 ——
      它曾经让截图比真实窗口宽 32px，从而**藏住了标题被省略号截断**这回事。
  * 真实窗口尺寸会被用户拖动改变并写回 config.json，所以不能只看配置文件，
    要直接问操作系统。

@return 打印一行 JSON：窗口矩形、物理尺寸、DPI 缩放、CSS 尺寸。
"""

from __future__ import annotations

import ctypes
import ctypes.wintypes as w
import json
import os

user32 = ctypes.windll.user32

# 让进程 DPI 感知，否则 GetWindowRect 返回的是被系统缩放过的假值。
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)  # PER_MONITOR_AWARE_V2
except Exception:  # noqa: BLE001 - 老系统没有 shcore，退回系统级 DPI 感知
    try:
        user32.SetProcessDPIAware()
    except Exception:  # noqa: BLE001
        pass


def dpi_for(hwnd: int) -> int:
    try:
        return int(ctypes.windll.user32.GetDpiForWindow(hwnd)) or 96
    except Exception:  # noqa: BLE001
        return 96


def main() -> None:
    hits: list[dict] = []
    enum_proc = ctypes.WINFUNCTYPE(ctypes.c_bool, w.HWND, w.LPARAM)

    def _cb(hwnd, _lparam):
        n = user32.GetWindowTextLengthW(hwnd)
        if not n:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        title = buf.value
        if "Prompter" not in title:
            return True
        rect = w.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        pw, ph = rect.right - rect.left, rect.bottom - rect.top
        dpi = dpi_for(hwnd)
        scale = dpi / 96.0
        hits.append(
            {
                "title": title,
                "hwnd": int(hwnd),
                "rect": [rect.left, rect.top, rect.right, rect.bottom],
                "physical": [pw, ph],
                "dpi": dpi,
                "scale": round(scale, 3),
                "css": [round(pw / scale, 1), round(ph / scale, 1)],
                "visible": bool(user32.IsWindowVisible(hwnd)),
            }
        )
        return True

    user32.EnumWindows(enum_proc(_cb), 0)

    cfg = os.path.join(
        os.environ.get("APPDATA", ""), "cn.bobby.prompter", "config.json"
    )
    saved = None
    if os.path.exists(cfg):
        try:
            with open(cfg, encoding="utf-8") as fh:
                saved = json.load(fh).get("window")
        except Exception as e:  # noqa: BLE001
            saved = f"读取失败：{e}"

    print(json.dumps({"windows": hits, "config_window": saved}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
