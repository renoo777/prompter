//! 应用状态：把稿件、对位器、识别引擎、隐身后端装进一个可被 Tauri 管理的容器。
//!
//! # 为什么对位器要单独用 `Arc<Mutex<..>>` 包一层
//!
//! 识别引擎在**自己的工作线程**里回调结果，回调闭包里必须能改到对位器。
//! 而 Tauri 的 `State<'_, T>` 是借用的、不能 move 进闭包。所以这里把
//! 「稿件 + 对位器 + 当前假设」这套需要被跨线程访问的数据单独放进
//! [`Engine`]，用 `Arc<Mutex<..>>` 共享给命令层和识别回调两方。

use crate::align::Aligner;
use crate::asr::{AsrConfig, AsrEngine, AsrUpdate, EngineKind, ModelPaths, UpdateSink};
use crate::script::{self, Script};
use crate::stealth::{self, CaptureGuard, StealthMode, StealthReport, SupportLevel};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 全局快捷键配置。字符串格式为 `修饰键+Code名`，例如 `Ctrl+Alt+ArrowDown`。
///
/// ⚠️ 这里的 `#[serde(default)]` **不是可选的**。
/// 它挂在外层 `AppConfig` 上只保证「`hotkeys` 这个键整个缺失时用默认值」，
/// 一旦用户配置里已经有 `hotkeys`（所有老用户都是这样），
/// 再往里加一个字段而没有 `serde(default)`，整个 `HotkeyConfig` 就会反序列化失败
/// —— 而失败会一路冒泡到 `AppConfig`，结果是**用户所有设置被静默重置**
/// （`load_config` 用 `.ok()` 吞掉错误后回落 `default()`，界面上看不出任何异常）。
/// 新增热键字段时务必确认这一行还在。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub next_sentence: String,
    pub prev_sentence: String,
    pub toggle_stealth: String,
    pub toggle_visibility: String,
    pub toggle_recognition: String,
    /// 收起 / 展开控制栏。
    pub toggle_collapse: String,
    /// 鼠标穿透开关。
    pub toggle_click_through: String,
}

/// 默认热键的**修饰键**部分。两个平台不一样，理由如下。
///
/// # ★★ 为什么 macOS 上是 `Cmd+Option` 而不是照搬 `Ctrl+Alt`
///
/// macOS 上 `Ctrl` 不是"那个"修饰键：
/// * 系统层面 `Ctrl+左键` = **右键**，`Ctrl+方向键` = **切桌面/全屏**
///   （Mission Control 的默认绑定）。照搬过去会跟系统抢键 —— 表现是
///   "热键时灵时不灵"，而且失败得毫无规律（取决于焦点在哪个应用）。
/// * 用户心智里 macOS 的"命令键"是 `Command`。
///
/// 所以 macOS 用 `Command+Option`，等价位置上仍是三个键的组合、也不易误触。
/// ⚠️ 这一条**未在真机验证**（`Ctrl+方向键` 与 Mission Control 的实际冲突
///    需要在真 Mac 上按一次才知道），见 `docs/UI改版-PM拆解.md` 的未验证清单。
///
/// ★ 用 `const fn` 而不是写死字符串：这样"平台不同、热键不同"是一处判断，
///   而不是散在七个字段里的七份复制粘贴。
const fn mods() -> &'static str {
    if cfg!(target_os = "macos") {
        "Command+Option"
    } else {
        "Ctrl+Alt"
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            next_sentence: format!("{}+ArrowDown", mods()),
            prev_sentence: format!("{}+ArrowUp", mods()),
            toggle_stealth: format!("{}+KeyS", mods()),
            toggle_visibility: format!("{}+KeyH", mods()),
            toggle_recognition: format!("{}+Space", mods()),
            toggle_collapse: format!("{}+KeyM", mods()),
            toggle_click_through: format!("{}+KeyP", mods()),
        }
    }
}

/// 窗口几何偏好。用户拖一次就该一直生效，所以持久化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    pub width: u32,
    pub height: u32,
    /// 屏幕坐标，可为负（副屏在主屏左侧）。
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// 位置锁定：开启后拖不动窗口，防会议中误拖。
    pub locked: bool,
}

impl Default for WindowPrefs {
    fn default() -> Self {
        Self {
            width: crate::win::geometry::DEFAULT_W,
            height: crate::win::geometry::DEFAULT_H,
            x: None,
            y: None,
            locked: false,
        }
    }
}

/// 正文对齐方式。
///
/// 序列化成 `"left" | "center" | "right"`，**直接就是 CSS `text-align` 的取值** ——
/// 前端把它写进 `--align` 变量即可，不需要再做一次映射。
/// 这里刻意不提供 `css()` 之类的转换方法：多一层映射就多一处会分叉的地方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptAlign {
    Left,
    Center,
    Right,
}

impl Default for ScriptAlign {
    fn default() -> Self {
        Self::Left
    }
}

/// 界面主题。
///
/// 默认深色**不是审美偏好**，是使用场景决定的：窗口长期浮在 PPT 之上、
/// 视线要在两者间高频切换，浅色底会让瞳孔反复收缩（见 `styles.css` 开头）。
/// 浅色只作为"打印/投屏前校对"的可选项存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiTheme {
    Dark,
    Light,
    System,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self::Dark
    }
}

