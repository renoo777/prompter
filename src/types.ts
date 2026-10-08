// 与 Rust 侧 serde 输出严格对应的类型定义。
//
// 这里的字段名保持 snake_case，与后端一致 —— 不做自动转换，
// 因为一旦两边命名规则分叉（比如后端改一个字段名），
// 前端不会报错、只会静默拿到 undefined，排查成本远高于忍住不驼峰。

export type SupportLevel =
  | { kind: "native" }
  | { kind: "fragile"; note: string }
  | { kind: "unavailable"; note: string };

export interface Sentence {
  index: number;
  text: string;
  align_start: number;
  align_end: number;
}

export interface Paragraph {
  index: number;
  text: string;
  sentences: Sentence[];
}

export interface ScriptPayload {
  title: string;
  paragraphs: Paragraph[];
  total_chars: number;
}

export interface CursorPayload {
  cursor: number;
  total: number;
  progress: number;
  paragraph: number;
  sentence: number;
  /** 当前句内的字符偏移 —— 用来把高亮精确到「读到的那个字」。 */
  offset_in_sentence: number;
  sentence_align_start: number;
  sentence_align_end: number;
  score: number;
  moved: boolean;
  miss_streak: number;
  realigned: boolean;
}

export type StealthMode = "hidden" | "visible";

export interface StealthReport {
  mode: StealthMode;
  applied: boolean;
  detail: string;
  support: SupportLevel;
  fallback_hint: string | null;
}

export interface PlatformInfo {
  os: string;
  support: SupportLevel;
  fallback_hint: string | null;
  /**
   * 「收起」这颗入口在当前平台叫什么。
   *
   * ★ 由后端给出（`win::platform::collapse_entry_label`），前端**不要**按
   *   `os` 自己拼一份 —— 后端决定"这一下到底是收起还是最小化"用的是同一个判断，
   *   两处各写一遍就会漂成"按钮写着收进任务栏、按下去却是收起"。
   */
  collapse_label: string;
  /** 上面那颗按钮下面的说明文字。同样由后端给。 */
  collapse_hint: string;
  /**
   * 「任务栏」这个词在当前平台上是否存在 —— 决定那颗「收进任务栏」按钮该不该出现。
   *
   * Windows `true`；macOS 及其它平台 `false`（那里收起入口只有一颗按钮）。
   */
  has_taskbar: boolean;
}

export interface AsrPayload {
  text: string;
  level: number;
  is_final: boolean;
}

export interface AsrStatusPayload {
  running: boolean;
  engine: string;
  detail: string;
}

/** 麦克风拾音判定档位。 */
export type MicVerdict = "ok" | "weak" | "silent";

/**
 * 一次麦克风自检的结果。
 *
 * 之所以要做这个自检：设备送数字静音时，识别文本就是空的，
 * 跟「用户没说话」长得一模一样 —— 只靠界面上的文字根本区分不了。
 */
export interface MicProbe {
  device: string;
  sample_rate: number;
  channels: number;
  format: string;
  seconds: number;
  frames: number;
  /** 峰值 0..1 */
  peak: number;
  /** 均方根 0..1 */
  rms: number;
  verdict: MicVerdict;
  /** 结论 + 处置建议（多行） */
  detail: string;
  devices: string[];
}

/** 档位转成界面上的一句话标签。 */
export function micVerdictText(v: MicVerdict): string {
  switch (v) {
    case "ok":
      return "拾音正常";
    case "weak":
      return "信号偏弱";
    case "silent":
      return "数字静音";
  }
}

/** 幅度转 dBFS，与后端 `asr::to_dbfs` 同一个口径。 */
export function toDbfs(amplitude: number): number {
  return 20 * Math.log10(Math.max(amplitude, 1e-12));
}

export interface ModelPaths {
  encoder: string;
  decoder: string;
  joiner: string;
  tokens: string;
  label: string;
}

/**
 * **谁决定文稿往下走**（设置页里那一行「推进方式」）。
 *
 * 名字沿用后端的 `EngineKind`，但它列的其实是四个并列的答案：
 * 我的声音（`local` / `cloud`）、一只时钟（`auto`）、我的手（`manual`）。
 * 设置页把前两个并成「语音跟随」，而不是让用户去挑"引擎"这种黑话 ——
 * 那会给出「自动滚动＝开，引擎＝本地离线」这种自相矛盾的组合，
 * 而屏幕上没有任何东西能告诉用户该听谁的。
 */
export type EngineKind = "local" | "cloud" | "manual" | "auto";

