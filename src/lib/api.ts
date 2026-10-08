// Tauri 命令与事件的统一出口。
//
// 组件里不直接写 invoke("xxx")，全部走这里 —— 命令名散落在各个组件里的话，
// 后端改一次名字要全局搜索着改，且拼错了只会在运行时才炸。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AppConfig,
  AsrPayload,
  AsrStatusPayload,
  BootstrapPayload,
  CursorPayload,
  DragHint,
  MicProbe,
  ModelPaths,
  PreflightRow,
  ScriptEntry,
  ScriptPayload,
  StealthReport,
} from "../types";

export const api = {
  bootstrap: () => invoke<BootstrapPayload>("bootstrap"),

  loadScript: (path: string) => invoke<ScriptPayload>("load_script_from_path", { path }),
  setScriptText: (text: string, title?: string) =>
    invoke<ScriptPayload>("set_script_text", { text, title: title ?? null }),
  clearScript: () => invoke<ScriptPayload>("clear_script"),

  /** 把一段文本当作识别结果喂给对位器（无需麦克风的联调入口）。 */
  feed: (text: string) => invoke<CursorPayload>("feed_hypothesis", { text }),

  nudge: (delta: number) => invoke<CursorPayload>("nudge", { delta }),
  goto: (pos: number) => invoke<CursorPayload>("goto", { pos }),
  stepSentence: (forward: boolean) => invoke<CursorPayload>("step_sentence", { forward }),
  rewind: () => invoke<CursorPayload>("rewind"),

  setStealth: (enabled: boolean) => invoke<StealthReport>("set_stealth", { enabled }),

  startAsr: () => invoke<AsrStatusPayload>("start_asr"),
  stopAsr: () => invoke<AsrStatusPayload>("stop_asr"),
  asrStatus: () => invoke<AsrStatusPayload>("asr_status"),

  /**
   * 麦克风自检：录一小段，判断设备到底有没有在送声音。
   *
   * 会阻塞 `seconds` 秒（默认 2.5），期间界面应当给出「正在采集」的提示，
   * 否则用户会以为程序卡住了。
   */
  micCheck: (seconds?: number) =>
    invoke<MicProbe>("mic_check", { seconds: seconds ?? null }),

  saveConfig: (config: AppConfig) => invoke<void>("save_config", { config }),
  getConfig: () => invoke<AppConfig>("get_config"),

  /**
   * 窗口置顶。**窗口属性与配置一起改**，返回实际生效后的状态。
   *
   * ★ 不要退回去用 `getCurrentWindow().setAlwaysOnTop()`：那条路只动窗口、
   *   不落配置，于是"设好了、下次打开又没了"。它有两个调用者
   *   （开始提词 / 收摊、设置里的开关），合成一个命令才不会两处打架。
   */
  setAlwaysOnTop: (on: boolean) => invoke<boolean>("set_always_on_top", { on }),

  /**
   * 收进任务栏：窗口缩下去，但**任务栏留一枚按钮**，点它原样回来。
   *
   * ★ 不要退回去用 `getCurrentWindow().minimize()`：任务栏按钮的放出与收回
   *   是一个事务 —— 后端要在最小化前 `set_skip_taskbar(false)`（走
   *   `ITaskbarList::AddTab`），再在窗口回到屏幕上时把它收回来（平时这个窗口
   *   不占任务栏，那是设计）。窗口 API 单独调只能做一半，结果是任务栏**永远**
   *   挂着它，或者收起之后任务栏里什么都没有、用户再也找不到窗口。
   */
  minimizeToTaskbar: () => invoke<void>("minimize_to_taskbar"),

  rescanModels: () => invoke<ModelPaths[]>("rescan_models"),
  modelsDir: () => invoke<string>("models_dir"),
  openPath: (path: string) => invoke<void>("open_path", { path }),

  /** 自检清单：平台 / 句柄 / 模型 / 识别能力。 */
  diagnostics: () => invoke<[string, string][]>("diagnostics"),

  // ---- 稿件库 ----
  listScripts: () => invoke<ScriptEntry[]>("list_scripts"),
  activateScript: (id: string) => invoke<ScriptPayload>("activate_script", { id }),
  forgetScript: (id: string) => invoke<void>("forget_script", { id }),
  clearHistory: () => invoke<void>("clear_script_history"),
  /** 每条历史对应的文件还在不在 —— 列表里用它标灰。 */
  scriptAvailability: () => invoke<[string, boolean][]>("script_availability"),

  // ---- 窗口层 ----
  /** 返回**实际生效**的穿透状态（不是用户按下的意图）。 */
  setClickThrough: (enabled: boolean) => invoke<boolean>("set_click_through", { enabled }),
  /**
   * 上报「可交互区」矩形（CSS px，相对内容区左上角）。
   *
   * 穿透态下前端收不到 mousemove，所以"鼠标在不在控制区里"只能由后端判断；
   * 而"控制区长什么样"只有 DOM 知道 —— 两边各出一半信息。
   * 传 null 表示整窗可交互（后端会据此关掉穿透）。
   *
   * ★★ 是**一组**矩形，不是一个。展开态下控制栏有上下两条，
   * 把它们并成一个包围盒会得到一个覆盖整窗的矩形 ——
   * 于是"在控制区外"永远不成立，穿透一次都不会触发（真机踩过）。
   * 语义是「光标落在其中任意一块里都算可点」。
   */
  reportZone: (rects: [number, number, number, number][] | null) =>
    invoke<void>("report_interactive_zone", { rects }),
  /**
   * 前端进度打点。release 产物没有 devtools，前端"跑到哪一步停了"从外面看不见；
   * 日志里有没有 `[bootstrap]` 只能区分"页面跑没跑"，分不出"跑到一半停了"。
   * 所以在启动链路的关键节点各打一点。
   */
  beat: (step: string) => invoke<void>("frontend_beat", { step }),
  setCollapsed: (collapsed: boolean) => invoke<void>("set_collapsed", { collapsed }),
  isCollapsed: () => invoke<boolean>("is_collapsed"),
  setWindowLocked: (locked: boolean) => invoke<void>("set_window_locked", { locked }),

  // ---- 读稿手感 ----
  /**
   * 改匹配下限并立即作用于正在跑的对位器。
   * 返回**夹取后**的实际值 —— 界面显示的和引擎用的是同一个数。
   */
  setMatchFloor: (value: number) => invoke<number>("set_match_floor", { value }),
  matchFloor: () => invoke<number>("match_floor"),
  /**
   * 改跳读范围（句）并立即作用于正在跑的对位器。
   * 同样返回夹取后的值 —— 滑杆两端之外的值由**引擎**说了算，不由界面说了算。
   */
  setJumpRange: (value: number) => invoke<number>("set_jump_range", { value }),
  jumpRange: () => invoke<number>("jump_range"),
  /**
   * 改匀速滚动的速度并**立即**作用于正在跑的引擎 —— 开讲中调速走的就是这里，
   * 拖完不必重新开始。返回夹取后的实际值。
   *
   * 当前若不是匀速滚动，后端那边是空操作（trait 的默认实现），
   * 所以调用方不必先判断"现在是哪种推进方式"。
   */
  setAutoWpm: (value: number) => invoke<number>("set_auto_wpm", { value }),

  // ---- 省事类 ----
  /**
   * 会前自检。**会阻塞约 1.2 秒**（里面真录一小段麦克风）——
   * 界面必须给出「正在检查」的反馈，否则用户会以为卡住了。
   */
  preflight: () => invoke<PreflightRow[]>("preflight"),
  /** 导出迁移包，返回写出的字节数（界面用它证明"确实写出去了"）。 */
  exportBundle: (path: string) => invoke<number>("export_bundle", { path }),
  /** 导入迁移包，返回导入后的配置，调用方据此重刷界面。 */
  importBundle: (path: string) => invoke<AppConfig>("import_bundle", { path }),
};

