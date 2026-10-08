// 全局状态：一个 reducer 收所有状态变更。
//
// 不引第三方状态库 —— 这个应用的状态来源只有三处（启动时的 bootstrap、
// Tauri 推来的事件、用户操作），用 useReducer 已经能把它们收进一条清晰的
// 单向数据流；再引一层库只会多一份需要维护的约定。

import {
  createContext,
  useContext,
  useReducer,
  type Dispatch,
  type ReactNode,
} from "react";

import type {
  AppConfig,
  AsrStatusPayload,
  CursorPayload,
  DragHint,
  ModelPaths,
  PlatformInfo,
  ScriptEntry,
  ScriptPayload,
  StealthReport,
} from "../types";

import { readColorRgb } from "./readColors";

/**
 * 隐身的**用户意图**（三态）。
 *
 * 后端只有 `hidden | visible` 两态，于是"临时露一下、等下还要继续隐身"
 * 和"不用了"在后端是同一个值。但用户心里这是两件事 ——
 * 少了这一层，界面上就给不出「继续」这个动作：演示中途按一下热键，
 * 状态就退化成"未开启"，想回到隐身只能当成重新开启。
 */
export type StealthIntent = "off" | "on" | "paused";

/**
 * 顶栏那个**常驻**状态槽的黄色警告。
 *
 * ★ 为什么要带 `label` 而不是只有一个字符串（本轮改的）：
 *   那个状态槽原先把标签写死成「麦克风异常 · 点击自检」。可是会往这里写东西的
 *   前后有六处（麦克风送数字静音、穿透可交互区上报失败、隐身没生效、
 *   演示没就位、收起跟读没停干净……），写死的标签于是会**说假话** ——
 *   隐身没开却显示"麦克风异常"，比不提示更坏：它把人引到错的地方去查。
 *
 *   标签由生产者给、状态槽只负责显示。这与「一个颜色只能有一个意思」
 *   是同一条纪律：那一行也只能有一个意思。
 */
export interface StatusWarning {
  /** 铺在顶栏状态槽里的短标签（要放得下，所以由生产者负责短）。 */
  label: string;
  /** 完整说明，放 `title` —— 长成什么样都行。 */
  detail: string;
}

export interface State {
  ready: boolean;
  config: AppConfig | null;
  script: ScriptPayload | null;
  cursor: CursorPayload | null;
  platform: PlatformInfo | null;
  stealth: StealthReport | null;
  /** 隐身的用户意图（三态）。与 `stealth`（后端回报的事实）分开存，理由见 `StealthIntent`。 */
  stealthIntent: StealthIntent;
  /** 隐身正在切换（已下指令、等后端确认）。驱动「开启中」那段动画。 */
  stealthBusy: boolean;
  /** 最近一次的识别文本，界面上回显「听成了什么」 */
  asrText: string;
  asrLevel: number;
  asrStatus: AsrStatusPayload | null;
  modelsDir: string;
  foundModels: ModelPaths[];
  appVersion: string;
  /** 可载入的稿件扩展名（不含点）—— 由后端给出，前端只消费。 */
  scriptExts: string[];
  /** 上面那份清单给用户看的样子。 */
  scriptExtsHint: string;
  /** 阻断性错误，显示在顶栏状态槽 */
  error: string | null;
  /**
   * 非致命警告，显示在顶栏状态槽（黄色）。
   *
   * 与 `error` 分开存：警告出现时识别仍在跑，状态槽不该变成红色的
   * 「已停止」观感 —— 那会让用户去重启识别，而问题根本不在那儿。
   */
  warning: StatusWarning | null;
  /** 一次性提示（如「已应用」），显示在抽屉里 */
  notice: string | null;
  settingsOpen: boolean;
  busy: boolean;
  diag: [string, string][] | null;

  /** 稿件历史（按最后使用时间倒序）。 */
  scripts: ScriptEntry[];
  /** 每条历史对应的文件还在不在 —— 用来标灰。 */
  scriptAvail: Record<string, boolean>;
  historyOpen: boolean;

  /**
   * 稿内搜索。
   *
   * 查询串与当前序号放在全局状态里，而不是查找栏组件内部 ——
   * 稿件区要靠它们决定标哪几处、滚到哪一处。放在组件里就得靠 props
   * 一层层传下去，而 PrompterView 与 TopBar 是兄弟节点。
   */
  findOpen: boolean;
  findQuery: string;
  /** 当前命中在**全部命中**里的序号（0 基）。没有命中时无意义。 */
  findIndex: number;