export interface HotkeyConfig {
  next_sentence: string;
  prev_sentence: string;
  toggle_stealth: string;
  toggle_visibility: string;
  toggle_recognition: string;
  toggle_collapse: string;
  toggle_click_through: string;
}

/**
 * 把配置里的热键串（`Ctrl+Alt+KeyH`）翻成**给用户看的样子**（`Ctrl+Alt+H`）。
 *
 * # 为什么必须走这个函数，不能在前端各处手写 `"Ctrl+Alt+H"`
 *
 * 手写的话，两个平台上会**同时**出两种错：
 *
 * 1. **真机不一致**：macOS 上后端注册的是 `Command+Option+H`（理由见 Rust 侧
 *    `HotkeyConfig::default` 的注释 —— `Ctrl+方向键` 在 macOS 上被 Mission Control
 *    占着），而界面还写着 `Ctrl+Alt+H` ⇒ 用户照着按，没反应，
 *    而且他不可能猜到该按什么。
 * 2. **改键后说谎**：配置里的热键是用户能改的。界面写死字符串，
 *    用户改完键之后所有提示就全在说假话 —— 这类"文案与事实不符"
 *    没有任何编译期检查能拦住，只能靠**只留一个来源**。
 *
 * ⇒ 所以：热键串从 `config.hotkeys.*` 来，显示格式由本函数统一收口。
 *
 * ★ `KeyH` → `H`、`ArrowUp` → `↑`：`Code` 名是给解析器看的，
 *   直接显示会变成一串"KeyH"这种用户看不懂的东西。
 */
export function hotkeyLabel(spec: string | undefined, fallback = ""): string {
  if (!spec) return fallback;
  const named: Record<string, string> = {
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    Space: "空格",
  };
  return spec
    .split("+")
    // `KeyS` / `Digit1` 这种 Code 名剥掉前缀
    .map((p) => p.replace(/^Key/, "").replace(/^Digit/, ""))
    .map((p) => named[p] ?? p)
    .join("+");
}

// ---------------------------------------------------------------------------
// 稿件库
// ---------------------------------------------------------------------------

export type ScriptSource = "file" | "pasted";

/**
 * 历史列表里的一条。
 *
 * `chars` 是**载入当时**的快照 —— 文件类稿件不存正文副本，
 * 用户改了原文件之后这个数字会过期。列表里因此只在载入后显示字符数。
 */
export interface ScriptEntry {
  id: string;
  source: ScriptSource;
  path: string | null;
  title: string;
  chars: number;
  last_used: number;
  use_count: number;
  copy_rel: string | null;
}

/** "3 分钟前" 这种相对时间。历史列表要的是"多久之前"，不是精确时间戳。 */
export function relTime(unixSec: number, now: number = Date.now() / 1000): string {
  const d = Math.max(0, now - unixSec);
  if (d < 60) return "刚刚";
  if (d < 3600) return `${Math.floor(d / 60)} 分钟前`;
  if (d < 86400) return `${Math.floor(d / 3600)} 小时前`;
  if (d < 86400 * 7) return `${Math.floor(d / 86400)} 天前`;
  const dt = new Date(unixSec * 1000);
  return `${dt.getFullYear()}-${String(dt.getMonth() + 1).padStart(2, "0")}-${String(
    dt.getDate(),
  ).padStart(2, "0")}`;
}

// ---------------------------------------------------------------------------
// 窗口
// ---------------------------------------------------------------------------

export interface WindowPrefs {
  width: number;
  height: number;
  x: number | null;
  y: number | null;
  /** 位置锁定（防会议中误拖）。 */
  locked: boolean;
}

/** 拖放悬停期间后端推来的提示，用来画覆盖层。 */
export interface DragHint {
  kind: "enter" | "leave";
  names: string[];
  /** 拖入的东西看起来能不能载入 —— 只决定覆盖层的颜色/文案。 */
  ok: boolean;
}

export type ScriptAlign = "left" | "center" | "right";
export type UiTheme = "dark" | "light" | "system";
/** 背景磨砂档位：关 / 柔光（CSS 玻璃）/ 系统磨砂（Acrylic · hudWindow）。 */
export type GlassMode = "off" | "css" | "system";

export interface CloudConfig {
  endpoint: string;
  api_key: string;
  model: string;
}

export interface AsrConfig {
  engine: EngineKind;
  local_model: ModelPaths | null;
  cloud: CloudConfig | null;
  num_threads: number;
}

