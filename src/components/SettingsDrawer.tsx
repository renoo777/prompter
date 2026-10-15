import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { api, errText } from "../lib/api";
import { useToggleCollapsed } from "../lib/collapse";
import { stopPrompting, togglePrompting } from "../lib/prompting";
import { READ_COLORS, effectiveReadColor, readColorRgb } from "../lib/readColors";
import { useSendToTaskbar } from "../lib/sendToTaskbar";
import { resolveTheme, useApp } from "../lib/store";
import { checkForUpdate, installPendingUpdate } from "../lib/update";
import {
  micVerdictText,
  supportNote,
  toDbfs,
  type AppConfig,
  type EngineKind,
  type GlassMode,
  type MicProbe,
  type PreflightRow,
  type ScriptAlign,
  type UiTheme,
} from "../types";

// ---------------------------------------------------------------------------
// 结构件：Apple 的 inset grouped list
// ---------------------------------------------------------------------------

/**
 * 一个分组卡片。
 *
 * 为什么不用 `<div class="sect"><h4>` 那套：Apple 的分组**是一个卡片**，
 * 页边距把卡片和页面底色分开，卡片内部靠"同色系不同明度"分层而不是靠描边。
 * 描边一多，在 560px 宽的小窗口里整屏都是线。
 *
 * # 说明文字为什么收进 `tip`
 *
 * 之前每组底部都压着一两行长文（讲原理、讲取舍）。问题是：**每次打开设置
 * 都会重读一遍不需要的信息** —— 常用的那几行被推到了滚动区外面。
 * 现在只有"看之前得先知道"的话才留，其余收进标题旁的感叹号，
 * 想看的人鼠标一移就有，不想看的人永远不占他一行。
 *
 * # `collapsible`（本轮新加）
 *
 * 「更多」那一组里装的是**装完机器就不用再碰**的东西（引擎与模型、迁移、
 * 诊断、校对级排版微调）。它们必须还在（换电脑、换模型时要用），
 * 但不该和"字号""跟读灵敏度"占同样的出场顺序。
 * 折叠态的默认值是**收起来** —— 因为它装的全是低频项；
 * 而前四组默认展开，因为那里每一行都可能在一场直播里被碰到。
 */
/**
 * 一个分组。
 *
 * ★★ 2026-09-28：分组标题上的 `tip` **整个去掉了**。
 *
 *    原来六个分组标题（提词 / 跟读 / 稿件 / 外观 / 窗口 / 更多）各自挂着
 *    一段 3~5 行的原理说明，鼠标扫过去就弹一大块。用户的原话是
 *    「特别特别 low，而且交互体验会特别差」。
 *
 *    去掉的**依据**是一条更基本的原则：**分组标题不是可交互元素**。
 *    它没有点击行为（除了「更多」那个折叠开关），却被做成了悬停热区 ——
 *    于是用户每次想去点下面那一行，路过的第一站就是一个弹窗。
 *    ★ 那些说明文字本身没有丢：真正需要它的取值理由（比如"深色是刻意默认"）
 *      已经写在 `sub`（常驻可见）或各行自己的 `tip` 里；
 *      剩下的"这一组为什么长这样"属于**给开发看的**，不是给用户看的 ——
 *      它现在在这个文件的注释里。
 */
function Group({
  title,
  collapsible,
  children,
}: {
  title: string;
  /** 默认展开；传 `true` 则默认收起（用于装低频项的「更多」）。 */
  collapsible?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(!collapsible);
  return (
    <section className="grp">
      <div
        className={`grp-h${collapsible ? " foldable" : ""}`}
        onClick={collapsible ? () => setOpen((v) => !v) : undefined}
      >
        {collapsible && <span className={`grp-caret${open ? " open" : ""}`}>›</span>}
        <span>{title}</span>
      </div>
      {open && <div className="grp-box">{children}</div>}
    </section>
  );
}

/**
 * 一行：左标签 ＋ 右控件。整行可点时给按压反馈。
 *
 * ★★★ 2026-09-28：`sub`（行下方那行小字）这个 prop **从组件上整个拆掉了**，
 *    不是逐个调用点传 `undefined`。两次改动的区别是决定性的：
 *    留 prop 的话下次谁都能再加一行小字回来，而"这页不该有小字"这条规则
 *    只存在于注释里 —— 按本项目的判据（能靠读注释记住的规则一律不成立），
 *    那等于没有约束。拆掉 prop 之后 `tsc` 才是那个真正的守卫。
 *
 *    用户的原话：「功能介绍小字太多了，全部都删除，一个不要留。
 *    在更多里面加一个详细的各个功能介绍就可以。」
 *    —— 注意后半句：**信息没有消失，是换了一个地方**（收进「更多」里的
 *    功能介绍区）。判断依据是「这一页每行的垂直空间最贵」：设置面板是
 *    边讲边改的地方，每行压一行小字，能被看到的开关数就少一行。
 *
 *    代价里有一条必须认下来：**有几行原来靠小字说的不是"功能介绍"，
 *    是状态**（提词中/已暂停/未开始、自检进行中、识别是否在跑）。
 *    状态按本项目的判据是「运行事实要就地常驻」，这一版按用户的
 *    明确要求一并去掉了，替代出口写在下面各处注释里 —— 谁负责说、
 *    说不出口时用户会去哪找，都已经定好，不留给下一个人临场决定。
 */
function Row({
  label,
  right,
  onClick,
  children,
}: {
  label?: ReactNode;
  right?: ReactNode;
  onClick?: () => void;
  children?: ReactNode;
}) {
  const cls = `grp-row${onClick ? " tap" : ""}`;
  if (onClick) {
    return (
      <div className={cls} onClick={onClick}>
        <div className="grp-l">
          <span className="grp-t">{label}</span>
        </div>
        <div className="grp-r">{right ?? children}</div>
      </div>
    );
  }
  return (
    <div className={cls}>
      {label !== undefined && (
        <div className="grp-l">
          <span className="grp-t">{label}</span>
        </div>
      )}
      <div className="grp-r">{right ?? children}</div>
    </div>
  );
}

/**
 * 滑杆行：标签与当前值同一行，滑杆占满下一行。
 *
 * 这是 Apple 的做法（在 iOS 设置里调字号/亮度都是这个形状）。
 * 好处是**滑杆有整行的行程** —— 塞在右侧那点宽度里根本拖不准，
 * 而这个地方用户是"边拖边看正文"的，拖不准等于白给。
 */
function SliderRow({
  label,
  value,
  min,
  max,
  step,
  fmt,
  tone,
  onChange,
}: {
  label: ReactNode;
  value: number;
  min: number;
  max: number;
  step: number;
  fmt: (v: number) => string;
  /** `"read"` ＝ 轨道画成跟读色（绿）。默认蓝（accent）。 */
  tone?: "read";
  onChange: (v: number) => void;
}) {
  return (
    <div className={`grp-row col${tone === "read" ? " read-tone" : ""}`}>
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
          gap: 10,
        }}
      >
        <span className="grp-t">{label}</span>
        <span className="grp-v">{fmt(value)}</span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </div>
  );
}

/**
 * 功能介绍里的一组小节（如「提词」「窗口」）。
 *
 * ★ 它**不是** `Group`：那个组件会渲染成一张带底色的卡片，而功能介绍
 *   是卡片**里面**的内容 —— 套一层会多出一道卡片边和一次折叠箭头，
 *   变成"折叠里套折叠"。这里要的只是"一段小标题 + 一串条目"。
 */
function GuideGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="guide-sec">
      <div className="guide-h">{title}</div>
      <div className="guide-items">{children}</div>
    </div>
  );
}

/**
 * 一条介绍：**术语**加粗，说明文字跟在后面。
 *
 * ★ 为什么不复用 `Row`：那一行是"标签 + 控件"的横排结构，
 *   控件在右、文字单行省略；功能介绍要的是"标签 + 段落"的纵排，
 *   而且说明会长到两三行，省略号会把最要紧的后半句吃掉
 *   （"…别只依赖其中一个"这种话被截掉就等于没说）。
 *
 * ★ 术语与说明之间的间距**不在这里给**（`column-gap` 与 JSX 里的
 *   `{" "}` 都实测过、都无效），而是走 CSS 里 `.guide-k::after`。
 *   理由与踩坑记录见 `styles.css` 里那条注释。
 */
function GuideItem({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="guide-item">
      <span className="guide-k">{k}</span>
      <span className="guide-v">{children}</span>
    </div>
  );
}

/**
 * 跟读颜色：一行色点，当前那一枚画成**空心环**。
 *
 * 为什么不是下拉或一排文字：颜色这东西**说出来没有用**。
 * "青绿"和"蓝绿"的区别只有在色块上才一眼可信，让人选一个 `#2DD4BF`
 * 之类的名字只会多一次猜错的机会。
 *
 * 色板的色值归 `lib/readColors.ts`（纯视觉，不该进后端）；
 * 这里只负责把选中的那个标出来。若配置里的 key 已失效（老配置、手改过
 * config.json），`effectiveReadColor` 会收敛到出厂色 —— 所以永远**有一枚**
 * 是亮着的，不会出现"一个都没选中"。
 *
 * ★ 按主题取色：色点要显示的是**正文里真会出现的那个颜色**。
 *   若色点固定用深色版，浅色主题下用户挑了一个"看着很清楚"的色，
 *   回到正文却是一条几乎看不见的浅线 —— 那正是设置页最该避免的误导。
 */
function ColorDots({
  value,
  theme,
  onPick,
}: {
  value: string;
  theme: "dark" | "light";
  onPick: (key: string) => void;
}) {
  const cur = effectiveReadColor(value);
  return (
    <div className="dots">
      {READ_COLORS.map((c) => (
        <ColorDot
          key={c.key}
          name={c.name}
          on={c.key === cur}
          color={`rgb(${readColorRgb(c.key, theme)})`}
          onPick={() => onPick(c.key)}
        />
      ))}
    </div>
  );
}