/// 背景磨砂档位。
///
/// 第十二轮（磨砂玻璃）把上一代的 `glass: bool` 升级成三档：
///   * `Off`    —— 实色底，什么都不磨（对应老配置 `glass: false`）；
///   * `Css`    —— 柔光：栏自己的 `backdrop-filter`（对应老配置 `glass: true`，现状默认）；
///   * `System` —— 系统材质：Windows Acrylic / macOS hudWindow，
///     **磨的是窗口后面的东西**（PPT），这是 CSS 在原理上做不到的那一档。
///
/// 老配置里没有这个键：`load_config` 在反序列化之前按旧 `glass` 布尔折算
/// （`migrate_legacy_glass`），所以这里的 `Default` 只在"全新安装"时生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlassMode {
    Off,
    Css,
    System,
}

impl Default for GlassMode {
    fn default() -> Self {
        // 默认「柔光」＝接出这个开关之前的现状（`glass: true`）。
        // 不能默认 `System`：系统材质会改变窗体合成方式，
        // 而隐身/穿透是立身之本，默认路径绝不能替用户冒这个险（spike 结论先行验证）。
        Self::Css
    }
}

/// 「跟读颜色」的出厂色。
///
/// 色板清单与各自的 RGB 都归**界面**（`src/lib/readColors.ts`）—— 亮色主题下
/// 同一个色号需要微调，那是视觉的事，不该写进后端。后端只保留"默认挑哪一个"，
/// 因为 `AppConfig::default()` 需要它；前端则**完全消费**这个值
/// （bootstrap 把 config 发下来，前端照着查表），所以前端不需要第二份默认值。
///
/// 取值必须能在前端色表里查到，否则表现为"选了个颜色但正文没变"。
pub const READ_COLOR_DEFAULT: &str = "green";

/// 持久化配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub asr: AsrConfig,
    /// 启动时自动打开隐身。
    pub stealth_on_start: bool,
    /// 正文字号（px）。
    pub font_size: u32,
    /// 窗口不透明度 0.3~1.0。会议里常调低以便透看后面的 PPT。
    pub opacity: f32,
    /// 行高倍数。
    pub line_height: f32,
    /// 左右镜像 —— 给分光镜（提词器玻璃）用。
    pub mirror: bool,
    /// 已读部分是否变暗。
    pub dim_read: bool,
    pub hotkeys: HotkeyConfig,
    /// 上次使用的稿件路径，下次启动自动载入。
    ///
    /// **不再是唯一真相源** —— 稿件库（`library.rs`）建立后，它是"上次那一条"
    /// 的冗余记录。保留不删的理由：直接删字段会让老用户"上次的稿子"这个
    /// 可见行为静默消失（`serde(default)` 不会报错，只会悄悄少一个功能）。
    pub last_script: Option<String>,

    // ---- 窗口 ----
    pub window: WindowPrefs,

    // ---- 排版工具箱（R1）----
    /// 正文左右内边距（px）。控制每行字数，直接影响眼跳难度。
    pub padding_x: u32,
    /// 字距（em）。长句挤在一起时管用。
    pub letter_spacing: f32,
    /// 当前句在视口里的垂直锚点（0.15~0.60）。
    /// 之前是硬编码 0.36，现在是可调的 —— 这条实用价值最高：
    /// 有人喜欢"念的那句在中间"，有人喜欢"在偏上，下面多留些备读"。
    pub anchor: f32,
    pub align: ScriptAlign,
    /// 界面主题。
    pub theme: UiTheme,
    /// 背景磨砂档位（见 [`GlassMode`]）。
    ///
    /// 前身是 `glass: bool`（栏的 backdrop-filter 开关，默认开但**必须可关** ——
    /// 这个窗口同时在做透明合成 + 长期置顶 + 实时 ASR，
    /// 会议场合为观感引入掉帧风险不划算）。
    /// `System` 档同样必须能一键退回 —— 它改的是窗体合成，比 CSS 那一层更重。
    pub glass_mode: GlassMode,

    // ---- 鼠标穿透（R4c）----
    /// 默认 **false**。与 Snipaste 一致：默认关闭才不会有"莫名其妙点不动"。
    pub click_through: bool,

    // ---- 控制栏形态 ----
    /// 「固定控制栏」：控制栏浮在正文之上（`true`，默认），
    /// 还是占独立一行、正文从它下面开始（`false`）。
    ///
    /// 两种都保证控制栏不随正文滚动；差别只在**占不占地方**。
    /// 默认 `true` 的理由：小窗口里每一行都宝贵，控制栏压住第一行
    /// 比永久吃掉 68px 高度划算；而且只有浮着时玻璃才有东西可采样。
    pub bar_pinned: bool,

    // ---- 跟读灵敏度（P1）----
    /// 对位引擎的匹配下限 0.0~1.0。调低 = 跟得紧但容易乱跳，
    /// 调高 = 稳但遇到口音/快语速会跟不上。默认值来自 `align` 模块的调参。
    pub match_floor: f32,

    // ---- 跟读手感（P3，设置页「跟读」那一组）----
    /// 跳读范围（句）：跟读时允许自动向前/向后跳多远去找你的声音。
    ///
    /// 默认 20 句 —— 经 `align::jump_window` 换算是 240 字，
    /// **与引擎此前的硬编码窗口逐字相同**，所以接出这条滑杆不改变任何出厂手感。
    pub jump_range: u32,
    /// 当前句的高亮色，存的是色板 key（如 `"green"`）而不是 RGB。
    ///
    /// 存 key 的理由：色板本身属于**界面**（亮色主题下同一色号可能要调整），
    /// 而配置里该留下的是"用户挑了哪一个"这个意图。前端查不到 key 就退默认色，
    /// 不会因为改过色板而让老配置崩掉。
    pub read_color: String,
    /// 平滑滚动。关掉＝每次跳句瞬间到位，眼睛不用追动画。
    pub smooth_scroll: bool,
    /// 快捷跳读：单击正文任意一句即把进度接过去。
    /// 关掉不是禁用，而是**提高门槛**——改成双击才跳，防的是误触。
    pub quick_jump: bool,

    // ---- 语速标尺（P2）----
    /// 参考语速（字/分钟）。用来把「还剩多少」换算成「还要讲多久」。
    /// 默认 300 取自演讲场景的常见区间中值。
    pub wpm_ref: u32,

    // ---- 匀速滚动（推进方式三选一里的第三种）----
    /// 匀速滚动的速度（字/分钟）。
    ///
    /// 与 `wpm_ref` **分开存**，尽管出厂值相同：
    /// `wpm_ref` 回答"我大概讲多快"（把剩余字数换算成剩余时间），
    /// 这个回答"这一场我要稿子以多快往下走"。开讲一边听一边微调的是后者，
    /// 而调它不该把"还剩几分钟"的估算方式一起改掉。
    /// 出厂取与 `wpm_ref` 相同的 300，让两根滑杆一开始看起来是一回事。
    pub auto_wpm: u32,

    // ---- 置顶 ----
    /// 窗口置顶（永远盖在别的窗口之上）。
    ///
    /// ★ 它曾经是**纯运行时属性**（设置页里一个 `useState`，读完就丢），
    ///   当时的论证是"它是摆窗口时的一次性动作"。真用起来才知道那条论证不成立：
    ///   提词器的常态就是盖在 PPT 上，每次开机都要重新点一次 —— 而"设置不记住"
    ///   在用户那里没有第二种解释，只会被当成 bug。
    ///
    /// ★ 它同时被两处改：设置里的开关，以及「开始提词 / 收摊」
    ///   （前者顺带置顶、后者撤置顶）。所以**不能只在前端存** ——
    ///   两边各写一份迟早会出现"界面说置顶、窗口没置顶"。
    ///   落在这里、并且只由后端写（`commands::set_always_on_top`），是唯一一份。
    ///
    /// 默认 `false`：与"接出这个开关之前"的行为一致 —— 默认置顶会让窗口
    /// 莫名其妙盖住别的东西，而用户找不到原因。
    pub always_on_top: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            asr: AsrConfig::default(),
            stealth_on_start: true,
            font_size: 34,
            opacity: 0.94,
            line_height: 1.7,
            mirror: false,
            dim_read: true,
            hotkeys: HotkeyConfig::default(),
            last_script: None,
            window: WindowPrefs::default(),
            padding_x: 28,
            letter_spacing: 0.01,
            anchor: 0.36,
            align: ScriptAlign::default(),
            theme: UiTheme::default(),
            glass_mode: GlassMode::default(),
            click_through: false,
            bar_pinned: true,
            match_floor: crate::align::DEFAULT_MIN_SCORE,
            jump_range: crate::align::DEFAULT_JUMP_RANGE,
            read_color: READ_COLOR_DEFAULT.to_string(),
            // 平滑滚动默认开：与接出这条开关之前的行为一致（此前是硬编码 smooth）。
            smooth_scroll: true,
            // 快捷跳读默认开：同理，接出开关之前单击就是跳。
            quick_jump: true,
            wpm_ref: 300,
            auto_wpm: crate::asr::auto_scroll::DEFAULT_WPM as u32,
            // 默认不置顶：与"这个开关存在之前"的行为一致。
            always_on_top: false,
        }
    }
}