  /**
   * 粘贴 / 编辑稿件的浮层。
   *
   * `mode` 决定这个浮层的语义：
   *   * `new`  —— 新稿：空白开始，保存后在历史里新增一条；
   *   * `edit` —— 改当前的稿：预填当前正文，保存后**另存为一份新副本**
   *               （原文件不动，也不覆盖历史里的旧条目 —— 改了原文件却
   *               不告诉用户，是比"多一条历史"严重得多的失败）。
   */
  pasteOpen: boolean;
  pasteMode: "new" | "edit";

  /** 拖放悬停提示。非空时显示覆盖层。 */
  drag: DragHint | null;
  /**
   * 穿透是否**真的在工作**（不是用户的意图）。
   *
   * 判据是"开关已开 **且** 后端拿到了可信的可交互区"（后端 `ct_working`）——
   * 刻意**不是**"此刻光标恰好落在稿件区"：后者随鼠标每 40ms 翻，
   * 拿它驱动状态灯会闪成一片，而且分不清"功能没生效"和"光标恰好在栏上"。
   *
   * 与 `config.click_through` 分开：意图存在配置里、要等后端确认才生效，
   * 两者在切换瞬间会不一致。界面上显示的必须是这个实际状态。
   */
  ctActive: boolean;
  /**
   * 穿透「已下指令、还没生效」。驱动按钮的「开启中」动画。
   *
   * 与 `ctActive` 的区别正是"路上"与"到了"：`ctActive` 由后端推来的真实状态驱动，
   * 这个由用户那一次点击驱动。两者同时为真时以 `ctActive` 为准（组件里判）。
   */
  ctPending: boolean;
  /** 演示模式是否正在进行 —— 由「▶ 演示」置位、退出时清除。 */
  demoActive: boolean;
  /**
   * 演示**暂停**中：跟读停了，但隐身与置顶原样保留。
   *
   * 与 `demoActive` 分开是同一个理由（见 `StealthIntent`）：后端只有
   * 「跟读在跑 / 没在跑」这一维，于是"脱稿讲一段、等下接上"和"讲完了收摊"
   * 在后端是同一个值。少了这一层，顶栏就只能原地打转回「▶ 演示」——
   * 用户按下"暂停"却看到按钮回到最初的样子，会以为整个演示被关掉了。
   */
  demoPaused: boolean;
  /**
   * 匀速滚动**自己滚到了底**（正常结束），而不是被用户停下的。
   *
   * ★ 为什么不能拿 `demoActive === false` 顶替：那条路上有**两种**情形 ——
   *   「讲完了，引擎自己停的」和「讲到一半用户按了暂停/收摊」。前者该说
   *   「已讲完」，后者什么都不该说。少了这一维，两者只能挑一个，
   *   总有一个会说错话。
   * ★ 也不能拿「光标在末尾」顶替：用户把进度条直接拖到底也会到末尾，
   *   那会儿他并没有讲过任何东西。所以它记的是"**发生过**一次滚到底"。
   */
  demoEnded: boolean;
  /** 收起态。 */
  collapsed: boolean;
  /**
   * 「正在收起」的那一拍（时长见 CSS `--t-collapse`）。
   *
   * ★ 它单独成一维，而不是让 `collapsed` 兼着表达：
   *   收起必须分两拍 —— 先让正文滑上去淡出，**再**把窗口压成一条。
   *   中间那一拍里窗口还是原来的高度、正文已经淡了，
   *   而 `collapsed` 一旦为真，界面就该是"已经收好了"的样子，描述不了它。
   *   少了这一维，收起的动画就只能砍掉（上一版正是如此）。
   */
  collapsing: boolean;
}

