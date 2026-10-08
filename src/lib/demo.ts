// 「一键演示模式」的编排逻辑。
//
// 抽出来是为了让**顶栏按钮和设置里的按钮走同一条路径** ——
// 两边各写一份的话，迟早会出现"顶栏那个开了隐身、设置里那个没开"这种
// 只在真会议上才发现的差异。
//
// 它刻意**不含「全屏」**：这个窗口是浮在 PPT 之上的提词层，
// 全屏会把 PPT 整个盖掉，与产品定位直接冲突。全屏是"外接显示器 / 平板"
// 那种单屏独占场景的另一件事，不该混进"一键就位"里。

import { api, errText } from "./api";
import type { AsrStatusPayload, ScriptPayload, StealthReport } from "../types";

export interface DemoOutcome {
  /** 一切就位。false 时 `problems` 一定非空。 */
  ok: boolean;
  /** 每一项都是**用户能看懂并据此处理**的一句话。 */
  problems: string[];
  stealth?: StealthReport;
  asr?: AsrStatusPayload;
}

/**
 * 把「隐身 + 置顶 + 开始跟读」一次做完。
 *
 * 为什么它值得存在：真实场景是「会议已经开始了，我得在两秒内让一切就位」。
 * 逐个去点、每次还要确认状态，那个间隙正好是别人等你的时候。
 *
 * 三条纪律：
 *   1. **没有稿件就拒绝**，并说明怎么办 —— 演示模式不替用户编稿子，
 *      静默地开一场没有稿子的会议比报错更糟。
 *   2. **隐身失败必须上报**。它是这个产品的立身之本，"以为开了其实没开"
 *      是这里唯一不可接受的失败。
 *   3. **置顶失败只算问题、不算致命** —— 它不影响隐身与跟读，只是窗口可能被压住。
 */
export async function runDemoMode(opts: {
  script: ScriptPayload | null;
  asrRunning: boolean;
}): Promise<DemoOutcome> {
  const problems: string[] = [];

  if (!opts.script || opts.script.total_chars === 0) {
    return {
      ok: false,
      problems: [
        "还没有稿件。演示模式不替你编稿子 —— 把稿件文件拖进窗口，点顶栏的 ✎ 粘贴一段，或按 Ctrl+K 从历史里选一篇。",
      ],
    };
  }

  // ① 隐身
  let stealth: StealthReport | undefined;
  try {
    stealth = await api.setStealth(true);
    if (!stealth.applied) problems.push(`投屏隐身没生效：${stealth.detail}`);
  } catch (e) {
    problems.push(`投屏隐身没设上：${errText(e)}`);
  }

  // ② 置顶
  // ★ 走 `api.setAlwaysOnTop` 而不是窗口 API：它顺手把"置顶"落进配置，
  //   下次启动才会自动置顶。只调窗口 API 的话，用户每次开机都得重点一次
  //   —— 那是被报过的问题（"设置不记住"），别再改回去。
  try {
    await api.setAlwaysOnTop(true);
  } catch {
    problems.push("置顶没设上，窗口可能被别的窗口压住。");
  }

  // ③ 按当前的推进方式往下走（语音跟随 ⇒ 起识别；匀速滚动 ⇒ 起那只时钟）
  let asr: AsrStatusPayload | undefined;
  if (!opts.asrRunning) {
    try {
      asr = await api.startAsr();
      // ★ 文案不说"语音识别"：推进方式有三种，匀速滚动压根不用麦克风。
      //   写死"语音识别没起来"会让那条路上的用户去查一个不存在的问题。
      if (!asr.running) problems.push(`没能开始：${asr.detail}`);
    } catch (e) {
      problems.push(`没能开始：${errText(e)}`);
    }
  }

  return { ok: problems.length === 0, problems, stealth, asr };
}

/**
 * 暂停演示：**只停跟读**，隐身与置顶原样保留。
 *
 * ★ 与「收摊」（`stopDemoMode`）的区别是"保护还在不在"：
 *   * 暂停 —— 窗口仍然隐身、仍然置顶，只是不再自动跟着语音走。
 *     讲者可以脱稿讲一段，回头按「继续」接上。
 *   * 收摊 —— 跟读停、置顶撤，演示结束。
 *
 *   把暂停做成"等于收摊"（上一版就是这样）会有一个很难受的后果：
 *   用户只是想临时脱稿，结果置顶和跟读一起没了，还得把整个演示重开一遍。
 */
export async function pauseDemoMode(asrRunning: boolean): Promise<string[]> {
  // 本来就没在跑 —— 没有可暂停的东西，也就不算失败。
  if (!asrRunning) return [];
  try {
    await api.stopAsr();
    return [];
  } catch (e) {
    return [`没能停下来：${errText(e)}`];
  }
}

/**
 * 继续演示：把跟读重新拉起来。
 *
 * 隐身与置顶在暂停期间**没被动过**，所以这里不必重设它们 ——
 * 重设只会多两次可能失败的调用，而失败面越大越容易在会议上翻车。
 */
export async function resumeDemoMode(): Promise<{
  problems: string[];
  asr?: AsrStatusPayload;
}> {
  try {
    const asr = await api.startAsr();
    return {
      problems: asr.running ? [] : [`没能续上：${asr.detail}`],
      asr,
    };
  } catch (e) {
    return { problems: [`没能续上：${errText(e)}`] };
  }
}

/**
 * 退出演示模式：停跟读 + 取消置顶。
 *
 * 与 `runDemoMode` 对称、也共用同一份编排 —— 顶栏按钮和设置里那个按钮
 * 都走这里，免得"一个收摊了、另一个以为还在演示"。
 *
 * ★★ **刻意不动隐身。** "退出演示"不等于"关掉保护"：隐身是用户自己的开关，
 *    顺手把它撤掉会造成一种很难察觉的后果 —— 以为讲完了就安全了，
 *    其实连保护一起撤了。要临时把屏幕给观众看，隐身 pill 上那一下「暂停」
 *    就在手边，那是可逆的、有记忆的。
 */
export async function stopDemoMode(asrRunning: boolean): Promise<string[]> {
  const problems: string[] = [];

  if (asrRunning) {
    try {
      await api.stopAsr();
    } catch (e) {
      problems.push(`跟读没能停下来：${errText(e)}`);
    }
  }

  try {
    await api.setAlwaysOnTop(false);
  } catch {
    /* 取消置顶失败没有实际后果，不值得打断收摊 */
  }

  return problems;
}