type Handler<T> = (payload: T) => void;

export const events = {
  onCursor: (cb: Handler<CursorPayload>): Promise<UnlistenFn> =>
    listen<CursorPayload>("prompter://cursor", (e) => cb(e.payload)),

  onAsr: (cb: Handler<AsrPayload>): Promise<UnlistenFn> =>
    listen<AsrPayload>("prompter://asr", (e) => cb(e.payload)),

  onStealth: (cb: Handler<StealthReport>): Promise<UnlistenFn> =>
    listen<StealthReport>("prompter://stealth", (e) => cb(e.payload)),

  onAsrStatus: (cb: Handler<AsrStatusPayload>): Promise<UnlistenFn> =>
    listen<AsrStatusPayload>("prompter://asr-status", (e) => cb(e.payload)),

  onVisibility: (cb: Handler<boolean>): Promise<UnlistenFn> =>
    listen<boolean>("prompter://visibility", (e) => cb(e.payload)),

  /** 稿件被换掉（拖放、托盘切稿走的都是这条路 —— 它们不经过 invoke，没有返回值）。 */
  onScript: (cb: Handler<ScriptPayload>): Promise<UnlistenFn> =>
    listen<ScriptPayload>("prompter://script", (e) => cb(e.payload)),

  /** 拖放悬停提示，用来画覆盖层。 */
  onDrag: (cb: Handler<DragHint>): Promise<UnlistenFn> =>
    listen<DragHint>("prompter://drag", (e) => cb(e.payload)),

  /** 收起 / 展开（热键触发的也会走这里）。 */
  onCollapsed: (cb: Handler<boolean>): Promise<UnlistenFn> =>
    listen<boolean>("prompter://collapsed", (e) => cb(e.payload)),

  /** 穿透状态变了（热键/托盘触发的也会走这里）。 */
  onClickThrough: (cb: Handler<boolean>): Promise<UnlistenFn> =>
    listen<boolean>("prompter://click-through", (e) => cb(e.payload)),

  /**
   * 非致命警告：识别还在跑，但结果大概不对（典型场景是麦克风在送数字静音）。
   *
   * 与 `onError` 分开是刻意的 —— 错误意味着功能已停，警告意味着功能在跑
   * 但不会出结果。两者的用户动作完全不同：前者去看为什么停了，
   * 后者去查麦克风。
   */
  onWarning: (cb: Handler<string>): Promise<UnlistenFn> =>
    listen<string>("prompter://warning", (e) => cb(e.payload)),

  /** 来自托盘等外部入口的一次性提示。 */
  onNotice: (cb: Handler<string>): Promise<UnlistenFn> =>
    listen<string>("prompter://notice", (e) => cb(e.payload)),

  onError: (cb: Handler<string>): Promise<UnlistenFn> =>
    listen<string>("prompter://error", (e) => cb(e.payload)),
};

/** 把 invoke 抛出的东西统一成可显示的一行字。 */
export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}
