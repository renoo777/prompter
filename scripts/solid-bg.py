"""铺一块全屏纯色窗口，用来当提词器的受控背景。

被 `alpha-probe.py` 拉起，也可以单独跑：

    python scripts/solid-bg.py 255 0 255

不设 topmost —— 提词器窗口声明了 alwaysOnTop，会自然地压在它上面。
"""

from __future__ import annotations

import sys
import tkinter as tk


def main() -> int:
    r, g, b = (int(x) for x in sys.argv[1:4])
    color = f"#{r:02x}{g:02x}{b:02x}"

    root = tk.Tk()
    root.overrideredirect(True)
    w, h = root.winfo_screenwidth(), root.winfo_screenheight()
    root.geometry(f"{w}x{h}+0+0")
    root.configure(bg=color)
    root.attributes("-topmost", False)
    print(f"solid-bg: {color} {w}x{h}，Ctrl+C 结束", flush=True)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
