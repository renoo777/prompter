//! 跨平台「投屏隐身」能力抽象层。
//!
//! 本层唯一职责：让提词器窗口不出现在任何屏幕捕获结果中
//! （系统截图 / 录屏 / Zoom / Teams / 飞书会议共享 / OBS）。
//!
//! # 平台能力矩阵（实测 + 官方文档核实）
//!
//! | 平台 | 机制 | 状态 |
//! |---|---|---|
//! | Windows 10 2004+ | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` | 稳定，已实测通过飞书会议 |
//! | macOS ≤ 14 | `NSWindow.sharingType = .none` | 稳定 |
//! | macOS ≥ 15 | 公开 API 被系统架构变更废掉，仅剩私有 API | **脆弱，可能随系统升级失效** |
//! | Linux | 无通用方案（Wayland 明确不支持） | 不可用 |
//!
//! # 设计原则
//!
//! 「隐身」是**增强项**，不是产品依赖项。任何一个平台上失效时，产品都必须
//! 能退到兜底路径（只共享指定窗口 / 第二设备），所以这一层永远返回
//! [`SupportLevel`] 让前端**提前**告知用户，而不是等用户在真会议上踩坑。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

mod fallback;

/// macOS 后端的**离线编译检查**。
///
/// # 它解决的问题
///
/// `stealth/macos.rs` 里全文是 `objc_msgSend` 强转 + `dlsym` 私有符号查找，
/// 是整个项目最不可能靠肉眼看出错的一段。但它被 `#[cfg(target_os = "macos")]`
/// 圈着，而开发机是 Windows ⇒ **`cargo test` 从来不看它一眼**。
/// 于是"能不能编过"这件事，只有到 Mac 上第一次 `cargo build` 才会被回答 ——
/// 而那一刻同时还有 SDK / 链接 / Tauri 一堆报错挤在一起，根本分不清是哪一个。
///
/// # 它怎么工作
///
/// 把 `macos.rs` **原地再包一层** `#[cfg(target_os = "macos")]` 模块，
/// 用 `--cfg prompter_check_macos` 强制编译。
///
/// ★★ 关键手法：`mod macos_check { #![cfg(target_os = "macos")] ... }`
///    这一条 inner attribute 会**先于一切**把整个模块（含 `libc` 的路径解析）
///    判死，所以那些 macOS 专属 API 在 Windows 上根本不会被 name-resolve。
///    换成 `#[cfg] mod macos_check { ... }` 就**做不到这件事** ——
///    cfg 在解析阶段就把它剔了，模块里的 `#[cfg]` 属性完全不会求值。
///
/// # 为什么要有 `__check`
///
/// `mod macos;` 在 Windows 上是**不存在**的。若这里直接 `use super::macos;`，
/// Windows 开这个 cfg 时会报一条"找不到 `macos`"的假错，把人引向错误方向。
/// 所以先补一个空壳：它只在 Windows + 开了检查 cfg 时存在，只为了占位。
/// macOS 上 `super::macos` 是真的那个模块，这个壳靠 `cfg` 自动消失。
#[cfg(not(target_os = "macos"))]
mod __check {
    pub mod macos {}
}

#[cfg(prompter_check_macos)]
pub mod macos_check {
    #![cfg(target_os = "macos")]
    #[allow(unused_imports)]
    use super::__check::macos;
    include!("macos.rs");

    /// 让这个只做编译检查的模块**至少有一个名字被用到**。
    ///
    /// 不加它，开了 `prompter_check_macos` 但代码里没人提 `macos_check` 时，
    /// `unused` 那一族 lint 会开始抱怨 —— 而门槛要求"零警告"，
    /// 于是检查本身反而把门槛弄红了。
    pub fn _touch() {}
}

use serde::{Deserialize, Serialize};

/// 原生窗口句柄。Windows 下是 `HWND`，macOS 下是 `NSWindow*`。
/// 用不透明指针传递，避免上层代码依赖任何平台类型。
pub type NativeHandle = *mut std::ffi::c_void;