/// 把**后端自己拥有**的字段从当前内存态盖回待写的配置上。
///
/// # 为什么需要这一步
///
/// 配置是**整份**写的，而有两个写者：
///   * 前端（设置页改一项 ⇒ 防抖 400ms ⇒ `save_config` 一整份），
///   * 后端（窗口几何 / `last_script` / 穿透 / 锁定 / 置顶，各自即时写）。
///
/// 前端的副本是**启动那一刻**的快照。于是这个序列会出事：
///
///   ```text
///   ① 用户把窗口拖到 700×400        → 后端写 window = 700×400
///   ② 用户顺手关掉「背景模糊」      → 前端把自己那份（还写着 560×274）整份写回
///   ③ 配置文件里 window 退回 560×274
///   ⇒ 下次启动：窗口位置/大小、"上次那篇稿子"全部回到旧值
///   ```
///
/// 症状在用户那里只有一个说法：**"我设的东西没记住"** —— 而它指向的是配置，
/// 于是会被当成"保存坏了"，很难找到真正的原因（内存里谁都对，只有文件被反复覆盖）。
///
/// ⇒ 后端拥有的字段，一律以**内存态**为准；前端只决定它真正拥有的那些。
///   这条不是"顺便补一下"，它是"两个写者只有一个文件"这件事的**唯一**解法：
///   谁拥有谁负责，不靠时序侥幸。
///
/// 刻意做成纯函数（两个 `&AppConfig` 进出、不碰文件与窗口）—— 这样它可被独立单测，
/// 而"哪几个字段归后端"这件事就钉在测试里，不会随改动漂移。
pub fn merge_backend_owned(incoming: &mut AppConfig, current: &AppConfig) {
    // 窗口几何：由 `lib.rs` 的 Resized / Moved 处理就地更新，前端从不知道它变了。
    incoming.window.width = current.window.width;
    incoming.window.height = current.window.height;
    incoming.window.x = current.window.x;
    incoming.window.y = current.window.y;
    // 「上次那篇稿子」：载稿/切稿时由后端写，前端同样不知道。
    incoming.last_script = current.last_script.clone();
    // 穿透与置顶同理：**设置页那个开关并不改前端的 config 副本**
    // （它只发命令 + 改自己那份运行态），所以前端每次整份写回都在拿
    // 启动时的旧值覆盖它 —— 表现是"开了穿透/置顶，改个别的设置就失效"。
    incoming.click_through = current.click_through;
    incoming.always_on_top = current.always_on_top;
    // ★ `window.locked` **不在此列** —— 它由设置页写，前端是拥有者。
}

