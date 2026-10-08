/// 「收进任务栏」—— 供顶栏与设置两处共用。
///
/// # 为什么要有这个文件（而不是两边各写一遍）
///
/// 这颗按钮有两个入口：**顶栏**（`TopBar.tsx`，紧挨着关闭键）和**设置 › 窗口**
/// （`SettingsDrawer.tsx`）。它们必须表现完全一致：
///
///   * 首次点击要先给提示再收 —— 窗口"啪"地缩下去、用户不知道去哪找，
///     是这个功能最糟的失败方式（静默消失）；
///   * 提示只在**本次运行第一次**给 —— 知道之后每次都拦一下反而烦；
///   * 必须走后端命令，不能用窗口 API 的 `minimize()`。
///
/// ★★ 最后一条最容易做错：任务栏按钮的放出与收回是一个**事务**
///   （`set_skip_taskbar(false)` + 三态握手，见 Rust 侧 `win::taskbar`）。
///   只调 `getCurrentWindow().minimize()` 的话窗口确实缩下去了，
///   但任务栏上**不会**长出按钮 —— 而"缩下去之后从任务栏回来"正是这个功能的全部意义。
///   那样写等于把用户关在门外，且不报任何错。
///
/// ★ 提示里写的是"点任务栏上的提词器按钮"。**不要**改回"点托盘图标" ——
///   Windows 11 默认把托盘图标折进 `^` 溢出区（本机实测 `IsPromoted=0`），
///   且没有 API 能让程序自己提升出来，那句话等于指一个用户看不见的东西。

import { useRef } from "react";

import { api, errText } from "./api";
import { useApp } from "./store";
import { hotkeyLabel } from "../types";

/** 提示在收起前停留的时长 —— 太短了看不清，太长了用户以为卡住。 */
const HINT_MS = 1700;

export function useSendToTaskbar(): () => Promise<void> {
  const { state, dispatch } = useApp();
  // ★ `useRef` 而不是 `useState`：这个标记只影响"要不要给提示"，
  //   它一变就重渲染整条顶栏是白费；而它也不需要被渲染读到。
  const shown = useRef(false);

  // 热键以 `config.hotkeys` 为准（用户能改；两个平台的默认值也不同）。
  // 写死 `Ctrl+Alt+H` 会在 macOS 上说假话 —— 那边注册的是 `Command+Option+H`。
  const hk = hotkeyLabel(state.config?.hotkeys.toggle_visibility, "Ctrl+Alt+H");

  return async function sendToTaskbar() {
    try {
      if (!shown.current) {
        shown.current = true;
        dispatch({
          type: "NOTICE",
          message: `已收进任务栏 · 点任务栏上的提词器按钮回来（或按 ${hk}）`,
        });
        await new Promise((r) => setTimeout(r, HINT_MS));
      }
      await api.minimizeToTaskbar();
      // 窗口都缩下去了，面板不该还开着 —— 回来时应该是一个干净的界面。
      // ★ 顶栏那个入口本来就没有面板可关，这条是无害的空操作；
      //   但设置那个入口必须关，否则回来时抽屉还挡在正文前面。
      dispatch({ type: "SETTINGS", open: false });
      dispatch({ type: "HISTORY", open: false });
    } catch (e) {
      // 收不起来必须说出来。沉默地什么都不发生，用户只会一直点这颗按钮。
      dispatch({ type: "ERROR", message: errText(e) });
    }
  };
}
