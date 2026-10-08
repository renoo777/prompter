import { useEffect, useState } from "react";

import { api } from "../lib/api";
import { promptState, stepSentence, togglePrompting } from "../lib/prompting";
import { useApp } from "../lib/store";
import { IconChevronLeft, IconChevronRight, IconPause, IconPlay } from "./Icons";

/**
 * 推进组 —— 那条**唯一的栏**里属于「播放」的那一段。
 *
 * # 它为什么是一段而不是一条栏
 *
 * 上一版把播放控制单独放在窗口底部的一条（82px、两行）里：第一行读数与工具、
 * 第二行主角与它的两侧。问题是 560×274 的窗口里这条栏吃掉 **30% 的高度**，
 * 而它的左右两侧**全是空的** —— 一行只有三个元素，却要占满整窗宽度。
 *
 * 现在它并进顶栏那**一行**里（参考图的结构：左工具 · 中播放 · 右状态），
 * 底栏整个取消，正文因此多出 82px。这个组件只负责"中间那一段"：
 *
 *   [‹] [⏸] [›]   N/M   ▬▬▬▬▬   状态 / 速度条
 *
 * 它**不持有**栏本身的位置与玻璃 —— 那是 TopBar 的事。这样拆开之后，
 * "栏里有什么"和"这一段怎么动"可以分别改，不会互相牵扯。
 *
 * # 从底栏搬过来时改掉的两处口径
 *
 *   * **剩余时间不再常显**。它原来和"位置"并排占着底栏一行的地方，而现在
 *     这一行只有 560px 宽、要塞下十四件事。它没有被删掉，只是挪进了进度条的
 *     悬停说明 —— 常显的读数让位给"要注意的事"（错误 / 警告 / 已讲完）。
 *   * **状态槽从"常占一格"变成"有事才占"**。它现在 `flex: 0 1 auto`，
 *     没话可说时宽度为 0，把地方还给进度条。这是这一版能塞进一行的主要原因。
 */
