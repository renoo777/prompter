/* Tauri IPC 桩 —— 只给「离线看 UI」用，不参与任何产品构建。
 *
 * 它是怎么工作的：@tauri-apps/api 在前端只做一层很薄的转发 ——
 * `invoke()` 会调 `window.__TAURI_INTERNALS__.invoke`，`listen()` 会先要一个
 * `transformCallback` 生成的 handler id。把这两个（外加 `metadata.currentWindow`
 * 给 getCurrentWindow 用）造出来，整个前端就能在没有 Rust 进程的情况下跑起来。
 *
 * 于是改完界面不必等 release 出包、也不受「投屏隐身抓不到屏」的影响，
 * 还能精确控制点击 —— 只有交互时才出现的状态才验得到。
 *
 * 数据通过 URL 查询串控制：
 *   ?open=settings        打开设置抽屉
 *   &tap=icon1            按**结构**点第 n 颗顶栏图标（0 起；见 iconAt）
 *   &tap=last0            按**结构**点从右数第 n 颗（0＝关闭；见 iconFromEnd）
 *   &click=stealth        点状态灯 / &click=demo 点大圆按钮
 *   &theme=light          覆盖主题
 *   &noScript=1           模拟「还没载入稿件」
 *   &noModel=1            模拟「没检测到模型」（看警告态）
 *   &glass=system         预置「系统磨砂」档（桩把窗体材质摆成已挂）
 *   &glassclick=off|css|system  点「背景磨砂」行的某一档（走真落盘路径，量四联测）
 *   &moreclick=1          展开「更多」那一组（它默认收起）
 *   &guideclick=1         展开「更多」里的「功能介绍」长文（默认也收起）
 *   &donateclick=1        展开「支持作者」并按下赞助按钮（组默认收起；
 *                         报 donateQr/donateQrW/openedUrl，图有没有
 *                         解码出来看收尾的 donateImgOkLate）
 *   &guidescroll=<px>     把抽屉正文滚到指定位置（长文要往下翻才看得到下面几组）
 *   &update=<版本号>      让桩假装查到一个新版本（默认报"没有更新"，那才是常态）。
 *                         ★ 必须再配 `&settle=3200` —— 前端启动后要等 3 秒才查，
 *                           不配的话量到的是"提示条还没长出来"，看着像功能没做。
 *
 * ★ 第十一轮起**不再有 `hover=`**：提示整个删了，没有"悬停才出现"的东西
 *   可截。`tap=` 报的是「序号 + 名字」（`iconName`），因为按位置找的代价是
 *   "插删一颗图标就静默指错" —— 报名字让漂移能被看出来。
 */

const params = new URLSearchParams(location.search);

const paragraphs = [
  {
    index: 0,
    text: "各位下午好。今天的汇报分三个部分：先看上半年的整体完成情况，再讲三个重点项目的进展，最后是下半年的安排。",
  },
  {
    index: 1,
    text: "先说第一部分。上半年我们一共交付了十四个版本，比去年同期多了五个，其中三个是给外部客户做的定制。",
  },
  {
    index: 2,
    text: "这里有一件事要单独提一下：六月份那次上线比原计划晚了四天，原因是第三方接口的联调比预期复杂。",
  },
  {
    index: 3,
    text: "第二个部分讲重点项目。第一个是数据中台，目前已经完成主体功能，正在做压测，预计下个月可以正式上线。",
  },
].map((p) => {
  // 句号切开，和后端 script.rs 的口径一致（逐句可点）
  const parts = p.text.split(/(?<=[。！？])/).filter(Boolean);
  let cursor = 0;
  const sentences = parts.map((t, i) => {
    const s = { index: i, text: t, align_start: cursor, align_end: cursor + t.length };
    cursor += t.length;
    return s;
  });
  return { ...p, sentences };
});

const totalChars = paragraphs.reduce(
  (n, p) => n + p.sentences.reduce((m, s) => m + s.text.length, 0),
  0,
);

const nowSec = Math.floor(Date.now() / 1000);

/*
 * ---------------------------------------------------------------- 状态预置
 *
 * ★ 为什么需要它：本轮把「隐身 / 演示 / 穿透」三处都改成了**多态**——
 *   隐身四态（未隐身 / 开启中 / 投屏隐身 / 已暂停）、穿透三态（穿透 / 开启中 /
 *   穿透中）、演示两态（▶ 演示 / 演示中）。其中「开启中」在真机上只存在
 *   几十到几百毫秒，**靠真机截图根本截不到**；而它恰恰是最容易做错的一态
 *   （动画卡住不放、或者一闪而过看不见）。所以只能离线把它按住。
 *
 * ★ 位置有讲究：这段必须在 `config` **之前**。
 *   第一版写在了 `config` 后面，`stealth_on_start: stealthMode !== "off"`
 *   于是在 TDZ 里读到未初始化的绑定 → ReferenceError → **整个桩静默失效**，
 *   表现为截图空白、`--measure` 只回一个 "Prompter"。
 *   （这正是桩本身最该防的那种失败：它不报错，只是量不出东西。）
 *
 *   stealth=on|unapplied|starting|off   启动时的隐身状态
 *     on        已隐身（绿）
 *     unapplied 下了开启指令但没生效 ⇒ 琥珀「未生效」（最不能出错的一态）
 *     starting  set_stealth 悬住不返回 ⇒ 停在「开启中」
 *     off       未隐身
 *   click=stealth|demo|ct  点真按钮（走真实代码路径，不是把状态塞进去）
 *     └ 想看隐身的「已暂停」用 `stealth=on&click=stealth`：那才是用户真的按了暂停
 *   click=demo2            演示 pill **点两次**（开启 → 暂停）。
 *                          演示的三态循环只能这样验：单点一次只会走到「演示中」，
 *                          而本轮改的恰恰是"第二下不再是收摊"。
 *   click=doc2             「稿件」按钮点两次 —— 第二次必须**收起**浮层。
 *                          原来收不起来：外点关闭挂在 mousedown，点图标时它先把
 *                          面板关了，紧跟着 click 又把已置 false 的状态取反 ⇒ 重开。
 *                          量 `historyOpen=` 判。
 *   click=doc2out          对照：第一下开面板，第二下点**标题**（真·面板外）。
 *                          ★ 两条必须一起看 —— 单独 `doc2=no` 也可能是"那条 mousedown
 *                            监听根本是死的"（那样点两下同样会关）。`doc2out=no`
 *                            才证明 mousedown 那条路是活的。
 *   click=doc2nogt         反向对照：先把 `data-panel-toggle` 摘掉再点两下，
 *                          **必须复现**"再点收不起来"（`historyOpen=yes`）。
 *                          实测：doc2=no / doc2out=no / doc2nogt=yes ⇒ 三者互证。
 *                          ★ 断言里三个事件之间要**隔一个宏任务**（见 `realTap`），
 *                            否则 React 不插进来重渲染，正向反向都会通过。
 *   click=seekgap          点正文**两行之间的行距**——那里没有任何元素（改版前
 *                          处理器挂在 inline 的 `.sentence` 上，所以点了没反应）。
 *   click=seekright        点某一行**末尾右侧的空白**（同一行、x 移到段右缘）。
 *   click=seekdrag         按下后横拖 24px 再抬起：那是选字复制，**不该**跳进度。
 *                          这三条都量 `gotoN=` / `gotoPos=`，并先自报
 *                          `pointIsSentence=` 证明选中的点真的不在字形上
 *                          （否则断言会退化成"点了个字"，改版前也通过）。
 *   click=taskbar          「收进任务栏」（**设置**里那个入口）。★ 键名 2026-09-27
 *                          从 `tray` 改过来 —— 这个动作的落点改成了任务栏，
 *                          留着旧名字会让人照着一个已经不存在的行为去验（名字就是接口）。
 *                          ★ 这里会先点齿轮开抽屉、再按行的标签找按钮。
 *                          ★ 必须配合下面的 `at=` 用。
 *                          量四项：`taskbarGear=` `taskbarBtn=` `taskbarTopBtn=`
 *                          （顶栏那颗在不在）与 `taskbarToast=` / `taskbarCalled=`。
 *   click=taskbar-top      「收进任务栏」（**顶栏**那颗，2026-09-27 新增）。
 *                          与上面走同一条后端命令、同一份 `lib/sendToTaskbar` ——
 *                          两个入口必须表现一致，所以两个都要能单独验：
 *                          只验设置那个入口的话，"顶栏那颗接错了命令"照样能通过。
 *                          量 `topEntry=`（找没找到）与 `taskbarToast=` / `taskbarCalled=`。
 *   fold=open              展开设置里那个默认折叠的「更多」组
 *                          （只看一眼折叠组里长什么样时用）。
 *   collapsed=1            以**收起态**渲染：顶栏还在，正文区高度为 0。
 *                          验的是"收起之后**出路**还在不在" ——
 *                          这个 bug 上一版真的出过（展开键长在被收起的那条栏里）。
 *                          ★★ 2026-09-27 修正：这里原先是按位置取的
 *                          （`btns[length - 2]`），随本轮改版失效 ——
 *                          收起键搬进设置后，那个位置换成了别的按钮，而断言
 *                          **照样会报 yes**（位置上确实有颗按钮，只是职责变了）。
 *                          现在改按语义找，量 `collapseExitBtn=` /
 *                          `collapseExitHittable=`（收起态那唯一不靠热键的出路）。
 *   prompting=1             启动即推一条 `prompter://asr-status`（running=true），
 *                          把界面带进「提词中」。
 *                          ★ 必须走**事件**：前端拿 running 走的是
 *                          `events.onAsrStatus`，启动时不调 `asr_status` 命令 ——
 *                          改那个命令的返回值不会有人读。
 *                          ⇒ 验的是动效总策略①（提词中停装饰性动效）真的命中。
 *   at=<ms>                把点击**延后到绝对时刻**执行，而不是默认的 400ms 起步。
 *                          给"只在某段时间内存在"的状态用：「已收进托盘」那条提示
 *                          1.7s 后自己就没了，点击太早的话 dump-dom 时它已经消失，
 *                          截出来是一张空图（看起来像功能没实现）。
 *                          用法：`--hold` 略大于 at，再用 `settle` 把测量也推后。
 *   engine=local|cloud|auto 推进方式。`auto` ⇒「匀速滚动」：顶栏那一格换成
 *                          速度条，顶栏状态灯与大圆按钮也跟着换。不传时按 local。
 *   wpm=<60..600>           匀速滚动的速度（写进 `config.auto_wpm`）。
 *                          配 `engine=auto` 用：`wpm=420` 能看速度条拖到右端。
 *   ended=1                 回放一次「讲完了」：推 `prompter://cursor`（progress=1）
 *                          + `prompter://asr-status`（running=false）。
 *                          ⇒ 验的是"滚到头之后界面说什么"，不是"滚动本身"。
 *                          用法：`engine=auto&click=demo&ended=1&settle=1600`
 *   slow=1                 配合 click=ct / stealth=starting：让命令悬住不返回，
 *                          好把「开启中」这一帧定住
 *   ctBroken=1             让 set_click_through 即便收到可交互区也回 false
 *                          ⇒ 复现控制台那条「穿透未生效」
 *                          （用法：`click=ct&ctBroken=1&settle=2600`，
 *                            settle 必须大于 2.2s 的宽限期，否则量到的是"还没到期"）
 *   ctPending=1            让配置里 `click_through` 就是 true（走**启动路径**）
 *                          ⇒ 不必点开设置抽屉，控制台整条都能看见那行琥珀字
 *                          （`click=ct` 那条路会被 352px 宽的抽屉盖住状态槽）
 *                          须配 ctBroken=1，否则启动那次调用会回 true 把 pending 清掉
 */
const stealthMode = params.get("stealth") || "on";
const stealthStarting = stealthMode === "starting";
const slow = params.get("slow") === "1";

/**
 * `warn=<key>` 要推的示例警告。
 *
 * 底栏那条常驻警告的标签现在是**生产者给的**（见 `StatusWarning`）。
 * 推一条真事件，才能看出"它会显示成什么"—— 而不是相信它显示对了。
 */
const WARN_SAMPLES = {
  mic: "麦克风在送数字静音：设备看着正常，但采到的样本振幅恒为 0。\n请检查系统输入设备或换一个麦克风。",
};

/** 慢速应答：mock 一个"下发了指令、后端还没回"的窗口期。 */
const never = () => new Promise(() => {});

const STEALTH_REPORTS = {
  on: {
    mode: "hidden",
    applied: true,
    detail: "已启用窗口内容保护（WDA_EXCLUDEFROMCAPTURE）",
  },
  paused: {
    mode: "visible",
    applied: true,
    detail: "内容保护已暂停：窗口会重新出现在投屏/录屏里",
  },
  unapplied: {
    mode: "hidden",
    applied: false,
    detail:
      "SetWindowDisplayAffinity 调用失败（错误码 5）。常见原因：窗口尚未创建完成、或系统版本低于 Windows 10 2004。",
  },
  starting: {
    mode: "hidden",
    applied: false,
    detail: "正在切换内容保护…",
  },
  off: {
    mode: "visible",
    applied: false,
    detail: "未启用窗口内容保护",
  },
};

const config = {
  asr: {
    // `engine=auto` ⇒ 推进方式切到「匀速滚动」，控制台那一格会换成速度条。
    // 语音那两条（local / cloud）留给「语音跟随」的用例。
    engine: params.get("engine") || "local",
    local_model: params.get("noModel")
      ? null
      : {
          encoder: "encoder.onnx",
          decoder: "decoder.onnx",
          joiner: "joiner.onnx",
          tokens: "tokens.txt",
          label: "sherpa-onnx-streaming-zipformer-bilingual",
        },
    cloud: null,
    num_threads: 2,
  },
  stealth_on_start: stealthMode !== "off",
  font_size: 34,
  opacity: 0.94,
  line_height: 1.7,
  mirror: false,
  dim_read: true,
  hotkeys: {
    next_sentence: "CommandOrControl+Down",
    prev_sentence: "CommandOrControl+Up",
    toggle_stealth: "CommandOrControl+Alt+S",
    toggle_visibility: "CommandOrControl+Alt+V",
    toggle_recognition: "CommandOrControl+Alt+R",
    toggle_collapse: "CommandOrControl+Alt+M",
    toggle_click_through: "CommandOrControl+Alt+P",
  },
  last_script: null,
  window: { width: 560, height: 360, x: null, y: null, locked: false },
  padding_x: 28,
  letter_spacing: 0.01,
  anchor: 0.36,
  align: "left",
  theme: params.get("theme") || "dark",
  /*
   * 背景磨砂档位（第十二轮，前身 `glass: true`）。
   * 与 Rust `GlassMode` 的 serde 值逐字一致：off / css / system。
   * `glass=system` 预置成系统磨砂档（桩同时会把 `glassEffects` 摆成已挂，
   *   与真机"启动时按配置挂材质"同构 —— 见 lib.rs setup 里的 apply_glass_mode）。
   */
  glass_mode: params.get("glass") || "css",
  /*
   * `ctPending=1`：模仿"配置里穿透是开着的，但后端没把它跑起来"。
   *
   * ★ 为什么不复用 `click=ct`：那条路会**先把设置抽屉点开**，而抽屉 352px 宽、
   *   正好盖住控制台第一行那个状态槽 ⇒ 截出来看不到那行琥珀字，
   *   一张"什么都没发生"的图（看着像功能没做）。
   *   这条走的是**启动路径**（`App.tsx` 把 `config.click_through` 直接当作
   *   `ctPending` 初值），完全不碰设置，控制台整条都能看见。
   *
   *   用法：`ctPending=1&ctBroken=1&settle=2600`（`ctBroken` 负责让后端回 false，
   *   否则启动时那次 `set_click_through` 会回 true 把 pending 清掉）。
   */
  click_through: params.get("ctPending") === "1" ? true : false,
  bar_pinned: true,
  match_floor: 0.38,
  wpm_ref: 300,
  // 匀速滚动速度。`wpm=420` 能看速度条拖到右边、以及"剩余时间"跟着变。
  auto_wpm: Number(params.get("wpm")) || 300,
  /*
   * 跟读手感（设置 › 跟读那一组）。
   *
   * ★ 桩里的 config 是**手写的**，而后端的 `AppConfig` 每加一个字段，
   *   这里就得跟着加一个 —— 漏了不会报错，界面上是这样：
   *   滑块显示 `undefined`、开关一律显示"关"。
   *   看着像"新功能没做"，其实只是桩没跟上。
   *
   * 取值与 Rust 侧 `AppConfig::default()` 对齐（20 / green / true / true），
   * 这样桩里量出来的默认态就等于真机首次启动的样子。
   * 用 `read=` / `jump=` / `smooth=0` / `quick=0` 可以摆成别的状态。
   */
  jump_range: Number(params.get("jump") || 20),
  read_color: params.get("read") || "green",
  smooth_scroll: params.get("smooth") !== "0",
  quick_jump: params.get("quick") !== "0",
  /*
   * 窗口置顶。★ 这一项**归后端写**（`set_always_on_top`），前端只读 ——
   * 桩里那个假的 set_always_on_top 必须跟着改这份 config，
   * 否则"点开关 → 关掉抽屉 → 再打开，开关还亮着吗"会答错。
   * 取值与 Rust 侧默认对齐（false），`ctPin=1` 摆成已置顶。
   */
  always_on_top: params.get("pin") === "1",
};