const initialState: State = {
  ready: false,
  config: null,
  script: null,
  cursor: null,
  platform: null,
  stealth: null,
  stealthIntent: "off",
  stealthBusy: false,
  asrText: "",
  asrLevel: 0,
  asrStatus: null,
  modelsDir: "",
  foundModels: [],
  appVersion: "",
  // 兜底值只在 bootstrap 回来之前生效（那一瞬间）。写成和 Rust 侧
  // `SCRIPT_EXTS` 一致，是为了"打开应用的头 50 毫秒里点开对话框"
  // 也能选到正确的一批 —— 虽然几乎撞不上，但兜底值本身不该是错的。
  scriptExts: ["txt", "md", "markdown", "docx", "pptx"],
  scriptExtsHint: ".txt / .md / .docx / .pptx",
  error: null,
  warning: null,
  notice: null,
  settingsOpen: false,
  busy: false,
  diag: null,
  scripts: [],
  scriptAvail: {},
  historyOpen: false,
  findOpen: false,
  findQuery: "",
  findIndex: 0,
  pasteOpen: false,
  pasteMode: "new",
  drag: null,
  ctActive: false,
  ctPending: false,
  demoActive: false,
  demoPaused: false,
  demoEnded: false,
  collapsed: false,
  collapsing: false,
};