/**
 * 一枚色点。
 *
 * ★ 单成组件（第十一轮改写）：原来是因为 `useHint` 不能在 `.map()` 里调。
 *   气泡删掉之后那个约束没了，但 `READ_COLORS.map()` 里内联一个带
 *   `aria-label` / `aria-pressed` 的按钮会让那段 JSX 变长，包着更清楚。
 *
 * ★ 颜色名现在只剩 `aria-label`（**无障碍**通道，不受"删提示"约束）。
 *   ⇒ 眼睛看不见颜色名了，但**选中的那一枚画成空心环**（见 `ColorDots`
 *   的注释），"现在用的是哪个"这件事实由形状承载，不靠文字。
 */
function ColorDot({
  name,
  on,
  color,
  onPick,
}: {
  name: string;
  on: boolean;
  color: string;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      className={`dot${on ? " on" : ""}`}
      style={{ color }}
      aria-label={`跟读颜色：${name}`}
      aria-pressed={on}
      onClick={onPick}
    />
  );
}

/**
 * 「推进方式」—— **谁决定文稿往下走**。
 *
 * 配置里是四个值（`EngineKind`），界面上是**三个答案**：语音跟随（含本地/云端）、
 * 匀速滚动、纯手动。合成三选一而不是"自动滚动开关 + 一个叫引擎的下拉框"，
 * 是为了不给出「自动滚动＝开，引擎＝本地离线」这种自相矛盾的组合 ——
 * 那样用户没有任何依据判断该听谁的。
 */
type DriveMode = "voice" | "auto" | "manual";

const DRIVE_MODES: { key: DriveMode; label: string }[] = [
  { key: "voice", label: "语音跟随" },
  { key: "auto", label: "匀速滚动" },
  { key: "manual", label: "纯手动" },
];

/** `EngineKind`（四个值）→ 界面上的三选一。 */
function driveOf(kind: EngineKind): DriveMode {
  if (kind === "auto") return "auto";
  if (kind === "manual") return "manual";
  return "voice";
}

/**
 * 上一次用的**语音来源**（`local` / `cloud`）。
 *
 * 为什么要记：配置里只有一个 `engine` 字段，切到「匀速滚动」时它被覆盖成
 * `auto`，"之前用的是本地还是云端"就没了 —— 切回「语音跟随」只能落到一个
 * 默认值上，而用户记得自己刚才选的不是它。
 *
 * 放在模块变量里（不落配置）是权衡后的选择：
 *   * 为它单开一个配置字段，会给老配置多一次迁移，而那次迁移没有好答案
 *     （老配置里 `engine = "cloud"` 的用户，`voice_source` 该取什么？）；
 *   * 它只影响"切回来的落点"，重启后回到 `local` 没有实际损失 ——
 *     云端识别目前尚未接入（`CloudAsr` 是占位实现）。
 */
let lastVoiceSource: EngineKind = "local";

/**
 * iOS 式开关。
 *
 * ★ 动画全在 `.switch` 的样式里（位移 + 按下去抻开，都是 `transform`）——
 *   所以这里**不要**再加行内样式或额外的 class：一条 `width` 之类会触发重排的
 *   属性就能把"抻开"这件事从合成层拽回布局层，动画立刻掉帧。
 *
 * ★ `role="switch" + aria-checked` 不是装饰：它是一个"状态"控件（不是一个动作），
 *   而状态必须能被读屏念出来。缺了它，读屏用户听到的只是一个没有名字的按钮。
 */
function Switch({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      className={`switch${on ? " on" : ""}`}
      onClick={onClick}
    />
  );
}

// ---------------------------------------------------------------------------

/**
 * 一键预设。
 *
 * 存在的理由很实际：会议开始前 30 秒没人愿意去拖五个滑杆。
 * 预设给一组**经过搭配**的值（字大则行高与边距也要跟着变，单独调一项容易调歪），
 * 需要微调再动下面的单项。
 */
const PRESETS: { key: string; label: string; patch: Partial<AppConfig> }[] = [
  {
    key: "small",
    label: "小",
    patch: { font_size: 26, line_height: 1.6, letter_spacing: 0.005, padding_x: 22 },
  },
  {
    key: "medium",
    label: "中",
    patch: { font_size: 34, line_height: 1.7, letter_spacing: 0.01, padding_x: 28 },
  },
  {
    key: "large",
    label: "大",
    patch: { font_size: 44, line_height: 1.85, letter_spacing: 0.02, padding_x: 38 },
  },
];

/** 主题三态。亮暗快切收进这里之后，`system` 也只有这儿能选（不做两态快切）。 */
const THEMES: [UiTheme, string][] = [
  ["dark", "深色"],
  ["light", "浅色"],
  ["system", "系统"],
];

/**
 * 「支持作者」的收款信息 —— **唯一出处**（改码、改备注只动这里）。
 *
 * # 收款人备注为什么是**常驻文字**，不是悬停提示
 *
 * 本项目的硬规则是**界面零悬停提示**（守它的门槛见 `scripts/check-tooltips.py`
 * 与 `check-hover-silent.py`）。而"付款前核对收款人"本身是**防错付**的运行时
 * 信息 —— 按「信号只换位置、不消失」的判据，它只能改成看得见的文字。
 *
 * # 为什么**没有**「保存原图」按钮（参考实现 Flow Navigator 那边有一个）
 *
 * Tauri 的 webview 没有接下载处理器，`<a download>` 会**静默什么都不做** ——
 * 用户按了没反应，比根本没有这个按钮更坏。而"手机对着屏幕扫"本来就不需要它；
 * 真正需要交代的是**反过来的那一句**（截图后再扫可能识别不了），
 * 所以它写进了下面展开区的正文里。
 *
 * # 为什么整块默认收起
 *
 * 二维码从不首屏平铺，这是主流软件的惯例（IINA / Motrix / Obsidian 都把
 * 打赏收在关于页）；更要紧的是本项目那条判据：**设置面板每一行的垂直空间
 * 都很贵** —— 一个多数人不会点开的东西，不配占首屏。
 */
const DONATE = {
  /** 微信赞赏码的收款人。付款时核对，对不上就别付。 */
  weixinNickname: "renoo",
  /** 支付宝收款码的收款人。 */
  alipayNickname: "周先生的小铺(*明)",
  /** 海外打赏通道（需要 Visa/MasterCard）。 */
  sponsorUrl: "https://github.com/sponsors/renoo777",
  /**
   * 码图放 `public/donate/`，vite 原样拷进 `dist/donate/`。
   * ★ 路径必须写**相对**的 `./donate/...`：Tauri 把 `dist/` 挂在自定义协议
   *   的根上（`frontendDist: "../dist"`），相对路径在开发服务器上同样成立。
   * ★ 这两张图**自带白底与静区**，所以直接渲染即可 —— 不要给它套深色底，
   *   窗口外壳是半透明的（`--win-alpha`），底色一透过来就扫不出来了。
   */
  weixinQr: "./donate/weixin.jpg",
  alipayQr: "./donate/alipay.jpg",
} as const;

/**
 * 意见反馈表的地址 —— **唯一出处**（换表单只动这里）。
 *
 * # 为什么是飞书表单，而不是"发我邮箱"或"去 GitHub 提 issue"
 *
 * 用这个工具的人多半不是开发者。让他为了说一句"字幕滚得有点快"去注册
 * GitHub 账号、学 Markdown、自己挑 label，等于**把提意见的门槛抬得比问题本身还高**
 * —— 那样收到的反馈会少到没有参考价值。飞书表单点开就能填，不要账号。
 *
 * # 为什么是"按钮跳浏览器"，不是"显示二维码"
 *
 * 提意见是**在电脑上打字**的事（要说清"哪一步不对"得写几句），不是扫码付款。
 * 手机扫码之后还得在手机上把问题重新描述一遍，比直接在电脑上填更麻烦。
 * 而且二维码要占一块常驻版面，这个入口只值一行。
 *
 * # ★ 这是**分享直链**，不是我们自己的域名
 *
 * 它挂在飞书的 `share/base/webpage/` 下，靠一串分享 id 定位。
 * ⇒ 换表单、或改这张表的分享设置时，**地址是会变的**（它不像
 * `github.com/<owner>/<repo>` 那样稳定）。变了就改这一处 ——
 * 所以它必须只有这一处，不能在组件里就地再写一遍字符串。
 */
const FEEDBACK = {
  formUrl:
    "https://qiangqiangkeji.feishu.cn/share/base/webpage/shrcnE4mhUTHPxJ9hoCSttGlUDc",
} as const;

