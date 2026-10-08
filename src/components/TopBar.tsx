import { getCurrentWindow } from "@tauri-apps/api/window";

import { api, errText } from "../lib/api";
import { useSendToTaskbar } from "../lib/sendToTaskbar";
import { useApp, type StealthIntent } from "../lib/store";
import { Transport } from "./Transport";
import {
  IconClose,
  IconDoc,
  IconSearch,
  IconSettings,
  IconShield,
  IconToTaskbar,
} from "./Icons";

/** 「开启中」的保底显示时长：切换快到看不见时，动画一闪而过反而像没反应。 */
const MIN_BUSY_MS = 260;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * 顶栏 —— 现在它是**唯一**的一条栏。
 *
 * # 本轮改版：两条栏 → 一条
 *
 * 上一版把控件分成两处：顶栏（隐身 · 标题 · 查找 · 稿件 · 置顶 · 托盘 · 关闭）
 * 和底部控制台（读数 · 进度 · 状态 · 设置 · 收起 ＋ 上一句 · 主角 · 下一句）。
 * 问题出在控制台那一半：它是**两行 82px**，在 560×274 的窗口里吃掉 **30%** 的高度，
 * 而一行里只有三个元素 —— 左右两侧全是空的。
 *
 * 现在全部压进这**一行**（结构取自参考图：左状态 · 中推进 · 右工具）：
 *
 *   ⊘隐身中 │ 标题 │ ‹ ⏸ › N/M ▬▬▬▬ 状态 │ 🔍 📄 ⚙ ⌃ ✕
 *
 * 底栏整个取消，正文因此多出 82px。
 *
 * # 两处取舍（都有代价）
 *
 *   * **置顶 · 收托盘 移进设置**。它们是"一场会议最多用一次"的东西，
 *     却和"每一眼都要确认"的隐身灯抢同一行的宽度。★ 搬走的同时核过退出口：
 *     托盘**没有**因此少一条路（托盘图标本身 + `Ctrl+Alt+H`），
 *     置顶则仍在「开始提词/收摊」里被自动设置。
 *   * **那条 2px 的被动进度线删掉了**。它的职责是"我念到哪"的眼角余光，
 *     而这一行里已经有了一条可点的进度条 —— 同一件事画两遍是这一版要消掉的东西。
 */
