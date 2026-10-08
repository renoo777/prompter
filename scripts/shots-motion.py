"""动效换血的交付图：把「关键交互态」各拍一张，拼成一张对照带。

★ 为什么需要它：动效单看一张静态图看不出东西 —— 那正是它难验的原因。
  能做的是把**受动效影响的那些静态端点**（悬停前/按压态/开关两态）
  并排放在一起，让"哪几处变了"这件事至少可见。
  真正的动效验证靠桩里那几条 `motion*=` 断言（见 tauri-stub.js）。
"""

import subprocess
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "shots" / "final"
PREVIEW = ROOT / "scripts" / "ui-preview.py"


def shoot(name: str, w: int, h: int, query: str, hold: int) -> None:
    cmd = [
        sys.executable,
        str(PREVIEW),
        "--out",
        str(OUT / name),
        "--w",
        str(w),
        "--h",
        str(h),
        "--query",
        query,
        "--hold",
        str(hold),
        "--snapshot",
    ]
    subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, encoding="utf-8")


def main() -> None:
    # ① 设置抽屉：开关 + 滑杆（Uiverse 里最核心的两类元素）都在这一屏
    shoot("09-motion-settings.png", 560, 1980, "open=settings", 3600)
    # ② 提词中：装饰性动效被停的那一态（用来和前一张对比"哪条策略在生效"）
    shoot("10-motion-prompting.png", 560, 274, "prompting=1", 5200)

    im = Image.open(OUT / "09-motion-settings.png")
    w, h = im.size
    crop = im.crop((300 - 65, 300, w, h - 260))
    crop = crop.resize((int(crop.width * 1.35), int(crop.height * 1.35)), Image.LANCZOS)
    crop.save(OUT / "09-motion-settings-zoom.png")
    print("saved", crop.size)

    im2 = Image.open(OUT / "10-motion-prompting.png")
    crop2 = im2.crop((0, 0, 560, 46))
    crop2 = crop2.resize((crop2.width * 2, crop2.height * 2), Image.LANCZOS)
    crop2.save(OUT / "10-motion-prompting-zoom.png")
    print("saved", crop2.size)


if __name__ == "__main__":
    main()