export interface AppConfig {
  asr: AsrConfig;
  stealth_on_start: boolean;
  font_size: number;
  opacity: number;
  line_height: number;
  mirror: boolean;
  dim_read: boolean;
  hotkeys: HotkeyConfig;
  last_script: string | null;

  window: WindowPrefs;

  /** 正文左右内边距（px）。控制每行字数。 */
  padding_x: number;
  /** 字距（em）。 */
  letter_spacing: number;
  /** 当前句在视口里的垂直锚点 0.15~0.60。之前是硬编码 0.36。 */
  anchor: number;
  align: ScriptAlign;
  theme: UiTheme;
  /**
   * 背景磨砂档位（第十二轮，前身是 `glass: boolean`）：
   * `off`＝实色底；`css`＝柔光（栏的 backdrop-filter，默认）；
   * `system`＝系统材质（Windows Acrylic / macOS hudWindow），磨的是窗口后面。
   */
  glass_mode: GlassMode;

  /** 鼠标穿透。默认关 —— 与 Snipaste 一致。 */
  click_through: boolean;

  /**
   * 控制栏形态。
   *
   * `true`（默认）＝钉住：控制栏浮在正文**之上**，正文从它下方滚过去。
   * `false`＝占独立一行：正文从控制栏下面开始，两者永不重叠。
   * 两种都不会让控制栏随正文滚走，差别只在**占不占地方**。
   */
  bar_pinned: boolean;

  /** 跟读灵敏度：对位引擎的匹配下限 0.20~0.60。 */
  match_floor: number;

  /**
   * 跳读范围（句）：跟读时允许自动向前/向后跳多远去找你的声音。
   *
   * 出厂 20 句 —— 换算成 240 字，**与引擎此前的硬编码窗口逐字相同**，
   * 所以这个字段接出来不改变任何出厂手感。
   */
  jump_range: number;

  /** 跟读颜色：`lib/readColors.ts` 里的色板 key（如 `"green"`）。 */
  read_color: string;

  /** 平滑滚动。关掉＝跳句时瞬间到位，眼睛不用追动画。 */
  smooth_scroll: boolean;

  /** 快捷跳读：单击正文即把进度接过去。关＝改成双击才跳（防误触）。 */
  quick_jump: boolean;

  /** 参考语速（字/分钟），用来把剩余字数换算成剩余时间。 */
  wpm_ref: number;

  /**
   * 匀速滚动的速度（字/分钟）。
   *
   * 与 `wpm_ref` 分开存，尽管出厂值相同：`wpm_ref` 回答"我大概讲多快"
   * （把剩余字数换算成时间），这个回答"这一场我要稿子以多快往下走"。
   * 开讲中一边听一边微调的是后者，而调它不该把"还剩几分钟"的
   * 估算口径一起改掉。
   */
  auto_wpm: number;

  /**
   * 窗口置顶（永远盖在别的窗口之上）。
   *
   * ★ 它由**后端**写（`set_always_on_top` 同时改窗口属性和这一项），
   *   前端只读。曾经它是纯运行时状态、不落配置，结果每次开机都要重新点一次
   *   —— 用户对这件事的说法只有一个：「设置不记住」。
   */
  always_on_top: boolean;
}

/** 会前自检的一行结论。 */
export interface PreflightRow {
  key: string;
  verdict: string;
  /** 三档，**每一档界面上都有确定的显示**；不让前端猜。 */
  level: "ok" | "warn" | "bad";
  /** 出问题时具体怎么办。ok 时为空串。 */
  hint: string;
}

export interface BootstrapPayload {
  config: AppConfig;
  script: ScriptPayload;
  cursor: CursorPayload;
  platform: PlatformInfo;
  stealth: StealthReport | null;
  models_dir: string;
  found_models: ModelPaths[];
  asr_engine: string | null;
  app_version: string;
  /**
   * 可载入的稿件扩展名（不含点），由后端给出。
   *
   * **不要在前端再写一份**。曾经前端 dialog filter 里硬编码 `["txt","md"]`，
   * 后端加了新格式之后，表现是"拖进去能载入、但从「打开稿件」里选不到"。
   * 现在两边都从这一个数组取。
   */
  script_exts: string[];
  /** 上面那份清单给用户看的样子。 */
  script_exts_hint: string;
}

/** 平台能力等级转成给用户看的一句话。 */
export function supportText(level: SupportLevel): string {
  switch (level.kind) {
    case "native":
      return "原生支持";
    case "fragile":
      return "受限支持";
    case "unavailable":
      return "不可用";
  }
}

export function supportNote(level: SupportLevel): string | null {
  return level.kind === "native" ? null : level.note;
}