export function Transport() {
  const { state, dispatch } = useApp();
  const { cursor, asrStatus, asrLevel, error, warning, config, ctActive, ctPending } = state;

  const [busy, setBusy] = useState(false);

  /**
   * 鼠标穿透「说了要开、后端一直没确认生效」。
   *
   * ★★ 为什么必须有这一条（这是**回归点**，不是新增功能）：
   *   穿透那个文字胶囊收进设置之后，`ctActive` / `ctPending` 一度变成没人消费的
   *   死状态 —— 于是「开了但没生效」在界面上完全静默：用户按下穿透（尤其走热键
   *   `Ctrl+Alt+P` 时人不在设置页），开关没生效，什么都不会发生 ——
   *   正好退回"点了没反应"。
   *
   *   处置沿用同一条纪律：**信号不能消失，只能换个位置**。
   *   位置就是这一行里那个**唯一的状态槽**。
   *
   * ★ 判据用 `ctPending && !ctActive`（意图已下、后端没确认在跑），
   *   **不是** `!ctActive` —— 后者在"用户压根没开穿透"时为真，会常年挂着一条假警告。
   *
   * ★ 2.2 秒宽限期沿用旧值：正常切换要经过"前端上报可交互区 → 后端确认"这一圈，
   *   立刻亮警示色会让**每一次正常开启**都闪一下琥珀。
   */
  const ctWanted = ctPending && !ctActive;
  const [ctOverdue, setCtOverdue] = useState(false);
  useEffect(() => {
    if (!ctWanted) {
      setCtOverdue(false);
      return;
    }
    const t = window.setTimeout(() => setCtOverdue(true), 2200);
    return () => window.clearTimeout(t);
  }, [ctWanted]);

  const pos = cursor?.cursor ?? 0;
  const total = cursor?.total ?? 0;
  const pct = total > 0 ? (pos / total) * 100 : 0;

  const running = asrStatus?.running ?? false;

  /**
   * 匀速滚动的三个派生量。
   *
   * ★ 剩余时间的**口径要跟着推进方式换**：稿子由那只时钟推着走的时候，
   *   "还剩几分钟"就必须按那只时钟的速度算。继续拿「参考语速」去算，
   *   会得出一个跟屏幕上实际推进速度无关的数字 —— 屏幕上明明每 2 秒走一行，
   *   它却按另一个速度告诉你还剩多久，而两个数都不报错，只有感觉不对。
   */
  const autoMode = config?.asr.engine === "auto";
  const autoWpm = config?.auto_wpm ?? 300;
  const autoRunning = autoMode && running;

  /**
   * 状态槽 —— 一个位置，**几个**生产者，**按优先级依次占用**。
   *
   * ★ 这就是「这一行只能有一个意思」那条纪律的实现方式：
   *   不是把它们并排挤成一小堆（改版前就是这样），而是让更要紧的那一个
   *   把位置占掉。标签由**生产者**给 —— 这里绝不写死任何一句，
   *   否则这条通道上多一个生产者，它就开始说假话。
   *
   * 优先级 order 的判据是「**这条消息说不出口的时候，用户会不会去错的地方找原因**」：
   *   错误 > 后端警告 > 穿透没生效 > 跟读失准 > 讲完了
   *   （"穿透没生效"排在"跟读失准"前面，是因为它往往发生在**动手那一刻**，
   *     而跟读失准只在提词进行中才有意义。）
   *
   * ★ 没有生产者时返回 `null` ⇒ 这一格宽度为 0。**刻意不留一个空占位的槽**
   *   —— 顶栏这一行比原来的底栏更窄，空槽会直接从进度条嘴里抢走 60px。
   *
   * ★★ 这里每个生产者原先还带一个 `title`（长解释，走悬停气泡）。
   *    第十一轮气泡全删了，`title` 字段**一并去掉** —— 但每条消息的
   *    **详细口径**都以注释形式留在各自的分支上：那些话之所以写下来，
   *    是因为「用户看到这四个字时该往哪想」这件事必须有人定，
   *    不能因为界面上不显示了就丢掉（下次改这行文案要用）。
   */
  const statusLine = ((): { text: string; cls: string; act?: () => void } | null => {
    if (error) {
      return {
        text: error,
        cls: "err",
        act: () => dispatch({ type: "ERROR", message: null }),
      };
    }
    if (warning) {
      return {
        text: warning.label,
        cls: "warn",
        // 点它不是"关掉"，而是打开设置：这几类提示出现之后，
        // 用户要做的事基本都在设置里（自检 / 跟读 / 穿透）。
        act: () => dispatch({ type: "SETTINGS", open: true }),
      };
    }
    if (ctOverdue) {
      return {
        text: "穿透未生效",
        cls: "warn",
        // 口径：穿透已经打开、但后端还没确认生效（多半是可交互区没上报上来），
        // 此刻鼠标可能仍被窗口挡住。出口是 `设置 › 窗口 › 鼠标穿透` 关掉再开，
        // 或按热键从托盘关（`hkClickThrough`）。这段以前是悬停气泡的正文。
        act: () => dispatch({ type: "SETTINGS", open: true }),
      };
    }
    if (cursor && cursor.miss_streak >= 3) {
      return {
        text: `跟读失准 · ${cursor.miss_streak}`,
        cls: "warn",
        // 口径：语音与稿件长时间对不上。出路是手动翻句，
        // 或点正文正确的位置把进度接回来。
        act: () => dispatch({ type: "SETTINGS", open: true }),
      };
    }
    // 匀速滚动的**正常结束**：稿子滚到了底，引擎自己停住了。
    //
    // ★ 缺了这一条，界面会停在"提词中"（实心绿）而光标一动不动 ——
    //   那看起来就是"卡住了"，而实际是讲完了。这两件事必须分得开，
    //   否则用户会去重启一个本来好好的功能（而重启后它会从头再滚一遍）。
    if (autoMode && !running && state.demoEnded) {
      return {
        text: "已讲完",
        cls: "ok",
        // 口径：不是出错，再按一次大圆按钮会从开头重新滚一遍。
      };
    }
    return null;
  })();

  /**
   * 改匀速滚动的速度。
   *
   * 本地先落（拖动要跟手，不能等一次 IPC 往返），再推给引擎立刻生效。
   * 与设置页那根滑杆走的是同一条后端命令 —— 两处各写一份的话，
   * 迟早会出现"栏上调了、设置里显示的还是旧值"。
   */
  function setAutoWpm(v: number) {
    if (!config) return;
    dispatch({ type: "CONFIG", config: { ...config, auto_wpm: v } });
    void api.setAutoWpm(v).catch(() => {});
  }

  async function seek(e: React.MouseEvent<HTMLDivElement>) {
    if (total <= 0) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    try {
      const c = await api.goto(Math.round(ratio * total));
      dispatch({ type: "CURSOR", cursor: c });
    } catch {
      /* 忽略 */
    }
  }

  const p = promptState(state, busy);

  /**
   * 主角的形态。四态见 `promptState`。
   *
   * ★★ 颜色的意思（全界面一个颜色只有一个意思）：
   *   绿 = 已生效 / 进行中 —— 所以「提词中」是实心绿。
   *   蓝 = 用户自己按下的（暂停）—— 所以「已暂停」是蓝色描边。
   *   琥珀在这里**一次都不出现**：它不是"暂停"，它是"受限/没生效"，
   *   那类事由隐身灯和这一格状态文字承载。
   *
   * ★ 四态各自的**口径**还写在这里（它们进过悬停气泡，现在气泡没了 ——
   *   但判断依据留着，改这四种形态时要用）：
   *   · 就绪：隐身 + 置顶 + 按当前推进方式往下走，一次做完。刻意不含全屏
   *     （全屏会把 PPT 整个盖住）。
   *   · 提词中：暂停不会动隐身与置顶；要收摊去设置里。
   *   · 已暂停：提词停住，隐身与置顶都还在。
   */
  const heroCls: string = (() => {
    switch (p) {
      case "starting":
        return "starting";
      case "running":
        return "on";
      case "paused":
        return "paused";
      default:
        return "";
    }
  })();

  return (
    <>
      {/*
        上一句 · 主角 · 下一句。
        ★ 上一句/下一句是**完整可用的另一条路**，不是"自动跟读的降级" ——
          语音跟不准、或者用户就是不想开麦时，手动翻句要能撑起整场。
          所以它们留在主界面上，而不是收进设置。
      */}
      <div className="tb-transport">
        <button className="tb-step" onClick={() => void stepSentence(false, dispatch)}>
          <IconChevronLeft size={16} />
        </button>

        {/*
          主角。整块界面上唯一一个实心图标、唯一一个圆形的按钮 ——
          "从哪开始"的线索全在这两件事上。
        */}
        <button
          className={`hero-btn ${heroCls}`}
          onClick={() => void togglePrompting(state, dispatch, setBusy)}
          disabled={p === "starting"}
          /* 收音电平驱动外圈的呼吸环：麦克风有没有在收音，是这根按钮上
             最该被确认的一件事。放在这里，它跟"正在提词"是同一个视线焦点。 */
          style={
            running
              ? ({ "--mic": Math.min(1, Math.max(0, asrLevel * 3)).toFixed(3) } as React.CSSProperties)
              : undefined
          }
        >
          {p === "running" ? <IconPause size={16} /> : <IconPlay size={16} />}
        </button>

        <button className="tb-step" onClick={() => void stepSentence(true, dispatch)}>
          <IconChevronRight size={16} />
        </button>
      </div>

      <span className="tb-pos">
        {pos}/{total}
      </span>

      <div className="progress-track" onClick={seek}>
        <div className="progress-fill" style={{ width: `${pct}%` }} />
      </div>

      {statusLine ? (
        /*
         * 状态槽。
         *
         * ★★ 文字**只留「是什么」**（「麦克风异常」「穿透未生效」…），
         *    可点这件事交给末尾那个 `›`。原来每个标签都自带四个字的
         *    「· 点击查看」后缀 —— 它在底栏时代不贵（那一行本来就空），
         *    搬进顶栏之后就贵了：这一格每多四个字，标题和进度条就少四个字。
         *    而"能不能点"用一个小箭头说，比用四个字说省得多。
         *
         * ★ 两个 span 是**分开的**：文字要允许被省略号截（长错误会很长），
         *   而箭头不能被截 —— 它是这一格唯一的可点线索。
         *   桩里的 `slotText` 因此读 `.tb-status-tx`，不带箭头。
         */
        <span
          className={`tb-status ${statusLine.cls}`}
          onClick={statusLine.act}
          style={statusLine.act ? { cursor: "pointer" } : undefined}
        >
          <span className="tb-status-tx">{statusLine.text}</span>
          {statusLine.act && (
            <span className="tb-status-go" aria-hidden="true">
              ›
            </span>
          )}
        </span>
      ) : autoRunning ? (
        /*
          匀速滚动的调速条 —— **开讲中调速只有这一个入口**。
          设置页那根滑杆要打开抽屉（352px 宽，会盖掉半张稿子），
          讲的过程中根本来不及用它。主流提词器（teleprompter.com / Detail）
          也都把速度条放在播放界面上，是同一个道理。
        */
        <div className="tb-speed">
          <input
            type="range"
            min={60}
            max={600}
            step={10}
            value={autoWpm}
            aria-label="匀速滚动速度"
            onChange={(e) => setAutoWpm(Number(e.target.value))}
          />
          <span className="cbs-val">{autoWpm}</span>
        </div>
      ) : null}
    </>
  );
}