const emptyScript = { title: "", paragraphs: [], total_chars: 0 };

/*
 * 窗口置顶的**真实窗口属性**。
 *
 * ★ 它和 `config.always_on_top` 是两层东西：一层是"窗口现在是不是置顶"
 *   （设置里那个开关读的就是它），一层是"配置里记下的偏好"（决定下次启动
 *   要不要自动置顶）。桩里两层都要有，否则"点一下开关 → 关掉抽屉再打开，
 *   它还亮着吗"会答错。
 */
let pinFlag = params.get("pin") === "1";

/*
 * 「系统磨砂」当前挂在窗口上的材质 —— 桩里的**窗体合成层**。
 *
 * ★ 与 pinFlag 同一个道理：config.glass_mode 是"配置里记下的档位"，
 *   这个变量是"窗口现在真的挂没挂材质"。真后端在 save_config 里检测档位
 *   变化并调 set_effects（见 commands::apply_glass_mode）；桩这里模拟同一
 *   动作，让门槛能量到「切到关之后，材质是真的被卸了」（P0-6），
 *   而不是只看到一个"已保存"的假象。
 *
 * 取值与 win::effects::plan_for 同源：windows ⇒ "acrylic"；无 ⇒ "none"。
 */
let glassEffects = params.get("glass") === "system" ? "acrylic" : "none";

/*
 * 平台信息 —— 与后端 `win::platform` 的那张表**逐字一致**。
 *
 * ★ 为什么在桩里也维护一份：macOS 与 Windows 的**界面用语不同**，
 *   而"文案对不对"正是那种改了看不出来的东西（写错了界面照样渲染）。
 *   有了它，`ui-preview.py` 可以 `plat=macos` 直接把 mac 版面量出来：
 *   收起入口只剩一颗按钮、并且文案里**不再出现「任务栏」**。
 *
 * ★ 键名与 `win::platform::platform_key()` 同源（windows / macos / other）。
 */
function platformInfo(p) {
  const os = p.get("plat") || "windows";
  if (os === "windows") {
    return {
      os: "windows",
      support: { kind: "native" },
      fallback_hint: null,
      collapse_label: "收起",
      collapse_hint: "收起＝留一条细栏；收进任务栏＝缩下去、任务栏留按钮",
      has_taskbar: true,
    };
  }
  return {
    os,
    support: os === "macos"
      ? { kind: "native" }
      : { kind: "unavailable", note: "当前平台没有可用的窗口级捕获保护方案。" },
    fallback_hint: os === "macos" ? null : "建议使用「第二设备」模式。",
    collapse_label: "收起窗口",
    collapse_hint: "收起＝只留顶栏一条，正文让开；点「展开」原样回来",
    has_taskbar: false,
  };
}

/*
 * 「收进任务栏」有没有**真的走到后端**。
 *
 * ★ 为什么要单独量它：按钮点了、提示也弹出来了，但 `api.minimizeToTaskbar()`
 *   可能压根没被调到（名字写错也一样 —— 桩对不认识的命令是
 *   `Promise.resolve(undefined)`，**静默成功**）。
 *   只验"提示出现了"的话，一个空函数体也能通过。
 */
let taskbarCalled = false;
/*
 * 点下去 300ms 时抓到的那条提示文本。
 *
 * ★ `minimize_to_taskbar` 是在提示播完 1.7s 之后才调的，而提示本身还会自动消失，
 *   所以"什么时候去量"会直接决定量得着量不着 —— 把文本**在点击后不久就快照下来**，
 *   后面无论 settle 给多大都拿得到，不靠调用方把时间掐准。
 */
let taskbarToastSnap = "";

const bootstrap = {
  config,
  script: params.get("noScript")
    ? emptyScript
    : { title: "2026 上半年经营汇报", paragraphs, total_chars: totalChars },
  cursor: params.get("noScript")
    ? { cursor: 0, total: 0, progress: 0, paragraph: 0, sentence: 0, offset_in_sentence: 0,
        sentence_align_start: 0, sentence_align_end: 0, score: 0, moved: false,
        miss_streak: 0, realigned: false }
    : { cursor: 61, total: totalChars, progress: 61 / totalChars, paragraph: 1, sentence: 0,
        // `off=<n>`：当前句里已经念到第几个字。
        // ★ 默认 8 恰好落在"这一句正好 8 个字"上 ⇒ 没有"正在念的那个字"，
        //   于是 `.singing` 那段量出来是 none（看着像没实现）。
        //   要验它，传一个小于句长的值（如 off=3）。
        offset_in_sentence: Number(params.get("off") || 8),
        sentence_align_start: 53, sentence_align_end: 92,
        score: 0.91, moved: true, miss_streak: 0, realigned: false },
  // ★ `plat=` 查询串切平台：`windows`（默认）/ `macos` / `other`。
  //   两个平台的**界面用语不同**（macOS 上没有"任务栏"这个词，收起入口只有一颗按钮），
  //   所以文案由桩按真实后端 `win::platform` 的那张表给出来 —— 预览里能直接看 mac 版面。
  platform: platformInfo(params),
  stealth: {
    ...STEALTH_REPORTS[stealthMode],
    support: { kind: "native" },
    fallback_hint: null,
  },
  models_dir: "C:\\Users\\26611\\AppData\\Roaming\\cn.bobby.prompter\\models",
  found_models: params.get("noModel")
    ? []
    : [
        {
          encoder: "e.onnx",
          decoder: "d.onnx",
          joiner: "j.onnx",
          tokens: "tokens.txt",
          label: "sherpa-onnx-streaming-zipformer-bilingual",
        },
      ],
  asr_engine: "sherpa-onnx（本地）",
  app_version: "0.1.0",
  script_exts: ["txt", "md", "markdown", "docx", "pptx"],
  script_exts_hint: "txt / md / docx / pptx",
};

const scriptEntry = {
  id: "s1",
  source: "file",
  path: "D:\\稿件\\2026 上半年经营汇报.docx",
  title: "2026 上半年经营汇报",
  chars: totalChars,
  last_used: nowSec - 3600,
  use_count: 3,
  copy_rel: null,
};

const preflight = [
  { key: "稿件", verdict: "已载入 4 段 / 178 字", level: "ok", hint: "" },
  { key: "投屏隐身", verdict: "已生效", level: "ok", hint: "" },
  {
    key: "麦克风",
    verdict: "拾音正常",
    level: "ok",
    hint: "",
  },
  { key: "语音模型", verdict: "1 个可用", level: "ok", hint: "" },
];

/* 命令表。只实现界面会真正读返回值的那几个，其余一律 resolve。 */

/*
 * ★ 记下 `report_interactive_zone` 的每一次调用。
 *
 * 这条上报是「鼠标穿透」能不能触发的**唯一**前提（后端据此才知道控制区在哪）。
 * 而在真机上它几乎不可观测：前端只管发，发失败了被 `.catch` 吞掉，
 * 表现就是"穿透开关点了没反应"——最费时间的那一类。
 * 在桩里把参数存下来、写进 <title>，就能离线回答两个问题：
 *   「前端到底报没报？」「报的是几块、坐标是多少？」
 * 缓存 `--measure` 的输出里会带 `zoneCalls=` / `zoneN=` / `zoneRects=`。
 */
const zoneReports = [];

/*
 * ★ 记下每一次 `goto`（＝「把进度接到这一句」的落点）。
 *
 * 这条路的失败方式是**静默**的：它本来就该"点了就有反应"，没反应只能得出
 * "这个功能坏了"。而它在真机上几乎没法定量 —— 截图证明不了"我点的那一下
 * 到底落在第几句、有没有落在字上"。把参数存下来写进 <title> 就能离线回答：
 *   「点在行距上（不是字形上）算不算命中？」「拖选会不会把进度误跳走？」
 * 缓存输出里会带 `gotoN=` / `gotoPos=`。
 */
const gotoPos = [];

const COMMANDS = {
  bootstrap: () => bootstrap,
  report_interactive_zone: (a) => {
    zoneReports.push(a && "rects" in a ? a.rects : null);
    return undefined;
  },
  list_scripts: () => [scriptEntry],
  script_availability: () => [[scriptEntry.id, true]],
  preflight: () => preflight,
  get_config: () => config,
  /*
   * 整份写配置。桩以前对它静默 `undefined`（"保存成功"的假象）——
   * 第十二轮起不行：真后端会在这里检测**磨砂档位变化**并挂/卸窗体材质
   * （commands::apply_glass_mode），P0-6 的验收就是"切到关，材质真没了"。
   * 桩模拟同一个动作，把结果记在 `glassEffects` 上供量测。
   */
  save_config: (a) => {
    const mode = a && a.config && a.config.glass_mode;
    if (mode && mode !== config.glass_mode) {
      config.glass_mode = mode;
      glassEffects = mode === "system" ? "acrylic" : "none";
    }
    return undefined;
  },
  /*
   * 置顶。**同时改窗口属性与配置**，并回一个布尔 —— 与 Rust 侧
   * `set_always_on_top` 同构。两边形状不一致的话，"开关显示的是窗口实际状态"
   * 这条就无从验起。
   */
  set_always_on_top: (a) => {
    pinFlag = !!(a && a.on);
    config.always_on_top = pinFlag;
    return pinFlag;
  },
  /*
   * 收进任务栏。桩只记"被调到了"，不模拟窗口行为 ——
   * 任务栏上有没有那枚按钮是 Windows 的事，离线验不了；
   * 那一段由真机脚本 `scripts/check-taskbar-button.py` 负责。
   */
  minimize_to_taskbar: () => {
    taskbarCalled = true;
    return undefined;
  },
  /*
   * ★ 注意：这个命令前端**启动时并不调用** —— `running` 走的是
   *   `prompter://asr-status` 事件（见 `App.tsx` 的 `events.onAsrStatus`）。
   *   想做"提词中"的预览态请用查询串 `prompting=1`（它发那条事件），
   *   别改这里的返回值 —— 改了也没人读。
   */
  asr_status: () => ({ running: false, engine: "sherpa-onnx（本地）", detail: "" }),

  /* 收起态：`collapsed=1` 时从启动就是收起的（同名开关见文件下部的说明）。 */
  is_collapsed: () => params.get("collapsed") === "1",
  models_dir: () => bootstrap.models_dir,
  rescan_models: () => bootstrap.found_models,
  /*
   * 打开外部链接（设置里的「支持作者 › 海外用户：GitHub 赞助」）。
   *
   * ★ 桩必须**记下来**，不能像没接的命令那样回个 `undefined` 了事：
   *   前端那条路唯一可见的证据就是"后端被调到了"。桩要是静默吞掉，
   *   "点了没反应"与"点了但压根没接后端"在离线预览里长得**一模一样**
   *   —— 而前者正是本项目最忌讳的失败模式。
   *   记到 `window.__openedUrls`，由 `donateclick` 那一步读出来报进 diag。
   */
  open_url: (a) => {
    (window.__openedUrls = window.__openedUrls || []).push(a && a.url);
    return undefined;
  },
  diagnostics: () => [
    ["版本", "0.1.0（离线预览）"],
    ["平台", "windows x86_64"],
    ["隐身支持", "原生（WDA_EXCLUDEFROMCAPTURE）"],
    ["模型目录", bootstrap.models_dir],
    ["识别引擎", "sherpa-onnx（本地）"],
  ],
  match_floor: () => config.match_floor,
  set_match_floor: (a) => a.value,
  jump_range: () => config.jump_range,
  /* 「接进度」。返回值形状照抄 Rust 的 `cursor_payload` —— 前端会把它塞进 store，
   * 形状不对会让 CursorPayload 的消费方读到 undefined（看着像"点了没用"）。 */
  goto: (a) => {
    const pos = a && typeof a.pos === "number" ? a.pos : -1;
    gotoPos.push(pos);
    return { ...bootstrap.cursor, cursor: pos, progress: pos / (bootstrap.cursor.total || 1) };
  },
  /*
   * 夹取边界与 Rust 侧 `align::JUMP_RANGE_RANGE` 一致。
   * ★ 这是同一个数的**第三处**出现（Rust 常量 / 设置页滑杆的 min-max / 这里）。
   *   前两处是必须的；这一处是桩的代价 —— 桩要能像真后端那样"回一个夹取后的值"，
   *   否则测不出「滑到边界外会被引擎纠正」这条路径。改 Rust 常量时记得看这里。
   */
  set_jump_range: (a) => Math.max(1, Math.min(40, a.value)),
  // 与后端 `auto_scroll::clamp_wpm` 同一个区间（60~600）。
  set_auto_wpm: (a) => Math.max(60, Math.min(600, a.value)),
  /*
   * 穿透：返回「**已经在工作**吗」——与真后端同一个语义
   * （`ct_working` = 开关开着 && 可交互区可信），不是"用户按下的意图"。
   *
   * ★ 别在这里直接回 `true`：那会把"点了、但还没生效"这条**最该被测到**的
   *   路径藏掉。桩里前端是真的会上报可交互区的（见 report_interactive_zone
   *   的注释），所以这里可以如实回答。
   *
   * ★ `ctBroken=1`：模拟"可交互区报了、但后端仍然判它没生效"。
   *   本轮补的那条控制台状态（`穿透未生效`）**只有**这一条路径能复现 ——
   *   正常跑时 zoneReports 非空 ⇒ 一定回 true，那条分支永远走不到，
   *   于是就会变成"写了但从没验过"。
   */
  set_click_through: (a) => {
    if (!a.enabled) return false;
    if (slow) return never(); // 定住「穿透 · 开启中」这一帧
    if (params.get("ctBroken") === "1") return false;
    return zoneReports.some((r) => Array.isArray(r) && r.length > 0);
  },
  /*
   * 隐身。`applied` 的语义与真后端一致：**系统确认当前是保护态**，
   * 不是"调用了 API"。所以 `stealth=unapplied` 那一档刻意返回
   * `mode: hidden` + `applied: false` —— 这正是真机失败时的形状，
   * 也是"以为开了其实没开"最需要被界面说出来的那一种。
   */
  set_stealth: (a) => {
    // `slow=1` 也让它悬住：这是复现「演示 · 就位中」的办法 ——
    // 一键演示的第一步就是开隐身，它不回来，pill 就停在扫光动画上。
    if (stealthStarting || slow) return never();
    const base = !a.enabled
      ? STEALTH_REPORTS.paused
      : stealthMode === "unapplied"
        ? STEALTH_REPORTS.unapplied
        : STEALTH_REPORTS.on;
    return { ...base, support: { kind: "native" }, fallback_hint: null };
  },
  start_asr: () => ({
    running: true,
    engine: "sherpa-onnx（本地）",
    detail: "",
  }),
  stop_asr: () => ({
    running: false,
    engine: "sherpa-onnx（本地）",
    detail: "",
  }),
  mic_check: () => ({
    device: "麦克风阵列 (Realtek)",
    sample_rate: 48000,
    channels: 1,
    format: "f32",
    seconds: 2.5,
    frames: 120000,
    peak: 0.42,
    rms: 0.11,
    verdict: "ok",
    detail: "拾音正常，可以开始识别。",
    devices: ["麦克风阵列 (Realtek)", "立体声混音 (Realtek)"],
  }),
};

