/// 「收起成一条栏 / 展开」—— 供顶栏与设置共用。
///
/// # 为什么要有这个文件
///
/// 这个动作**不是**一次 `setCollapsed()` 调用那么简单，它有两拍动画，顺序不能反：
///
///     收起：先让正文滑上去淡出（`--t-collapse`），**再**把窗口压成一条。
///           反过来的话窗口先变矮、正文被生硬裁掉，动画根本没机会播。
///     展开：先把窗口放开，正文自己淡入下滑（CSS transition 接管）。
///
/// ★★ 这个顺序的原因值得记住：它修的是一个真 bug ——
///   上一版 `.app.collapsed .controlbar { height: 0; opacity: 0; pointer-events: none }`，
///   而那颗「展开」按钮**长在被收起的那个东西里面**。点完收起，按钮跟着一起消失，
///   窗口上没有任何出路，唯一办法是记住 `Ctrl+Alt+M`。
///   这与「穿透必须配托盘」是同一条纪律：
///   **任何会关掉某个通道的能力，都要附一个不受该通道影响的退出口。**
///   现在的做法是栏**一直留着**（`.app.collapsed .topbar` 只改 `position`，高度不变），
///   所以收起态下顶栏那颗「收进任务栏」依然点得到。
///
/// ★ 动画时长从 CSS 变量 `--t-collapse` 现读，不写死 —— 见下面 `collapseMs`。

import { api, errText } from "./api";
import { useApp } from "./store";

/**
 * 收起/展开的动画时长。
 *
 * ★★ 它**必须**与 `styles.css` 里 `.viewport` 的 `transition` 是同一个数，
 *    所以这里不写死，而是去读那个 CSS 变量（`--t-collapse`）。
 *    两处各写一份的话，只要有人调了 CSS、忘了调这里，窗口就会在正文滑完之前
 *    先变矮 —— 用户看到的不是"滑走"而是"被裁掉"，而这**不会报任何错**。
 */
function collapseMs(): number {
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue("--t-collapse")
    .trim();
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n)) return 180;
  return raw.endsWith("ms") ? n : n * 1000;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function useToggleCollapsed(): () => Promise<void> {
  const { state, dispatch } = useApp();

  return async function toggleCollapsed() {
    const next = !state.collapsed;
    try {
      if (next) {
        // 收起前先把会占地方的面板关掉，否则它们会被 46px 的窗口裁掉。
        dispatch({ type: "SETTINGS", open: false });
        dispatch({ type: "HISTORY", open: false });
        dispatch({ type: "COLLAPSING", on: true });
        await sleep(collapseMs());
        await api.setCollapsed(true);
        dispatch({ type: "COLLAPSED", collapsed: true });
        dispatch({ type: "COLLAPSING", on: false });
      } else {
        await api.setCollapsed(false);
        dispatch({ type: "COLLAPSED", collapsed: false });
      }
    } catch (e) {
      // 收起失败要把动画状态收回来，否则正文会一直停在"滑走了"的样子。
      dispatch({ type: "COLLAPSING", on: false });
      dispatch({ type: "ERROR", message: errText(e) });
    }
  };
}