// ---------------------------------------------------------------------------
// 跨线程共享的对位核心
// ---------------------------------------------------------------------------

pub struct Engine {
    pub script: Script,
    /// 归一化字符流，对位算法的操作对象。与 `script.chars` 同步重建。
    pub flat: Vec<char>,
    pub aligner: Aligner,
    /// 最近一次的识别假设，用于界面上回显「听成了什么」。
    pub hyp: String,
    /// 最近一次的音量电平。
    pub level: f32,
}

impl Engine {
    fn empty() -> Self {
        Self {
            script: Script {
                title: "未载入稿件".into(),
                paragraphs: Vec::new(),
                chars: Vec::new(),
            },
            flat: Vec::new(),
            aligner: Aligner::new(Vec::new()),
            hyp: String::new(),
            level: 0.0,
        }
    }

    /// 换稿：重建归一化序列与对位器。
    pub(crate) fn set_script(&mut self, s: Script) {
        self.flat = script::flat_chars(&s);
        self.aligner.reset_with(self.flat.clone());
        self.script = s;
        self.hyp.clear();
    }

    /// 当前光标位置的完整描述。
    ///
    /// `pub(crate)` 是因为全局快捷键回调（`lib::dispatch`）也要用 ——
    /// 它没有命令层那些由 Tauri 注入的参数，只能直接操作 `AppState`。
    pub(crate) fn cursor_payload(
        &self,
        score: f32,
        moved: bool,
        miss: u32,
        realigned: bool,
    ) -> CursorPayload {
        let cursor = self.aligner.cursor();
        let total = self.aligner.total();
        let (paragraph, sentence, offset, s_start, s_end) =
            match self.script.chars.get(cursor.min(total.saturating_sub(1))) {
                Some(c) => {
                    let s = self
                        .script
                        .paragraphs
                        .get(c.paragraph as usize)
                        .and_then(|p| p.sentences.get(c.sentence as usize));
                    (
                        c.paragraph,
                        c.sentence,
                        c.offset_in_sentence,
                        s.map(|x| x.align_start).unwrap_or(0),
                        s.map(|x| x.align_end).unwrap_or(0),
                    )
                }
                None => (0, 0, 0, 0, 0),
            };

        CursorPayload {
            cursor,
            total,
            progress: self.aligner.progress(),
            paragraph,
            sentence,
            offset_in_sentence: offset,
            sentence_align_start: s_start,
            sentence_align_end: s_end,
            score,
            moved,
            miss_streak: miss,
            realigned,
        }
    }
}