let cbId = 0;
const callbacks = new Map();

/*
 * 事件名 → `transformCallback` 给的回调 id。
 *
 * ★ 为什么要记这个：桩原先对 `plugin:event|listen` 只回一个 id、从不真推事件 ——
 *   于是**所有靠后端事件驱动的界面**（底栏那条警告、未就位提示、
 *   托盘切稿…）在离线预览里永远看不到，只能靠读代码相信它。
 *   记下来之后 `emit()` 就能把一条真事件送进前端的同一条代码路径。
 */
const eventHandlers = new Map();

/** 把一条事件推给前端 —— 形状与 Tauri v2 的 EventCallback 参数一致。 */
function emitTauri(event, payload) {
  const id = eventHandlers.get(event);
  if (id === undefined) {
    diagEmit.push(`no-listener:${event}`);
    return false;
  }
  const entry = callbacks.get(id);
  if (!entry) {
    diagEmit.push(`no-callback:${event}`);
    return false;
  }
  entry.cb({ event, id, payload });
  return true;
}

const diagEmit = [];

window.__TAURI_INTERNALS__ = {
  metadata: {
    currentWindow: { label: "main" },
    currentWebview: { windowLabel: "main", label: "main" },
  },
  transformCallback(cb, once = false) {
    const id = ++cbId;
    callbacks.set(id, { cb, once });
    return id;
  },
  unregisterCallback(id) {
    callbacks.delete(id);
  },
  convertFileSrc: (p) => p,
  invoke(cmd, args = {}) {
    // plugin:event|listen / unlisten 是 listen() 的实现细节，回个 id 就行
    if (cmd === "plugin:event|listen") {
      eventHandlers.set(args.event, args.handler);
      return Promise.resolve(++cbId);
    }
    if (cmd === "plugin:event|unlisten") return Promise.resolve();
    /*
     * 置顶也要有**窗口属性**这一层，不能一律回 undefined。
     *
     * 设置里那个开关读的是**窗口**（`isAlwaysOnTop()`），不是 config ——
     * 全回 undefined 的话它永远显示"关"，于是"点一下开关，状态真的变了吗"
     * 这个问题在桩里根本答不出来（看着像开关坏了，其实是桩没接）。
     */
    if (cmd === "plugin:window|is_always_on_top") return Promise.resolve(pinFlag);
    if (cmd === "plugin:window|set_always_on_top") {
      pinFlag = !!(args && args.value);
      return Promise.resolve();
    }
    if (cmd.startsWith("plugin:window|")) return Promise.resolve(undefined);
    /*
     * 应用内更新。
     *
     * ★ 默认**报"没有更新"**（返回 `undefined`，前端 `check()` 于是拿到 `null`）
     *   —— 那才是真正的常态，而且它让预览出来的界面与"还没发过新版"时逐像素一致。
     *   这一点很重要：如果不接这个分支、让 `COMMANDS` 兜底，返回值同样是 undefined，
     *   但那是"碰巧一样"，下次谁改了兜底就会静默变味。
     *
     * ★ 想看提示条长什么样，加 `update=<版本号>`：桩就报一个新版本，
     *   前端那条提示条会在启动后 3 秒（`App.tsx` 里的延后，产品决定）浮出来。
     *   ⇒ **必须再配 `settle=3200`** 才截得到 —— 这是桩唯一的用法要点。
     *   不配的话量到的是"提示条还没长出来"，看起来像功能没做。
     */
    if (cmd === "plugin:updater|check") {
      const v = params.get("update");
      return Promise.resolve(
        v
          ? {
              rid: ++cbId,
              currentVersion: "0.1.0",
              version: v,
              body: "（桩给的更新说明）",
            }
          : undefined,
      );
    }
    if (cmd === "plugin:updater|close") return Promise.resolve();
    if (cmd.startsWith("plugin:updater|")) return Promise.resolve(undefined);
    if (cmd.startsWith("plugin:process|")) return Promise.resolve(undefined);
    const fn = COMMANDS[cmd];
    if (fn) return Promise.resolve(fn(args));
    return Promise.resolve(undefined);
  },
};

/* --------------------------------------------------------------- 驾驶脚本 */

function waitFor(sel, cb, tries = 300) {
  const el = document.querySelector(sel);
  if (el) return cb(el);
  if (tries <= 0) {
    document.title = "PREVIEW_FAIL:" + sel;
    return;
  }
  setTimeout(() => waitFor(sel, cb, tries - 1), 25);
}

/**
 * 顶栏第 n 颗图标按钮（`n` 从 0 起）—— **按位置找，不再是按文字找**。
 *
 * ★★★ 这里换过两代，第二代的失败值得记下来（2026-09-28 第十一轮）。
 *
 * **第一代**：按钮上有原生 `title`，桩就按 title 文字找
 *  （`byTitle(".icon-btn", "设置")`）。
 *
 * **第二代**：第十轮把 `title` 清零、换成自绘气泡，于是给每个触发元素摊了
 *   一个 `data-hint`（说明首行），桩改读它 —— 还是"按文字找"。
 *   这一代当时就踩了一次坑（详见下面 `doc` 那条的注释：`"稿件"` 会命中
 *   查找按钮的 `data-hint`「在稿件里查找（Ctrl+F）」），靠改用 `"Ctrl+K"`
 *   绕过去了。
 *
 * **第三代（现在）**：第十一轮**提示整个删了** ⇒ `data-hint` 也没了。
 *   如果继续"按文字找"，`byTitle()` 会全线返回 `undefined`、
 *   `el?.click()` 静默什么都不做，`diag` 里只留一个 `tap1=none` ——
 *   而这正是本项目最忌讳的失败模式（"点了没反应"），**而且它藏在桩里、
 *   不在产物里**，任何产物侧的门槛都拦不住。
 *
 * ⇒ 结论：**桩不该依赖界面上的文字**。文字是产品文案，会随用户口味变
 *   （用户三番两次让改的就是它）；而"顶栏从左数第几颗、它是什么 class"
 *   是**结构**，结构才是点击步骤真正该锚定的东西。
 *
 * 顶栏布局（见 `TopBar.tsx`）：`.st-pill` 隐身灯 → `.title` → `<Transport/>`
 * → `.icon-btn` × N（查找 / 稿件 / 设置 / [收进任务栏] / 关闭）。
 * 所以图标按钮用 0 / 1 / 2 … 定位，末两颗用 `iconFromEnd()` 从右数。
 *
 * ★ 代价要说清：**插入或删掉一颗图标，这些下标就会静默指错**。
 *   缓解办法是每一步都往 `diag` 里报出该按钮的 `aria-label`（有的话）
 *   或 class，让人能一眼看出"点到的不是我以为的那颗"。
 *   这是刻意选的取舍：产品文案比图标数量**更容易变**，锚结构更稳。
 */
function iconAt(n) {
  return document.querySelectorAll(".icon-btn")[n] || null;
}

/** 从右数第 n 颗顶栏图标（`n` 从 0 起）：0＝关闭，1＝收进任务栏。 */
function iconFromEnd(n) {
  const els = document.querySelectorAll(".icon-btn");
  return els[els.length - 1 - n] || null;
}

/** 这一颗叫什么（给 diag 用）—— 让"点错了"能被看出来。 */
function iconName(el) {
  if (!el) return "none";
  return el.getAttribute("aria-label") || el.className || "?";
}

/**
 * 按 `.grp-t` 标签文字找设置里的一行。
 *
 * ★ 与上面那段的判据不同：**这里的文字不是提示，是控件的名字**
 *   （「窗口置顶」「跟随灵敏度」）。一行可以在组里上下移动，但它叫什么
 *   不会因为别处增删而变 —— 所以按名字找是对的，按位置找反而是错的。
 */
function rowByLabel(text) {
  return (
    Array.from(document.querySelectorAll(".grp-row")).find((r) =>
      (r.querySelector(".grp-t")?.textContent || "").includes(text),
    ) || null
  );
}

/*
 * 等「顶栏第一批图标按钮」出现才开始 —— 它比 `.topbar` 更晚：
 * `.topbar` 一挂载就有了，而图标要等 bootstrap 回来的 config 渲染完才有。
 * 早于这个时机会点空（icons.length === 0），表现为"点了没反应"。
 *
 * ★★ 这里等的东西换过一次（2026-09-28 第十一轮）：原先是 `.hint-ico`
 *   （提示图标），第十一轮把提示整个删了 ⇒ 这个选择器**永远等不到**，
 *   于是 `waitFor` 在 300×25ms ≈ 7.5s 后写 `PREVIEW_FAIL:.hint-ico`，
 *   桩一句话都没执行、`title` 里一个 `=` 都没有。
 *   ⇒ 症状是**产物侧门槛全线报"预览没跑起来（先 npm run build 试试）"** ——
 *   一个假的、会让人反复重 build 前端的提示（同 `check-platform-layout.py`
 *   文件头记的那个 `--out nul` 坑，同一族）。
 *   ⇒ 等的东西必须是**结构的、且这轮不会消失**的：`.icon-btn`。
 */