export type Action =
  | { type: "BOOTSTRAP"; payload: Partial<State> }
  | { type: "CURSOR"; cursor: CursorPayload }
  | { type: "SCRIPT"; script: ScriptPayload }
  | { type: "ASR"; text: string; level: number }
  | { type: "ASR_STATUS"; status: AsrStatusPayload }
  | { type: "STEALTH"; report: StealthReport }
  /** 用户改了隐身意图（点了 pill）。`busy` = 已下指令、等后端确认。 */
  | { type: "STEALTH_INTENT"; intent: StealthIntent; busy?: boolean }
  | { type: "CONFIG"; config: AppConfig }
  | { type: "MODELS"; models: ModelPaths[] }
  | { type: "DIAG"; rows: [string, string][] }
  | { type: "ERROR"; message: string | null }
  | { type: "WARNING"; warning: StatusWarning | null }
  | { type: "NOTICE"; message: string | null }
  | { type: "SETTINGS"; open: boolean }
  | { type: "BUSY"; busy: boolean }
  | { type: "SCRIPTS"; scripts: ScriptEntry[] }
  | { type: "SCRIPT_AVAIL"; avail: Record<string, boolean> }
  | { type: "HISTORY"; open: boolean }
  | { type: "FIND"; open: boolean }
  | { type: "FIND_QUERY"; query: string }
  | { type: "FIND_INDEX"; index: number }
  | { type: "PASTE"; open: boolean; mode?: "new" | "edit" }
  | { type: "DRAG"; hint: DragHint | null }
  | { type: "CT"; active: boolean }
  | { type: "CT_PENDING"; pending: boolean }
  /** `paused` 只在 `active` 为真时有意义；带 `active:false` 就是收摊。 */
  | { type: "DEMO"; active: boolean; paused?: boolean }
  | { type: "COLLAPSED"; collapsed: boolean }
  /** 「收起动画进行中」。与 `COLLAPSED` 分开，理由见 `State.collapsing`。 */
  | { type: "COLLAPSING"; on: boolean };

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "BOOTSTRAP":
      return { ...state, ...action.payload, ready: true };

    case "CURSOR":
      // 光标一旦离开末尾（用户把进度拖回去、或者跳到别处），
      // 「已讲完」就不再是真的了 —— 它是"此刻停在末尾"这句话，不是一次性标签。
      return {
        ...state,
        cursor: action.cursor,
        demoEnded: action.cursor.progress >= 0.999 ? state.demoEnded : false,
      };

    case "SCRIPT":
      return {
        ...state,
        script: action.script,
        // 换稿必须同时把光标归零，否则会拿旧稿的进度去高亮新稿
        cursor: state.cursor
          ? {
              ...state.cursor,
              cursor: 0,
              progress: 0,
              paragraph: 0,
              sentence: 0,
              offset_in_sentence: 0,
              sentence_align_start: 0,
              sentence_align_end: 0,
              total: action.script.total_chars,
            }
          : state.cursor,
      };

    case "ASR":
      return { ...state, asrText: action.text, asrLevel: action.level };

    case "ASR_STATUS": {
      const next = { ...state, asrStatus: action.status };
      /*
       * ★ 匀速滚动会**自己走到头**，这件事得在两处同时说清楚，而不是只说一半。
       *
       *   后端滚到头时把 `running` 打成 false，状态槽于是显示「已讲完」。
       *   但大圆按钮读的是 `demoActive/demoPaused` 那套**演示编排状态** ——
       *   它记的是"用户按下过开始"，不会自己退出。于是滚完之后：
       *     状态槽说「已讲完」，大圆按钮还亮着实心绿（= 正在提词）。
       *   而人扫一眼屏幕，看的是那个大按钮 ⇒ 以为还在滚，实际早停了。
       *   这正是「界面显示实际在工作，不是用户按下的意图」的反面。
       *
       *   ⇒ 在这里把编排状态一并收回 idle。判据必须带 `progress >= 0.999`：
       *     启动瞬间后端也会先报一次 running=false（`start_asr_inner` 之前），
       *     只按 `!running` 判会把「就位中」当场取消掉，按钮闪一下退回就绪。
       *   ⇒ 也只对「匀速滚动」生效：语音跟读不会自己结束，它的 running=false
       *     只可能来自用户主动停，那条路 `stopDemoMode` 已经清过了。
       */
      if (
        !action.status.running &&
        state.config?.asr.engine === "auto" &&
        (state.cursor?.total ?? 0) > 0 &&
        (state.cursor?.progress ?? 0) >= 0.999
      ) {
        next.demoActive = false;
        next.demoPaused = false;
        next.demoEnded = true;
      }
      return next;
    }

    case "STEALTH": {
      // 收到后端确认 ⇒ 这次切换的等待结束。
      const r = action.report;
      // 第三态（暂停）后端表达不出来，得在这里推：
      //   * 变成 hidden        ⇒ 意图就是「隐身中」
      //   * 变成 visible、而原本是「隐身中」⇒ 这是**暂停**，不是「关掉」。
      //     热键与托盘走的都是这条路 —— 一并按"暂停"理解，否则演示中途
      //     按一下热键，界面上就只剩「重新开启」，再也找不到「继续」。
      let intent = state.stealthIntent;
      if (r.mode === "hidden" && r.applied) {
        intent = "on";
      } else if (r.mode === "visible" && state.stealthIntent === "on") {
        intent = "paused";
      }
      return { ...state, stealth: r, stealthIntent: intent, stealthBusy: false };
    }

    case "STEALTH_INTENT":
      return {
        ...state,
        stealthIntent: action.intent,
        stealthBusy: action.busy ?? state.stealthBusy,
      };

    case "CONFIG":
      return { ...state, config: action.config };

    case "MODELS":
      return { ...state, foundModels: action.models };

    case "DIAG":
      return { ...state, diag: action.rows };

    case "ERROR":
      return { ...state, error: action.message };

    case "WARNING":
      return { ...state, warning: action.warning };

    case "NOTICE":
      return { ...state, notice: action.message };

    case "SETTINGS":
      return { ...state, settingsOpen: action.open };

    case "BUSY":
      return { ...state, busy: action.busy };

    case "SCRIPTS":
      return { ...state, scripts: action.scripts };

    case "SCRIPT_AVAIL":
      return { ...state, scriptAvail: action.avail };

    case "HISTORY":
      return { ...state, historyOpen: action.open };

    case "FIND":
      // 关掉查找时**保留**查询串。用户多半只是想让高亮消失、稍后接着找，
      // 清掉的话每次重开都要重新输入一遍。
      return { ...state, findOpen: action.open };

    case "FIND_QUERY":
      // 换查询必须把序号归零 —— 否则「第 7 处」会落在新结果集里
      // 一个毫不相干的位置上，表现为"一输入就跳到稿子中间"
      return { ...state, findQuery: action.query, findIndex: 0 };

    case "FIND_INDEX":
      return { ...state, findIndex: action.index };

    case "PASTE":
      return {
        ...state,
        pasteOpen: action.open,
        pasteMode: action.mode ?? state.pasteMode,
      };

    case "DRAG":
      return { ...state, drag: action.hint };

    case "CT":
      // 后端推来「在工作」⇒ 这次点击的等待结束。
      // ★ 只在 true 时清 pending：刚点开、还没就绪时后端推的是 false，
      //   清掉的话按钮会在「开启中」和「未生效」之间来回跳。
      return {
        ...state,
        ctActive: action.active,
        ctPending: action.active ? false : state.ctPending,
      };

    case "CT_PENDING":
      return { ...state, ctPending: action.pending };

    case "DEMO":
      // 收摊（active:false）时把暂停一并清掉 —— 否则下一次开启会带着
      // 上一次的暂停态进来，pill 一上来就显示「已暂停」而跟读其实在跑。
      return {
        ...state,
        demoActive: action.active,
        demoPaused: action.active ? (action.paused ?? false) : false,
        // 重新开跑 ⇒ 「已讲完」这句话不再成立。
        demoEnded: action.active ? false : state.demoEnded,
      };

    case "COLLAPSED":
      return { ...state, collapsed: action.collapsed };

    case "COLLAPSING":
      return { ...state, collapsing: action.on };

    default:
      return state;
  }
}