export function SettingsDrawer() {
  const { state, dispatch } = useApp();
  const {
    config,
    script,
    platform,
    stealth,
    foundModels,
    modelsDir,
    notice,
    diag,
    asrStatus,
    busy,
    updateBusy,
    settingsOpen,
    scriptExts,
  } = state;

  /*
   * 原来这里有一行 `hkClickThrough`，只为在「鼠标穿透」那行的小字里
   * 写出「忘关从托盘或 Ctrl+Alt+P 关」。小字删掉后它没有消费者，随之删除。
   *
   * ★ 穿透的**退出口一个都没少**（判据级的能力：关掉输入通道必须配
   *   不依赖该通道的退出口）：热键仍在后端 `lib.rs` 注册全局快捷键，
   *   托盘菜单里也仍有那一项，设置里这颗开关本身还在。
   *   变的只是「界面不再把快捷键串抄一遍」—— 按 `types.ts` 里
   *   `hotkeyLabel` 的判据，那本来也是**唯一来源**问题：
   *   界面写死字符串会在用户改键后说假话。
   */

  const [testText, setTestText] = useState("");
  /**
   * 「功能介绍」那块长文的展开态。**默认收起** ——
   * 二十多条展开来会把「更多」里真正的设置项全推出视野。
   */
  const [guideOpen, setGuideOpen] = useState(false);
  const [probe, setProbe] = useState<MicProbe | null>(null);
  const [probing, setProbing] = useState(false);
  const [pf, setPf] = useState<PreflightRow[] | null>(null);
  const [pfBusy, setPfBusy] = useState(false);
  const [promptBusy, setPromptBusy] = useState(false);
  /** 当前是否全屏 —— 只为了让这一行说得出**现在**是哪种形态。 */
  const [fullscreen, setFullscreen] = useState(false);
  /**
   * 「检查更新」按钮正在等后端。
   *
   * ★ 为什么要一个本地标志、而不复用 `state.updateBusy`：
   *   那个表示的是**正在装**（会替换整个应用），这个表示的是**正在查**。
   *   两者都能让按钮变灰，但说出口的话完全不同 ——
   *   "正在检查…"和"正在下载更新…"混成一句，用户就分不清
   *   "它在忙什么、我要不要等"。
   */
  const [updateChecking, setUpdateChecking] = useState(false);

  /**
   * 置顶状态。
   *
   * ★ 它**既是配置项也是窗口的运行时属性**，而权威的那一份是窗口 ——
   *   所以这里读回来、不从 config 推。有两处会让它变化：这个开关自己，
   *   以及「开始提词 / 收摊」（前者顺带置顶、后者撤置顶）。所以除了挂载时
   *   读一次，`demoActive` 每次变化也要重读 —— 否则收摊之后这个开关还亮着，
   *   它在说假话。
   *
   * ★ 这个开关原先长在顶栏上（第五颗图标）。本轮把顶栏压成一行之后它搬到这里：
   *   它是"摆窗口"时的一次性动作，不该和"每场会都要确认一眼"的隐身灯
   *   抢同一行的宽度。**搬走的同时核过：它没有唯一的入口** ——
   *   「开始提词」本来就会自动置顶，而"收摊"会自动撤掉。
   *
   * ★ 它现在**会**落进配置（`always_on_top`）：改版前它是纯运行时状态，
   *   于是每次开机都得重新点一次 —— 用户对这件事只有一个说法：「设置不记住」。
   */
  /**
   * ★ 原先这里有一个 `trayHintShownRef`（"首次提示给过没有"），随
   *   `sendToTaskbar` 一起搬进了 `lib/sendToTaskbar`。
   *   搬走的必要性不只是去重：**两个入口共用一份提示状态**才对 ——
   *   否则"在设置里收过一次"之后，顶栏那颗按钮又会重新提示一遍。
   */

  const [pinned, setPinned] = useState(false);
  useEffect(() => {
    let alive = true;
    getCurrentWindow()
      .isAlwaysOnTop()
      .then((v) => alive && setPinned(v))
      .catch(() => {
        /* 读不到就当没置顶 —— 那是默认值，也是它的初始状态 */
      });
    return () => {
      alive = false;
    };
  }, [state.demoActive, state.demoPaused]);

  /*
   * 全屏是**窗口的运行时属性**，不是配置项 —— 所以只能读回来。
   * 订阅 `onResized` 是因为 Esc 也能退出全屏，那条路不经过这个组件。
   */
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const sync = () => {
      void win
        .isFullscreen()
        .then((v) => alive && setFullscreen(v))
        .catch(() => {});
    };
    sync();
    const un = win.onResized(sync);
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, [settingsOpen]);

  /**
   * ★★ 这两个 hook **必须**在所有提前返回之前调用（React 规则 #310）。
   *
   * 本轮踩过：它们原先写在 `if (!config) return null;` **之后**，
   * 于是首帧 `config` 还是空 → 提前返回、这两个 hook 一次都没执行；
   * 等配置到位再渲染时 → 突然多出两个 hook ⇒ React 抛
   *   `Minified React error #310`
   * 整个界面**白屏**（本机表现为一片红 = 项目自己的错误边界底色）。
   *
   * ★★ 教训：往一个组件里加 hook 时，**先看它有没有提前返回** ——
   *   "hook 不能写在 return 之后"这条不只是风格问题，
   *   违反了是**整页白屏**，而报错信息（#310）在压缩产物里完全看不出是 hook 的问题。
   */
  const toggleCollapsed = useToggleCollapsed();
  const sendToTaskbar = useSendToTaskbar();
  if (!config) return null;

  function patch(p: Partial<AppConfig>) {
    dispatch({ type: "CONFIG", config: { ...config!, ...p } });
  }

  function patchAsr(p: Partial<AppConfig["asr"]>) {
    patch({ asr: { ...config!.asr, ...p } });
  }

  const drive = driveOf(config.asr.engine);

  /**
   * 切「推进方式」。
   *
   * ★ 只改配置，**不重启正在跑的引擎**。与下面「语音来源」同一处置：
   *   换推进方式必然要停掉一路、另起一路（放麦克风 / 起时钟），
   *   开讲中途悄悄重启，用户看到的会是跟读"停了一下又接上"，
   *   而原因只写在设置里一个不显眼的地方。
   *   ⇒ 下一次按大圆按钮时生效，这句话写在那一行的副标题里。
   */
  function setDrive(mode: DriveMode) {
    if (mode === "auto") {
      patchAsr({ engine: "auto" });
    } else if (mode === "manual") {
      patchAsr({ engine: "manual" });
    } else {
      // 回到**上一次用的**语音来源，而不是写死 `local` ——
      // 否则用过云端的人切走再切回来，会发现自己的选择被改掉了。
      patchAsr({ engine: lastVoiceSource });
    }
  }

  function fail(e: unknown) {
    dispatch({ type: "ERROR", message: errText(e) });
  }

  /**
   * 全屏提词 —— 从常驻栏搬来这里（低频，而且它会盖住 PPT，不该放在手边）。
   *
   * ★ 它与产品的主场景（浮在 PPT 上的提词层）是**互斥**的：全屏会盖住 PPT。
   *   所以它的定位是「外接显示器 / 平板 / 分光镜」那种**单屏独占**场合。
   *   正因为它与主场景冲突、又是一场最多用一次，才不配占顶栏上的位置
   *   —— 那两行要留给每场都用几十次的东西。
   */
  async function toggleFullscreen() {
    try {
      const win = getCurrentWindow();
      const next = !(await win.isFullscreen());
      await win.setFullscreen(next);
      setFullscreen(next);
      // 不弹提示：窗口有没有变全屏，是最不需要用文字复述的一件事。
      // 而"全屏会盖住 PPT"这类**风险说明**留在按钮的悬停里 —— 那儿才是它该在的地方。
    } catch (e) {
      fail(e);
    }
  }

  /**
   * 收起成一条栏 / 收进任务栏 —— 两个 hook 都已在**上方**（提前返回之前）调过，
   * 这里只留说明：
   *
   * ★★ `toggleCollapsed` **必须**走 `lib/collapse` 而不是直接 `api.setCollapsed()`：
   *   收起是**两拍**动画（正文先滑走，再压窗口），直接调命令会跳掉第一拍，
   *   用户看到的是正文被生硬裁掉。本轮这颗按钮从顶栏搬进来之后，
   *   它成了设置里唯一的收起入口 —— 手感必须和原来一模一样。
   *
   * ★★ `sendToTaskbar` 与顶栏那颗共用一份实现。这里原先调的是
   *   `getCurrentWindow().hide()`（按钮文案叫「收进托盘」）；改掉它是因为
   *   那条提示在**说假话** —— 它写着"点托盘图标"，而 Windows 11 默认把新图标
   *   折进 `^` 溢出区（实测本机 `IsPromoted=0`），等于指着用户看不见的东西
   *   让他去点。用户的原话就是「没有办法收起到这里（任务栏）」。
   *   现在两个入口（顶栏 · 这里）共用同一份：提示文案、首次提示只给一次、
   *   失败不静默 —— 这些判断分散成两份迟早走偏。
   */

  /**
   * 置顶开关。
   *
   * ★ 它**同时**是配置项（落 config，下次启动自动置顶）和窗口的运行时属性，
   *   所以两件事必须在一次调用里做完 —— 走后端 `set_always_on_top`。
   *   只调窗口 API 的写法（改版前就是这样）等于每次都忘掉，用户每次开机
   *   都要重新点一次；而只 patch 配置不动窗口，则是"设置说置顶、窗口没置顶"。
   *
   * ★ 失败必须说出来 —— 静默失败会让用户以为置顶了，讲的时候窗口被别的窗口压住。
   * ★ 开关显示的是**窗口实际的状态**（`pinned` 读自窗口），不是"用户点了什么"。
   */
  async function togglePin() {
    const next = !pinned;
    try {
      setPinned(await api.setAlwaysOnTop(next));
    } catch (e) {
      fail(e);
    }
  }

  // ------------------------------------------------------------------ 稿件

  async function pickScript() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        // 扩展名清单**从后端来**。以前这里硬编码 ["txt","md","markdown"]，
        // 后端支持了 .docx 之后就会出现"拖进来能载入、这里选不到"。
        filters: [{ name: "稿件", extensions: scriptExts }],
      });
      if (typeof picked !== "string") return;
      const s = await api.loadScript(picked);
      dispatch({ type: "SCRIPT", script: s });
      dispatch({ type: "NOTICE", message: `已载入 ${s.total_chars} 个可定位字` });
    } catch (e) {
      fail(e);
    }
  }

  // ------------------------------------------------------------------ 语音

  async function toggleAsr() {
    dispatch({ type: "BUSY", busy: true });
    dispatch({ type: "ERROR", message: null });
    try {
      const s = asrStatus?.running ? await api.stopAsr() : await api.startAsr();
      dispatch({ type: "ASR_STATUS", status: s });
      dispatch({ type: "NOTICE", message: s.detail });
    } catch (e) {
      fail(e);
    } finally {
      dispatch({ type: "BUSY", busy: false });
    }
  }

  /**
   * 麦克风自检。
   *
   * 解决的问题是「点了开始识别，光标一动不动，而程序每处都没报错」——
   * 那多半是设备在送数字静音。这种情况只看识别文本永远查不出来，
   * 因为"设备死了"和"用户没说话"在文本层面完全一样。
   */
  async function runMicCheck() {
    setProbing(true);
    setProbe(null);
    dispatch({ type: "ERROR", message: null });
    try {
      const p = await api.micCheck(2.5);
      setProbe(p);
      if (p.verdict === "ok") {
        dispatch({ type: "WARNING", warning: null });
        dispatch({ type: "NOTICE", message: "麦克风自检通过：拾音正常" });
      } else {
        dispatch({
          type: "WARNING",
          warning: { label: "麦克风异常 · 点击自检", detail: p.detail },
        });
      }
    } catch (e) {
      fail(e);
    } finally {
      setProbing(false);
    }
  }

  // ------------------------------------------------------------------ 提词

  /**
   * 提词的编排全在 `lib/prompting.ts` —— 底部那个大圆按钮走的是同一条路径。
   *
   * ★ 本轮改版把这里的「开始识别」按钮和顶栏那个「▶ 演示」胶囊**合并**了：
   *   它们是同一个意图（"我要开始讲这段稿子"）的两个入口，用户分不清差别，
   *   两边状态还会不同步。现在设置里只保留**另一件事** ——
   *   「结束提词」（收摊）。
   *
   * ★ 为什么「结束」留在这里而不是那个大圆按钮上：
   *   与隐身「彻底关掉放设置」同一条约定 —— **可逆的、高频的留在按钮上；
   *   不可逆的、低频的收进设置。** 圆按钮做成三态循环后，用户只想脱稿讲一段
   *   就绝不会误触到"把整个提词结束掉"。
   */
  async function togglePrompt() {
    dispatch({ type: "ERROR", message: null });
    await togglePrompting(state, dispatch, setPromptBusy);
  }

  async function stopPrompt() {
    dispatch({ type: "ERROR", message: null });
    await stopPrompting(state, dispatch, setPromptBusy);
  }

  async function runPreflight() {
    setPfBusy(true);
    setPf(null);
    dispatch({ type: "ERROR", message: null });
    try {
      setPf(await api.preflight());
    } catch (e) {
      fail(e);
    } finally {
      setPfBusy(false);
    }
  }

  async function exportBundle() {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const stamp = new Date().toISOString().slice(0, 10);
      const path = await save({
        defaultPath: `prompter-备份-${stamp}.json`,
        filters: [{ name: "Prompter 迁移包", extensions: ["json"] }],
      });
      if (typeof path !== "string") return;
      const bytes = await api.exportBundle(path);
      dispatch({
        type: "NOTICE",
        message: `已导出 ${(bytes / 1024).toFixed(1)} KB 到 ${path}`,
      });
    } catch (e) {
      fail(e);
    }
  }

  async function importBundle() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "Prompter 迁移包", extensions: ["json"] }],
      });
      if (typeof path !== "string") return;
      const cfg = await api.importBundle(path);
      // 不弹"已导入：配置与稿件历史都换成包里的了"。
      // 导入成功后主题、字号、稿件立刻换成包里那套 —— 界面本身就是回执，
      // 而且比一句话更全面（那句话只说了"换了"，没说换成什么样）。
      dispatch({ type: "CONFIG", config: cfg });
    } catch (e) {
      fail(e);
    }
  }

  async function runDiagnostics() {
    try {
      dispatch({ type: "DIAG", rows: await api.diagnostics() });
    } catch (e) {
      fail(e);
    }
  }

  const stealthOn = stealth?.mode === "hidden" && stealth.applied;
  /**
   * 「当前状态」那一行得说得出第三态。
   *
   * 少了它就会出现"设置里写着未隐身、顶栏却蓝着「已暂停」"这种自相矛盾 ——
   * 而"当前到底有没有被投屏看得到"恰恰是这个产品最不能含糊的一件事。
   */
  const stealthLabel =
    state.stealthIntent === "paused"
      ? "已暂停"
      : stealthOn
        ? "已隐身"
        : state.stealthIntent === "on"
          ? "未生效"
          : "未隐身";
  const chars = script?.total_chars ?? 0;
  /*
   * 原来这里还有三个派生量，本轮随全部小字一起删了：
   *   · `at` / `leftChars` / `leftMin` —— 只被「参考语速」那行的小字
   *     （"顶栏按它算出剩下 X 字约 Y 分钟"）使用，属于同一件事的三段。
   *     ★ 那句话其实**已经过期**：顶栏的剩余时间在第十一轮就摘掉了
   *       （见 `Transport.tsx` 顶部「剩余时间不再常显」），
   *       所以即便留着也是一句对不上界面的话。
   *   · `p`（大圆按钮的四态）—— 只被「当前」那行的小字使用。
   *     状态本身没消失：底部那颗大圆按钮的图标与底色就是它。
   */

  return (
    <aside className={`drawer${settingsOpen ? " open" : ""}`} data-interactive>
      <div className="drawer-head">
        <span style={{ flex: 1 }}>设置</span>
        <button
          className="icon-btn"
          onClick={() => dispatch({ type: "SETTINGS", open: false })}
        >
          ✕
        </button>
      </div>

      <div className="drawer-body">
        {notice && (
          <div className="banner ok" onClick={() => dispatch({ type: "NOTICE", message: null })}>
            {notice}
          </div>
        )}

        {platform && platform.support.kind !== "native" && (
          <div className="banner">
            {supportNote(platform.support)}
            {platform.fallback_hint && (
              <>
                <br />
                建议：{platform.fallback_hint}
              </>
            )}
          </div>
        )}

        {/* ============================================================ 提词
         *
         * 改版前这里是「开讲前」组，装着「会前自检」和「一键演示模式」。
         * 后者搬去了窗口底部那个大圆按钮 —— 它与设置里的「开始识别」是同一个意图，
         * 两个入口只会让用户分不清差别。这里留下的是圆按钮**装不下**的东西：
         * 结束，以及开讲前要确认的几项。
         */}
        <Group
          title="提词"
        >
          <Row
            label="当前"
            /*
             * ★ 原来这行挂着一句状态小字（正在就位 / 提词中：隐身·置顶·跟读 /
             *   已暂停 / 未开始·还没有稿件），本轮随全部小字一起去掉了。
             *
             *   **谁来说这些话**（信号不许消失，只换位置）：
             *   · 提词中 / 已暂停 —— 底部那颗大圆按钮。它是全界面唯一的实心
             *     圆形按钮，图标在播放/暂停之间切、底色也在变，本来就是
             *     "现在什么状态"的第一出口；顶栏的状态槽还会补一条文字。
             *   · 未开始 · 还没有稿件 —— 「开始」按钮在 `chars === 0` 时
             *     是**禁用**的（灰的、点不动），这个事实本身就说明了原因，
             *     不需要一句小字再重复一遍。
             *   · 隐身与置顶是否在生效 —— 「更多」里有一行「当前状态」，
             *     它把隐身 / 收起 / 穿透三件事并成一行常驻读数。
             */
            right={
              state.demoActive ? (
                <button className="btn" onClick={() => void stopPrompt()} disabled={promptBusy}>
                  结束
                </button>
              ) : (
                <button
                  className="btn primary"
                  onClick={() => void togglePrompt()}
                  disabled={promptBusy || chars === 0}
                >
                  开始
                </button>
              )
            }
          />

          <Row
            label="会前自检"
            /*
             * 原来 `pfBusy` 时会挂一句「正在检查（会真录 1.2 秒麦克风）…」，
             * 本轮随全部小字一起去掉了。
             *
             * ★ **可见反馈并没有消失**，改由下面那颗按钮承担：
             *   它在检查期间是 `disabled`（点不动、变灰），1.2 秒后
             *   `pf` 那一整块结论（圆点 + 判词 + 处理建议）才出现。
             *   也就是说"正在查"由**控件的禁用态**说、"查到了什么"由
             *   下面的结论块说 —— 两段都是常驻可见的，不依赖悬停。
             *   依据是本项目对静默失败的判据：宁可不写解释，也不能让
             *   用户面对一个"点了没反应"的界面。
             */
            right={
              <button className="btn" onClick={runPreflight} disabled={pfBusy}>
                检查
              </button>
            }
          />
          {pf && (
            <div className="grp-row col" style={{ gap: 10 }}>
              {pf.map((r) => (
                <div key={r.key} style={{ display: "flex", gap: 9, alignItems: "flex-start" }}>
                  <span className={`pf-dot ${r.level}`} />
                  <div style={{ minWidth: 0 }}>
                    <div style={{ fontSize: 12.5 }}>
                      <span style={{ color: "rgb(var(--fg-dim))" }}>{r.key}</span>
                      {" · "}
                      {r.verdict}
                    </div>
                    {r.hint && <div className="grp-s">{r.hint}</div>}
                  </div>
                </div>
              ))}
              {pf.some((r) => r.level === "bad") && (
                <div className="grp-s" style={{ color: "rgb(var(--err))" }}>
                  红点的项建议处理完再开讲。
                </div>
              )}
            </div>
          )}

          {/* ---- 推进方式：谁决定文稿往下走 ---- */}
          {/*
            「推进方式」＝稿子往下走由谁决定。三个选项的区别写在**选项名里**
            （语音跟随 / 匀速滚动 / 纯手动），改完在下一次按大圆按钮时生效。

            ★ 第十一轮删掉的是一句复述（"麦克风听到哪…"）+ 一个悬停气泡
              （把三个模式名又解释一遍）。分段控件本身就是答案 ——
              三选一不需要注释，选中的那个就是当前口径。
          */}
          <div className="grp-row col">
            <span className="grp-t">推进方式</span>
            <div className="seg fill">
              {DRIVE_MODES.map((m) => (
                <button
                  key={m.key}
                  type="button"
                  className={`seg-btn${drive === m.key ? " on" : ""}`}
                  onClick={() => setDrive(m.key)}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>

          {/*
            速度这一行在两个模式下是**同一个位置**，只是换了个生产者：
            匀速滚动时它是"稿子走多快"，其余模式是"我讲多快（用来估剩余时间）"。
            同一件事的两种口径各占一行，用户就会问"我调了 300，它为什么按 280 估时间"。
          */}
          {drive === "auto" ? (
            <SliderRow
              label="滚动速度"
              value={config.auto_wpm}
              /* 范围与后端 `asr::auto_scroll::WPM_RANGE` 一致（60~600）。
                 真越界了也由后端夹取 —— 它返回的是夹取后的值。 */
              min={60}
              max={600}
              step={10}
              fmt={(v) => `${v} 字/分`}
              onChange={(v) => {
                patch({ auto_wpm: v });
                // 立刻推给正在跑的引擎 —— 开讲中调速走的就是这里，拖完不必重开。
                // 当前不是匀速滚动时，后端那边是空操作（trait 的默认实现）。
                void api.setAutoWpm(v).catch(() => {});
              }}
            />
          ) : (
            <SliderRow
              label="参考语速"
              value={config.wpm_ref}
              min={180}
              max={420}
              step={5}
              fmt={(v) => `${v} 字/分`}
              /*
               * ★★ 本轮删掉的 sub 是「顶栏按它算出「剩下 X 字约 Y 分钟」」，
               *   删的过程中查出一件必须记下来的事：
               *
               *   **`wpm_ref` 在整个代码库里只剩这一个消费者。** 后端只存它、
               *   不读它；顶栏的剩余时间在第十一轮搬进顶栏时就已经被摘掉
               *   （见 `Transport.tsx` 顶部「剩余时间不再常显」那段），
               *   只留下一句注释说"挪进了进度条的悬停说明"，而进度条上
               *   并没有那个悬停说明 —— 那一版它就断了。
               *
               *   ⇒ 所以这根滑杆现在拖动**没有任何可见后果**。
               *   这不是本轮造成的，但本轮让它彻底无处可说了。
               *
               *   按「显式失败不静默失败」的判据，处理是：
               *   **先不删这根滑杆**（删它属于改产品范围，需要你拍板），
               *   但在「更多」的功能介绍里如实写明它当前的处境，
               *   并在此处留下判据 —— 哪天要把剩余时间重新显示出来，
               *   第一个该接的就是这里。
               */
              onChange={(v) => patch({ wpm_ref: v })}
            />
          )}
        </Group>

        {/* ============================================================ 跟读 */}
        <Group
          title="跟读"
        >
          <SliderRow
            tone="read"
            label="跳读范围"
            value={config.jump_range}
            min={1}
            max={40}
            step={1}
            fmt={(v) => String(v)}
            /* 原来有一句「只影响「语音跟随」」（非语音模式时才显示），
               随全部小字一起去掉了。口径写进「更多」的功能介绍。 */
            onChange={(v) => {
              // 先落配置（界面即时反映），再推给引擎 —— 与「跟读灵敏度」同一条纪律：
              // 只改配置不推引擎，用户拖完会发现"没变化"，那才是真的坑。
              // 返回值是**夹取后**的值，若被引擎改过就以引擎为准回写，
              // 免得滑杆显示 1、引擎其实用的是别的数。
              patch({ jump_range: v });
              void api
                .setJumpRange(v)
                .then((n) => {
                  if (n !== v) patch({ jump_range: n });
                })
                .catch((e) => fail(e));
            }}
          />

          <SliderRow
            tone="read"
            label={
              <>
                跟读位置
              </>
            }
            value={config.anchor}
            min={0.15}
            max={0.6}
            step={0.01}
            fmt={(v) => `屏幕 ${Math.round(v * 100)}% 高处`}
            /* 右侧那个「屏幕 42% 高处」已经把方向说清了（百分比变大＝往下）。
               「盯着不累比标准值重要」是安慰，不是信息 —— 删。 */
            onChange={(v) => patch({ anchor: v })}
          />

          <Row
            label={
              <>
                跟读颜色
              </>
            }
            /* 「只改提示色」是解释 —— 那三个色点本身就是答案，删。 */
            right={
              <ColorDots
                value={config.read_color}
                theme={resolveTheme(config.theme)}
                onPick={(k) => patch({ read_color: k })}
              />
            }
          />

          <Row
            label="平滑滚动"
            /* 原来挂着「换句时滚动过去 / 关掉＝每次换句瞬间到位」，
               随全部小字一起去掉了；口径写进「更多」的功能介绍。 */
            right={
              <Switch
                on={config.smooth_scroll}
                onClick={() => patch({ smooth_scroll: !config.smooth_scroll })}
              />
            }
          />

          <Row
            label="快捷跳读"
            /* ★ **保留**：这条是**当前口径**（单击跳还是双击跳）——
               它直接决定用户点正文会发生什么。关掉时"单击不动"这件事
               反直觉，不说会被当成点击失灵。 */
            /* 原来挂着「单击正文任意一句＝从那里接着念 / 已关：改成双击才跳」，
               随全部小字一起去掉了；口径写进「更多」的功能介绍。 */
            right={
              <Switch
                on={config.quick_jump}
                onClick={() => patch({ quick_jump: !config.quick_jump })}
              />
            }
          />

          <SliderRow
            tone="read"
            label="跟读灵敏度"
            value={config.match_floor}
            min={0.2}
            max={0.6}
            step={0.01}
            fmt={() => (config.match_floor <= 0.32 ? "跟得紧" : config.match_floor >= 0.46 ? "跟得稳" : "适中")}
            /* 原来在非语音模式下会挂「只影响「语音跟随」」，随全部小字一起去掉了。 */
            onChange={(v) => {
              // 先落配置（界面即时反映），再推给引擎（正在跑的对位器立刻用新值）。
              // 只改配置不推引擎的话，用户拖完会发现"没变化"，那才是真的坑。
              patch({ match_floor: v });
              void api.setMatchFloor(v).catch(() => {});
            }}
          />
        </Group>

        {/* ============================================================ 稿件 */}
        <Group
          title="稿件"
        >
          <Row
            label="当前稿件"
            /* 原来挂着稿件标题 / 「未载入」。标题在顶栏与「更多」里都有出口，
               右侧那格「N 字 · M 段」仍然常驻。 */
            right={
              <span className="grp-v">
                {chars > 0 ? `${chars} 字 · ${script?.paragraphs.length} 段` : "—"}
              </span>
            }
          />
          <Row>
            <button className="btn primary" onClick={pickScript}>
              打开文件
            </button>
            <button
              className="btn"
              onClick={() =>
                dispatch({ type: "PASTE", open: true, mode: chars > 0 ? "edit" : "new" })
              }
            >
              {chars > 0 ? "改这份稿" : "粘贴一段"}
            </button>
            <button
              className="btn"
              onClick={() => {
                void api.clearScript().then((s) => dispatch({ type: "SCRIPT", script: s }));
              }}
            >
              清空
            </button>
          </Row>
        </Group>

        {/* ============================================================ 外观 */}
        <Group
          title="外观"
        >
          <Row
            label="主题"
            right={
              <div className="seg">
                {THEMES.map(([v, label]) => (
                  <button
                    key={v}
                    className={`seg-btn${config.theme === v ? " on" : ""}`}
                    onClick={() => patch({ theme: v })}
                  >
                    {label}
                  </button>
                ))}
              </div>
            }
          />
          <Row label="预设">
            {PRESETS.map((pp) => (
              <button key={pp.key} className="btn" onClick={() => patch(pp.patch)}>
                {pp.label}
              </button>
            ))}
          </Row>
          <SliderRow
            label="字号"
            value={config.font_size}
            min={18}
            max={72}
            step={1}
            fmt={(v) => `${v}px`}
            onChange={(v) => patch({ font_size: v })}
          />
          <SliderRow
            label="行高"
            value={config.line_height}
            min={1.2}
            max={2.4}
            step={0.05}
            fmt={(v) => v.toFixed(2)}
            onChange={(v) => patch({ line_height: v })}
          />
          <SliderRow
            label="左右留白"
            value={config.padding_x}
            min={8}
            max={120}
            step={2}
            fmt={(v) => `${v}px`}
            onChange={(v) => patch({ padding_x: v })}
          />
          <Row
            label="对齐"
            right={
              <div className="seg">
                {(
                  [
                    ["left", "左"],
                    ["center", "中"],
                    ["right", "右"],
                  ] as [ScriptAlign, string][]
                ).map(([v, label]) => (
                  <button
                    key={v}
                    className={`seg-btn${config.align === v ? " on" : ""}`}
                    onClick={() => patch({ align: v })}
                  >
                    {label}
                  </button>
                ))}
              </div>
            }
          />
          {/* 系统磨砂档下外壳透明度由材质接管（`--win-alpha` 固定让位），
              滑杆在这一档没有意义 —— 一个意思只留一个控件，藏起来。 */}
          {config.glass_mode !== "system" && (
            <SliderRow
              label="不透明度"
              value={config.opacity}
              min={0.3}
              max={1}
              step={0.02}
              fmt={(v) => `${Math.round(v * 100)}%`}

              onChange={(v) => patch({ opacity: v })}
            />
          )}
          <Row
            label="背景磨砂"
            /*
             * ★ 三条 sub 都是**常驻**的，理由各不相同（第十一轮判据：
             *   「状态/警告/口径」才配常驻，只解释原理的不配）：
             *   · css    —— 代价警告（吃性能），用户会照着"卡就关掉"做；
             *   · system —— **当前口径**：提词时（失焦）材质自动退实底。
             *     不说这句话，用户开会时一定会把"磨砂没了"当成 bug；
             *   · off    —— 没有需要说的话（实色底不需要解释）。
             *
             * ★★ 每条都压在 10 个字以内：这一行右边是三颗 seg 按钮，
             *   左边只剩不到一半宽 —— 超出的部分会被省略号吃掉，
             *   而"被截掉一半的口径"等于没说（第十一轮删提示的同一条理由）。
             */
            /* 原来按档位挂着「提词时自动变实底 / 吃性能，卡就关掉」，
               随全部小字一起去掉了；三档的区别写进「更多」的功能介绍。 */
            right={
              <div className="seg">
                {(
                  [
                    ["off", "关"],
                    ["css", "柔光"],
                    ["system", "系统磨砂"],
                  ] as [GlassMode, string][]
                ).map(([v, label]) => (
                  <button
                    key={v}
                    className={`seg-btn${config.glass_mode === v ? " on" : ""}`}
                    onClick={() => patch({ glass_mode: v })}
                  >
                    {label}
                  </button>
                ))}
              </div>
            }
          />
        </Group>

        {/* ============================================================ 窗口 */}
        <Group
          title="窗口"
        >
          <Row
            label="顶栏浮在正文上"
            /* ★ **保留**：同一开关的另一半语义 + 代价（盖住第一行）。 */
            /* 原来挂着「盖住正文第一行 / 不遮正文」，随全部小字一起去掉了。 */
            right={
              <Switch
                on={config.bar_pinned}
                onClick={() => patch({ bar_pinned: !config.bar_pinned })}
              />
            }
          />
          <Row
            label="锁定窗口位置"
            /* 原来挂着「拖动顶栏不再移动窗口」，随全部小字一起去掉了。 */
            right={
              <Switch
                on={config.window.locked}
                onClick={() => {
                  const next = !config.window.locked;
                  patch({ window: { ...config.window, locked: next } });
                  void api.setWindowLocked(next).catch(() => {});
                }}
              />
            }
          />
          <Row
            label="窗口置顶"
            /* 原来挂着「永远盖在 PPT / 浏览器之上 · 开始提词会自动打开」，
               随全部小字一起去掉了；那条联动关系写进「更多」的功能介绍。 */
            right={<Switch on={pinned} onClick={() => void togglePin()} />}
          />
          <Row
            label="鼠标穿透"
            /* 原来挂着穿透的生效状态与「忘关从托盘或热键关」这句退出口指引，
               随全部小字一起去掉了。
               ★ 退出口本身**没有**减少：托盘菜单与热键都还在，
                 顶栏也仍会在「开了但后端没确认」时挂出警示。 */
            right={
              <Switch
                on={state.ctActive}
                /*
                 * 与顶栏那个胶囊走**同一套状态**（CT_PENDING → CT）。
                 * 之前这里只 dispatch CT，于是"在设置里打开穿透"不会点亮
                 * 顶栏的「开启中」—— 同一个功能、两个入口，状态却不一致。
                 *
                 * ★★ 穿透的**退出口有三条**，一条都不能少（它是"会关掉某个
                 *    输入通道"的能力，必须有不依赖该通道的退出方式）：
                 *    ① 这里；② 托盘菜单；③ 热键。
                 */
                onClick={() => {
                  const want = !state.ctActive;
                  dispatch({ type: "CT_PENDING", pending: want });
                  void api
                    .setClickThrough(want)
                    .then((active) => dispatch({ type: "CT", active }))
                    .catch((e) => {
                      dispatch({ type: "CT_PENDING", pending: false });
                      fail(e);
                    });
                }}
              />
            }
          />
          <Row
            label="全屏提词"
            /* ★ **保留**：这是**警告**（共享屏幕时别用，全屏会盖住 PPT）——
               真按下去 PPT 就被盖住了，那时才说已经晚了。
               「适合外接显示器 / 平板 / 分光镜」是推荐场景，已随提示一起删。 */
            /* 原来挂着「全屏中 · Esc 退出 / 共享屏幕时别用」，
               随全部小字一起去掉了；共享屏幕时的注意事项写进功能介绍。 */
            right={
              <button className="btn" onClick={() => void toggleFullscreen()}>
                {fullscreen ? "退出" : "进入"}
              </button>
            }
          />
          <Row
            label="启动时自动隐身"
            /* 原来挂着「开机就把窗口从投屏里藏起来」，随全部小字一起去掉了。 */
            right={
              <Switch
                on={config.stealth_on_start}
                onClick={() => patch({ stealth_on_start: !config.stealth_on_start })}
              />
            }
          />
          <Row
            /*
             * ★★ 标签里「收进任务栏」是**条件出现**的，不是常驻。
             *    在 macOS 上这一行只剩「收起窗口」一颗按钮，标签还写着
             *    「收起 / 收进任务栏」就是在说一件用户做不到的事。
             */
            label={platform?.has_taskbar ? "收起 / 收进任务栏" : "收起窗口"}
            /* 原来挂着收起态说明与平台回落提示（`platform.collapse_hint`），
               随全部小字一起去掉了。
               ★ 注意：`collapse_hint` 现在**没有消费者**了 —— 后端仍然产出它，
                 但界面上不再显示。留着无害（后端那份是平台判定的唯一来源），
                 只是它暂时失去了出口。 */
            right={
              <>
                <button className="btn" onClick={() => void toggleCollapsed()}>
                  {state.collapsed ? "展开" : platform?.collapse_label ?? "收起"}
                </button>
                {/*
                  「收进任务栏」这颗**只在真的有任务栏按钮的平台上出现**。
                  ★★ macOS 上它必须消失，理由是判据级的：那边按下它等于收起
                     （见 Rust 侧 `commands::minimize_to_taskbar` 里那段关于 Dock 的说明），
                     留着就变成**两颗按钮干同一件事** —— 用户会以为它们有区别。
                     后端把 `has_taskbar` 给过来，前端不自己按 os 判。
                  ★ 默认（platform 还没到手时）**不渲染**：宁可少一颗，
                    也不要先长出一颗再消失 —— 那会闪一下。
                */}
                {platform?.has_taskbar && (
                  <button className="btn" onClick={() => void sendToTaskbar()}>
                    收进任务栏
                  </button>
                )}
              </>
            }
          />
        </Group>

        {/* ============================================================ 更多
         *
         * 装载的都是"装完机器就不用再碰"的东西。它们必须还在（换电脑、换模型、
         * 出问题要发诊断时要用），但不该和"字号""跟读灵敏度"抢出场顺序 ——
         * 所以整组默认折叠。
         */}
        <Group
          title="更多"
          collapsible
        >
          {/* ---- 功能介绍 ----
           *
           * 本轮（2026-09-28）把设置面板里**所有**行内小字删掉了，
           * 用户的原话是「功能介绍小字太多了，全部都删除，一个不要留。
           * 在更多里面加一个详细的各个功能介绍就可以」。
           *
           * ★ 为什么落在这里、而不是做成一个独立页面或弹窗：
           *   设置面板是**边讲边改**的地方，每行下面压一行字，能被看到的
           *   控件就少一行；而"这东西到底是干什么的"是**低频**需求
           *   （第一次用、忘了的时候才查）。所以它必须在一个默认收起的
           *   地方 —— 「更多」正是这个角色。
           *
           * ★ 它自己也默认收起（`guideOpen` 初值 false）：功能介绍有二十多条，
           *   展开会把下面所有真正的设置项推出视野 —— 那就本末倒置了。
           *
           * ★ 一个字都没丢。原来散在各行下面的话，全部并进了下面的文案，
           *   包括几条不是"功能介绍"而是**运行事实**的（穿透的退出口、
           *   换机器包里的内容清单、参考语速当前的处境）。
           */}
          <div className="grp-sub-h">功能介绍</div>
          <Row
            label={guideOpen ? "收起功能介绍" : "查看各项功能是做什么的"}
            onClick={() => setGuideOpen((v) => !v)}
            right={
              <span className={`grp-caret${guideOpen ? " open" : ""}`} aria-hidden="true">
                ›
              </span>
            }
          />
          {guideOpen && (
            <div className="grp-row col">
              <div className="guide">
                <GuideGroup title="提词">
                  <GuideItem k="当前">
                    开始或结束这一场提词。没有稿件时「开始」是灰的。
                    开讲后窗口自动隐身并置顶，状态看底部那颗大圆按钮。
                  </GuideItem>
                  <GuideItem k="会前自检">
                    开讲前查一遍麦克风与配置，会真的录约 1.2 秒。
                    查完每一项都给出结论，红色的建议先处理完再开讲。
                  </GuideItem>
                  <GuideItem k="推进方式">
                    决定稿子往下走由谁推动：语音跟随（你念到哪它跟到哪）、
                    匀速滚动（按固定速度自己走）、纯手动（只靠上一句/下一句）。
                  </GuideItem>
                  <GuideItem k="滚动速度">
                    仅匀速滚动时出现：稿子每分钟走多少字。
                    开讲中在顶栏那条速度条上也能调，不必回来开抽屉。
                  </GuideItem>
                  <GuideItem k="参考语速">
                    你预计的讲话速度，用来把剩余字数换算成剩余时间。
                    注意：顶栏目前不再常显剩余时间，所以这一项现在拖动
                    看不到直接效果。保留它是因为剩余时间一旦重新显示，
                    第一个要接的就是这里。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="跟读（仅语音跟随时生效）">
                  <GuideItem k="跳读范围">
                    跟丢之后允许自动向前或向后跳多少句去找你的声音，
                    跳得太远容易追错段落。默认 20 句。
                  </GuideItem>
                  <GuideItem k="跟读位置">
                    当前这一句停在屏幕多高的地方，百分比越大越靠下。
                    按自己的坐姿与视线调，眼睛不用一直往下追。
                  </GuideItem>
                  <GuideItem k="跟读颜色">
                    正在念与已念部分的提示色，只改颜色，不影响其他。
                  </GuideItem>
                  <GuideItem k="平滑滚动">
                    开着＝换句时滚动过去；关掉＝每次换句瞬间到位，
                    眼睛不用追动画。长时间照读关掉更省力。
                  </GuideItem>
                  <GuideItem k="快捷跳读">
                    开＝单击正文任意一句，就从那里接着念；
                    关＝改成双击才跳（防手抖点飞），单击不动。
                  </GuideItem>
                  <GuideItem k="跟读灵敏度">
                    跟得多紧：调到"跟得紧"更容易立刻响应，
                    "跟得稳"更不容易被环境杂音带跑。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="稿件">
                  <GuideItem k="当前稿件">
                    右侧显示这份稿的字数与段数。导入的文件只记路径，
                    粘贴进来的稿会留一份正文副本。
                  </GuideItem>
                  <GuideItem k="打开文件 / 粘贴一段 / 清空">
                    也可以直接把文件拖进窗口；改这份稿会进编辑，粘贴则是新建。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="外观">
                  <GuideItem k="主题">
                    深色适合暗光场合，浅色适合亮场；「系统」跟随 Windows 设置。
                  </GuideItem>
                  <GuideItem k="预设">
                    小/中/大是排版组合（字号、行高、字距、左右留白一起调），
                    改完仍可逐项微调。
                  </GuideItem>
                  <GuideItem k="字号 / 行高 / 左右留白">
                    左右留白决定每行放多少字，通常照着提词器的视线宽度调。
                  </GuideItem>
                  <GuideItem k="对齐">
                    正文靠左、居中还是靠右，配合不同排版的稿子。
                  </GuideItem>
                  <GuideItem k="不透明度">
                    外壳背景的透明度。系统磨砂那一档由材质接管，这一项会自动隐藏。
                  </GuideItem>
                  <GuideItem k="背景磨砂">
                    关＝实色底；柔光＝窗口内的 CSS 毛玻璃（会吃一点性能，
                    卡就关掉）；系统磨砂＝调用系统材质（Windows Acrylic /
                    macOS hudWindow），磨的是窗口后面的东西，开始提词、
                    窗口失焦时会自动变回实底。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="窗口">
                  <GuideItem k="顶栏浮在正文上">
                    开＝顶栏盖住正文第一行；关＝正文从顶栏下方开始，两者不重叠。
                  </GuideItem>
                  <GuideItem k="锁定窗口位置">
                    锁上以后拖动顶栏不再移动窗口，防止开会时误拖。
                  </GuideItem>
                  <GuideItem k="窗口置顶">
                    永远盖在 PPT / 浏览器之上。开始提词时会自动打开。
                  </GuideItem>
                  <GuideItem k="鼠标穿透">
                    开着＝稿件区点得到下面的窗口，适合一边翻 PPT 一边看稿。
                    提醒一句：三个退出口都能关 —— 这里这颗开关、托盘菜单、
                    全局热键（默认 Ctrl+Alt+P）。别只依赖其中一个。
                  </GuideItem>
                  <GuideItem k="全屏提词">
                    整屏只显示稿子。共享屏幕时别用 —— 它会盖住 PPT。
                    全屏中按 Esc 退出。
                  </GuideItem>
                  <GuideItem k="启动时自动隐身">
                    开机就把窗口从投屏里藏起来，防的就是"还没准备好就被投出去"。
                  </GuideItem>
                  <GuideItem k="收起 / 收进任务栏">
                    收起＝压成一条细栏，正文暂时让出来；
                    收进任务栏＝缩到任务栏留一颗按钮（macOS 没有任务栏，
                    那一项不会出现，那里只提供收起）。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="高级排版">
                  <GuideItem k="字距">
                    字与字之间的额外间距，投影仪上字太挤时略微加一点更清楚。
                  </GuideItem>
                  <GuideItem k="左右镜像">
                    把文字左右翻过来，配合分光镜（beam splitter）使用 ——
                    只在有分光镜的 setups 下才需要开。
                  </GuideItem>
                  <GuideItem k="已读变暗">
                    开＝念过的段落降一档明度，眼睛更容易找到当前句；
                    关＝整篇一样亮。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="麦克风与识别">
                  <GuideItem k="来源">
                    本地离线＝音频不外发，在本机跑 sherpa-onnx 流式模型；
                    云端还没接入，需要先配好端点。
                  </GuideItem>
                  <GuideItem k="模型">
                    本地识别用的模型文件，放在模型目录里，点「重新扫描」生效。
                  </GuideItem>
                  <GuideItem k="麦克风自检">
                    录约两秒，判断有没有真正采到声音 ——
                    设备显示正常但送出数字静音时，只有这一步查得出来。
                  </GuideItem>
                  <GuideItem k="识别">
                    单独启动或停止识别引擎。跑起来时按钮写「停止」，
                    没跑时写「单独启动」—— 按钮本身就是状态。
                  </GuideItem>
                  <GuideItem k="联调：试一句">
                    把稿子里任意一句粘进去回车，看匹配得分与位置，
                    用来判断"跟不准"是对位器的问题还是稿子的问题。
                  </GuideItem>
                </GuideGroup>

                <GuideGroup title="迁移与诊断">
                  <GuideItem k="换机器">
                    导出成一个迁移包。里面含配置、稿件历史与粘贴稿的正文副本
                    —— 发给别人排障前先知道内容物。它会往盘上落一份数据，
                    用完记得自己删。
                  </GuideItem>
                  <GuideItem k="自检信息">
                    生成一段配置与运行状态汇总。出问题时把这段发出来，
                    比截图有用得多。
                  </GuideItem>
                  <GuideItem k="当前状态">
                    隐身 / 收起 / 穿透三件事的常驻读数，
                    用来确认"到底有没有被投屏看得到"。
                  </GuideItem>
                </GuideGroup>
              </div>
            </div>
          )}

          {/* 「语音引擎」这一小节只在**语音跟随**时才显示 —— 匀速滚动与纯手动
              都不碰麦克风，那时把模型选择摆在这里，只会让人去查一个用不上的东西。 */}
          {drive === "voice" && <div className="grp-sub-h">语音跟随</div>}
          {drive === "voice" && (
            <Row
              label="来源"
              /* 只在左半边留一句**不能藏**的：本地不外发音频。
                 "云端更准、要先配端点"是取舍说明，悬停见。 */
              /* ★ **保留**：这是**当前选择的后果**（音频出不出机器 / 要不要配端点），
                 属于"选了它要付什么"的运行时事实。 */
              /* 原来挂着「音频不外发 / 需先配好端点」，随全部小字一起去掉了。 */
              right={
                <select
                  value={config.asr.engine}
                  onChange={(e) => {
                    const k = e.target.value as EngineKind;
                    // 记住它 —— 切到匀速滚动再切回来时要落回这里，
                    // 而不是落回写死的默认值。
                    lastVoiceSource = k;
                    patchAsr({ engine: k });
                  }}
                  style={{ width: 168 }}
                >
                  <option value="local">本地离线</option>
                  <option value="cloud">云端（未接入）</option>
                </select>
              }
            />
          )}
          {drive === "voice" && config.asr.engine === "local" && (
            <>
              <Row
                label="模型"
                right={
                  <select
                    value={config.asr.local_model?.label ?? ""}
                    onChange={(e) => {
                      const m = foundModels.find((x) => x.label === e.target.value);
                      if (m) patchAsr({ local_model: m });
                    }}
                    style={{ width: 168 }}
                  >
                    <option value="">未选择</option>
                    {foundModels.map((m) => (
                      <option key={m.label} value={m.label}>
                        {m.label}
                      </option>
                    ))}
                  </select>
                }
              />
              {foundModels.length === 0 && (
                <div className="grp-row col">
                  <div className="grp-s" style={{ color: "rgb(var(--warn))" }}>
                    还没有检测到模型。把 sherpa-onnx 流式模型解压进模型目录，再点「重新扫描」。
                  </div>
                </div>
              )}
              <Row>
                <button className="btn" onClick={() => void api.openPath(modelsDir)}>
                  打开模型目录
                </button>
                <button
                  className="btn"
                  onClick={() => {
                    void api.rescanModels().then((m) => {
                      dispatch({ type: "MODELS", models: m });
                      dispatch({ type: "NOTICE", message: `扫描到 ${m.length} 个模型` });
                    });
                  }}
                >
                  重新扫描
                </button>
              </Row>
            </>
          )}

          <div className="grp-sub-h">高级排版</div>
          <SliderRow
            label="字距"
            value={config.letter_spacing}
            min={0}
            max={0.12}
            step={0.005}
            fmt={(v) => `${v.toFixed(3)} em`}
            onChange={(v) => patch({ letter_spacing: v })}
          />
          {/* 「当前句位置」已搬到上面的「跟读」组（现在叫「跟读位置」）。
              它本来埋在这个折叠组里，可它其实是**一边讲一边会想动**的东西
              —— 判据是"一场直播里按几次"，不是"它属于排版还是跟读"。 */}
          <Row
            label="左右镜像"
            /* 原来挂着「配合分光镜使用（文字会左右翻过来）」，
               随全部小字一起去掉了。 */
            right={
              <Switch on={config.mirror} onClick={() => patch({ mirror: !config.mirror })} />
            }
          />
          <Row
            label="已读变暗"
            /* 开关的另一半语义 —— 可见。 */
            /* ★ **保留**：同一开关的另一半语义（关掉＝整篇一样亮）。 */
            /* 原来挂着「念过的段落降一档明度 / 整篇一样亮」，
               随全部小字一起去掉了。 */
            right={
              <Switch on={config.dim_read} onClick={() => patch({ dim_read: !config.dim_read })} />
            }
          />

          <div className="grp-sub-h">麦克风</div>
          <Row
            label="麦克风自检"

            /* ← 采集中的操作指引必须看得见（它有时间窗，不看就错过）。
               为什么需要这个检查（数字静音那种查不出来的故障）收进悬停。 */
            /* ★ **保留**：**正在测**这件事要看得见（约 2 秒，界面上必须有反应）。 */
            /* 原来采集时挂着「对着麦克风说一句话（约 2 秒）」，
               随全部小字一起去掉了。
               ★ 采集中的反馈改由按钮文案承担：它会从「开始」变成
                 「正在采集…」，所以「正在录」这件事仍然看得见。 */
            right={
              <button className="btn" onClick={runMicCheck} disabled={probing}>
                {probing ? "正在采集…" : "开始"}
              </button>
            }
          />
          {probe && (
            <div className="grp-row col">
              <div
                className="grp-s"
                style={{
                  whiteSpace: "pre-line",
                  color: probe.verdict === "ok" ? "rgb(var(--ok))" : "rgb(var(--warn))",
                }}
              >
                {micVerdictText(probe.verdict)} · {probe.device}
                {"\n"}
                {probe.sample_rate}Hz / {probe.channels}声道 / {probe.format} · 峰值{" "}
                {toDbfs(probe.peak).toFixed(0)} dBFS · RMS {toDbfs(probe.rms).toFixed(0)} dBFS
                {"\n"}
                {probe.detail}
                {probe.verdict !== "ok" && probe.devices.length > 0 && (
                  <>
                    {"\n"}
                    可用输入设备：{probe.devices.join("；")}
                  </>
                )}
              </div>
            </div>
          )}
          <Row
            label="识别"
            /* ★ **保留**：这是**实际在工作**（不是"按下的意图"）—— 见判断基线的
               同一颜色一个意思 / 显示实际状态那条。 */
            /* 原来挂着「运行中 · <engine> / 未运行」，随全部小字一起去掉了。
               ★ 状态仍由右侧那颗按钮承担：它在跑时写「停止」，没跑写「单独启动」。 */
            right={
              <button className="btn" onClick={toggleAsr} disabled={busy}>
                {asrStatus?.running ? "停止" : "单独启动"}
              </button>
            }
          />
          {state.asrText && <div className="grp-row col"><div className="grp-s">最近听到：{state.asrText.slice(-60)}</div></div>}

          <div className="grp-sub-h">联调：试一句</div>
          <div className="grp-row col">
            <input
              type="text"
              placeholder="粘贴稿件里的任意一句，回车喂进去…"
              value={testText}
              onChange={(e) => setTestText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && testText.trim()) {
                  void api
                    .feed(testText)
                    .then((c) => {
                      dispatch({ type: "CURSOR", cursor: c });
                      dispatch({
                        type: "NOTICE",
                        message: `匹配得分 ${(c.score * 100).toFixed(0)}% · 位置 ${c.cursor}/${c.total}`,
                      });
                    })
                    .catch(fail);
                }
              }}
            />
          </div>

          <div className="grp-sub-h">迁移</div>
          <Row
            label="换机器"
            /* 原来挂着「含配置、稿件历史、粘贴稿正文副本」，随全部小字一起去掉了。
               ★ 它是**包里到底有什么**（隐私披露），不能凭空消失 ——
                 已原样搬进下面「功能介绍」里「换机器」那一条。
                 判据：用户是拿这个包去发给别人排障的，不知道内容物就不敢发。 */
            right={
              <>
                <button className="btn" onClick={exportBundle}>
                  导出
                </button>
                <button className="btn" onClick={importBundle}>
                  导入
                </button>
              </>
            }
          />

          <div className="grp-sub-h">诊断</div>
          <Row
            label="自检信息"
            /* 原来挂着「出问题时把这段发出来」，随全部小字一起去掉了；
               怎么用写进「更多」的功能介绍。 */
            right={
              <button className="btn" onClick={runDiagnostics}>
                生成
              </button>
            }
          />
          {diag && diag.length > 0 && (
            <div className="grp-row col">
              <div className="diag">
                {diag.flatMap(([k, v]) => [
                  <span className="dk" key={`k-${k}`}>
                    {k}
                  </span>,
                  <span className="dv" key={`v-${k}`}>
                    {v}
                  </span>,
                ])}
              </div>
            </div>
          )}
          <Row
            label="当前状态"
            right={
              <span className="grp-v">
                {stealthLabel} · {state.collapsed ? "已收起" : "展开"}
                {state.ctActive ? " · 穿透中" : ""}
              </span>
            }
          />
        </Group>

        {/* ======================================================== 关于
         *
         * ★ 这一组装的是**"这个软件自己"的事**：它是什么版本、要不要更新、
         *   有话想跟作者说。与「更多」里那些装的东西性质不同 ——
         *   后者是**这台机器的设置项**（引擎、迁移、诊断），改它会影响使用；
         *   而这里的动作一个都不改配置。
         *
         * ★★ 为什么「意见反馈」是并进这一组、**不是**单开一组：
         *   设置面板里的**组标题本身也占一行**，而本项目那条判据是
         *   「设置面板每一行的垂直空间都很贵」。两个各只有一行的组，
         *   不如一个两行的组 —— 后者少一个标题、少一道视觉断点。
         *   这也是"新功能嵌入已有区块、不单开面板"那条一贯做法的落点。
         *
         * ★ 为什么这一组**不能**塞进「更多」或「支持作者」：
         *   那两个都是 `collapsible`（默认收起）。藏进去就是三层深
         *   （更多 → 展开 → 再找），而**用户找不到的入口和不存在的入口
         *   是一回事**。这一组默认展开，点开设置往下翻就到了。
         *
         * ★ 位置挑在「更多」**之后**：「更多」默认收起，所以这一组实际上
         *   紧挨着「窗口」，点开设置就能看到；而它又不会挤到「提词」「跟读」
         *   那几组真正天天要用的设置上面去。
         *
         * ★ 两行都**没有行内小字**（同设置页全局判据）：版本号在标签里、
         *   动作在按钮上、状态也在按钮文字里（"更新中 42%"）——
         *   各占一个位置，不需要谁去解释谁。
         *
         * ★ 「检查更新」与顶栏下方那条提示条是**同一套实现**
         *   （`lib/update.ts` 里那两个函数），不是两份：两处各写一遍
         *   迟早漂成"提示条上点得动、这里点不动"。
         *   这里多出来的是**主动检查**那一半 —— 提示条只在"已经有更新"时才存在。
         *
         * ★ 「意见反馈」跳的是外部浏览器（`api.openUrl`，后端只放行 http/https）。
         *   地址的唯一出处是上面的 `FEEDBACK`。**不做二维码**：提意见是
         *   在电脑上打字的事，手机扫码还得把问题重打一遍（理由详见 `FEEDBACK`）。
         */}
        <Group title="关于">
          <Row
            label={`当前版本 ${state.appVersion || "—"}`}
            right={
              <button
                className="btn"
                disabled={updateBusy || updateChecking}
                onClick={() => {
                  // 已经有更新待装 ⇒ 这一下就是"装它"；否则是"再查一遍"。
                  // ★ 两件事共用一颗按钮，判据只有"手里有没有东西"——
                  //   分成两颗按钮就得回答"我该点哪一颗"，而那个问题
                  //   界面上没有地方能说清楚。
                  if (state.update) {
                    void installPendingUpdate(dispatch);
                    return;
                  }
                  setUpdateChecking(true);
                  void checkForUpdate(dispatch).finally(() =>
                    setUpdateChecking(false),
                  );
                }}
              >
                {updateBusy
                  ? state.updateProgress === null
                    ? "更新中…"
                    : `更新中 ${Math.round(state.updateProgress * 100)}%`
                  : updateChecking
                    ? "检查中…"
                    : state.update
                      ? `更新到 ${state.update.version}`
                      : "检查更新"}
              </button>
            }
          />
          {/*
            意见反馈 —— 一行一个动作：说清这是什么（标签）、去哪（按钮）。
            ★ 按钮文字用「填写反馈」而不是「去反馈」/「打开表单」：
              前者的动作对象是**用户要做的事**（填），后者是**实现**（打开表单）。
              界面上该说前者。
            ★ 它是**普通按钮**，不是整行可点：整行可点的语义是"这一行是个开关/
              跳转"，而这一行左边只是个标签，点左边没有任何事会发生 ——
              让点击落在没反应的地方，比缩小点击区更伤（这一条的判据见
              `Transport` 上那颗穿透开关的注释）。
          */}
          <Row
            label="意见反馈"
            right={
              <button
                className="btn"
                onClick={() => {
                  void api.openUrl(FEEDBACK.formUrl).catch(fail);
                }}
              >
                填写反馈
              </button>
            }
          />
        </Group>

        {/* ======================================================== 支持作者
         *
         * ★ 它是**独立的一组**，没有塞进上面的「更多」里 —— 两者装的东西
         *   性质不同：「更多」装的是**这台机器的设置项**（引擎、迁移、诊断），
         *   而"要不要请作者喝杯茶"跟配置无关。塞进去就等于把这个入口埋到
         *   三层以下（更多 → 支持作者 → 展开双码），而一个三层深的入口
         *   等于没有入口 —— 用户找不到的入口和不存在的入口是一回事。
         *
         * ★ 整组默认收起（`collapsible`）：二维码从不首屏平铺。
         *   顺带一个好处：`Group` 收起时不渲染 children，所以两张码图
         *   在用户点开之前**根本不会被请求**。
         *
         * ★ 组里没有任何行内小字（`.grp-l .grp-s`）—— 那是设置行才有的东西，
         *   而这里是一块**被用户主动展开的内容**，正文与说明本来就该在。
         */}
        <Group title="支持作者" collapsible>
          <div className="donate">
            <p className="donate-pitch">
              这个工具我一个人在做，免费、也没有付费才能解锁的功能。
              如果它帮你省下过几场直播的力气，欢迎请我喝杯茶。
            </p>
            <div className="donate-grid">
              <div className="donate-card">
                <div className="donate-name">微信赞赏</div>
                <img
                  className="donate-qr"
                  src={DONATE.weixinQr}
                  alt={`微信赞赏码（收款人 ${DONATE.weixinNickname}）`}
                  draggable={false}
                />
                <div className="donate-nick">收款人 {DONATE.weixinNickname}</div>
              </div>
              <div className="donate-card">
                <div className="donate-name">支付宝</div>
                <img
                  className="donate-qr"
                  src={DONATE.alipayQr}
                  alt={`支付宝收款码（收款人 ${DONATE.alipayNickname}）`}
                  draggable={false}
                />
                <div className="donate-nick">收款人 {DONATE.alipayNickname}</div>
              </div>
            </div>
            <div className="donate-note">
              对着这个屏幕扫最稳；截图之后再扫，微信和支付宝多半会认不出来。
            </div>
            <div className="donate-foot">
              <button
                className="btn"
                onClick={() => {
                  void api.openUrl(DONATE.sponsorUrl).catch(fail);
                }}
              >
                海外用户：GitHub 赞助
              </button>
            </div>
          </div>
        </Group>
      </div>
    </aside>
  );
}