waitFor(".topbar .icon-btn", () => {
  const steps = [];
  const diag = [];

  /*
   * 去掉所有空白。
   *
   * 测量结果是通过 `<title>` 回传的，格式是空格分隔的 `key=value` ——
   * 值里只要有空格就会被劈成两个 token（`▶ 演示` 就是最典型的受害者）。
   *
   * ★ 定义在**这里**（而不是收尾那个块里）是因为驾驶脚本也要用：
   *   点演示 pill 的前后各要读一次按钮文字，而那两处都在这个回调的作用域内，
   *   拿不到收尾块里的局部 const —— 写在那儿会直接 ReferenceError，
   *   而桩一抛异常只会**静默**变成一张空图（title 不回话、截图是白的）。
   */
  const nowrap = (v) => String(v).replace(/\s+/g, "");

  /** 设置里那个「窗口置顶」开关现在是开是关（按 class 读）。 */
  const pinSwitch = () => {
    const row = Array.from(document.querySelectorAll(".grp-row")).find((r) =>
      (r.querySelector(".grp-t")?.textContent || "").includes("窗口置顶"),
    );
    const sw = row?.querySelector(".switch");
    if (!sw) return "none";
    return sw.classList.contains("on") ? "on" : "off";
  };

  /**
   * 像真手指那样按一下：`mousedown` → （隔一拍）→ `mouseup` → `click`。
   *
   * ★★ 两个细节都不是装饰，缺一个这条断言就废：
   *   ① 不能用 `el.click()` —— 它**只发 click**，不发 mousedown。而「点面板外面
   *      就把它关掉」恰恰挂在 mousedown 上，于是用 `.click()` 去验"再点一下收起来"，
   *      **旧代码也会通过**（本轮真踩）。
   *   ② `mouseup`/`click` 必须**推后一个宏任务**。真浏览器里这三下之间隔着真实的
   *      task 边界，React 18 的批处理会在中间把重渲染 flush 掉——**bug 就藏在那一下
   *      flush 里**（mousedown 把状态置 false，重渲染后 onClick 的闭包变成"现在关着"，
   *      于是 click 把它取反成开）。同一个 task 里连着发三个事件，React 不会插进来
   *      重渲染，闭包还是旧的 ⇒ **复现不出来**，正反对照双双通过，等于没验。
   */
  const realTap = (el, onDone) => {
    const r = el.getBoundingClientRect();
    const o = {
      bubbles: true,
      cancelable: true,
      clientX: r.left + r.width / 2,
      clientY: r.top + r.height / 2,
      button: 0,
    };
    el.dispatchEvent(new MouseEvent("mousedown", o));
    setTimeout(() => {
      el.dispatchEvent(new MouseEvent("mouseup", o));
      el.dispatchEvent(new MouseEvent("click", o));
      onDone?.();
    }, 30);
  };


  if (params.get("open") === "settings") {
    steps.push(() => {
      const btn = iconAt(2);
      diag.push("btn=" + (btn ? "found" : "none"), "btnName=" + iconName(btn));
      btn?.click();
    });
  }

  /*
   * 兜底：用一条 CSS 把抽屉按出来。
   *
   * 抽屉的可见性只由 `.drawer.open`（translateX(0)）决定。点击那条路依赖
   * 「React 已经挂载 + 状态已 flush」，时序上不一定每次赶得上 ——
   * 于是就可能截到一张「抽屉没开」的空图，看起来像功能坏了。
   * 这条 !important 让它与状态无关：只要要看外观，就一定看得到。
   */
  if (params.get("open") === "settings") {
    const st = document.createElement("style");
    st.textContent = ".drawer{transform:translateX(0) !important}";
    document.head.appendChild(st);
    diag.push("css=forced");
  }

  // 滚到抽屉底部 —— 用来验证"图标靠近窗口下沿时气泡向上翻转"这条分支，
  // 它是常态而非兜底：设置里靠底部的行占了一大半。
  if (params.get("scroll") === "bottom") {
    steps.push(() => {
      const b = document.querySelector(".drawer-body");
      if (b) {
        b.scrollTop = b.scrollHeight;
        diag.push("scrolled=yes");
      }
    });
  }

  /*
   * `fold=open` —— 把设置里**默认折叠**的那一组（「更多」）点开。
   *
   * ★ 为什么需要：本轮「推进方式」是个三选一，它决定了「更多 › 语音跟随」
   *   那一小节**在不在**（匀速滚动/纯手动时它整节不该出现）。那一节默认
   *   折叠在「更多」里 —— 不看它，就只能验到"我点了选中的那格变了"，
   *   验不到"跟着该出现的那一节有没有出现、该消失的有没有消失"。
   *   而"控件以为自己在三个模式里长得一样"正是这类改动的典型缺陷。
   *
   * 走真实点击（`grp-h.foldable`），不是塞状态 —— 参照 `click=*` 的处置。
   */
  if (params.get("fold") === "open") {
    steps.push(() => {
      const heads = Array.from(document.querySelectorAll(".drawer .grp-h.foldable"));
      for (const h of heads) h.click();
      diag.push(`foldable=${heads.length}`);
    });
  }

  const click = params.get("click") || "";
  /*
   * ★ 这里原本有一条 `click=hint:N`（点第 N 个提示图标）的驾驶分支。
   *   第十一轮把提示整个删了 ⇒ 分支里的 `.hint-ico` 永远查不到、静默 no-op。
   *   没有任何脚本还在用它，**整条删掉**而不是留着发霉 —— 一条永远选不中
   *   东西的分支，下次有人照它写断言时会得到一个"点了没反应"的假象。
   */

  /*
   * 点几个**真按钮**（走真实代码路径，不是把状态塞进 store）——
   * 后者只能验样式，验不到"点一下到底发生了什么"。
   *
   * ★ 本轮改版后，顶栏只剩一个状态灯（`.st-pill`），
   *   而「演示」和「穿透」都换了家：
   *     演示 → 底部控制台正中那个大圆按钮 `.hero-btn`（改名「开始提词」）
   *     穿透 → 设置 › 窗口里的一行开关（顶栏那个文字胶囊被摘掉了）
   *   所以这三个入口的选法各不相同，见下。
   */
  const PILL_SEL = {
    stealth: ".st-pill",
    demo: ".hero-btn",
  };
  if (PILL_SEL[click]) {
    steps.push(() => {
      const el = document.querySelector(PILL_SEL[click]);
      diag.push(`pill:${click}=${el ? "found" : "none"}`);
      el?.click();
    });
  }

  /*
   * 穿透现在只在设置里。先开设置，再按**那一行的标签文字**找到它的开关 ——
   * 按位置（第几个 .switch）取会在别人往组里插一行的时候静默点错东西，
   * 而"点错一行"和"功能坏了"在截图上是分不出来的。
   */
  if (click === "ct") {
    steps.push(() => {
      const gear = iconAt(2);
      diag.push(`ctGear=${gear ? "found" : "none"}`, `ctGearName=${iconName(gear)}`);
      gear?.click();
    });
    steps.push(() => {
      const row = Array.from(document.querySelectorAll(".grp-row")).find((r) =>
        (r.querySelector(".grp-t")?.textContent || "").includes("鼠标穿透"),
      );
      const sw = row?.querySelector(".switch");
      diag.push(`ctRow=${row ? "found" : "none"}`);
      sw?.click();
    });
  }

  /*
   * 「快捷跳读」关掉之后 —— 单击**不该**跳，双击才跳。
   *
   * 这条约定很容易在改版时静默丢掉（"关掉不是禁用，而是提高门槛"，见设置页文案），
   * 而它一旦丢掉，用户不会觉得"改版了"，只会觉得"误触被取消了"或"点不动了"。
   * 用法：`quick=0&click=quickoff`
   *   * `offTap`  —— 单击之后 `gotoN` 必须仍是 0
   *   * `offDbl`  —— 双击之后必须变成 1
   *
   * ★★ 名字**不能**叫 `seekoff`：上面那条通用探针是按 `startsWith("seek")`
   *    分派的，`seekoff` 会被它一起收走 —— 于是一次点击额外多跳一次，
   *    而多出来的那一次会伪装成"双击跳了两下"（本轮真踩：作者正照着
   *    `gotoN=4` 找一个不存在的 bug）。分派用前缀匹配时，名字就是接口。
   */
  if (click === "quickoff") {
    const gapPoint = () => {
      const vp = document.querySelector(".viewport");
      const cur = document.querySelector(".viewport [data-current='1']");
      const par = cur && cur.closest(".para");
      if (!par) return null;
      const r = document.createRange();
      r.selectNodeContents(par);
      const lines = [...r.getClientRects()].filter((x) => x.width > 4);
      if (!lines.length) return null;
      const first = lines[0];
      const second = lines.find((x) => x.top > first.top + 1);
      const y = second
        ? (first.bottom + second.top) / 2
        : first.bottom + 4;
      const v = vp.getBoundingClientRect();
      const x = Math.max(first.left, second ? second.left : first.left) + 6;
      return { el: par, x, y: Math.min(Math.max(y, v.top + 2), v.bottom - 2) };
    };
    steps.push(() => {
      const p = gapPoint();
      if (!p) {
        diag.push("offPoint=none");
        return;
      }
      const o = { bubbles: true, clientX: p.x, clientY: p.y, button: 0 };
      p.el.dispatchEvent(new PointerEvent("pointerdown", o));
      p.el.dispatchEvent(new PointerEvent("pointerup", o));
      p.el.dispatchEvent(new MouseEvent("click", o));
      diag.push(`offTap=${gotoPos.length}`);
    });
    steps.push(() => {
      const p = gapPoint();
      if (!p) return;
      const o = { bubbles: true, clientX: p.x, clientY: p.y, button: 0 };
      // 真双击就是两下 click + 一个 dblclick，这里照原样发。
      // ★ 每个 dispatch 后各记一次数：曾经出现过"比事件数多一次"的跳转，
      //   而"哪一下多出来的"靠猜是猜不出来的，只能在事件之间取数。
      p.el.dispatchEvent(new MouseEvent("click", o));
      const n1 = gotoPos.length;
      p.el.dispatchEvent(new MouseEvent("click", o));
      const n2 = gotoPos.length;
      p.el.dispatchEvent(new MouseEvent("dblclick", o));
      const n3 = gotoPos.length;
      diag.push(`dbl1=${n1}`, `dbl2=${n2}`, `dbl3=${n3}`, `offDbl=${gotoPos.length}`);
    });
  }

  /*
   * 置顶开关按一下。
   *
   * 验的是两件事同时发生（这正是这个 bug 的形状）：
   *   ① `pinSw=`   —— 开关自己亮起来（它读的是**窗口**属性）
   *   ② `pinCfg=`  —— 配置里那份也落上了（下次启动才会自动置顶）
   * 改版前只有①（而且关掉抽屉再打开时连①也没有），配置里一个字段都没有。
   */
  if (click === "pin") {
    steps.push(() => {
      const gear = iconAt(2);
      gear?.click();
      diag.push(`pinSw0=${pinSwitch()}`, `pinGear=${iconName(gear)}`);
    });
    steps.push(() => {
      const row = Array.from(document.querySelectorAll(".grp-row")).find((r) =>
        (r.querySelector(".grp-t")?.textContent || "").includes("窗口置顶"),
      );
      diag.push(`pinRow=${row ? "found" : "none"}`);
      row?.querySelector(".switch")?.click();
    });
    steps.push(() => {
      diag.push(`pinSw=${pinSwitch()}`, `pinCfg=${config.always_on_top}`);
    });
  }

  /*
   * 开关动画。
   *
   * 静态截图**看不出"有没有动画"** —— 一张"开着"的图在直切和 340ms 回弹下
   * 长得一模一样。所以这里在**过渡中途取一次样**：
   *   * `swMid` 既不是起点也不是终点 ⇒ 真在动；
   *   * `swMid` 直接等于终点 ⇒ 是直切（或时长被压成 0），动画没生效。
   * 顺带把曲线的时值与缓动函数报出来（它们是"苹果手感"那部分的唯一凭证）。
   */
  if (click === "swanim") {
    // ★ 先开抽屉 —— `.grp-row` 只在抽屉里，不动它下面 `querySelector` 回 none，
    //   症状是 `sw=none`（看着像开关没渲染，其实只是抽屉没开）。
    steps.push(() => {
      iconAt(2)?.click();
      diag.push(`swGear=${document.querySelector(".grp-row") ? "ok" : "miss"}`);
    });
    steps.push(() => {
      const sw = document.querySelector(".grp-row .switch");
      if (!sw) {
        diag.push("sw=none");
        return;
      }
      const cs = getComputedStyle(sw, "::after");
      diag.push(`swDur=${nowrap(cs.transitionDuration)}`);
      diag.push(`swEase=${nowrap(cs.transitionTimingFunction)}`);
      diag.push(`swFrom=${nowrap(cs.transform)}`);
      /*
       * 「按住时抻开」那条**在无头环境里验不了**：合成的鼠标事件不会让元素
       * 进入 `:active`（浏览器只对真实输入设置这个状态）。
       * 能验的只有"这条规则确实编译进了产物" —— 至少挡住"写了但没生效"。
       * 真手感只能真机按一下（这条局限记在文档里，别当成已验）。
       */
      let hasActive = false;
      for (const sheet of document.styleSheets) {
        try {
          for (const r of sheet.cssRules) {
            if (r.selectorText && r.selectorText.includes(".switch:active::after")) {
              hasActive = true;
            }
          }
        } catch {
          /* 读不了的样式表跳过（本产物同源，实际不会走到） */
        }
      }
      diag.push(`swActiveRule=${hasActive ? "yes" : "no"}`);
      sw.click();
      setTimeout(() => {
        diag.push(`swMid=${nowrap(getComputedStyle(sw, "::after").transform)}`);
      }, 90);
      setTimeout(() => {
        diag.push(`swEnd=${nowrap(getComputedStyle(sw, "::after").transform)}`);
      }, 700);
    });
  }

  /*
   * 主角按钮**点两次**：开始 → 暂停。
   *
   * ★ 为什么必须真点两次、而不是把一个 paused 状态塞进 store：
   *   最该验的正是"第二下按下去到底发生什么"—— 上一版第二下是收摊
   *   （按钮退回「▶ 演示」），现在是暂停（按钮变蓝）。
   *   塞状态只能验样式，验不到这条状态机。
   * ★ 中间隔 800ms：第一次点击要等编排里几个 await 走完、`busy` 落回 false。
   *   否则第二次点会被 `disabled` 吃掉（点了没反应），
   *   而症状恰恰是"看起来像暂停没做"。
   * ★ 主角按钮**没有文字**（它是一个图标），所以这里读的是 class 而不是
   *   textContent —— 状态全在 class 上（`on` / `paused` / `starting`）。
   */
  if (click === "demo2") {
    steps.push(() => {
      const el = document.querySelector(PILL_SEL.demo);
      diag.push(`hero1=${el ? nowrap(Array.from(el.classList).join(".")) : "none"}`);
      el?.click();
      setTimeout(() => {
        const el2 = document.querySelector(PILL_SEL.demo);
        diag.push(`hero2=${el2 ? nowrap(Array.from(el2.classList).join(".")) : "none"}`);
        el2?.click();
      }, 800);
    });
  }

  /*
   * 点「收进任务栏」—— 验"点了之后真的走到后端"以及那条提示说了什么。
   * ★ 必须配 `at=`（见文件头的参数说明），否则点击发生在 400ms，
   *   而提示 1.7s 后就没了，dump-dom 时什么都截不到。
   *
   * ★★ 这颗键**不在顶栏**（它和「置顶」一起收进了 设置 › 窗口），
   *    所以这里要先开设置、再按行的标签找按钮。
   *    如果照旧去顶栏找，`byTitle` 会返回 none、`el?.click()` 静默什么都不做 ——
   *    截图里看起来完全像是"提示没做出来"，而它其实根本没被点到。
   *    所以下面把落点也一并打出来，让"没找到"显式可见。
   *
   * ★★ 名字从 `tray` 改成了 `taskbar`（2026-09-27）：这个动作的落点从
   *    "收进托盘"改成了"收进任务栏"，键名必须跟着改 ——
   *    键名就是接口，留着旧名字会让下一个人照着它去验一个已经不存在的行为。
   *    （同族纪律：桩里的键名不得撞已有前缀，见文件头的 `seekoff` 那条。）
   */
  if (click === "taskbar") {
    steps.push(() => {
      const gear = iconAt(2);
      diag.push(`taskbarGear=${gear ? "found" : "none"}`);
      /*
       * 同一行里那颗「收进任务栏」就在关闭键左边 —— 顺带确认它在。
       * ★★ 第十一轮：不能按 `data-hint` 文字找了（提示已删）。
       *   改成**从右数第二颗**（0=关闭，1=收进任务栏），并把它的 class
       *   报进 diag —— 序号漂了的话这里会变成 `taskbarTopBtnName=icon-btn danger`
       *   （那是关闭键的 class），一眼能看出来。
       */
      const topbarEntry = iconFromEnd(1);
      diag.push(
        `taskbarTopBtn=${topbarEntry ? "found" : "none"}`,
        `taskbarTopBtnName=${iconName(topbarEntry)}`,
      );
      gear?.click();
    });
    steps.push(() => {
      const row = Array.from(document.querySelectorAll(".grp-row")).find((r) =>
        (r.querySelector(".grp-t")?.textContent || "").includes("收进任务栏"),
      );
      const el = Array.from(row?.querySelectorAll("button") ?? []).find((b) =>
        (b.textContent || "").includes("收进任务栏"),
      );
      diag.push(`taskbarBtn=${el ? "found" : "none"}`);
      el?.click();
      // 提示一出现就先快照下来（见 `taskbarToastSnap` 的说明）。
      setTimeout(() => {
        const t = document.querySelector(".toast");
        taskbarToastSnap = t ? t.textContent.trim() : "";
      }, 300);
    });
  }

  /*
   * ★★ 新增（2026-09-27）：点**顶栏**那颗「收进任务栏」。
   *
   * 与 `click=taskbar` 走的是同一条命令，但入口不同 —— 两个入口必须
   * 表现一致（同一份 `lib/sendToTaskbar`），所以两边都要能被单独验。
   * 只验设置那个入口的话，"顶栏那颗接错了命令"这类改动照样能通过。
   */
  if (click === "taskbar-top") {
    steps.push(() => {
      // ★★ 第十一轮：按位置取（从右数第二颗）—— 提示已删，不能再按文字找。
      const el = iconFromEnd(1);
      diag.push(`topEntry=${el ? "found" : "none"}`, `topEntryName=${iconName(el)}`);
      el?.click();
      setTimeout(() => {
        const t = document.querySelector(".toast");
        taskbarToastSnap = t ? t.textContent.trim() : "";
      }, 300);
    });
  }

  /*
   * 打开「稿件」面板（顶栏那个 📄）。改版后它一个入口承担了
   * 「最近 / 打开文件 / 粘贴」三件事，所以要能单独截出来看。
   *
   * ★★ 这里**必须**用 "Ctrl+K" 而不是 "稿件" 去认按钮：
   *   查找按钮的提示是「在稿件里查找（Ctrl+F）」—— 它**包含**"稿件"两个字，
   *   而 `byTitle` 是 `includes` 匹配、查找按钮又排在前面。
   *   第一次写 "稿件" 的结果就是点开了查找栏：截图里出现一条
   *   「在稿件里找…」的输入框，而**看起来完全像是稿件面板没做出来**。
   */
  if (click === "doc") {
    steps.push(() => {
      const el = iconAt(1);
      diag.push(`docBtn=${el ? "found" : "none"}`);
      el?.click();
    });
  }

  /*
   * 「稿件」按钮点两次 —— 第二次必须**收起**。
   *
   * 原来第二次收不起来：浮层的外点关闭挂在 `mousedown`，点图标时它先把面板关了，
   * 紧接着 `click` 又把已经被置为 false 的 `historyOpen` 取反 ⇒ 重新打开。
   * 于是"点一下开、再点一下还是开"。这条断言就是钉住它。
   *
   * ★★ 两条一起看才有鉴别力，单独看 `doc2` **不能**证明修好了：
   *    若那条 mousedown 监听根本是死的，`doc2` 也会是"关着"（点一下开、点第二下关）。
   *    `doc2out` 就是那个对照：第一下开面板，第二下点标题（真·面板外）。
   *      `doc2out=no` ⇒ mousedown 那条路是活的，于是 `doc2=no` 只能解释为"标记生效"；
   *      `doc2out=yes` ⇒ 监听死了，`doc2=no` 是假通过。
   */
  if (click === "doc2" || click === "doc2out" || click === "doc2nogt") {
    const targets =
      click === "doc2out"
        ? ["Ctrl+K", "title"]
        : // `doc2nogt`：把标记摘掉再点两下 —— **复现**改版前那个"再点收不起来"。
          // 反方向也要能跑出来，断言才算钉住了一件真事（而不是"恰好通过"）。
          ["Ctrl+K", "Ctrl+K"];
    const strip = click === "doc2nogt";
    steps.push(() => {
      if (strip) {
        for (const b of document.querySelectorAll("[data-panel-toggle]")) {
          b.removeAttribute("data-panel-toggle");
        }
        diag.push(`stripped=${document.querySelectorAll("[data-panel-toggle]").length}`);
      }
    });
    /*
     * ★★ 第十一轮：提示删了 ⇒ `data-hint` 也没了 ⇒ **不能按文字找按钮**。
     *   `tap=` 里现在报的写的是**序号 + 名字**（`iconAt` / `iconName`），
     *   这样"点错了"能在 diag 里直接看出来（名字对不上就是序号漂了）。
     *   位置与代价见 `iconAt` 上面那段注释。
     */
    targets.forEach((which, i) =>
      steps.push(() => {
        let el = null;
        if (which === "title") {
          el = document.querySelector(".title");
        } else if (which.startsWith("icon")) {
          // "icon0" / "icon1" / "icon2" …（0=查找 1=稿件 2=设置）
          el = iconAt(Number(which.slice(4)) || 0);
        } else if (which.startsWith("last")) {
          // "last0"＝关闭  "last1"＝收进任务栏
          el = iconFromEnd(Number(which.slice(4)) || 0);
        }
        diag.push(`tap${i + 1}=${el ? "found" : "none"}`, `tap${i + 1}name=${iconName(el)}`);
        if (el) realTap(el);
      }),
    );
  }

  /*
   * 点正文「接进度」—— 而且专挑**点不到字形**的位置点。
   *
   * 用户报的"有时候点不上"就是从这里来的：句子是 inline 盒，只包住字形，
   * 行距（line-height 1.7 ⇒ 每行约三分之一的行盒高度）与行尾空白都不属于它。
   * 这三条分别把三类位置钉住：
   *   `seekgap`   —— 同一段**两行之间的行距**正中（`Range.getClientRects()`
   *                  逐行给矩形，取第 1、2 行中间的缝）
   *   `seekright` —— 某一行**末尾右侧的空白**（同一行，x 移到段右缘）
   *   `seekdrag`  —— 按下后横拖 24px 再抬起：**不该**跳（那是选字复制，不是点）
   *
   * ★★ 两个坑都真踩过，所以断言里各留了一道自证：
   *   ① 必须挑**当前可见**的段落。第一段在自动滚动之后往往已经在视口上方
   *      （`top` 是负数）—— 那样 `elementFromPoint` 回 null、
   *      `caretRangeFromPoint` 也回 null，于是"一条都没跳"看起来像功能没做。
   *      所以优先挑含 `[data-current]` 的那段（它就锚在视口 36% 处，一定可见）。
   *   ② 必须自报 `pointIsSentence=`。若选中的点其实落在字形上，这条断言就退化成
   *      "点了个字"——**改版前也通过**，等于什么都没验。
   */
  if (click.startsWith("seek")) {
    steps.push(() => {
      const vp = document.querySelector(".viewport");
      const cur = document.querySelector(".viewport [data-current='1']");
      // 可见性判据用几何（与穿透上报同一条纪律：不看 `offsetParent`）
      const visible = (p) => {
        const r = p.getBoundingClientRect();
        const v = vp.getBoundingClientRect();
        return r.height > 0 && r.top >= v.top && r.bottom <= v.bottom;
      };
      const paras = [...document.querySelectorAll(".viewport .para")];
      const par = (cur && cur.closest(".para")) || paras.find(visible);
      if (!par) {
        diag.push("seekTarget=none");
        return;
      }

      // 逐行的**字形盒**（inline 片段），行盒比它高一截，差的那一截就是行距
      const range = document.createRange();
      range.selectNodeContents(par);
      const lines = [...range.getClientRects()].filter((r) => r.width > 4);
      if (!lines.length) {
        diag.push("seekLines=none");
        return;
      }
      const first = lines[0];
      const second = lines.find((r) => r.top > first.top + 1);

      // 行距带的正中：上一行字形底边 → 下一行字形顶边
      const yGap = second
        ? (first.bottom + second.top) / 2
        : first.bottom + Math.max(1, (parseFloat(getComputedStyle(par).lineHeight) - first.height) / 2);
      // x 取两行都盖得住的地方，再往左留一点余量
      const xLeft = Math.max(first.left, second ? second.left : first.left) + 6;
      const xGap = Math.min(xLeft, Math.min(first.right, second ? second.right : first.right) - 6);

      const y = click === "seekright" ? first.top + first.height / 2 : yGap;
      const x = click === "seekright" ? par.getBoundingClientRect().right - 3 : xGap;

      const at = document.elementFromPoint(x, y);
      diag.push(`pointIsSentence=${at && at.closest(".sentence") ? "yes" : "no"}`);
      diag.push(`pointIsPara=${at && at.closest(".para") ? "yes" : "no"}`);

      const opts = { bubbles: true, clientX: x, clientY: y, button: 0 };
      par.dispatchEvent(new PointerEvent("pointerdown", opts));
      if (click === "seekdrag") {
        // 拖 24px 再抬手 —— 位移超过阈值，必须被当成选字而不是点击。
        const moved = { ...opts, clientX: x + 24 };
        par.dispatchEvent(new PointerEvent("pointerup", moved));
        par.dispatchEvent(new MouseEvent("click", moved));
      } else {
        par.dispatchEvent(new PointerEvent("pointerup", opts));
        par.dispatchEvent(new MouseEvent("click", opts));
      }
    });
  }

  /*
   * ★★ 这里原来有一个 `hover=<选择器|说明文字>` 动作 —— 把鼠标"停"上去、
   *   等自绘气泡浮出来，然后量它的位置/是否越界/有没有换行。
   *
   *   第十一轮**整个删掉了**：提示已经不存在，没有东西可量。
   *
   *   ★ 把这段史留下来是因为它记着两个**以后还会用到**的坑：
   *     ① 要让 React 收到"鼠标进来了"，必须派发**会冒泡的 `mouseover`**
   *        —— React 18 把监听挂在根容器靠冒泡收；`mouseenter` **不冒泡**，
   *        直接 dispatch 到元素上 React 收不到，会误判成"功能没做"。
   *     ② portal 出去的东西（`document.body` 上的浮层）**必须全局找**，
   *        不能在触发元素下面 `querySelector`。
   *     这两条对将来任何"浮层 + 悬停/焦点"的功能（比如找上门的自己写的
   *     下拉、轻提示）同样成立。
   */

  /*
   * 展开设置里那个折叠的「更多」组。
   * ★ 它的默认态是**收起**，所以要额外点一下才看得到里面 ——
   *   而"默认收起"这件事本身也该被验（截图里它就不该展开）。
   */
  if (click === "more") {
    steps.push(() => {
      const gear = iconAt(2);
      gear?.click();
    });
    steps.push(() => {
      const h = document.querySelector(".grp-h.foldable");
      diag.push(`moreHead=${h ? "found" : "none"}`);
      h?.click();
    });
  }

  /*
   * 推一条后端警告事件，看控制台那行显示成什么。
   * 用的是前端**真实**的监听回调（`events.onWarning`），不是把状态塞进 store。
   */
  if (params.get("warn")) {
    steps.push(() => {
      const msg = WARN_SAMPLES[params.get("warn")];
      if (!msg) {
        diag.push(`warn=unknown:${params.get("warn")}`);
        return;
      }
      diag.push(`emitWarn=${emitTauri("prompter://warning", msg) ? "ok" : "miss"}`);
    });
  }

  /*
   * 匀速滚动「讲完了」的复现。
   *
   * 它由**两个**后端事实合成，缺一不可：
   *   * 光标到了末尾（界面靠它区分"讲完了"和"中途被停"）；
   *   * 引擎停了（`asr-status.running = false`）。
   * 只造其中一个都到不了那条路径 —— 而那条路径的作用正是把
   * 「讲完了」和「卡住了」分开，恰恰是最该被看到差别的一处。
   *
   * 用法：`engine=auto&click=demo&ended=1&settle=1600`
   * （settle 要晚于 click=demo，否则 demo 那一步会把 running 又置回 true。）
   */
  if (params.get("ended")) {
    steps.push(() => {
      const posText = document.querySelector(".tb-pos")?.textContent || "0/0";
      const total = Number(posText.split("/")[1]) || 0;
      const cur = emitTauri("prompter://cursor", {
        cursor: total,
        total,
        progress: 1,
        paragraph: 0,
        sentence: 0,
        offset_in_sentence: 0,
        sentence_align_start: Math.max(0, total - 6),
        sentence_align_end: total,
        score: 1,
        moved: true,
        miss_streak: 0,
        realigned: false,
      });
      const st = emitTauri("prompter://asr-status", {
        running: false,
        engine: "",
        detail: "",
      });
      diag.push(`endedCursor=${cur ? "ok" : "miss"} endedStatus=${st ? "ok" : "miss"}`);
    });
  }

  /*
   * 预置「提词中」—— `prompting=1`
   *
   * ★★ 为什么要发**事件**而不是改桩的返回值：
   *   前端拿 `running` 走的是 `events.onAsrStatus(...)` 这条事件通道
   *   （见 `App.tsx`），启动时并**不**调 `asr_status` 命令。
   *   所以改 `asr_status` 的桩返回是没用的 —— 那个函数根本没人调。
   *   必须顺着前端真实的监听路径把事件推过去，
   *   这样验到的才是"真跑起来会不会命中"，而不是"CSS 能不能命中"。
   *
   * ★ 它驱动的是本轮的**全局动效策略①**（`prompting` 时停装饰性动效）。
   */
  if (params.get("prompting") === "1") {
    steps.push(() => {
      const ok = emitTauri("prompter://asr-status", {
        running: true,
        engine: "sherpa-onnx（本地）",
        detail: "",
      });
      diag.push(`promptingEmit=${ok ? "ok" : "miss"}`);
    });
  }

  /*
   * 预置「收起态」—— `collapsed=1`
   *
   * ★★ 为什么必须要这个开关：本轮修的正是"点了收起之后，界面上没有任何
   *   可见入口能再展开"这个 bug（那颗展开键当时长在被收起的控制栏里面，
   *   跟着一起消失了）。而**正常跑的时候这条路径走不到** —— 得先点一下
   *   那颗收起键才会进收起态，而我们的量测是在加载后自动推状态的。
   *   不给它开关，就等于"改了但从没验过"（这个项目在这种事上栽过）。
   */
  if (params.get("collapsed")) {
    steps.push(() => {
      const ok = emitTauri("prompter://collapsed", true);
      diag.push(`collapsedEmit=${ok ? "ok" : "miss"}`);
    });
  }

  /*
   * 预置「把鼠标扫过所有控件」—— `hoverall=1`
   *
   * ★★ 这一条是第十一轮的**验收动作**，专门回答用户的投诉：
   *   「鼠标移到所有按钮的时候都会弹出文字提示」。
   *
   *   提示删掉之后，光看一张静态截图是**证明不了**"扫过去不弹东西"的 ——
   *   截图里本来就没有悬停态。所以这里主动把一整轮 pointer 事件
   *   （`pointerover` → `mouseover` → `mouseenter`）打到**每一个**可交互
   *   元素上，然后数：全文档里出现了几个 `.tip-bubble` / 带 `title` 的元素。
   *
   *   期望两个都是 0。只要有人把提示机制改回来（哪怕只在某一处），
   *   这个数就会 > 0，而**截图看不出来**。
   *
   *   注意：这里不是"模拟悬停"，是把真实事件按真实顺序发一遍 —— 气泡那套
   *   实现挂的就是 `mouseover`/`focus`，发这些事件就会触发它（如果还在）。
   */
  if (params.get("hoverall")) {
    steps.push(() => {
      const els = Array.from(
        document.querySelectorAll(
          "button,select,input,[role='button'],[tabindex],.icon-btn,.st-pill,.hero-btn,.grp-row,.hi-item",
        ),
      );
      for (const el of els) {
        for (const type of ["pointerover", "mouseover", "mouseenter", "mousemove"]) {
          el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
        }
        // 键盘焦点那条路也要走 —— 提示也可以挂在 focus 上
        try {
          el.focus?.();
        } catch {}
      }
      diag.push(`hoverSwept=${els.length}`);
      /*
       * 扫完之后立刻数残留。★ 两个都数：
       *   · `.tip-bubble` —— 自绘气泡的容器（第十轮的实现）
       *   · `[title]`     —— 原生 tooltip（第一代）
       * 任一非 0 就说明提示复活了。
       */
      diag.push(`hoverBubbles=${document.querySelectorAll(".tip-bubble").length}`);
      diag.push(`hoverTitles=${document.querySelectorAll("[title]").length}`);
    });
  }

  /*
   * 把某一行滚到视野中间 —— `scrollto=<行名>`
   *
   * ★ 存在的理由：设置抽屉很长，交付截图默认停在**第一组**（跟读），
   *   而「背景磨砂」在「排版」那一组 —— 想截到它就得先滚过去。
   *   `scroll=bottom` 只能滚到最底（那是「窗口」「语音」那几组），
   *   对"截中间某一行"没用。锚点用**行名**（控件的名字），不是位置。
   */
  if (params.get("scrollto")) {
    steps.push(() => {
      const label = params.get("scrollto");
      const row = rowByLabel(label);
      if (!row) {
        diag.push(`scrollRow=none(${label})`);
        return;
      }
      row.scrollIntoView({ block: "center" });
      diag.push(`scrolled=${label}`);
    });
  }

  /*
   * 展开「更多」组 —— `moreclick=1`
   *
   * ★「更多」默认收着（`Group` 的 `collapsible`），所以**功能介绍默认
   *   是看不见的** —— 交付截图要证明它存在，就必须先把这两层都点开。
   *
   * 锚点用**组标题的文字**（「更多」），不按位置：抽屉里分组会增删，
   * 按序号点早晚会指错地方。
   *
   * ★★ 必须用 `includes` 而不是全等：`Group` 在可折叠时会先渲染一枚
   *   箭头 `›`，所以 `textContent` 实际是「›更多」而不是「更多」。
   *   写成全等的话 `moreHead=none`，而且**它不报错** —— 只是 quietly
   *   什么都没点，截图看起来跟没加这个参数一样。
   */
  if (params.get("moreclick")) {
    steps.push(() => {
      const head = Array.from(document.querySelectorAll(".drawer .grp-h")).find((e) =>
        (e.textContent || "").includes("更多"),
      );
      if (!head) {
        diag.push("moreHead=none");
        return;
      }
      realTap(head);
      diag.push("moreOpen=1");
    });
  }

  /*
   * 展开「功能介绍」那块长文 —— `guideclick=1`
   *
   * ★ 必须在 `moreclick` **之后**：两层都默认收起，顺序反了第二个点不到
   *   （`diag` 里会出现 `guideRow=none`，那就是信号）。
   *
   * ★★ 锚点用**结构**（`.grp-box` 里第一枚带 `.grp-caret` 的可点行），
   *   不要匹配「功能介绍」这几个字 ——
   *   折叠态下那一行的标签是「查看各项功能是做什么的」，**根本不含**
   *   「功能介绍」四字（那是 `.grp-sub-h` 小节标题上的字，不在这行里）。
   *   写文字匹配会得到 `guideRow=none`，而且它 quietly 什么都不做。
   *
   *   另一个更普适的判据：那枚 `.grp-caret` 就是折叠箭头，
   *   整页只有「功能介绍」这一行在折叠态下**同时**满足
   *   「可点（.tap）」+「带箭头」—— 「更多」组标题虽然也带箭头，
   *   但它是 `.grp-h` 而不是 `.grp-row.tap`。
   *
   * ★★ `realTap` 的 click 是 **setTimeout 延后**发的（见它的实现），
   *   所以紧跟着量 `.guide-item` 必然量到 0 —— 那是**还没重渲染**，
   *   不是"点了没反应"。必须再排一个 setTimeout 去量。
   */
  if (params.get("guideclick")) {
    steps.push(() => {
      const row = document.querySelector(".drawer .grp-box .grp-row.tap .grp-caret")?.closest(".grp-row");
      if (!row) {
        diag.push("guideRow=none");
        return;
      }
      // 自报命中的是哪一行：靠结构匹配时更需要确认打对了地方。
      diag.push(`guideRowLabel=${nowrap(row.querySelector(".grp-t")?.textContent || "none")}`);
      realTap(row);
      setTimeout(() => {
        diag.push(`guideOpen=${document.querySelectorAll(".drawer .guide-item").length}`);
      }, 600);
    });
  }

  /*
   * 展开「支持作者」并按下赞助按钮 —— `donateclick=1`
   *
   * ★ 为什么需要这个参数：「支持作者」整组默认收起（`Group` 的 `collapsible`），
   *   默认截图里它只是一行标题 —— 收款码在不在、图路径对不对、赞助按钮接没接
   *   后端，全都**没被覆盖**。
   *
   * ★ 锚点用组标题文字 + `includes`：可折叠的标题前面会先渲染一枚 `›`，
   *   `textContent` 实际是「›支持作者」；写成全等会得到 `donateHead=none`，
   *   而且**它不报错** —— 只是 quietly 什么都没点。
   *
   * ★★「图到底加载出来没有」这一项**不在这里量，在收尾那一刻量**
   *   （收尾块里的 `donateImgOkLate` / `donateImgNatural`）。
   *   最初写成在这里量，得到 `donateImgOk=0` —— 看着像"图是裂的"，
   *   其实只是**没等它**：图片的解码是异步的，而这一步距展开只有 600ms。
   *   收尾那一刻已经过去好几秒，它要么已经好了，要么就真的是坏路径
   *   （服务器 200 而 DOM 里 `naturalWidth` 还是 0）。
   *   这正是本项目那类"量早了 = 看着像功能没做"的坑（见 guideclick 注释）。
   *   这里只报**展开的实况**：`donateQr` 有几张、`donateQrW` 有多大
   *   （码太小同样扫不出来，而那是这一步唯一的目的）、收款人是谁。
   *
   * ★ `openedUrl` 验的是赞助按钮**确实走到了后端**（见 COMMANDS 里的
   *   `open_url`）：在桩里"点了没反应"与"根本没接后端"长得一样，必须留证据。
   */
  if (params.get("donateclick")) {
    steps.push(() => {
      const head = Array.from(document.querySelectorAll(".drawer .grp-h")).find((e) =>
        (e.textContent || "").includes("支持作者"),
      );
      if (!head) {
        diag.push("donateHead=none");
        return;
      }
      realTap(head);
      diag.push("donateOpen=1");
      // `realTap` 的 click 是**延后**发的 —— 紧跟着量必然量到 0，
      // 那是"还没重渲染"，不是"点了没反应"（同一个坑见上面 guideclick）。
      setTimeout(() => {
        const qrs = Array.from(document.querySelectorAll(".drawer .donate-qr"));
        diag.push(`donateQr=${qrs.length}`);
        if (qrs.length) {
          diag.push(`donateQrW=${Math.round(qrs[0].getBoundingClientRect().width)}`);
        }
        const nick = document.querySelector(".drawer .donate-nick");
        diag.push(`donateNick=${nowrap((nick?.textContent || "none").trim())}`);

        const btn = document.querySelector(".drawer .donate-foot .btn");
        if (!btn) {
          diag.push("donateBtn=none");
          return;
        }
        realTap(btn);
        setTimeout(() => {
          const opened = window.__openedUrls || [];
          diag.push(`openedN=${opened.length}`);
          if (opened.length) diag.push(`openedUrl=${nowrap(String(opened[0]))}`);
        }, 400);
      }, 600);
    });
  }

  /*
   * 把抽屉正文滚到指定像素 —— `guidescroll=<px>`
   *
   * ★ 存在的理由：功能介绍有二十多条，一次截不完；而
   *   `scrollto=<行名>` 是按**行**定位的，介绍里的条目不是设置行，
   *   `rowByLabel` 找不到它们。
   */
  if (params.get("guidescroll")) {
    steps.push(() => {
      const body = document.querySelector(".drawer .drawer-body");
      if (!body) {
        diag.push("drawerBody=none");
        return;
      }
      body.scrollTop = Number(params.get("guidescroll")) || 0;
      diag.push(`drawerScroll=${body.scrollTop}`);
    });
  }

  /*
   * 点设置里「背景磨砂」行的某一档 —— `glassclick=off|css|system`
   *
   * ★ 这是第十二轮 P0-2/P0-3/P0-6 的**验收动作**（check-glass-mode.py）：
   *   切档之后量四件事 —— `glassFx`（桩模拟的窗体材质挂/卸）、
   *   `topBlur`（栏的 CSS 玻璃退没退）、`winAlpha`（外壳有没有让位）、
   *   `opacityRow`（不透明度滑杆在系统磨砂档该被藏起来）。
   *
   *   档位的落盘走真路径：patch → store → 400ms 防抖 → api.saveConfig，
   *   所以**量测必须等防抖发完**（门槛里 `settle=1600`）。
   *   锚点是行名「背景磨砂」+ 档位按钮的文字 —— 那是控件的名字，不是提示。
   */
  if (params.get("glassclick")) {
    steps.push(() => {
      const want = params.get("glassclick");
      const label = want === "system" ? "系统磨砂" : want === "off" ? "关" : "柔光";
      const row = rowByLabel("背景磨砂");
      if (!row) {
        diag.push("glassRow=none");
        return;
      }
      const btn = Array.from(row.querySelectorAll(".seg-btn")).find(
        (b) => nowrap(b.textContent) === label,
      );
      if (!btn) {
        diag.push("glassBtn=none");
        return;
      }
      realTap(btn);
      diag.push(`glassTap=${label}`);
    });
  }

  /*
   * ★ 延迟 400ms 再动手，别"一发现图标就点"。
   *
   * 实测：`waitFor` 一返回就立刻 `click()`，**点不中**（事件没人接）；留 400ms
   * 就正常。推测是开发模式下 React 的 StrictMode 会把组件挂载两次 ——
   * 第一次挂载的 DOM 节点随后被卸载，而 `waitFor` 恰好抓到并点了那一份。
   * 留一点时间让"最终那一份"渲染完，是成本最低的解法。
   */
  /*
   * ★ `at=<ms>`：把动作推到**绝对时刻**执行。
   *
   * 默认的"400ms 起步、每个动作隔 300ms"对多数场景够用，但对付不了
   * "只在某段时间内存在"的状态 —— 「已收进托盘」那条提示 1.7s 后自己就消失了，
   * 而 `dump-dom` 要等到 `--hold` 之后才发生。动作必须落在"dump 之前不久"，
   * 才既截得到、也量得到它。
   *
   * （第一版只把 `at=` 写进了文件头的参数表、忘了在这里实现，
   *   症状就是 `trayHint=no` —— 看着像功能没做，其实只是动作跑早了。）
   */
  const clickAt = Number(params.get("at") || 0);
  steps.forEach((fn, i) =>
    setTimeout(fn, clickAt > 0 ? clickAt : 400 + i * 300),
  );

  /*
   * 收尾：量关键尺寸写进 <title>，由 ui-preview.py 的 --measure 取回。
   *
   * 为什么非要量：内容被裁、控件溢出这类问题，缩略图上看不出来 ——
   * 窗口只有 560×360，"看起来在里面"和"数字上在里面"是两回事。
   */
  setTimeout(() => {
    const parts = [...diag];
    /*
     * ★ 这一层 try/catch 是为了修掉本桩最阴的一个失败模式：
     *   量测体里任何一处抛异常（例如在收尾块里用了一个只存在于外层的 const），
     *   异常被 setTimeout 吞掉 ⇒ `document.title` 从没被赋值 ⇒ `--measure`
     *   回一个光秃秃的 `Prompter`，截图还是一张看着挺正常的界面图。
     *   于是"桩挂了"和"功能没做"在证据上一模一样（本轮真踩，见 MEMORY）。
     *   ⇒ 出了错就**把错误写进 title**，让 `--measure` 直接说出来。
     */
    try {
    const body = document.querySelector(".drawer-body");
    if (body) {
      parts.push(`bodyScroll=${Math.round(body.scrollHeight)}`);
      parts.push(`bodyClient=${Math.round(body.clientHeight)}`);
    }
    /*
     * ★ 第十一轮起提示整个删了，这里原本量的是 `.hint-ico` 的个数
     *   （用来确认「摊了 trigger 忘渲染 bubbleEl」那类半成品）。
     *   现在改量 `topIcons`（下面顶栏那块已经量过了），这条留一个
     *   **总数校验**：焦点管理的可 tab 元素数 —— 删提示不该让任何
     *   可交互控件跟着消失。
     */
    parts.push(`focusable=${document.querySelectorAll("button,select,input,[tabindex]").length}`);
    parts.push(`drawerOpen=${document.querySelector(".drawer.open") ? "yes" : "no"}`);
    /*
     * 下拉框的运行时选中项。**必须在这里读，不能看快照截图** ——
     * select 的选中状态靠 `selectedIndex` 表达，它不写进 HTML，
     * 静态重放时会退回第一项。曾据此误判"模型没选中"，
     * 实际运行时是对的（0,1 → 第二个下拉选中了第 1 项）。
     */
    {
      const sels = Array.from(document.querySelectorAll(".drawer-body select"));
      parts.push(`selVal=${sels.map((s) => s.value || "·").join(",")}`);
    }

    /*
     * 顶栏 / 进度栏的几何。
     *
     * ★ 为什么要单独量这两条栏：本轮把顶栏放大、把进度栏从底部搬到顶栏下面，
     *   这两件事都会在 560px 宽的小窗口里撞到**宽度预算**——放大按钮、把
     *   两条栏叠到上半屏，随时可能把标题挤没或者让整条栏溢出。
     *   而"溢出"在缩略图上几乎看不出来（Flex 会把最后几个按钮压扁），
     *   只能靠 `scrollWidth > clientWidth` 判。标题同理：它被裁成省略号时
     *   截图看起来"就是这样设计的"，只有比对 `scrollWidth` 才知道被裁了。
     *
     * 值的格式必须是**没有空格**的 key=value（ui-preview.py 按空格切 token），
     * 所以 backdrop-filter 这种带空格的取值要先把空格去掉。
     */
    {
      const tb = document.querySelector(".topbar");
      if (tb) {
        const r = tb.getBoundingClientRect();
        parts.push(
          `topH=${Math.round(r.height)}`,
          `topScroll=${Math.round(tb.scrollWidth)}`,
          `topClient=${Math.round(tb.clientWidth)}`,
          `topOverflow=${tb.scrollWidth > tb.clientWidth + 1 ? "yes" : "no"}`,
        );
        const ib = Array.from(tb.querySelectorAll(".icon-btn"));
        parts.push(`topIcons=${ib.length}`);
        if (ib[0]) {
          const br = ib[0].getBoundingClientRect();
          parts.push(`iconW=${Math.round(br.width)}`, `iconH=${Math.round(br.height)}`);
        }
        // 状态灯与图标按钮的实测尺寸。
        // ★ 本轮改版把顶栏从「4 个文字胶囊 + 7 个图标」砍到「1 个状态灯 + 5 个图标」，
        //   所以这里要同时量**控件个数**和**固定占宽** —— 只看截图看不出"少了几个"。
        const pills = tb.querySelectorAll(".st-pill");
        parts.push(`pills=${pills.length}`);
        if (pills[0]) parts.push(`pillH=${Math.round(pills[0].getBoundingClientRect().height)}`);
        // SVG 图标是否真的挂上了（换字形这件事要能被自动验证）
        parts.push(`topSvg=${tb.querySelectorAll(".icon-btn svg").length}`);
        /*
         * 磨砂档位四联测（check-glass-mode.py 的输入）：
         *   glassFx    桩模拟的窗体材质（save_config 里换算的 set_effects 结果）
         *   topBlur    栏的 CSS 玻璃 —— 系统磨砂档必须为 none（不叠玻璃，P0-3）
         *   winAlpha   外壳透明度 —— 系统磨砂档固定让位给材质（0.22）
         *   opacityRow 不透明度滑杆 —— 系统磨砂档该被藏起来（一个意思一个控件）
         */
        parts.push(`glassFx=${glassEffects}`);
        parts.push(`topBlur=${nowrap(getComputedStyle(tb).backdropFilter)}`);
        parts.push(
          `winAlpha=${nowrap(getComputedStyle(document.documentElement).getPropertyValue("--win-alpha"))}`,
        );
        parts.push(`opacityRow=${rowByLabel("不透明度") ? "yes" : "none"}`);
        const title = tb.querySelector(".title");
        if (title) {
          const tr = title.getBoundingClientRect();
          /*
           * ★★ `titleClip` 必须按**文字自己的宽度**判，不能拿 `scrollWidth` 比。
           *
           *    真踩：`scrollWidth`/`clientWidth` 都是取整过的，于是
           *    114.6 与 115.6 报成 115 与 116，差值 1；原先的判据带了
           *    `+1` 容差 ⇒ 判成"没截断"，而**截图里明明挂着省略号**。
           *    一个会说反话的门槛比没有门槛更坏：它让人放心地不去看。
           *
           *    Range 量的是文字的真实排版宽度（小数），拿它跟盒宽直接比，
           *    0.5px 的余量只用来压掉亚像素噪声。
           */
          const rgEl = title.firstChild;
          let textW = tr.width;
          if (rgEl && rgEl.nodeType === 3) {
            const rg = document.createRange();
            rg.selectNodeContents(title);
            textW = rg.getBoundingClientRect().width;
          }
          parts.push(
            `titleW=${Math.round(tr.width)}`,
            `titleTextW=${textW.toFixed(1)}`,
            /* ★ 直接报"差多少 px"（正数＝放不下）。布尔值在边界上没法解释，
             *   而 `titleShort=0.4` 一眼就知道是"就差一点点"。 */
            `titleShort=${(textW - tr.width).toFixed(1)}`,
            /* 门槛 0.1px：实测文字比盒子宽 **0.4px** 时浏览器就已经挂上省略号了，
             * 所以容差不能取 0.5（那会把真实截断报成"没截断"）。 */
            `titleClip=${textW > tr.width + 0.1 ? "yes" : "no"}`,
            `titleL=${Math.round(tr.left)}`,
            `titleR=${Math.round(tr.right)}`,
            /*
             * ★★ 计算值 —— 光有尺寸不足以判断"文字为什么画到盒子外面"。
             *    真踩：一度按 `titleScrollW > titleW` 认定标题被截，
             *    可它其实一个不少地铺在盒子里；要分清是
             *    "盒子小"还是"裁切没生效"，只能看 `overflow`/`display` 的计算值。
             */
            `titleOv=${getComputedStyle(title).overflow}`,
            `titleDisp=${getComputedStyle(title).display}`,
          );
        }
      }

      /*
       * 三个状态 pill 的**实际呈现**（class / 文字 / 颜色 / 动画名）。
       *
       * ★ 为什么单量这四个：本轮改动全在"状态语义"上，而 pill 的语义
       *   都编码在 class 里（on / starting / paused / warn），文字和颜色只是
       *   给人看的。截图能看出颜色，但看不出**它有没有在动**，也读不出 class ——
       *   而"点了之后停在转圈状态不放"正是这一轮最需要防住的失败。
       *   文字里可能带空格（`▶ 演示`），必须 nowrap，否则会劈成两个 token。
       */
      const pillState = (sel, tag) => {
        const el = document.querySelector(sel);
        if (!el) {
          parts.push(`${tag}=none`);
          return;
        }
        const cs = getComputedStyle(el);
        parts.push(
          `${tag}=${nowrap(Array.from(el.classList).join("."))}`,
          `${tag}Text=${nowrap(el.textContent.trim())}`,
          `${tag}Color=${nowrap(cs.color)}`,
          // ★ 扫光挂在 `::after` 上，所以 `cs.animationName` 永远是 none ——
          //   真要看"它到底在不在动"，必须读伪元素的那一份。
          //   （第一版只读了元素本身，于是"开启中"看起来和静止的一模一样。）
          `${tag}Anim=${nowrap(getComputedStyle(el, "::after").animationName)}`,
        );
      };
      pillState(".st-pill", "spill");
      pillState(".hero-btn", "hero");

      /*
       * 主角按钮的几何 —— 它是本轮改版的核心，"够不够大"只能量。
       * 视觉重量（44px / 圆形 / 实心）是它成为"唯一答案"的全部依据，
       * 而这三件事缩略图上看着都"差不多"。
       */
      {
        const hb = document.querySelector(".hero-btn");
        if (hb) {
          const r = hb.getBoundingClientRect();
          const cs = getComputedStyle(hb);
          parts.push(
            `heroW=${Math.round(r.width)}`,
            `heroH=${Math.round(r.height)}`,
            `heroR=${nowrap(cs.borderRadius)}`,
            `heroCX=${Math.round(r.left + r.width / 2)}`,
            /*
             * ★ 边框色与文字色都要量。
             *   第一版把「未开始」和「已暂停」都做成了蓝色描边 + ▶，
             *   于是在 `--measure` 里两者**逐字节相同**，肉眼也分不出来。
             *   只量 `color` 是不够的 —— 那两个态的差别体现在**边框**上。
             */
            `heroBorder=${nowrap(cs.borderTopColor)}`,
            `heroBg=${nowrap(cs.backgroundColor)}`,
          );
        }
        /*
         * 那条栏里的进度填充。
         * ★ 它取代了旧的「顶栏下沿 2px 被动进度线」—— 那条线随本轮改版删掉了
         *   （同一件事不画两遍：这一行里已经有一条可点的进度条）。
         *   量测项仍叫 `topProg*`，因为它依然是"顶栏里那条进度"，
         *   只是从一条装饰线变成了一条能点的进度条。
         */
        const tp = document.querySelector(".topbar .progress-track .progress-fill");
        if (tp) {
          const r = tp.getBoundingClientRect();
          parts.push(
            `topProgW=${Math.round(r.width)}`,
            `topProgFill=${Math.round(parseFloat(tp.style.width || "0"))}`,
          );
        } else {
          parts.push("topProg=none");
        }
      }

      /*
       * 稿件区的**歌词式层次**：段落按距离递减的字号与透明度，
       * 以及当前句那三段的取色。
       *
       * ★ 为什么要量而不是看截图：这几条都是**渐变**。缩略图上盯一眼只会觉得
       *   "好像有点层次"，看不出字号是否真的按档递减、透明度有没有生效、
       *   三段是不是三个不同的颜色 —— 而写坏成"整段一模一样"或"三段同色"
       *   在图上几乎看不出区别。这类"看起来对"必须换成数字。
       */
      {
        const paras = Array.from(document.querySelectorAll(".para"));
        parts.push(`paras=${paras.length}`);
        parts.push(
          `paraDist=${paras
            .map((p) => getComputedStyle(p).getPropertyValue("--dist").trim() || "·")
            .join(",")}`,
        );
        parts.push(
          `paraFs=${paras
            .map((p) => Math.round(parseFloat(getComputedStyle(p).fontSize)))
            .join(",")}`,
        );
        parts.push(
          `paraOp=${paras
            .map((p) => Math.round(parseFloat(getComputedStyle(p).opacity) * 100) / 100)
            .join(",")}`,
        );
        for (const [tag, sel] of [
          ["sung", ".sentence.current .sung"],
          ["unsung", ".sentence.current .unsung"],
          ["singing", ".sentence.current .singing"],
        ]) {
          const el = document.querySelector(sel);
          parts.push(`${tag}=${el ? nowrap(getComputedStyle(el).color) : "none"}`);
        }
      }

      /*
       * 「已收进任务栏」那条提示 + 这个动作**有没有真的走到后端**。
       *
       * ★ 它**换过位置**：原来是自己一块固定居中的浮层（`.tray-hint`），
       *   那颗按钮搬进设置之后改走全局 toast。
       *
       * ★★ 这里量的是"**那条提示还在不在**"，而不是"它长得对不对" ——
       *    搬家的功能最容易出的错就是**信号在搬的过程中丢了**：
       *    窗口一缩下去，用户什么都看不见，而代码不报任何错。
       *    所以 `taskbarToast=` 这一项是搬迁的**验收项**，不是装饰。
       *
       * ★★ `taskbarCalled=` 是 2026-09-27 补的第二项：只验"提示出现了"的话，
       *    一个**空函数体**也能通过 —— 提示是前端自己弹的，跟后端收没收到无关。
       *    名字也不许含糊：这一项要同时盯住"命令名有没有接上"，
       *    所以桩对未注册命令静默 resolve 的那个行为，必须靠它兜住。
       *    （配 `click=taskbar&at=…` 用，见文件头。）
       */
      {
        const t = document.querySelector(".toast");
        parts.push(
          t ? "taskbarToast=yes" : "taskbarToast=no",
          `taskbarText=${nowrap(t ? t.textContent.trim() : taskbarToastSnap || "none")}`,
          `taskbarToastBottom=${t ? Math.round(innerHeight - t.getBoundingClientRect().bottom) : "none"}`,
          `taskbarCalled=${taskbarCalled ? "yes" : "no"}`,
        );
      }

      /*
       * ★★ 平台分岔的**界面用语**（2026-09-27 第九轮 macOS 移植时补）。
       *
       * 为什么必须量：macOS 上没有"任务栏"这个词，所以
       *   ① 设置里「收起 / 收进任务栏」那一行要变成「收起窗口」；
       *   ② 它的 `sub` 提示里**不能出现「任务栏」三个字**；
       *   ③ 右边的「收进任务栏」按钮**整个不渲染**。
       *
       * 这三件事**写错了界面照样渲染** —— 文案错误是那种"改了看不出来"的
       * 东西，正是断言该管的地方。`plat=macos` 时把它们对上，
       * 就能在 Windows 开发机上把 mac 的用语与按钮数验掉。
       *
       * ★ 定位方式：按**语义**找那一行（`.grp-t` 里含「收起」），不按位置索引
       *   —— 按位置写死的断言会在下次改版时静默指错行。
       * ★★ `collapseRowBtns` 是这组里最该盯的一项：macOS 必须 = 1
       *   （「收进任务栏」那颗不渲染），Windows = 2。写成 1 却还在截图里
       *   看到两颗，就说明选择器挂了 —— 所以它要跟截图一起看。
       *   行容器类名是 `.grp-row`（`grp` / `grp-l` / `grp-t` / `grp-r` 是同族的，
       *   漏写 `-row` 会让 `closest()` 一直往上找到抽屉根，把别行的按钮也数进来）。
       *
       * ★★ 这里**不再量 `collapseRowSub`**：2026-09-28 起设置面板的行内
       *   小字全部删除，那一项恒为 `none`，留着只会误导人
       *   （"量到了 none"看着像坏了，其实是被有意删掉的）。
       *   替代项是下面那条 `inlineSubs`。
       */
      {
        const rows = Array.from(document.querySelectorAll(".drawer .grp-t"));
        const tEl = rows.find((e) => /收起|收进/.test(e.textContent || ""));
        const row = tEl ? tEl.closest(".grp-row") : null;
        parts.push(
          `collapseRowLabel=${nowrap(tEl ? tEl.textContent.trim() : "none")}`,
          `collapseRowBtns=${row ? row.querySelectorAll(".grp-r button").length : "none"}`,
        );
      }

      /*
       * `inlineSubs` —— 设置抽屉里**行内小字**（`.grp-l .grp-s`）的处数。
       *
       * ★ 2026-09-28 新增，替掉原来那条已失效的 `collapseRowSub` 断言。
       *   行内小字全部删除后 `collapseRowSub` 恒为 `none`，
       *   「里面不许出现『任务栏』」于是变成永真 —— 而失效的门槛
       *   比没有门槛更危险，它给的是虚假的安全感。
       *
       * ★★ 选择器必须是 `.grp-l .grp-s`，不能是 `.grp-s`：
       *   块级说明（自检结论、模型缺失警告、"最近听到"、诊断结果）
       *   **仍然是** `.grp-s`，它们是运行事实、不是功能介绍小字，
       *   用 `.grp-s` 会把它们一起数进来导致误报。
       *   `.grp-l .grp-s` 精确命中"标签下面那行小字"——
       *   与 `styles.css` 里那条选择器是同一个判据，两边不会漂移。
       *
       * ★ 量的是**处数**而不是文本：断言只需要知道"有没有"，
       *   把全文抄进 title 只会让那一行长到没法读。
       */
      {
        const inlineSubs = document.querySelectorAll(".drawer .grp-l .grp-s").length;
        // 功能介绍的条目数：它是本轮**新加**的内容，顺带量一下，
        // 让人能一眼看出"小字删了、介绍补了"这两件事同时成立。
        // ★ 它默认收着（要 moreclick + guideclick 两步才展开），
        //   所以没点开时量到 0 属正常，不是坏了。
        const guideItems = document.querySelectorAll(".drawer .guide-item").length;
        parts.push(`inlineSubs=${inlineSubs}`, `guideItems=${guideItems}`);
      }

      /*
       * 那**一条栏**的实测几何。
       *
       * ★ 本轮改版把两条栏合成了一条：以前这里量的是 `.controlbar`
       *   （窗口底部那条），顶栏本身另有量测。现在只剩 `.topbar`，
       *   所以量测项前缀统一成 `tb*` —— 名字里再带 `cb`（control bar）
       *   会把人指向一条已经不存在的栏。
       *
       * ★ `tbOverflow` 是这一版最该盯的一项：一行里塞了十几件事，
       *   560px 宽下万一溢出，表现是**最后一个图标被裁掉**（关闭键），
       *   而那在截图里很容易被看成"设计本来就这样"。
       */
      const sb = document.querySelector(".topbar");
      if (sb) {
        const r = sb.getBoundingClientRect();
        const cs = getComputedStyle(sb);
        parts.push(
          `tbT=${Math.round(r.top)}`,
          `tbB=${Math.round(r.bottom)}`,
          `tbH=${Math.round(r.height)}`,
          `tbScroll=${Math.round(sb.scrollWidth)}`,
          `tbClient=${Math.round(sb.clientWidth)}`,
          `tbOverflow=${sb.scrollWidth > sb.clientWidth + 1 ? "yes" : "no"}`,
          `tbBlur=${nowrap(cs.backdropFilter || cs.webkitBackdropFilter || "none")}`,
          `tbBg=${nowrap(cs.backgroundColor)}`,
        );
        const pt = sb.querySelector(".progress-track");
        if (pt) parts.push(`progW=${Math.round(pt.getBoundingClientRect().width)}`);
        parts.push(`tbIcons=${sb.querySelectorAll(".icon-btn").length}`);
        parts.push(`tbSteps=${sb.querySelectorAll(".tb-step").length}`);
        /*
         * 宽度预算 —— 这一行到底谁占了多少。
         *
         * ★★ 为什么需要它：`tbOverflow=no` 只说明"没溢出"，不说明**分配得合不合理**。
         *    一行里塞了十几件事，最容易出现的毛病是"某一件被挤到只剩最小值"
         *    （进度条被挤到 52px、标题只剩 7 个字），而这两种情况**都不溢出**，
         *    看 `tbOverflow` 全绿，看截图也只是"标题短了点"，很像设计如此。
         *    逐个子元素报宽度，才能一眼看出预算被谁吃掉了。
         *    像素扫描替代不了它：扫描只能看见"有墨的地方"，
         *    按钮的 padding、被省略号截掉的文字都量不出来。
         */
        const partList = Array.from(sb.children)
          .map((el) => {
            const cr = el.getBoundingClientRect();
            return `${(el.className || el.tagName).toString().trim().split(/\s+/).pop()}`
              + `:${Math.round(cr.left)}-${Math.round(cr.right)}`;
          })
          .join(",");
        parts.push(`barParts=${nowrap(partList)}`);
      }

      /*
       * 收起态：栏 / 出路 / 正文 —— 三者是不是各就各位。
       *
       * ★★ 这三项合起来就是那个 bug 的回归断言：
       *   以前收起时 `.controlbar` 连同长在它里面的展开键被
       *   `height: 0 + opacity: 0 + pointer-events: none` 一起收掉，
       *   于是"没了"这件事在界面上**看不出少了什么**。
       *   现在要求：`.collapsed` 在、栏在、**栏上有点得着的出路**、正文高 0。
       *
       * ★★ 2026-09-27 修正：这里原来是按**位置**取的（`btns[length - 2]` =
       *   "倒数第二颗是收起/展开"）。那个假设随本轮改版失效了 ——
       *   收起键搬进设置，倒数第二颗变成了「调参」。按位置取会**照常
       *   报 `collapseBtn=yes`**，因为那个位置上确实有一颗按钮，
       *   只是它的职责已经不是"展开"了 —— 这就是最坏的那种假信号：
       *   界面错了、验收还是绿的。
       *   ⇒ 改成按**语义**找：收起态下，栏上必须存在一颗**不靠热键的出路**。
       *
       * ★★ 这个"语义锚点"换过三次，每次都因为界面把字删了：
       *     ① `title` 里带"任务栏"（第四轮之前）
       *     ② `data-hint` 里带"任务栏"（第十轮，title 清零后）
       *     ③ **从右数第二颗**（第十一轮，提示整个删了）
       *
       *   ★ 为什么退回"按位置"这次是**对的**：`collapseExitHittable` 要验的
       *     本来就是**"那个位置上有没有一颗点得着的东西"**（位置在这条断言里
       *     就是被验的对象）；而它的**语义**（"这是收进任务栏"）现在由
       *     `iconName`（class / aria-label）报出来，人核对一眼即可。
       *     这跟当初 `btns[len-2]` 那次事故不同 —— 那次是拿位置**冒充语义**，
       *     这次位置是真的在验位置、语义单独报了名。
       */
      const appEl = document.querySelector(".app");
      if (appEl && appEl.classList.contains("collapsed")) {
        const exit = iconFromEnd(1);
        const vpEl = document.querySelector(".viewport");
        parts.push(
          "collapsed=yes",
          `collapsedTopbar=${document.querySelector(".topbar") ? "yes" : "no"}`,
          // 收起态下顶栏那颗「收进任务栏」是唯一不靠热键的出路 ——
          // 它必须**可点**（在 46px 的窗口里没被裁掉、pointer-events 还在）。
          `collapseExitBtn=${exit ? "yes" : "none"}`,
          `collapseExitName=${iconName(exit)}`,
          `collapseExitHittable=${exit ? String(exit.offsetParent !== null) : "none"}`,
          `collapsedVpH=${vpEl ? Math.round(vpEl.getBoundingClientRect().height) : -1}`,
        );
      } else {
        parts.push("collapsed=no");
      }

      const vp = document.querySelector(".viewport");
      if (vp) {
        const cs = getComputedStyle(vp);
        parts.push(`vpPadT=${nowrap(cs.paddingTop)}`, `vpPadB=${nowrap(cs.paddingBottom)}`);
      }

      /*
       * 动效 token 与两条总策略 —— 2026-09-27「大换血」的回归断言。
       *
       * ★★ 为什么动效也需要断言：`transition` 写错**不会报错**。
       *    漏一个 `var(--t-xxx)` 的拼写、少一个分号吃掉下一行、
       *    把 token 定义写在被覆盖的位置 —— 浏览器全部静默忽略，
       *    界面看起来"只是没那么顺"，而没有任何东西会告诉你。
       *    这里的做法和 `--expect-bars` 一样：**期望值从唯一源现算**。
       *
       * 三项：
       *   ① 六个 token 都能被 `getComputedStyle` 解析出**非空**值
       *      （没定义的话读到的是空串 —— 这正是"写错了但没报错"的样子）
       *   ② `--t-press` 必须**小于** `--t-fast`：按压反馈比悬停更快，
       *      这是"手还在按钮上"与"手在按钮上方"的区别，反了就是手感bug
       *   ③ `.app.prompting` 那条规则真的能命中（用 `matches()` 问浏览器，
       *      而不是自己读 CSS 文本 —— 读文本只能证明"写了"，不能证明"生效"）
       */
      const rootCs = getComputedStyle(document.documentElement);
      const tokNames = [
        "--t-press",
        "--t-fast",
        "--t-base",
        "--t-enter",
        "--ease-back",
        "--ease-in",
      ];
      const missing = tokNames.filter((n) => !rootCs.getPropertyValue(n).trim());
      parts.push(`motionTokens=${missing.length ? "missing:" + missing.join("|") : "ok"}`);

      /*
       * ★★ 取值要把单位换算成毫秒，**不能直接 `parseFloat`**。
       *   实测（2026-09-27 被这条断言自己抓住）：`getComputedStyle` 读到
       *   `--t-press` 是 `90ms`、而 `--t-fast` 被**规范化成了 `.14s`** ——
       *   同一个 `:root` 里两条相邻的声明，读回来的单位不一样。
       *   直接 `parseFloat` 就成了「90 与 0.14 比较」，断言报错但 CSS 没错。
       *   ⇒ 凡"拿计算值做数值比较"的地方，都必须先归一单位。
       *     （同族坑：别把 `parseFloat` 当成"读 CSS 数值"的通用解法。）
       */
      const msOf = (n) => {
        const raw = rootCs.getPropertyValue(n).trim();
        if (!raw) return -1;
        const v = parseFloat(raw);
        if (Number.isNaN(v)) return -1;
        return raw.endsWith("ms") ? v : v * 1000; // 无单位/秒 ⇒ 按秒换算
      };
      const tp = msOf("--t-press");
      const tf = msOf("--t-fast");
      // ★ 判"按下比悬停快"。两个都读不到时是 -1，会正确判成 fail。
      parts.push(`motionPressOrder=${tp > 0 && tf > 0 && tp < tf ? "ok" : `bad:${tp}>=${tf}`}`);
      // 把归一后的值报出来 —— 断言失败时能直接看出是"值不对"还是"读法不对"。
      parts.push(
        `motionRaw=${["--t-press", "--t-fast", "--t-base", "--t-enter"]
          .map((n) => `${n.replace("--t-", "")}=${msOf(n)}`)
          .join(",")}`,
      );

      // 上限守卫：微交互不该超过 250ms（Uiverse 那条 150–250ms 的惯例）
      const over = ["--t-press", "--t-fast", "--t-base", "--t-enter"].filter(
        (n) => msOf(n) > 250,
      );
      parts.push(`motionUnderLimit=${over.length ? "over:" + over.join("|") : "ok"}`);

      // 两条总策略的规则存在且能命中
      const promptingRuleExists = [...document.styleSheets]
        .flatMap((ss) => {
          try {
            return [...ss.cssRules];
          } catch {
            return [];
          }
        })
        .some((r) => (r.selectorText || "").includes(".app.prompting"));
      parts.push(`motionPromptingRule=${promptingRuleExists ? "yes" : "no"}`);

      const rmRuleExists = [...document.styleSheets]
        .flatMap((ss) => {
          try {
            return [...ss.cssRules];
          } catch {
            return [];
          }
        })
        .some(
          (r) =>
            r.type === CSSRule.MEDIA_RULE &&
            (r.conditionText || r.media?.mediaText || "").includes("prefers-reduced-motion"),
        );
      parts.push(`motionReducedMotion=${rmRuleExists ? "yes" : "no"}`);

      /*
       * 提词中的**实际命中**：不是问"规则写了吗"，而是问
       * "此刻根节点上那个类在不在、以及它真的把悬停位移压成了 none 吗"。
       *
       * ★★ 只验"类存在"不够 —— `.app.prompting` 完全可能被后面某条
       *   同样特异性的规则盖掉，那时候类在、效果不在，断言照样绿。
       *   所以这里直接**读一个受它管辖的属性的计算值**：
       *   拿一颗 `.icon-btn`，问它的 `transition-property` 里还有没有
       *   `transform`（提词中应该被剥掉，只剩颜色类属性）。
       */
      const appCls = appEl ? appEl.className : "";
      parts.push(`promptingClass=${appCls.includes("prompting") ? "yes" : "no"}`);
      const probeBtn = document.querySelector(".topbar .icon-btn");
      if (probeBtn) {
        const tpProp = getComputedStyle(probeBtn).transitionProperty;
        // 提词中：不含 transform（装饰性位移被停）
        // 提词外：含 transform（正常有按压/悬停反馈）
        parts.push(`promptingCutsTransform=${tpProp.includes("transform") ? "no" : "yes"}`);
      }

      // 「接进度」的实况（点击正文那一下到底跳了没、跳到哪）
      parts.push(`gotoN=${gotoPos.length}`);
      if (gotoPos.length) parts.push(`gotoPos=${gotoPos.join(",")}`);
      // 「稿件」浮层现在是开着还是关着（点两次之后必须关着）
      parts.push(`historyOpen=${document.querySelector(".history") ? "yes" : "no"}`);

      // 「可交互区」上报的实况（穿透能否触发的唯一前提，见文件上方说明）
      parts.push(`zoneCalls=${zoneReports.length}`);      const lastZones = zoneReports.length
        ? zoneReports[zoneReports.length - 1]
        : "none";
      if (Array.isArray(lastZones)) {
        parts.push(`zoneN=${lastZones.length}`);
        parts.push(
          "zoneRects=" +
            lastZones.map((r) => r.map((v) => Math.round(v)).join(":")).join("|"),
        );
      } else {
        parts.push(`zoneLast=${String(lastZones)}`);
      }

      // 那一行里那个常驻状态槽的**实际呈现**（标签由生产者给，见 StatusWarning）。
      //   （原先这里还挂着一条"nowrap 是块级 const、挪出去就 ReferenceError"
      //    的警告 —— 那个坑本轮真的踩了（驾驶脚本里用了它），所以 `nowrap`
      //    已经被提到 `waitFor` 回调的最外层，这条警告不再适用。）
      const w = document.querySelector(".topbar .tb-status.warn, .topbar .tb-status.err");
      if (w) {
        /*
         * ★★ 第十一轮：`warnTitle`（原读 `data-hint`，即悬停气泡的正文）**删掉了**
         *   —— 提示已不存在。留下 `warnTip` 只报一件事：这一格**还能不能点**
         *   （末尾那个 `›` 是它唯一的可点线索，见 `Transport.tsx` 状态槽那段）。
         *   为什么不是"什么都不报"：这一格的语义是"信号只换位置、不消失"，
         *   而"能不能点"就是它与用户之间唯一的动作关系 —— 删提示不该顺手
         *   把它也删掉，那样"点不动了"就没人验了。
         */
        const go = w.querySelector(".tb-status-go");
        parts.push(
          `warnLabel=${nowrap((w.querySelector(".tb-status-tx")?.textContent || w.textContent).trim())}`,
          `warnCls=${nowrap(Array.from(w.classList).join("."))}`,
          `warnTap=${go ? "yes" : "no"}`,
          `warnCursor=${go ? getComputedStyle(w).cursor : "-"}`,
        );
      } else {
        parts.push("warnLabel=none");
      }
      // 状态槽里**任意**一条（不只警告）：匀速滚动跑着时这一格让给速度条，
      // 滚到头时它会换成绿色的「已讲完」。只量 `.warn/.err` 会把这两种都当成
      // "什么都没有"，于是"功能没实现"和"还没跑到"分不出来。
      const st = document.querySelector(".topbar .tb-status");
      parts.push(`slotText=${st ? nowrap((st.querySelector(".tb-status-tx")?.textContent || st.textContent).trim()) : "none"}`);
      parts.push(`slotCls=${st ? nowrap(Array.from(st.classList).join(".")) : "-"}`);
      /*
       * ★★ 状态槽**有没有被截断** —— 本轮新增的验收项。
       *
       * 为什么要它：这条槽是本项目唯一承载"警告"的常驻位置，而它挤在一行
       * 十几件事中间。改版后第一次量到的结果就是 `麦克风异…`（被压掉四个字），
       * 而**截图里那看起来像是文案写短了，不像故障** —— 所以必须有一个
       * 不靠眼睛判断的判据。`scrollWidth > clientWidth` 就是"字被吃掉了"。
       *
       * ★ 只量 `.tb-status-tx`（文字那一半），不量整个 `.tb-status`：
       *   末尾那个 `›` 是**允许**在空间不足时被挤掉的最后一样东西，
       *   量它会把这件正常的事报成故障。而长错误串本身就允许出省略号 ——
       *   所以 `slotClip=yes` 时还要看 `slotTxLen`，短标签（≤6 字）被截才算错。
       */
      if (st) {
        const tx = st.querySelector(".tb-status-tx");
        const tw = tx ? tx.scrollWidth : 0;
        const cw = tx ? Math.round(tx.clientWidth) : 0;
        parts.push(
          `slotClip=${tx && tw > cw + 1 ? "yes" : "no"}`,
          `slotTxW=${cw}/${Math.round(tw)}`,
        );
      } else {
        parts.push("slotClip=-", "slotTxW=-");
      }
      const sp = document.querySelector(".topbar .tb-speed");
      if (sp) {
        const inp = sp.querySelector("input[type=range]");
        parts.push(`speedW=${Math.round(sp.getBoundingClientRect().width)}`);
        parts.push(`speedVal=${inp ? inp.value : "?"}`);
        parts.push(`speedMinMax=${inp ? `${inp.min}-${inp.max}` : "?"}`);
      } else {
        parts.push("speedW=none");
      }
      if (diagEmit.length) parts.push(`emitFail=${diagEmit.join(",")}`);

      // ---- 跟读组（设置 › 跟读）的实际呈现 ----
      //
      // ★ 这几项存在的理由：这一组里"看着对"和"真的生效"是两件事 ——
      //   色点画出来很容易，但正文高亮有没有跟着换（`--read`）只有量才知道；
      //   开关画成绿的也很容易，但它背后那份配置有没有真的翻过来同样是另一回事。
      //   之前有过"两者逐字节相同"的教训（大圆按钮的就绪态与暂停态）。
      const readVar = getComputedStyle(document.documentElement)
        .getPropertyValue("--read")
        .trim();
      parts.push(`readVar=${nowrap(readVar)}`);

      const dots = document.querySelectorAll(".dots .dot");
      if (dots.length) {
        const on = document.querySelector(".dots .dot.on");
        const onCs = on ? getComputedStyle(on) : null;
        parts.push(
          `dots=${dots.length}`,
          `dotOn=${on ? nowrap(on.getAttribute("aria-label") || "") : "none"}`,
          // 选中态是**空心环**：背景应当透明、inset 阴影应当是那个颜色。
          // 两者一起看才能证明"它既是选中的、又没有丢掉自己的颜色"。
          `dotOnBg=${onCs ? nowrap(onCs.backgroundColor) : "none"}`,
          `dotOnRing=${onCs ? nowrap(onCs.boxShadow.slice(0, 28)) : "none"}`,
        );
      }
      const rowOf = (t) => {
        for (const r of document.querySelectorAll(".grp-row")) {
          if (r.textContent.includes(t)) return r;
        }
        return null;
      };
      for (const [k, t] of [
        ["jump", "跳读范围"],
        ["pos", "跟读位置"],
        ["sens", "跟读灵敏度"],
      ]) {
        const v = rowOf(t)?.querySelector(".grp-v")?.textContent?.trim();
        parts.push(`${k}Val=${v ? nowrap(v) : "none"}`);
      }
      for (const [k, t] of [
        ["smooth", "平滑滚动"],
        ["quick", "快捷跳读"],
      ]) {
        const sw = rowOf(t)?.querySelector(".switch");
        parts.push(`${k}Sw=${sw ? (sw.classList.contains("on") ? "on" : "off") : "none"}`);
      }

      // ---- 「推进方式」这个三选一控件的实际呈现 ----
      //
      // ★ 为什么单列一项：这是本轮唯一一个"换了它，别处的控件会跟着换家"的控件
      //   （选「匀速滚动」⇒ 参考语速那根滑杆换成匀速速度；选「语音跟随」⇒
      //    多出"来源"一行）。截图只能看出"有个分段控件"，看不出**选中的是哪一个**
      //   —— 而选错那一个，下面整组控件都是错的。
      const driveBtns = document.querySelectorAll(".drawer .seg.fill .seg-btn");
      const driveOn = document.querySelector(".drawer .seg.fill .seg-btn.on");
      parts.push(`driveN=${driveBtns.length}`);
      parts.push(`driveOn=${driveOn ? nowrap(driveOn.textContent.trim()) : "none"}`);
      // 速度/语速那根滑杆此刻挂的是哪个字段：`auto_wpm` 还是 `wpm_ref`。
      // 两者的区间不同（60-600 vs 180-420），看一眼 max 就知道挂对没有。
      const speedRow = rowOf("滚动速度") || rowOf("参考语速");
      const sr = speedRow?.querySelector('input[type="range"]');
      parts.push(`rateRow=${speedRow ? nowrap(speedRow.querySelector(".grp-t")?.textContent?.trim() || "?") : "none"}`);
      if (sr) parts.push(`rateMinMax=${sr.min}-${sr.max}`, `rateVal=${sr.value}`);

      // ---- 正文：当前句的明读指示色 ----
      const cur = document.querySelector(".sentence.current");
      if (cur) {
        parts.push(`curBg=${nowrap(getComputedStyle(cur).backgroundColor)}`);
        const sg = cur.querySelector(".singing");
        if (sg) parts.push(`curSing=${nowrap(getComputedStyle(sg).color)}`);
      }
    }

    /*
     * ★★ 这里原本量 `.tip-bubble` 的包围盒（`tipL/tipT/tipW/tipH/inside`），
     *   用来验"气泡有没有跑出窗口"。
     *
     *   第十一轮提示整个删了 ⇒ 这个块**永远走 else 报 `tip=none`**。
     *   关键：`tip=none` 现在是**正常且期望**的结果，而它恰好又是原来
     *   "气泡没渲染出来（半成品）"的失败信号 —— 同一个值同时代表两种
     *   完全相反的含义。这种"值还在、含义反转"的东西留着最坏事：
     *   下一个人看到 `tip=none` 会去查气泡为什么没出来。
     *   ⇒ 整块删掉。要守"提示不许复活"有 `check-tooltips.py`，不靠这里。
     */
    /*
     * 「支持作者」的码图**到底加载出来没有** —— 在收尾这一刻量。
     *
     * ★ 为什么不放在 `donateclick` 那一步里量：那一步距展开只有 600ms，
     *   而图片解码是异步的，量到的是"还没好"而不是"坏了"（实测
     *   `donateImgOk=0`，而同一个路径用 urllib 取是 200 + 59412 字节）。
     *   收尾这一刻已经过去好几秒，答案才有意义。
     *
     * ★ 这条检查守的是一个**没有任何门槛能发现**的失效：码图走的是
     *   `public/donate/*.jpg` → `dist/donate/*.jpg` 这条链。谁把 public
     *   目录挪走、或改了 `DONATE.weixinQr` 的相对路径，图就是裂的 ——
     *   而**裂图不会让构建失败、不会让 tsc 报错、截图上也只是一块空白**，
     *   它只是让"打赏"这件事悄悄失效。`naturalWidth` 是唯一的直接证据。
     *   `donateImgSrc` 顺手把**解析后的 src** 报出来，路径错了能一眼看出是哪一段。
     */
    const donorQrs = Array.from(document.querySelectorAll(".drawer .donate-qr"));
    if (donorQrs.length) {
      parts.push(
        `donateImgOkLate=${donorQrs.filter((i) => i.naturalWidth > 0).length}/${donorQrs.length}`,
        `donateImgNatural=${donorQrs.map((i) => i.naturalWidth).join(",")}`,
        `donateImgSrc=${nowrap(donorQrs[0].getAttribute("src") || "none")}`,
      );
    }

    /*
     * 更新提示条 + 齿轮上那颗圆点。
     *
     * ★ 为什么这两项必须量，而不是"出图看一眼就行"：
     *   ① 提示条是**条件渲染**的 —— 没有更新时整块 DOM 都不存在。
     *      所以"默认不出现"这件事只有 `updateBar=no` 能证明；在截图里
     *      "没有那条子"和"这条子根本没做出来"长得一模一样。
     *   ② 齿轮上那个圆点走 `::after` 伪元素，**DOM 里查不到节点**，
     *      只能查那个 class。而"它占不占布局宽度"是这一轮唯一的版面风险
     *      （顶栏的宽度预算是定数，多 6px 就要从标题嘴里出），
     *      所以下面把齿轮宽度一并量出来对。
     */
    const ub = document.querySelector(".update-bar");
    parts.push(`updateBar=${ub ? "yes" : "no"}`);
    parts.push(`updateBarH=${ub ? Math.round(ub.getBoundingClientRect().height) : 0}`);
    if (ub) {
      const msg = ub.querySelector(".ub-msg");
      parts.push(`ubMsg=${msg ? nowrap(msg.textContent || "") : "none"}`);
      parts.push(`ubBtns=${ub.querySelectorAll(".ub-btn").length}`);
      /* 几何：这条子必须真正**在顶栏下面**，而且正文要排在它下面。
       * ★ 这条断言是被一次出图逼出来的：`.topbar` 是 absolute，流内的
       *   提示条如果不给自己 margin，就会从 y=0 开始、被顶栏整个盖住
       *   —— 那时 DOM 里什么都量得到（`updateBar=yes`、高度 28），
       *   只有真去读几何才发现它在 y=0。 */
      const ur = ub.getBoundingClientRect();
      parts.push(`ubTop=${Math.round(ur.top)}`, `ubBottom=${Math.round(ur.bottom)}`);
    }
    const vp = document.querySelector(".viewport");
    if (vp) {
      const vr = vp.getBoundingClientRect();
      parts.push(`vpTop=${Math.round(vr.top)}`, `vpPadTop=${Math.round(
        parseFloat(getComputedStyle(vp).paddingTop) || 0,
      )}`);
    }
    const gear = document.querySelector(".topbar .icon-btn.has-update");
    parts.push(`gearDot=${gear ? "yes" : "no"}`);
    parts.push(`gearW=${gear ? Math.round(gear.getBoundingClientRect().width) : 0}`);

    parts.push(`vw=${innerWidth}`, `vh=${innerHeight}`);
    } catch (e) {
      document.title =
        "stubError=" + nowrap(String((e && e.message) || e)) + " @" + nowrap(String((e && e.stack || "").split("\n")[1] || ""));
      return;
    }
    document.title = parts.join(" ");
    // ★ 别量太早：无头环境下 React 把状态更新提交上来要几百毫秒（合成器帧少），
    //   量早了会读到中间态，看起来像功能没实现，其实只是没等到。
    //   收尾时间跟着动作数走，保证最后一个动作之后还留了余量。
    //
    //   `settle=<ms>` 再往后推一段，用来观察**依赖时间**的状态 ——
    //   例如穿透的「未生效」：它要等 2.2s 的宽限期过去才会从「开启中」变琥珀，
    //   默认那 1.3s 窗口根本看不到它。没有这个参数，这类状态就只能靠"读代码相信它"。
  }, 900 + steps.length * 400 + Number(params.get("settle") || 0));
});