// ---------------------------------------------------------------------------
// 传给前端的载荷
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPayload {
    pub cursor: usize,
    pub total: usize,
    pub progress: f32,
    pub paragraph: u32,
    pub sentence: u32,
    /// 句内字符偏移 —— 前端据此把当前句精确高亮到「读到的那个字」。
    pub offset_in_sentence: u32,
    pub sentence_align_start: u32,
    pub sentence_align_end: u32,
    pub score: f32,
    pub moved: bool,
    pub miss_streak: u32,
    pub realigned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptPayload {
    pub title: String,
    pub paragraphs: Vec<crate::script::Paragraph>,
    pub total_chars: usize,
}

impl From<&Script> for ScriptPayload {
    fn from(s: &Script) -> Self {
        Self {
            title: s.title.clone(),
            paragraphs: s.paragraphs.clone(),
            total_chars: s.chars.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrPayload {
    pub text: String,
    pub level: f32,
    pub is_final: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    /// 平台标识（windows / macos / linux）。
    ///
    /// 用 `String` 而不是 `&'static str`：后者没法 derive `Deserialize` ——
    /// serde 需要一个 `'de: 'static` 的约束，而它不会自动加。
    pub os: String,
    pub support: SupportLevel,
    pub fallback_hint: Option<String>,
    /// 「收起」这颗入口在当前平台叫什么、下面是哪行说明。
    ///
    /// # 为什么要从后端给，而不是前端按 `os` 自己分支
    ///
    /// 因为**同一个判断在后端已经存在一份**：`minimize_to_taskbar` 按
    /// `win::platform::action()` 决定是"最小化+放出任务栏按钮"还是"退化成收起"。
    /// 如果前端再按 `os === "macos"` 自己写一份文案分支，两处就会分头漂移 ——
    /// 表现是"按钮写着收进任务栏、按下去却是收起"，或者反过来。
    ///
    /// ★ 这正是本项目反复踩过的那类 bug（同一个判断存在两份实现）。
    ///   所以文案也由后端**给出**，前端只负责渲染，一个字都不自己拼。
    pub collapse_label: String,
    pub collapse_hint: String,
    /// 「任务栏」这个词在当前平台上**是否存在**，即那颗「收进任务栏」按钮该不该出现。
    ///
    /// Windows 上为 `true`；macOS 及其它平台为 `false` —— 那里的收起入口只有一颗按钮。
    pub has_taskbar: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapPayload {
    pub config: AppConfig,
    pub script: ScriptPayload,
    pub cursor: CursorPayload,
    pub platform: PlatformInfo,
    pub stealth: Option<StealthReport>,
    pub models_dir: String,
    pub found_models: Vec<ModelPaths>,
    pub asr_engine: Option<String>,
    pub app_version: String,
    /// 可载入的稿件扩展名（不含点）。**前端不自己维护这份清单** ——
    /// 文件对话框的 filter、空态文案、拖放报错都从这儿取，
    /// 保证"对话框里能选到的"和"拖进来能载入的"永远是同一批。
    pub script_exts: Vec<String>,
    /// 上面那份清单给用户看的样子，例如 `.txt / .md / .docx / .pptx`。
    pub script_exts_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrStatusPayload {
    pub running: bool,
    pub engine: String,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

pub struct AppPaths {
    pub config_file: PathBuf,
    pub models_dir: PathBuf,
    /// 稿件库索引（`scripts.json`）。
    pub library_file: PathBuf,
    /// 粘贴类稿件的正文副本目录。
    pub scripts_dir: PathBuf,
}

pub struct AppState {
    pub engine: Arc<Mutex<Engine>>,
    pub config: Mutex<AppConfig>,
    pub asr: Mutex<Option<Box<dyn AsrEngine>>>,
    pub guard: Box<dyn CaptureGuard>,
    pub stealth_mode: Mutex<StealthMode>,
    pub app: AppHandle,
    pub paths: AppPaths,
    /// 稿件库（最近使用过的稿件）。
    pub library: Mutex<crate::library::ScriptLibrary>,
    /// 窗口层共享状态（几何 / 收起 / 穿透）。
    pub hub: Arc<crate::win::WindowHub>,
    /// 托盘图标。为 `None` 时说明托盘没建起来 —— 那种情况下
    /// 穿透的"找回窗口"就只剩热键一条路，界面上要提醒用户（R5-3）。
    pub tray: Mutex<Option<tauri::tray::TrayIcon>>,
    /// 「收进任务栏」走到哪一步了（状态机见 [`crate::win::taskbar`]）。
    ///
    /// ★ 为什么是 **三态** 而不是一个 bool：从"放出任务栏按钮"到"窗口真的
    ///   最小化"之间有一段窗口期，那一期里送来的窗口事件必须**什么都不做**。
    ///   用 bool 就分不出"刚放按钮、还没最小化"和"已经最小化过"，
    ///   于是会把按钮提前撤掉 —— 症状正是窗口缩下去了、任务栏里什么都没有。
    taskbar: AtomicU8,
}

impl AppState {
    pub fn new(app: AppHandle) -> Self {
        let config_dir = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));

        let paths = AppPaths {
            config_file: config_dir.join("config.json"),
            models_dir: data_dir.join("models"),
            library_file: config_dir.join("scripts.json"),
            scripts_dir: config_dir.join(crate::library::COPY_DIR),
        };

        // 模型目录不存在就建出来 —— 设置页里的「打开模型目录」按钮
        // 不应该点开一个不存在的路径。
        let _ = std::fs::create_dir_all(&paths.models_dir);

        let mut config = load_config(&paths.config_file).unwrap_or_default();

        // 自动发现模型：用户把模型解压到 models/ 下就应该能被识别，
        // 不需要手填四个文件路径。
        let found = scan_models(&paths.models_dir);
        if config.asr.local_model.is_none() {
            if let Some(first) = found.first() {
                config.asr.local_model = Some(first.clone());
            }
        }

        // 老用户升级：把旧配置里的 `last_script` 迁进稿件库。
        // 必须在读配置之后、任何人用到库之前做掉，否则第一屏的"最近稿件"
        // 会是空的 —— 用户会以为历史功能没生效。
        let library = load_library(&paths).unwrap_or_default();
        let mut library = library;
        let mut migrated = false;
        if library.entries.is_empty() {
            if let Some(p) = config.last_script.clone() {
                if std::path::Path::new(&p).is_file() {
                    let title = PathBuf::from(&p)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("上次的稿件")
                        .to_string();
                    migrated = library.migrate_file(&p, &title, now_unix());
                    log::info!("[library] 已从 last_script 迁移历史首项");
                }
            }
        }
        if migrated {
            if let Err(e) = save_library(&paths, &library) {
                // 迁移失败不能挡住启动：历史是增强，不是主路径
                log::warn!("[library] 迁移结果落盘失败：{e}");
            }
        }

        Self {
            engine: Arc::new(Mutex::new(Engine::empty())),
            config: Mutex::new(config),
            asr: Mutex::new(None),
            guard: stealth::backend(),
            stealth_mode: Mutex::new(StealthMode::Visible),
            app,
            paths,
            library: Mutex::new(library),
            hub: Arc::new(crate::win::WindowHub::new()),
            tray: Mutex::new(None),
            taskbar: AtomicU8::new(crate::win::taskbar::TaskbarHold::None.as_u8()),
        }
    }

    /// 命令侧：准备收进任务栏（任务栏按钮即将放出）。
    pub fn arm_taskbar(&self) {
        use crate::win::taskbar::TaskbarHold;
        self.taskbar.store(TaskbarHold::Armed.as_u8(), Ordering::Relaxed);
    }

    pub fn taskbar_hold(&self) -> crate::win::taskbar::TaskbarHold {
        crate::win::taskbar::TaskbarHold::from_u8(self.taskbar.load(Ordering::Relaxed))
    }

    /// 收到一次窗口事件时推进握手，返回**该对窗口做的动作**。
    ///
    /// 判断与写状态放在一起，是为了让"读—算—写"不出现撕裂：
    /// 窗口事件一秒能来几十次，两个线程各自 add 一次就会重复回收。
    pub fn advance_taskbar(&self, minimized: bool) -> crate::win::taskbar::TaskbarStep {
        use crate::win::taskbar::{taskbar_step, TaskbarHold, TaskbarStep};
        let step = taskbar_step(self.taskbar_hold(), minimized);
        match step {
            TaskbarStep::NoteMinimized => {
                self.taskbar.store(TaskbarHold::Minimized.as_u8(), Ordering::Relaxed)
            }
            TaskbarStep::Release => {
                self.taskbar.store(TaskbarHold::None.as_u8(), Ordering::Relaxed)
            }
            TaskbarStep::Nothing => {}
        }
        step
    }

    /// 走"显式还原"这条路（托盘菜单 / 全局热键）时一次了结：
    /// 返回**刚才有没有欠着一次回收**，有就该去 `set_skip_taskbar(true)`。
    pub fn clear_taskbar_hold(&self) -> bool {
        use crate::win::taskbar::TaskbarHold;
        let was = self.taskbar_hold().is_held();
        if was {
            self.taskbar.store(TaskbarHold::None.as_u8(), Ordering::Relaxed);
        }
        was
    }

    pub fn cursor_snapshot(&self) -> CursorPayload {
        match self.engine.lock() {
            Ok(e) => e.cursor_payload(0.0, false, 0, false),
            Err(_) => CursorPayload {
                cursor: 0,
                total: 0,
                progress: 0.0,
                paragraph: 0,
                sentence: 0,
                offset_in_sentence: 0,
                sentence_align_start: 0,
                sentence_align_end: 0,
                score: 0.0,
                moved: false,
                miss_streak: 0,
                realigned: false,
            },
        }
    }

    /// 构造识别回调：把识别结果喂给对位器，再通过 Tauri 事件推给界面。
    ///
    /// 这里的锁范围刻意压到最小 —— 只用来算载荷，`emit` 在锁外调用。
    /// 否则界面一旦因为重渲染而响应变慢，就会反过来卡住识别线程。
    pub fn make_sink(&self) -> UpdateSink {
        let engine = self.engine.clone();
        let app = self.app.clone();

        Box::new(move |u: AsrUpdate| {
            if let Some(err) = u.error {
                let _ = app.emit("prompter://error", err);
                let _ = app.emit(
                    "prompter://asr-status",
                    AsrStatusPayload {
                        running: false,
                        engine: String::new(),
                        detail: "识别已停止".into(),
                    },
                );
                return;
            }

            // 非致命警告：识别仍在跑，只是结果大概不对。
            //
            // **绝不能塞进 error 通道** —— 那条通道会把状态改成「已停止」，
            // 而这时候引擎其实一切正常，只是拿不到声音。
            // 用户看到「已停止」会去重新点一次开始识别，当然不解决问题。
            if let Some(w) = u.warn {
                let _ = app.emit("prompter://warning", w);
                return;
            }

            let (cursor_payload, asr_payload, ended) = {
                let Ok(mut e) = engine.lock() else { return };
                e.level = u.level;

                if let Some(pos) = u.seek {
                    // 匀速滚动：位置是引擎按时间算出来的，**直接落**，不做匹配。
                    // 与下面那条文本匹配的路是并列的两个来源，不是一条路上的分支。
                    let total = e.aligner.total();
                    let pos = pos.min(total);
                    e.aligner.set_cursor(pos);
                    // 清掉「最近听到的」：这不是一次识别。留着上一次的残句
                    // 会让人以为麦克风还在工作 —— 而这条路根本不碰麦克风。
                    e.hyp.clear();
                    let done = total > 0 && pos >= total;
                    (
                        Some(e.cursor_payload(1.0, true, 0, false)),
                        AsrPayload {
                            text: String::new(),
                            level: 0.0,
                            is_final: done,
                        },
                        done,
                    )
                } else {
                    // 只有文本真的变了才重新对位。这一条同时解决两个问题：
                    //
                    //   * 电平心跳送的文本和上次一样（甚至为空）时，不能被当成一次
                    //     「识别结果」喂进对位器 —— 否则每次心跳都会按"没听清"
                    //     记一次 miss，几秒钟就能把 miss_streak 推高到误报「对不上」；
                    //   * 同一句文本重复推送时重算一遍纯属浪费 CPU。
                    let changed = !u.text.is_empty() && u.text != e.hyp;
                    let cursor_payload = if changed {
                        e.hyp = u.text.clone();
                        let outcome = e.aligner.update(&u.text);
                        Some(e.cursor_payload(
                            outcome.score,
                            outcome.moved,
                            outcome.miss_streak,
                            outcome.realigned,
                        ))
                    } else {
                        None
                    };

                    (
                        cursor_payload,
                        AsrPayload {
                            text: u.text,
                            level: u.level,
                            is_final: u.is_final,
                        },
                        false,
                    )
                }
            };

            // 光标只在真正重新对位过之后才推 —— 没有新信息时推一个和上次
            // 一模一样的载荷，只会让界面白重渲染一遍。
            if let Some(c) = cursor_payload {
                let _ = app.emit("prompter://cursor", c);
            }
            let _ = app.emit("prompter://asr", asr_payload);

            if ended {
                // ★ 稿子滚完了，引擎那边已经停住 —— 必须把这件事说出来。
                //
                //   不说的话，大圆按钮会一直停在「提词中」（实心绿），
                //   而光标一动不动。那正是"卡住了"的观感，而实际是**正常结束** ——
                //   "界面显示实际在工作，不是用户按下的意图"这条纪律的反面。
                //
                //   `engine` 留空与上面 error 分支同一个处置：名字由界面自己显示，
                //   这里塞一个写死的字符串，改文案时就会两处不一致。
                let _ = app.emit(
                    "prompter://asr-status",
                    AsrStatusPayload {
                        running: false,
                        engine: String::new(),
                        detail: String::new(),
                    },
                );
            }
        })
    }
}

// ---------------------------------------------------------------------------
// 配置读写与模型扫描
// ---------------------------------------------------------------------------

pub fn load_config(path: &PathBuf) -> Option<AppConfig> {
    let bytes = std::fs::read(path).ok()?;
    let (text, enc) = script::decode_bytes(&bytes);
    log::info!("[config] 载入 {} （{enc}）", path.display());
    let mut v: serde_json::Value = serde_json::from_str(&text).ok()?;
    migrate_legacy_glass(&mut v);
    serde_json::from_value(v).ok()
}

/// 老配置（第十二轮之前）没有 `glass_mode`，只有一个 `glass: bool`。
///
/// 折算规则：**`glass: false` ⇒ `"off"`，`glass: true` 或没有 `glass` 键 ⇒ `"css"`**。
///
/// ★ 为什么不用 `#[serde(default)]` 一笔带过：serde 的默认值是**固定**的（`Css`），
///   而老配置里 `glass: false` 是用户**明确关过**模糊 —— 替他折成 `Css`
///   等于升级一次就把他关掉的性能开销偷偷开回来。这正是本项目反复判过罪的
///   「我设的东西没记住」：不报错，但行为变了。
///
/// 放在反序列化**之前**的 JSON 层做，而不是给字段挂 `Option<GlassMode>`：
/// 后者会把"老配置"这个历史问题泄进类型系统，前端也得多处理一个 `null`。
fn migrate_legacy_glass(v: &mut serde_json::Value) {
    if v.get("glass_mode").is_some() {
        return;
    }
    let mode = if v.get("glass").and_then(|g| g.as_bool()) == Some(false) {
        "off"
    } else {
        "css"
    };
    log::info!("[config] 老配置迁移：glass ⇒ glass_mode={mode}");
    v["glass_mode"] = serde_json::Value::String(mode.into());
}

pub fn save_config(path: &PathBuf, cfg: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("无法创建配置目录：{e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("配置序列化失败：{e}"))?;
    std::fs::write(path, text).map_err(|e| format!("配置写入失败：{e}"))
}

// ---------------------------------------------------------------------------
// 稿件库读写
// ---------------------------------------------------------------------------

/// 当前 Unix 秒。抽出来是为了让 `now()` 的调用点集中在一处，便于排查时间相关问题。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 粘贴类稿件副本的绝对路径。
pub fn copy_path(paths: &AppPaths, rel: &str) -> PathBuf {
    // `copy_rel` 形如 `scripts/xxxx.txt`，去掉首段后拼到配置目录下
    let name = rel.rsplit('/').next().unwrap_or(rel);
    paths.scripts_dir.join(name)
}

/// 读稿件库。
///
/// 索引损坏时**不静默覆盖** —— 改名成 `scripts.json.bak` 再从空库开始。
/// 理由：用户的历史列表没了是小事，但如果他改配置时手抖写坏了文件，
/// 我们把损坏内容直接覆盖掉，就再也救不回来了。留个 `.bak` 成本几乎为零。
pub fn load_library(paths: &AppPaths) -> Result<crate::library::ScriptLibrary, String> {
    let bytes = match std::fs::read(&paths.library_file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(crate::library::ScriptLibrary::default())
        }
        Err(e) => return Err(format!("读取稿件库失败：{e}")),
    };
    let (text, _enc) = script::decode_bytes(&bytes);
    match serde_json::from_str::<crate::library::ScriptLibrary>(&text) {
        Ok(lib) => Ok(lib),
        Err(e) => {
            let bak = paths.library_file.with_extension("json.bak");
            let _ = std::fs::rename(&paths.library_file, &bak);
            log::warn!("[library] 索引损坏（{e}），已改名为 {} 后从空库开始", bak.display());
            Ok(crate::library::ScriptLibrary::default())
        }
    }
}

/// 写稿件库（原子性够用：先写临时文件再改名，避免写一半断电留下半个 JSON）。
pub fn save_library(
    paths: &AppPaths,
    lib: &crate::library::ScriptLibrary,
) -> Result<(), String> {
    if let Some(parent) = paths.library_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("无法创建配置目录：{e}"))?;
    }
    let text = serde_json::to_string_pretty(lib).map_err(|e| format!("稿件库序列化失败：{e}"))?;
    let tmp = paths.library_file.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("稿件库写入失败：{e}"))?;
    std::fs::rename(&tmp, &paths.library_file).map_err(|e| format!("稿件库落盘失败：{e}"))
}

/// 扫描模型目录，找出所有看起来完整的 sherpa-onnx 流式模型。
///
/// 判定标准是「同一目录下同时存在 encoder / decoder / joiner / tokens 四类文件」——
/// 用文件名匹配而不是读配置，因为用户从官网下载解压后就是这套命名。
pub fn scan_models(root: &PathBuf) -> Vec<ModelPaths> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if let Some(m) = probe_model_dir(&dir) {
            out.push(m);
        }
    }
    out
}

