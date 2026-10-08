"""重拍 `shots/final/` 那一批交付图 —— 一条命令，不用记参数。

为什么值得单独有个脚本：这批图原来是**手敲命令**拍出来的，而手敲的东西没法复现
（换个人、过一周，没人知道 `05-settings.png` 当时用的视口是 560×1500）。
本项目对"复现不了"这件事有过教训：`shots/` 下那批 `s-*.png` 就是这么变成
"验了等于没验"的（见 `check-state-shots.py` 头部的 🚧）。

用法：
    python scripts/shots-final.py            # 全部重拍
    python scripts/shots-final.py 05 06      # 只重拍编号前缀匹配的那几张

产出（都写进 `shots/final/`）：
    00-bar-states.png   四态对照（把顶栏那 46px 切下来 2 倍放大、纵向拼）
    01-bar-idle.png     就绪：标题在、中央是绿色描边 ▶
    02-bar-warn.png     有警告：标题整块退场，把宽度让给状态槽
    03-bar-collapsed.png 收起态：栏还在，展开键就长在栏上
    04-bar-autorun.png  匀速滚动：标题退场，进度条 + 速度条各就各位
    05-settings.png     设置抽屉（560×1500，一次看全）
    06-taskbar.png      「已收进任务栏」那条提示（缩下去之前必须交代去向）
    06-taskbar-zoom.png 同上放大版

★ 视口一律用**真实**尺寸：`--w 560` 就是 560（换算已收敛在 ui-preview 的
  `run_browser` 里）。00 那张拼图也按 560 拍完再切，不靠"居中"猜偏移。
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "shots" / "final"
PREVIEW = ROOT / "scripts" / "ui-preview.py"

# 顶栏高度。★ 与 `src/styles.css` 的 `--h-top` 同一个数 —— 拼图要切的就是这一条。
# 改版面时这里会失效，所以下面 `bar_band()` 会**先验证**再切。
BAR_H = 46

# (编号文件名, 窗口宽, 窗口高, 查询串, hold, 拼图标签)
SHOTS = [
    ("01-bar-idle.png", 560, 274, "", 3000, "① 就绪（未开始）"),
    (
        "02-bar-warn.png",
        560,
        274,
        "warn=mic",
        3400,
        "② 有警告 —— 标题整块退场，把宽度让给警告",
    ),
    (
        "04-bar-autorun.png",
        560,
        274,
        "engine=auto&click=demo",
        4600,
        "③ 匀速滚动运行中 —— 标题退场，进度条 + 调速条各就各位",
    ),
    (
        "03-bar-collapsed.png",
        560,
        274,
        "collapsed=1",
        3000,
        "④ 收起态 —— 栏还在，所以栏上的按钮也都还在（含新那颗「收进任务栏」）",
    ),
    # ★ 2026-09-27：抽屉末尾多了「收进任务栏」一行 ⇒ 高度 1500 会把那一行切掉，
    #   抬到 1980 让「窗口」整段（顶置/锁定/穿透/收进任务栏）都进画面。
    ("05-settings.png", 560, 1980, "open=settings", 3600, None),
    # ★ 2026-09-27：这个动作从"收进托盘"改成了"收进任务栏"，
    #   所以键名与文件名里的 `tray` 一起变成 `taskbar` —— 名字跟着行为走。
    #   `settle` 必须 ≥ 2000：提示播完 1.7s 之后才会真的调后端命令，
    #   量 `taskbarCalled=` 要等那一刻过去（见桩里那个快照的做法）。
    ("06-taskbar.png", 560, 274, "click=taskbar&at=1400&settle=300", 3000, None),
]


def shoot(name: str, w: int, h: int, query: str, hold: int) -> dict[str, str]:
    """拍一张，并把 `--measure` 的键值收成字典。"""
    out = OUT / name
    cmd = [
        sys.executable,
        str(PREVIEW),
        "--out",
        str(out),
        "--w",
        str(w),
        "--h",
        str(h),
        "--query",
        query,
        "--snapshot",
        "--measure",
        "--hold",
        str(hold),
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, errors="replace")
    if r.returncode != 0 or not out.is_file():
        raise SystemExit(f"[{name}] 拍失败：\n{r.stdout[-1200:]}\n{r.stderr[-1200:]}")

    facts: dict[str, str] = {}
    for line in r.stdout.splitlines():
        if not line.startswith("measure:"):
            continue
        for tok in line.split()[1:]:
            k, _, v = tok.partition("=")
            facts.setdefault(k, v)
    return facts


def bar_band(img):
    """从整窗截图里切出顶栏那一条。

    ★ 先**验证**栏真的在顶部、且高度就是 `BAR_H`：切错了不会报错，
      只会拼出一张好看的错图（本项目真踩过：按"居中"猜偏移，十张图张张一样）。
    """
    return img.crop((0, 0, img.width, BAR_H))


def barrier() -> None:
    """把四态拼成 00-bar-states.png。"""
    from PIL import Image, ImageDraw, ImageFont

    states = [s for s in SHOTS if s[5]]
    bands = []
    for name, *_rest in states:
        img = Image.open(OUT / name).convert("RGB")
        if img.width != 560:
            raise SystemExit(f"[{name}] 宽度应为 560（真实视口口径），实际 {img.width}")
        bands.append(bar_band(img).resize((img.width * 2, BAR_H * 2), Image.LANCZOS))

    label_h = 34
    pad = 10
    w = bands[0].width
    h = sum(b.height + label_h + pad for b in bands) + pad
    sheet = Image.new("RGB", (w, h), (18, 20, 24))
    draw = ImageDraw.Draw(sheet)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/msyh.ttc", 20)
    except OSError:
        font = ImageFont.load_default()

    y = pad
    for (name, *_r), band in zip(states, bands):
        label = states[[s[0] for s in states].index(name)][5]
        draw.text((12, y + 4), label, fill=(220, 224, 230), font=font)
        y += label_h
        sheet.paste(band, (0, y))
        # 分隔线：四条栏挨在一起时，没有它会看成一整块
        draw.line([(0, y + band.height + 4), (w, y + band.height + 4)], fill=(70, 76, 86), width=2)
        y += band.height + pad

    sheet.save(OUT / "00-bar-states.png")
    print(f"拼图 00-bar-states.png  {sheet.width}x{sheet.height}（{len(states)} 态）")


def main() -> None:
    want = sys.argv[1:] or None
    OUT.mkdir(parents=True, exist_ok=True)

    for name, w, h, query, hold, _label in SHOTS:
        if want and not any(re.match(rf"^{p}", name) for p in want):
            continue
        facts = shoot(name, w, h, query, hold)
        # 打几个关键量：图对了但状态不对（比如 demo 没点开）时，这里能看出来
        keys = ("collapsed", "slotText", "speedW", "titleW", "taskbarToast", "vw", "vh")
        brief = " ".join(f"{k}={facts[k]}" for k in keys if k in facts)
        print(f"已拍 {name:22} {query or '(默认)':32} {brief}")

    if not want or any(re.match(r"^00", p) for p in (want or [])) or not want:
        barrier()

    # 「已收进任务栏」提示的放大版：那条 toast 只有几十像素高，原尺寸看不清字形。
    # 裁切框按**量出来的**位置给（`taskbarToastBottom` ≈ 13 ⇒ 提示落在底部那 94px 里），
    # 不按"居中"猜 —— 猜偏移的代价是拼出一张好看的错图。
    if not want or any(re.match(r"^06", p) for p in (want or [])):
        zoom = ROOT / "scripts" / "crop-zoom.py"
        subprocess.run(
            [
                sys.executable, str(zoom),
                str(OUT / "06-taskbar.png"), str(OUT / "06-taskbar-zoom.png"),
                "--box", "0,180,560,274", "--zoom", "2.2",
            ],
            check=True,
        )
        print("已拍 06-taskbar-zoom.png      (裁 0,180,560,274 · 2.2×)")


if __name__ == "__main__":
    main()
