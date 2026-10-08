"""把各状态的截图拼成一张对照表，供人工一眼看全。

🚧 **本脚本已过期，先别拿它的产出下结论**（2026-09-27 记）
    它切的是**两条栏**（`BAR_H = 96`，因为旧版上面一条 + 下面一条），
    而 2026-09-26 起只有**一条**栏（46px）；`shots/` 下那批 `s-*.png`
    也还是改版前拍的。要重新用它：先把 `BAR_H` 改成一条栏的高度、
    按新布局重拍那批图，再把 `check-state-shots.py` 的期望值对齐。
    ★ 在此之前用 `shots/final/00-bar-states.png`（本轮按真实尺寸重拼的那张）。

为什么需要它：这一轮改的全是**状态语义**（隐身四态、演示三态、穿透四态），
而单张窗口图每次只能看到一个态。要把"十种状态放在一起看"
这件事做出来，才有机会发现"已暂停和未生效用了同一个颜色"这类问题 ——
分开看每一张都是对的。

用法：
    python scripts/make-state-sheet.py
产出：
    shots/states-sheet.png

★ **视口口径**：`ui-preview.py` 的 `--w/--h` 现在**就是**真实视口
  （2026-09-27 起换算收敛在 `run_browser` 里；此前截图比真实窗口宽 32px）。
  所以"要裁哪里"仍然必须**量出来**，不能按 `(W-560)/2` 之类猜。

  ★★ 这一条本轮真踩过：我按"居中"猜了 `(16, 48)`，裁出来十张图**张张一样**
    （都是正文区），而单看每一张都像"界面正常"。是拿调色板去数强调色像素
    才发现"绿/琥珀一个都没有"。
    ⇒ 判据：裁完之后必须**按颜色验证**，否则拼出来的是十张好看的错图。

  实测：栏在 **左上角，偏移 (0, 0)**，栏下边框落在 y=46
  —— 与 CSS 的 `--h-top: 46px` 一致，说明缩放是 1:1。
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
SHOTS = ROOT / "shots"
OUT = SHOTS / "states-sheet.png"

# 两条栏在截图里的位置（实测，见文件头说明）
PAD_L, PAD_T = 0, 0
WIN_W, WIN_H = 592, 360
# 只取两条栏那一段：状态全在栏上，正文区每张都一样，截进来只是浪费高度
BAR_H = 96

ROW_LABEL_W = 220
PAD = 14
TITLE_H = 40

# (文件名, 分组, 说明)
ROWS: list[tuple[str, str, str]] = [
    ("s-off.png", "投屏隐身", "未隐身"),
    ("s-sp-start.png", "", "开启中（扫光）"),
    ("s-sp-on.png", "", "投屏隐身"),
    ("s-sp-paused.png", "", "已暂停（点一下继续）"),
    ("s-sp-warn.png", "", "未生效（下了指令没生效）"),
    ("s-demo-start.png", "演示模式", "就位中（扫光）"),
    ("s-demo-on.png", "", "演示中"),
    ("s-ct-start.png", "鼠标穿透", "开启中（扫光）"),
    ("s-ct-on.png", "", "穿透中"),
    ("s-ct-warn.png", "", "未生效（等不到可交互区）"),
]


def load_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for name in ("msyh.ttc", "msyhbd.ttc", "simhei.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def main() -> int:
    missing = [r[0] for r in ROWS if not (SHOTS / r[0]).exists()]
    if missing:
        print("缺这些截图，先跑 ui-preview.py 生成：", ", ".join(missing))
        return 1

    font = load_font(17)
    font_bold = load_font(19)
    font_title = load_font(22)

    cell_h = BAR_H + PAD
    width = ROW_LABEL_W + WIN_W + PAD * 2
    height = TITLE_H + cell_h * len(ROWS) + PAD

    sheet = Image.new("RGB", (width, height), (24, 27, 33))
    d = ImageDraw.Draw(sheet)

    d.text((PAD, 12), "提词器 · 状态控件对照（离线渲染，真实 560px 宽窗口）",
           font=font_title, fill=(232, 236, 242))

    last_group = None
    y = TITLE_H
    for name, group, caption in ROWS:
        img = Image.open(SHOTS / name).convert("RGB")
        bar = img.crop((PAD_L, PAD_T, PAD_L + WIN_W, PAD_T + BAR_H))

        # 分组标签只在每组第一行画，并加一条分隔线
        if group and group != last_group:
            if last_group is not None:
                d.line([(PAD, y - PAD // 2), (width - PAD, y - PAD // 2)],
                       fill=(58, 63, 74), width=1)
            d.text((PAD, y + 2), group, font=font_bold, fill=(120, 190, 255))
            last_group = group

        d.text((PAD + 8, y + 30), caption, font=font, fill=(226, 232, 240))
        sheet.paste(bar, (ROW_LABEL_W, y))
        d.rectangle([ROW_LABEL_W - 1, y - 1, ROW_LABEL_W + WIN_W, y + BAR_H],
                    outline=(58, 63, 74), width=1)
        y += cell_h

    sheet.save(OUT)
    print(f"已生成 {OUT}  ({sheet.width}x{sheet.height})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