interface Ctx {
  state: State;
  dispatch: Dispatch<Action>;
}

const AppCtx = createContext<Ctx | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  return <AppCtx.Provider value={{ state, dispatch }}>{children}</AppCtx.Provider>;
}

export function useApp(): Ctx {
  const ctx = useContext(AppCtx);
  if (!ctx) {
    throw new Error("useApp 必须在 AppProvider 内使用");
  }
  return ctx;
}

/** 版面参数落到 CSS 变量上。集中在一处写，避免字号在三个组件里各改一遍。 */
export function applyLayoutVars(config: AppConfig | null) {
  const root = document.documentElement;
  if (!config) return;
  // 主题先解析出来（`system` 要解成实际用哪个），下面有两处要用到它。
  const th = resolveTheme(config.theme);
  root.style.setProperty("--fs", `${config.font_size}px`);
  root.style.setProperty("--lh", String(config.line_height));
  /*
   * 外壳透明度（`--win-alpha`）分两档来源：
   *   · off / css —— 用户的不透明度滑杆（`config.opacity`）；
   *   · system    —— **让位给系统材质**：Acrylic/hudWindow 磨的是窗口后面，
   *     外壳自己只留一层薄暗色保文字对比。滑杆在这一档被藏起来
   *     （设置页同步隐藏），一个意思只留一个控件。
   * ★ 0.22 不是拍的：失焦时系统会把材质退成实色（拍板②接受的行为），
   *   聚焦时磨砂由材质主导 —— 外壳 alpha 高了会盖住磨砂，低了文字发飘。
   */
  const systemGlass = config.glass_mode === "system";
  root.style.setProperty(
    "--win-alpha",
    systemGlass ? "0.22" : String(config.opacity),
  );
  root.style.setProperty("--ls", `${config.letter_spacing}em`);
  root.style.setProperty("--pad-x", `${config.padding_x}px`);
  root.style.setProperty("--align", config.align);
  // 跟读颜色：正文里的明读指示（当前句底色 / 竖条 / 正在念的那个字）都用它。
  // 走 `readColorRgb` 查表而不是直接用配置值 —— 配置里存的是 key，
  // 而且老配置可能指向一个已经删掉的 key，那时要退到出厂色而不是留空。
  // ★ 必须**按主题**取色：色板是按深色背景挑的，放到浅底上做 2px 下划线几乎看不见。
  //   而它不能交给 CSS 主题块去覆盖 —— 这个变量是内联写在根元素上的，
  //   内联的优先级高于任何主题选择器 ⇒ 只能在这里选。
  root.style.setProperty("--read", readColorRgb(config.read_color, th));

  // 磨砂与主题走 class / data 属性而不是 CSS 变量：
  // 它们要整片地改变一组样式，用选择器比逐个变量覆盖更不容易漏。
  //
  // ★ `no-glass`（关掉栏自己的 CSS 玻璃）在两种档位下成立：
  //   · off    —— 用户明确不要磨；
  //   · system —— 系统材质接管了"磨"，栏再叠一层 backdrop-filter 就是
  //     苹果 HIG 点名的「glass on glass」（两层磨＝视觉噪声 + 白烧 GPU）。
  root.classList.toggle("no-glass", config.glass_mode !== "css");
  root.dataset.theme = th;
}

/** `system` 解析成当前系统实际的主题。 */
export function resolveTheme(t: AppConfig["theme"]): "dark" | "light" {
  if (t === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return t;
}

/** 跟随系统主题时，系统换了要重新应用。返回取消订阅的函数。 */
export function watchSystemTheme(onChange: () => void): () => void {
  const mq = window.matchMedia("(prefers-color-scheme: light)");
  const handler = () => onChange();
  mq.addEventListener("change", handler);
  return () => mq.removeEventListener("change", handler);
}