fn probe_model_dir(dir: &std::path::Path) -> Option<ModelPaths> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();

    let pick = |needle: &str| -> Option<String> {
        // 优先 int8 量化版：体积小、推理快，精度损失在汇报场景可忽略
        let mut cands: Vec<&PathBuf> = files
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_ascii_lowercase().contains(needle))
                    .unwrap_or(false)
            })
            .collect();
        cands.sort_by_key(|p| {
            let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
            if n.contains("int8") {
                0
            } else {
                1
            }
        });
        cands.first().map(|p| p.to_string_lossy().to_string())
    };

    let tokens = pick("tokens.txt")?;
    let encoder = pick("encoder")?;
    let decoder = pick("decoder")?;
    let joiner = pick("joiner")?;

    let label = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("模型")
        .to_string();

    Some(ModelPaths {
        encoder,
        decoder,
        joiner,
        tokens,
        label,
    })
}

/// 把当前配置里的引擎种类落成一个人类可读的名字。
///
/// 目前由前端直接读 `cfg.asr.engine` 再自行映射（`EngineKind` 的序列化值是
/// `local` / `cloud` / `manual`），所以这里暂时没有调用方；
/// 保留它是为了将来在**后端**需要拼接提示文案时不用再写一遍映射。
#[allow(dead_code)]
pub fn engine_label(kind: EngineKind) -> String {
    kind.label().to_string()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 「哪个字段归谁」必须钉成断言 —— 它是"两个写者共用一个文件"这件事的唯一依据。
    /// 这条红了就说明有人往后端拥有的那几个字段里加了新语义，而前端会在
    /// 下一次整份写回时把它抹掉（症状：用户设的东西"没记住"）。
    #[test]
    fn merge_keeps_backend_owned_fields() {
        let mut incoming = AppConfig::default();
        // 前端拥有
        incoming.font_size = 44;
        incoming.window.locked = true;
        // 前端这份副本是**启动时的旧值**：几何、稿件、穿透、置顶都停在那一刻
        incoming.window.width = 560;
        incoming.window.height = 274;
        incoming.window.x = Some(10);
        incoming.window.y = Some(20);
        incoming.last_script = Some("C:/old.txt".into());
        incoming.click_through = false;
        incoming.always_on_top = false;

        let mut current = AppConfig::default();
        current.window.width = 700;
        current.window.height = 400;
        current.window.x = Some(300);
        current.window.y = Some(120);
        current.window.locked = false; // 内存里是 false，但前端说了 true ⇒ 前端赢
        current.last_script = Some("C:/new.txt".into());
        current.click_through = true;
        current.always_on_top = true;

        merge_backend_owned(&mut incoming, &current);

        // 后端拥有 ⇒ 一律以内存态为准
        assert_eq!(incoming.window.width, 700);
        assert_eq!(incoming.window.height, 400);
        assert_eq!(incoming.window.x, Some(300));
        assert_eq!(incoming.window.y, Some(120));
        assert_eq!(incoming.last_script.as_deref(), Some("C:/new.txt"));
        assert!(incoming.click_through, "穿透归后端：前端整份写回不该把它关掉");
        assert!(incoming.always_on_top, "置顶归后端");
        // 前端拥有 ⇒ 照前端说的写
        assert_eq!(incoming.font_size, 44);
        assert!(incoming.window.locked, "锁定归前端，不能被后端那份覆盖掉");
    }

    /// 新增字段必须能被**老配置文件**（没有这个键）读出来。
    /// 少了 `#[serde(default)]`，老用户升级后不是"少一个功能"，而是**整份反序列化失败**
    /// —— 表现是启动后所有设置回到出厂值，看起来像"配置丢了"。
    #[test]
    fn new_fields_survive_old_config_files() {
        let old = r#"{"font_size":44,"theme":"light"}"#;
        let cfg: AppConfig = serde_json::from_str(old).expect("老配置必须还能反序列化");
        assert_eq!(cfg.font_size, 44);
        assert!(!cfg.always_on_top, "老配置里没有这一项 ⇒ 落到出厂默认（不置顶）");
    }

    /// `glass` ⇒ `glass_mode` 的迁移是**用户意图**问题，不是兼容问题：
    /// 老配置里 `glass: false` 是用户明确关过模糊，升级后绝不能被偷偷开回来。
    /// （这条按 `migrate_legacy_glass` 的四种输入钉死 —— 它要是被改坏，
    ///   受害用户不会收到任何信号，只会觉得"这软件变卡了"。）
    #[test]
    fn legacy_glass_bool_migrates_to_mode() {
        // glass:false ⇒ off（他关过，尊重他）
        let mut v = serde_json::json!({"glass": false});
        migrate_legacy_glass(&mut v);
        assert_eq!(v["glass_mode"], "off");

        // glass:true ⇒ css（现状）
        let mut v = serde_json::json!({"glass": true});
        migrate_legacy_glass(&mut v);
        assert_eq!(v["glass_mode"], "css");

        // 连 glass 键都没有 ⇒ css（与出厂默认一致）
        let mut v = serde_json::json!({"font_size": 44});
        migrate_legacy_glass(&mut v);
        assert_eq!(v["glass_mode"], "css");

        // 已经有 glass_mode ⇒ 一个字都不许动
        let mut v = serde_json::json!({"glass": false, "glass_mode": "system"});
        migrate_legacy_glass(&mut v);
        assert_eq!(v["glass_mode"], "system");
    }

    /// 迁移之后整份配置必须还能反序列化，且档位落到枚举上。
    #[test]
    fn migrated_config_deserializes_with_mode() {
        let mut v = serde_json::json!({"glass": false, "font_size": 40});
        migrate_legacy_glass(&mut v);
        let cfg: AppConfig = serde_json::from_value(v).expect("迁移后的配置必须能反序列化");
        assert_eq!(cfg.glass_mode, GlassMode::Off);
        assert_eq!(cfg.font_size, 40);
    }
}
