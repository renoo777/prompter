/**
 * 顶栏的图标 —— 全部是内联 SVG，**不用字体字形**。
 *
 * # 为什么把字形换掉（本轮真机截图量出来的）
 *
 * 之前用的是 `⌕ ✎ ☰ ▾ ⚙ ─ ✕` 这类 Unicode 字符。放大顶栏看：
 *   * `⌕`(U+2315) 根本不是放大镜 —— Windows 上渲染成一个带尾巴的小圈，
 *     没人看得出那是"查找"；
 *   * `✎`(U+270E) 渲染成一根斜杠，看不出铅笔；
 *   * `☰`(U+2630) 看着就是"菜单"，而它其实代表"最近稿件"；
 *   * 这几个字符来自不同 Unicode 区块，字重、视觉大小、基线全不一致，
 *     一排排过去像临时拼的，而不是一套图标。
 * 更麻烦的是它们是**字体渲染**的：字号一变图标跟着变形/变糊，
 * 而且哪台机器缺哪个字形就只能看到一个方框。
 *
 * SVG 这几个问题一次都没有：同一条 `viewBox`、同一个 `stroke-width`、
 * 颜色统一取 `currentColor`（所以 hover 变色天然生效）、
 * 尺寸由 width/height 精确控制，永远不受字体影响。
 */

import type { ReactNode } from "react";

type IconProps = { size?: number };

/** 统一的画布与线条参数 —— 图标之间的"一致性"就靠这个函数收口。 */
function Svg({ size = 16, children }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** 在稿件里查找 */
export function IconSearch(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="M15.4 15.4 20 20" />
    </Svg>
  );
}

/** 粘贴 / 编辑稿件 */
export function IconEdit(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M16.5 3.5a2.6 2.6 0 0 1 3.7 3.7L8 19.4 3 21l1.6-5Z" />
      <path d="M14.5 5.5 18 9" />
    </Svg>
  );
}

/** 最近稿件（表盘 + 回头箭头 = "最近"） */
export function IconHistory(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.3" />
      <path d="M3.5 4v4.3h4.3" />
      <path d="M12 7.8V12l3 1.8" />
    </Svg>
  );
}

/** 收起成一条控制栏 */
export function IconCollapse(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M6 14.5 12 8.5l6 6" />
    </Svg>
  );
}

/** 展开回完整窗口 */
export function IconExpand(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M6 9.5 12 15.5l6-6" />
    </Svg>
  );
}

/** 设置。用「滑杆」而不是齿轮：齿轮在小字号下会糊成一团，
 *  滑杆的横线和圆点在 16px 下依然分得清。 */
export function IconSettings(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M20 7.5h-8.5" />
      <path d="M7.5 7.5H4" />
      <path d="M20 16.5h-3.5" />
      <path d="M12 16.5H4" />
      <circle cx="9.8" cy="7.5" r="2.6" />
      <circle cx="14.2" cy="16.5" r="2.6" />
    </Svg>
  );
}

export function IconClose(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M6.5 6.5 17.5 17.5" />
      <path d="M17.5 6.5 6.5 17.5" />
    </Svg>
  );
}

/** 深色（月亮）。当前主题是深色时显示它。 */
export function IconMoon(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M20.5 13.6A8.5 8.5 0 1 1 10.4 3.5a6.6 6.6 0 0 0 10.1 10.1Z" />
    </Svg>
  );
}

/** 浅色（太阳）。 */
export function IconSun(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="12" cy="12" r="4.2" />
      <path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2" />
      <path d="M5.2 5.2 6.8 6.8M17.2 17.2l1.6 1.6M18.8 5.2 17.2 6.8M6.8 17.2 5.2 18.8" />
    </Svg>
  );
}

/** 四角 = 全屏。比 `⛶` 那种字形清楚得多。 */
export function IconFullscreen(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M9 3.5H5.5A2 2 0 0 0 3.5 5.5V9" />
      <path d="M15 3.5h3.5a2 2 0 0 1 2 2V9" />
      <path d="M15 20.5h3.5a2 2 0 0 0 2-2V15" />
      <path d="M9 20.5H5.5a2 2 0 0 1-2-2V15" />
    </Svg>
  );
}

/* --------------------------------------------------------------- 主角与状态 */

/**
 * 盾牌 = 投屏隐身。
 *
 * 用盾而不是"眼睛加斜杠"：图标要在 15px 下认得出，
 * 眼睛加斜杠在这个尺寸里会糊成一坨，而盾牌的轮廓一眼就分得清。
 * 语义也对得上 —— 它保的是"别把我投出去"，不是"我看不见"。
 */
export function IconShield(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M12 3.2 5.2 5.9v5.7c0 4 2.8 7.2 6.8 9.1 4-1.9 6.8-5.1 6.8-9.1V5.9Z" />
    </Svg>
  );
}

/**
 * 播放三角 —— **实心**。
 *
 * 全项目只有它是实心的，因为全项目只有它是「主角」：
 * 实心与描边的差别，在没有文字的按钮上就是"能按"与"只是画了个形状"的差别。
 * 其余图标一律保持描边，这样主角的实心才是**唯一**的那种视觉重量。
 */
export function IconPlay(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M7.6 5.3v13.4L19 12Z" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** 暂停 —— 同样是实心（它与播放是同一个按钮的两个面）。 */
export function IconPause(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M8.4 5.4h2.8v13.2H8.4z" fill="currentColor" stroke="none" />
      <path d="M12.8 5.4h2.8v13.2h-2.8z" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** 上一句 / 下一句。与收起展开用同一族折角，保持"整套图标"的观感。 */
export function IconChevronLeft(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M14.6 5.5 8.1 12l6.5 6.5" />
    </Svg>
  );
}

export function IconChevronRight(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M9.4 5.5 15.9 12l-6.5 6.5" />
    </Svg>
  );
}

/** 置顶（窗口永远在最前）。 */
export function IconPin(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M9.5 3.6h5l-.7 5.3 3 3v1.5H7.2v-1.5l3-3Z" />
      <path d="M12 13.4v7" />
    </Svg>
  );
}

/** 稿件：一张带折角的纸。折角是"文件"这件事在全世界的通用画法。 */
export function IconDoc(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M6.6 3.6h6.9L18 8.1v12.3H6.6Z" />
      <path d="M13.3 3.7V8.1H18" />
      <path d="M9.4 12.4h5.2M9.4 15.7h5.2" />
    </Svg>
  );
}

/** 结束提词（收摊）：一个方块 —— 与播放/暂停同族，读作"停"。 */
export function IconStop(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="7.4" y="7.4" width="9.2" height="9.2" rx="1.6" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/**
 * 收进任务栏。
 *
 * # 为什么是一横
 *
 * ★★ 画箭头（下箭头 / 托盘 / 方框）是错的：那一套图形在全世界都读成**「下载」**
 *   —— "保存到本地"。而这里发生的事跟下载毫无关系：窗口从屏幕上消失，
 *   任务栏上**留下一条**。所以就画那一条 —— `M3.5 12h17`。
 *
 * ★ 判据不是"像不像"，是**"它会不会让人读成另一个动作"**。箭头 → 下载，
 *   是系统级约定，用户改不了、我们也不该对抗。宁可画得朴素，不可画得会误读。
 *
 * ★ 这条线必须与相邻的「收起」区分开：收起画的是 `⌃`（一个折角 / V 形），
 *   两者在同一行里挨着，一个横、一个折，形状上一眼能分开。
 */
export function IconToTaskbar(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3.5 12h17" />
    </Svg>
  );
}
