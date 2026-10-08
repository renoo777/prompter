//! 「收进任务栏」在不同平台上**到底该做什么** —— 纯函数，可单测。
//!
//! # 为什么需要这个文件
//!
//! Windows 上这套动作有一个明确的落点：把窗口最小化，并保证任务栏上有一枚
//! 按钮（`set_skip_taskbar(false)`），用户点它回来。整条链路
//! （`commands::minimize_to_taskbar` + `win::taskbar` 的三态握手）就是为它写的。
//!
//! 但 **macOS 根本没有"任务栏按钮"这个东西**：
//!
//! * macOS 的 Dock 只有在应用有 `activationPolicy = .regular` 时才显示图标，
//!   而这个提词器是 `.accessory` 语义（`skipTaskbar: true` + 托盘为主）。
//!   把策略临时改成 `.regular` 会露出 Dock 图标，但**只出现在冷启动时**
//!   （`NSApplicationActivationPolicy` 在启动后变更，Dock 不会立刻重排）——
//!   那正是 Windows 上那条 `ITaskbarList::AddTab` 的等价坑，只是更糟：
//!   Windows 上顺序对了就一定能补上，macOS 上根本补不上。
//! * 「最小化」在 macOS 上也是**窗口自己的事**（黄色圆钮 / `Cmd+M`），
//!   与 Dock 有没有图标无关。窗口缩下去之后，Dock 上没有图标就是真的没有入口。
//!
//! ⇒ 照搬 Windows 那套的结果是**静默失败**：窗口不见了、没有任何入口能找回来。
//! 这是本项目最不能接受的失败模式（见根 `MEMORY` 的「宁可显式失败」）。
//!
//! # 所以 macOS 上的决定是：不做这个动作，改用「收起」
//!
//! 理由不是"做不到就算了"，而是**收起达成本来那个目的**：
//! 用户按这颗键的意图是「让它现在别挡着我」。收起之后
//! * 窗口还在屏幕上（一条 46px 的细栏）；
//! * 顶栏整个还在，所以「展开」和「关闭」都点得到；
//! * 不依赖任何系统 API、不依赖热键、不依赖托盘。
//!
//! 它比 Windows 那条路**弱**（那块地方还是被占着 46px），
//! 但它是**可见的、可逆的、一定能回来的** —— 而 Windows 那条路一旦哪一环
//! 断了就是彻底找不回来。两害相权，选可达性。
//!
//! ⚠️ 更彻底的方案（应用级 `activationPolicy` 切换 + Dock 菜单）需要
//!    runtime FFI 操作 `NSApplication`，且必须在真机上验证 Dock 重排行为 ——
//!    本轮不做，见 `docs/UI改版-PM拆解.md` 的未验证清单。

/// 「收进任务栏」在当前平台上**实际会执行的动作**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarAction {
    /// 最小化窗口，并把任务栏按钮放出来（Windows）。
    MinimizeWithButton,
    /// 退化成「收起」：窗口压成一条细栏，不碰系统 API。
    CollapseInstead,
}

/// 当前平台的落点。
///
/// 写成 `const fn` 而不是读运行时状态：它是**平台属性**，
/// 同一个二进制里永远不会变，做成运行时可查只会让人以为它能被配置。
pub const fn action() -> TaskbarAction {
    if cfg!(target_os = "windows") {
        TaskbarAction::MinimizeWithButton
    } else {
        TaskbarAction::CollapseInstead
    }
}

/// 「收起」这个动作在当前平台上的**入口按钮该叫什么**。
///
/// ★★ 这不是文案小事：设置里那一行原先写的是「收起 / 收进任务栏」，
///    两颗按钮并排。在 macOS 上如果还这么写、而「收进任务栏」按下等于「收起」，
///    那就是**两颗按钮干同一件事** —— 正是本项目判据里明令禁止的
///    （见根 `MEMORY`：「同一件事只准一支按钮」）。
///    所以入口只留一颗，名字由平台决定，前端照着渲染就行。
pub const fn collapse_entry_label() -> &'static str {
    match action() {
        TaskbarAction::MinimizeWithButton => "收起",
        TaskbarAction::CollapseInstead => "收起窗口",
    }
}

/// 「收起」这颗按钮下面那行说明。
///
/// 同样按平台分岔：Windows 上要顺带解释另一颗按钮是干什么的，
/// macOS 上不提「任务栏」—— 用户系统里没有这个词，提了只会让人去找一个不存在的东西。
pub const fn collapse_entry_hint() -> &'static str {
    match action() {
        TaskbarAction::MinimizeWithButton => "收起＝留一条细栏；收进任务栏＝缩下去、任务栏留按钮",
        TaskbarAction::CollapseInstead => "收起＝只留顶栏一条，正文让开；点「展开」原样回来",
    }
}

/// 平台标识，直接给前端（它决定渲染哪一套界面用语）。
pub const fn platform_key() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_keeps_the_minimize_path() {
        // Windows 上的行为一个字都不能变：那边任务栏按钮是真实存在的，
        // 「缩下去、点任务栏回来」也是用户最熟的肌肉记忆。
        if cfg!(target_os = "windows") {
            assert_eq!(action(), TaskbarAction::MinimizeWithButton);
            assert_eq!(collapse_entry_label(), "收起");
        }
    }

    #[test]
    fn non_windows_degrades_to_collapse() {
        // ★ 这条钉住的是"绝不静默消失"：在不支持任务栏按钮的平台上，
        //   必须退化成**一定会回来**的收起，而不是调一个缩下去之后回不来的 API。
        if !cfg!(target_os = "windows") {
            assert_eq!(action(), TaskbarAction::CollapseInstead);
        }
    }

    #[test]
    fn platform_key_matches_the_stealth_layer() {
        // 前端只有一份平台用语表，键名必须与 `stealth::platform_name()` 同源，
        // 否则"隐身那张表"和"收起这张表"会分裂成两套判断。
        assert_eq!(platform_key(), crate::stealth::platform_name());
    }

    #[test]
    fn hint_never_mentions_taskbar_where_there_is_none() {
        // ★ 判据级的一条：文案里出现的名词，必须是用户系统里**真实存在**的东西。
        //   Windows 上必须提"任务栏"（那是按钮的落点）；
        //   其余平台绝不能提 —— macOS 用户会去找一个不存在的任务栏按钮。
        match action() {
            TaskbarAction::MinimizeWithButton => {
                assert!(collapse_entry_hint().contains("任务栏"));
            }
            TaskbarAction::CollapseInstead => {
                assert!(
                    !collapse_entry_hint().contains("任务栏"),
                    "没有任务栏按钮的系统上，文案里不能出现「任务栏」"
                );
            }
        }
    }
}
