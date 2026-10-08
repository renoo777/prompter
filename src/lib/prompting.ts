// 「开始提词 / 暂停 / 继续 / 收摊」的编排 —— 全应用**唯一**的一份。
//
// 抽出来的理由（本轮改版的核心动作就在这里）：
//
//   改版前「开始」有两个入口 —— 顶栏那个「▶ 演示」胶囊和设置里的「开始识别」按钮，
//   用户分不清差别，两边状态还会不同步。改版后它们合并成底部**一个大圆按钮**，
//   于是这套编排必须只有一个实现：按钮、设置页、热键都调这里。
//
// ★ 这一层只做「编排 + 落状态」，不碰任何界面。所以它能被独立读、独立测，
//   也不需要认识 TopBar / Transport（这正是把 `demo.ts` 再往上抽一层的理由）。

import { api, errText } from "./api";
import { pauseDemoMode, resumeDemoMode, runDemoMode, stopDemoMode } from "./demo";
import type { Action, State } from "./store";

type Dispatch = (a: Action) => void;
type SetBusy = (busy: boolean) => void;

/**
 * 大圆按钮的四态。
 *
 * 四态而不是三态：`starting` 是"已下指令、等后端确认"那段路。
 * 少了它，用户点下去之后到跟读真正起来之间会有几百毫秒**没有任何反馈** ——
 * 而这段时间恰好是最长的（要启动识别引擎）。
 */
export type PromptState = "idle" | "starting" | "running" | "paused";

export function promptState(state: State, busy: boolean): PromptState {
  if (busy) return "starting";
  if (!state.demoActive) return "idle";
  return state.demoPaused ? "paused" : "running";
}

/**
 * 大圆按钮：点一下，按当前态走到下一个态。
 *
 * ★★ 三条纪律（沿用已有论证，一条都没放宽）：
 *
 *   1. **暂停 ≠ 收摊。** 暂停只停跟读，隐身与置顶原样保留 ——
 *      "脱稿讲一段、回头接上"要的就是这个。上一版把"点一下"做成了收摊，
 *      用户只是想脱稿，结果整个演示被关掉、按钮回到最初，那不是他按下去的意思。
 *      「收摊」搬去了设置 › 提词，与隐身「彻底关掉放设置」同一条约定。
 *
 *   2. **暂停与收摊都不动隐身。** 隐身是用户自己的开关，不该被顺手撤掉。
 *
 *   3. **只有真的停住了才翻牌。** 停失败就停在原态 + 报警告 ——
 *      翻成一个写着「已暂停」但跟读还在推光标的牌子，比不翻更糟。
 */
export async function togglePrompting(
  state: State,
  dispatch: Dispatch,
  setBusy: SetBusy,
): Promise<void> {
  setBusy(true);
  try {
    const asrRunning = !!state.asrStatus?.running;

    // —— 进行中 ⇒ 暂停（只停跟读，保护原样留着）——
    if (state.demoActive && !state.demoPaused) {
      const problems = await pauseDemoMode(asrRunning);
      if (problems.length === 0) {
        dispatch({ type: "DEMO", active: true, paused: true });
      } else {
        dispatch({
          type: "WARNING",
          // 标签用「提词」而不是「跟读」：推进方式现在有三种（语音跟随 /
          // 匀速滚动 / 纯手动），这里说的是"整个提词没停下来"这件事本身。
          // 写死"跟读"会让匀速滚动的用户去查麦克风 —— 而那条路根本不碰麦克风。
          warning: { label: "提词没停下来", detail: problems.join("\n") },
        });
      }
      return;
    }

    // —— 已暂停 ⇒ 继续（把跟读拉回来）——
    if (state.demoActive && state.demoPaused) {
      const res = await resumeDemoMode();
      if (res.asr) dispatch({ type: "ASR_STATUS", status: res.asr });
      if (res.problems.length === 0) {
        dispatch({ type: "DEMO", active: true, paused: false });
      } else {
        dispatch({
          type: "WARNING",
          warning: { label: "提词没续上", detail: res.problems.join("\n") },
        });
      }
      return;
    }

    // —— 未开始 ⇒ 开始提词（隐身 + 置顶 + 跟读，一次做完）——
    const r = await runDemoMode({ script: state.script, asrRunning });
    if (r.stealth) dispatch({ type: "STEALTH", report: r.stealth });
    if (r.asr) dispatch({ type: "ASR_STATUS", status: r.asr });

    if (r.ok) {
      dispatch({ type: "DEMO", active: true });
      return;
    }
    dispatch({ type: "ERROR", message: r.problems.join(" ") });
    // 隐身在「开始提词」里失败是唯一必须当场处理的事 —— 直接开设置让人能看见原因
    if (r.problems.some((p) => p.includes("隐身"))) {
      dispatch({ type: "SETTINGS", open: true });
    }
  } catch (e) {
    dispatch({ type: "ERROR", message: errText(e) });
  } finally {
    setBusy(false);
  }
}

/**
 * 收摊 —— 演示的**终点**：停跟读 + 撤置顶。
 *
 * 只有设置页调它（大圆按钮在循环里，永远不会走到这一步）。
 * 与隐身「彻底关掉放在设置里」是同一条约定：
 * **可逆的、高频的留在按钮上；不可逆的、低频的收进设置。**
 */
export async function stopPrompting(
  state: State,
  dispatch: Dispatch,
  setBusy: SetBusy,
): Promise<void> {
  setBusy(true);
  try {
    const problems = await stopDemoMode(!!state.asrStatus?.running);
    dispatch({ type: "DEMO", active: false });
    if (problems.length > 0) {
      dispatch({
        type: "WARNING",
        warning: { label: "收摊没收干净", detail: problems.join("\n") },
      });
    }
  } catch (e) {
    dispatch({ type: "ERROR", message: errText(e) });
  } finally {
    setBusy(false);
  }
}

/**
 * 上/下一句。
 *
 * 与「开始提词」分开成两个动作是刻意的：**手动翻页不是自动跟读的降级**。
 * 语音识别跟不准、或者用户就是不想开麦时，手动翻页是**完整可用**的一条路 ——
 * 它不是"高级功能"，它是兜底通道，所以必须留在主界面上（主角两侧），
 * 而不是收进设置。
 */
export async function stepSentence(
  forward: boolean,
  dispatch: Dispatch,
): Promise<void> {
  try {
    const c = await api.stepSentence(forward);
    dispatch({ type: "CURSOR", cursor: c });
  } catch (e) {
    dispatch({ type: "ERROR", message: errText(e) });
  }
}