/// 隐身的两种目标状态。
///
/// 刻意只留两个值 —— 不做「半透明保护」「只在某分辨率下保护」这类
/// 中间态，因为底层的 `SetWindowDisplayAffinity` / `sharingType` 都是
/// 是非二元开关，任何中间态都只能靠应用层凑，反而会引入"以为开了其实没开"的风险。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StealthMode {
    /// 窗口不进入任何捕获结果。
    Hidden,
    /// 窗口正常参与捕获（关掉保护）。
    Visible,
}

impl StealthMode {
    pub fn is_hidden(self) -> bool {
        matches!(self, StealthMode::Hidden)
    }

    pub fn toggled(self) -> Self {
        match self {
            StealthMode::Hidden => StealthMode::Visible,
            StealthMode::Visible => StealthMode::Hidden,
        }
    }
}

/// 平台对该能力的支持等级。
///
/// 前端用这个决定「隐身」开关是默认打开、还是显示成需要用户确认的风险项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "note")]
pub enum SupportLevel {
    /// 公开 API 直接可用，系统升级不会破坏。
    Native,
    /// 可用，但依赖非公开路径或特定系统版本 —— 可能随系统升级失效。
    /// 附带的字符串是给用户看的具体原因。
    Fragile(String),
    /// 该平台没有任何可用方案。附带替代方案说明。
    Unavailable(String),
}

/// 平台后端必须实现的接口。
///
/// 做成 trait 而不是 `#[cfg]` 直接写在调用点，是为了让上层状态机
/// 完全不需要知道平台差异，也方便将来加「虚拟显示器」「第二设备」等后端。
pub trait CaptureGuard: Send + Sync {
    /// 当前平台的静态能力声明。**不涉及具体窗口**，可在 UI 初始化时调用。
    fn support(&self) -> SupportLevel;

    /// 对该平台不生效时的兜底建议。`None` 表示原生可用、无需兜底。
    fn fallback_hint(&self) -> Option<String>;

    /// 对指定窗口应用/取消保护。
    ///
    /// 实现方必须**验证**调用结果（例如 Windows 下回读
    /// `GetWindowDisplayAffinity`），不能只凭「调用没报错」就返回成功 ——
    /// 这个能力的失败模式是静默的，用户只会在真会议上才发现稿件被投出去了。
    fn apply(&self, handle: NativeHandle, mode: StealthMode) -> Result<StealthReport, StealthError>;
}

/// 一次切换操作的完整结果，直接序列化给前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StealthReport {
    /// 目标模式。
    pub mode: StealthMode,
    /// 是否**确认**生效（不是"调用了 API"，是"系统确认当前是保护态"）。
    pub applied: bool,
    /// 人类可读说明，成功与失败都给。
    pub detail: String,
    /// 平台能力等级快照，方便前端同步 UI。
    pub support: SupportLevel,
    /// 兜底建议，失败时必给。
    pub fallback_hint: Option<String>,
}

/// 隐身层的错误。
///
/// 刻意只有两种：「平台根本不支持」和「拿不到窗口句柄」。
///
/// 「API 调用失败」与「调用了但没生效」这两种情况**不走 `Err`**，
/// 而是通过 `StealthReport { applied: false, detail, fallback_hint }` 返回 ——
/// 对用户来说它们不是"出错了"，而是"这个能力此刻用不了，请这样做"，
/// 需要附带可以立刻照做的兜底指引。塞进错误类型反而会丢掉这部分信息。
#[derive(Debug, thiserror::Error)]
pub enum StealthError {
    #[error("当前平台没有可用的窗口捕获保护方案：{0}")]
    Unsupported(String),
    #[error("无法取得原生窗口句柄：{0}")]
    NoHandle(String),
}

/// 取得当前平台的后端实现。进程内只应创建一次并复用。
pub fn backend() -> Box<dyn CaptureGuard> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsGuard::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacGuard::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Box::new(fallback::UnsupportedGuard::new())
    }
}

/// 便于日志与 UI 显示的当前平台标识。
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}