export function TopBar() {
  const { state, dispatch } = useApp();
  const win = getCurrentWindow();
  // ★★ 「收进任务栏」在这个组件里也要有（顶栏那颗按钮），但它与设置里那颗
  //    必须是**同一份实现** —— 提示文案、首次提示只给一次、失败是否静默，
  //    这些判断分散成两份迟早走偏。所以抽进 `lib/sendToTaskbar`，两处共用。
  const sendToTaskbar = useSendToTaskbar();

  const stealthApplied = state.stealth?.mode === "hidden" && state.stealth.applied;
  const support = state.platform?.support ?? null;
  const unavailable = support?.kind === "unavailable";
  const fragile = support?.kind === "fragile";

  /**
   * 隐身状态灯的形态。
   *
   * 顺序有讲究：**正在切换**优先于一切（用户刚点完，先给反馈，否则会觉得没点上）；
   * 其次是「已暂停」——那是用户自己按住的状态，必须一眼能找回「继续」。
   *
   * 标签刻意压到 3 个字：它旁边就是稿件标题，而标题在 560px 的窗口里
   * 只剩百来像素。三字是"一眼认得出状态"与"给标题留够地方"的平衡点。
   */
  const stealth = ((): { cls: string; text: string } => {
    if (state.stealthBusy) {
      return { cls: "starting", text: "切换中" };
    }
    if (state.stealthIntent === "paused") {
      // 口径：隐身已暂停 —— 窗口现在会出现在投屏/录屏里；点一下继续。
      return { cls: "paused", text: "已暂停" };
    }
    if (stealthApplied) {
      // 口径：窗口不会出现在投屏/录屏里；点一下暂停（临时露出来）。
      return { cls: "on", text: "隐身中" };
    }
    if (unavailable) {
      return { cls: "warn", text: "不可用" };
    }
    if (state.stealthIntent === "on") {
      // 下了指令却没生效 —— 这是最需要被看见的一种状态（"以为开了其实没开"）。
      // ★ 顶栏那一行长文案删掉之后，这里是**唯一**还挂着这个状态的地方，
      //   所以它必须足够显眼（琥珀 + "未生效"三个字），不能只是褪色。
      return { cls: "warn", text: "未生效" };
    }
    if (fragile) {
      return { cls: "warn", text: "受限" };
    }
    return { cls: "off", text: "未隐身" };
  })();

  /**
   * 隐身状态灯的点击 —— **三态循环**：
   *
   *   未开启 ──开启──▶ 隐身中 ──暂停──▶ 已暂停 ──继续──▶ 隐身中 …
   *
   * 「彻底关掉隐身」刻意不放在这里：它是设置项（启动时自动隐身）。
   * 做进循环只会让人在演示中途手一抖把保护关掉，而且分不清
   * "我暂停了一下"和"我不用它了"。暂停之所以必须独立成第三态，
   * 就是因为它是**可逆、有记忆**的 —— 而"关掉"不是。
   */
  async function tapStealth() {
    const next: StealthIntent = stealthApplied ? "paused" : "on";
    dispatch({ type: "STEALTH_INTENT", intent: next, busy: true });
    const t0 = Date.now();
    try {
      const report = await api.setStealth(next === "on");
      // 保底停留：切换通常几十毫秒就完成，不兜一下的话"开启中"根本看不见。
      const spent = Date.now() - t0;
      if (spent < MIN_BUSY_MS) await sleep(MIN_BUSY_MS - spent);

      dispatch({ type: "STEALTH", report });

      if (next === "on" && !report.applied) {
        // 隐身失败是这个产品的立身之本，绝不能静默 —— 但也不必糊一段长句：
        // 灯会停在琥珀色的「未生效」，末尾那个状态槽再留一条 ——
        // 它是**常驻状态区**，不是一次性弹窗。
        //
        // ★★ `fallback_hint`（"退而求其次怎么办"）原先只出现在悬停气泡和这条
        //    warning 的 `detail` 里，而 `detail` 在第十一轮也随气泡一起没了。
        //    ⇒ **它现在被完全丢掉了，没有替代出口**。这是本轮唯一一处
        //    "删提示 = 删功能"的地方。为什么接受：这里的失败态本身还在
        //    （琥珀「未生效」+ 状态槽一条），而正文写着"投屏隐身没有生效："+ 后端
        //    的具体原因 —— 用户知道**出了什么事**；至于"换哪条路走"，它本来也
        //    只在悬停里能看到。要不要把这个回落路径搬进设置页的隐身自检行，
        //    留作未决（见 `docs/UI改版-PM拆解.md` 第二十节）。
        dispatch({
          type: "WARNING",
          warning: {
            label: "隐身未生效",
            detail:
              `投屏隐身没有生效：${report.detail}` +
              (report.fallback_hint ? `\n${report.fallback_hint}` : ""),
          },
        });
      }
    } catch (e) {
      dispatch({ type: "STEALTH_INTENT", intent: state.stealthIntent, busy: false });
      dispatch({ type: "ERROR", message: errText(e) });
    }
  }

  /** 打开「稿件」面板：最近稿件 + 打开文件 + 粘贴，三个入口合成一个。 */
  async function openScripts() {
    const next = !state.historyOpen;
    dispatch({ type: "HISTORY", open: next });
    if (!next) return;
    try {
      const [list, avail] = await Promise.all([
        api.listScripts(),
        api.scriptAvailability(),
      ]);
      dispatch({ type: "SCRIPTS", scripts: list });
      dispatch({ type: "SCRIPT_AVAIL", avail: Object.fromEntries(avail) });
    } catch {
      /* 读不出来就显示空列表，不给用户添堵 */
    }
  }

  /**
   * ★ 收起/展开的**按钮**已搬去设置（见下面那颗「调参」按钮的说明），
   *   所以这个组件不再调用 `useToggleCollapsed`。
   *
   *   但收起这个动作没消失，它的入口现在是三处，一条都不少：
   *     ① 设置 › 窗口 ›「收起」（走 `lib/collapse`，两拍动画）；
   *     ② 热键 `Ctrl+Alt+M`（走后端，不经过前端）；
   *     ③ 托盘菜单。
   *
   *   ★★ 为什么必须核过这三条：收起会把窗口压成一条 46px 的栏 ——
   *      这是"会关掉某个通道"的能力，按本项目纪律必须配**不受它影响的**退出口。
   */

  const locked = state.config?.window.locked ?? false;
  // 位置锁定之后顶栏不再是拖动区 —— 但**只有在没锁的时候**才允许拖。
  // （锁定本身已移进设置，这里只负责读它的结果。）
  const dragProps = locked ? {} : { "data-tauri-drag-region": true };

  const hasScript = !!state.script && state.script.total_chars > 0;

  return (
    <div className="topbar" {...dragProps} data-interactive>
      {/*
        隐身状态灯 —— 整条栏里唯一允许有颜色的常驻元素。
        它是这个产品最需要"一眼确认"的状态，开会前扫一眼就够了。
      */}
      <button className={`st-pill ${stealth.cls}`} onClick={() => void tapStealth()}>
        <IconShield size={14} />
        {stealth.text}
      </button>

      <span className="title" {...dragProps}>
        {hasScript ? state.script!.title : "未载入稿件"}
      </span>

      {/* 推进组：‹ ⏸ › · N/M · 进度条 · 状态槽（见 Transport.tsx） */}
      <Transport />

      {/*
        查找与稿件都是图标按钮而不是文字 —— 它们是"偶尔用一次"的动作，
        图标 + tooltip 足够；而左边那个隐身灯是"每场会都要确认一眼"的状态，
        必须占文字。两者的信息密度要求不同。

        ★ 「打开文件 / 粘贴 / 最近」三个入口合并进了「稿件」这一个按钮：
          它们服务的是同一件事（换稿），只是来源不同。

        ★★ `data-panel-toggle` 是给面板自己的**外点关闭**看的（见
          `ScriptHistory.tsx` 的 mousedown 处理）：不放过这三个按钮的话，
          "再点一下收起来"就永远做不到 —— mousedown 先把面板关了，
          紧接着 click 又把状态取反打开了。三颗都标上，将来给别的面板
          加外点关闭时不会重踩。
      */}
      <button
        className={`icon-btn${state.findOpen ? " active" : ""}`}
        data-panel-toggle
        onClick={() => dispatch({ type: "FIND", open: !state.findOpen })}
      >
        <IconSearch size={16} />
      </button>

      <button
        className={`icon-btn${state.historyOpen ? " active" : ""}`}
        data-panel-toggle
        onClick={() => void openScripts()}
      >
        <IconDoc size={16} />
      </button>

      {/*
        设置。
        ★★ 本轮它**没有**跟着收起键一起搬走，也没有被新增的按钮挤掉 ——
           这一行能分给「标题 + 进度条」的宽度是**定数**（实测 560px 下
           标题要 116、只给 93 ⇒ `titleShort=22.7`，标题已经被省略号截了）。
           所以顶栏的图标数**必须保持 5 个**：本轮是「收起」换成「收进任务栏」，
           一换一，不是加一颗。
        ★★ 一开始我在这里又加了一颗「调参」按钮（与这颗同样开设置抽屉），
           结果 `topIcons` 从 5 变 6、标题被压掉 23px —— 而且**两颗按钮干同一件事**。
           记住这条：这一行里加任何东西，代价都要从标题身上出。
        ★ 收起/展开搬走后的退出口核过（三处，一条不少）：
           设置 › 窗口 ›「收起」、热键 `Ctrl+Alt+M`、托盘菜单。

        ★★ 2026-09-27（macOS 移植轮）：「收进任务栏」那颗在非 Windows 平台上
           **整个消失**（`has_taskbar`，见下）。于是那些平台上这一行只有 4 颗图标，
           标题反而**多出约 28px**。这是**方向正确**的变化，不是回归：
           图标数是"最多 5"，不是"必须凑满 5"；而多出来的宽度归标题
           —— 本轮之前标题是被截断的（`titleShort` 为负）。
           ⇒ 所以**不要再补一颗图标把位置填满**。
           判据是"这一行有没有被撑爆"，不是"数量对不对得上 5"。
      */}
      <button
        className={`icon-btn${state.settingsOpen ? " active" : ""}`}
        data-panel-toggle
        onClick={() => dispatch({ type: "SETTINGS", open: !state.settingsOpen })}
      >
        <IconSettings size={16} />
      </button>

      {/*
        收进任务栏 —— 就在「关闭」左边。

        ★★ 这颗**只在真有任务栏按钮的平台上存在**（`has_taskbar`，由后端给）。
           理由不是"macOS 上做不到"，而是**做了会静默消失**：macOS 的 Dock
           图标只认冷启动时的 `activationPolicy`，应用启动后改它，Dock 不会重排
           ⇒ 窗口缩下去、没有任何入口回来。那边按下这颗键等于「收起」
           （见 Rust `commands::minimize_to_taskbar` 的平台分流），
           所以这里必须消失 —— 否则就是两颗按钮干同一件事。

        ★★ 落点是**任务栏**，不是托盘 —— 所以提示里写的是"点任务栏上的提词器按钮"，
          而不是"点托盘图标"（那个在 Windows 11 上默认被折进 `^` 溢出区，用户看不见）。

        ★★ 它与设置里那颗「收进任务栏」走**同一个后端命令**、同一份
           `lib/sendToTaskbar`，不是两份实现：`api.minimizeToTaskbar()` 里包含
           `set_skip_taskbar(false)` + 三态握手（`win::taskbar`），
           窗口 API 单独调 `minimize()` 只能做一半 —— 那样窗口缩下去了，
           任务栏上却不会长出按钮，用户就真的找不回来了。

        ★★ 为什么这颗能在**收起态**留着：窗口压成一条时顶栏整个还在
           （`.app.collapsed .topbar` 只是 `position: static`，高度不变），
           所以它在 46px 的窗口里依然点得到。
           这一条很重要 —— 收起态下它是**唯一不依赖热键**的出路。
      */}
      {state.platform?.has_taskbar && (
        <button className="icon-btn" onClick={() => void sendToTaskbar()}>
          <IconToTaskbar size={16} />
        </button>
      )}

      <button className="icon-btn danger" onClick={() => void win.close()}>
        <IconClose size={16} />
      </button>
    </div>
  );
}
