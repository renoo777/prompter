//! 「收进任务栏」的两步握手 —— 纯状态机，可单测。
//!
//! # 它解决的问题
//!
//! 这个动作分两半，而且**两半发生在不同的地方**：
//!
//! * 前半在命令里（`commands::minimize_to_taskbar`）：
//!   `set_skip_taskbar(false)` + `minimize()`；
//! * 后半在窗口事件里：用户点任务栏按钮把窗口弄回屏幕上之后，
//!   要把任务栏按钮收回去 —— 平时这个窗口是**不占任务栏**的（设计如此）。
//!
//! 中间隔着一段"窗口正在最小化"的窗口期。这段时间里如果来了任何一条
//! 窗口事件（Resized / Moved / Focused），拿它当"用户已经还原了"处理，
//! 就会**在窗口真正最小化之前把按钮撤掉** —— 症状是
//! 「点了收进任务栏，窗口缩下去了、任务栏里却什么都没有」，
//! 跟我们本来要修的那个 bug 长得一模一样。
//!
//! 所以握手必须是显式三态，而不是一个 bool：`Armed` 这一段
//! **什么都不做**，就是在为这个窗口期留白。
//!
//! 本文件只有纯函数与枚举，不认识 Tauri —— 「能不能单测」是这里选实现的硬判据。

/// 握手走到哪一步了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarHold {
    /// 没持有：窗口按设计不占任务栏。
    None = 0,
    /// 任务栏按钮已经放出去了，但**还没观察到窗口真的最小化**。
    Armed = 1,
    /// 窗口已最小化、按钮在位 —— 等它回到屏幕上。
    Minimized = 2,
}

impl TaskbarHold {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => TaskbarHold::Armed,
            2 => TaskbarHold::Minimized,
            _ => TaskbarHold::None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// 有没有"欠着"一次回收。两个非 `None` 状态都算。
    pub fn is_held(self) -> bool {
        self != TaskbarHold::None
    }
}

/// 收到一次窗口事件后，该对窗口做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarStep {
    /// 什么都不做。
    Nothing,
    /// 观察到窗口确实最小化了 ⇒ 把状态推进到 `Minimized`。
    NoteMinimized,
    /// 窗口回到屏幕上了 ⇒ 收回任务栏按钮（`set_skip_taskbar(true)`）。
    Release,
}

/// 状态转移。`minimized` 是这一刻窗口**真的**最小化了吗。
///
/// ★ `Armed` 且 `!minimized` ⇒ `Nothing`：这一条就是本模块存在的理由，
///   别图省事把它并进 `Release`（详见文件头）。
pub fn taskbar_step(hold: TaskbarHold, minimized: bool) -> TaskbarStep {
    match hold {
        TaskbarHold::None => TaskbarStep::Nothing,
        TaskbarHold::Armed if minimized => TaskbarStep::NoteMinimized,
        TaskbarHold::Armed => TaskbarStep::Nothing,
        TaskbarHold::Minimized if !minimized => TaskbarStep::Release,
        TaskbarHold::Minimized => TaskbarStep::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_does_not_release_before_the_window_is_actually_minimized() {
        // ★ 这条钉住的是一个真会发生的竞态：命令里"放按钮"和"最小化"之间
        //   可能插进来一条 Resized（此刻 minimized 还是 false）。
        //   如果在这一步就收回按钮，窗口随后缩下去、任务栏却什么都没有 ——
        //   和我们要修的那个 bug 一模一样。
        assert_eq!(taskbar_step(TaskbarHold::Armed, false), TaskbarStep::Nothing);
        assert_eq!(taskbar_step(TaskbarHold::Armed, true), TaskbarStep::NoteMinimized);
    }

    #[test]
    fn restored_window_releases_exactly_once() {
        assert_eq!(taskbar_step(TaskbarHold::Minimized, true), TaskbarStep::Nothing);
        assert_eq!(taskbar_step(TaskbarHold::Minimized, false), TaskbarStep::Release);
        // 回收之后状态回到 None，再来多少事件都不该重复动作
        assert_eq!(taskbar_step(TaskbarHold::None, false), TaskbarStep::Nothing);
        assert_eq!(taskbar_step(TaskbarHold::None, true), TaskbarStep::Nothing);
    }

    #[test]
    fn hold_round_trips_through_u8() {
        for h in [TaskbarHold::None, TaskbarHold::Armed, TaskbarHold::Minimized] {
            assert_eq!(TaskbarHold::from_u8(h.as_u8()), h);
        }
        // 越界值一律当"没持有"——绝不能因为一个坏字节就以为欠着一次回收
        assert_eq!(TaskbarHold::from_u8(200), TaskbarHold::None);
        assert!(!TaskbarHold::None.is_held());
        assert!(TaskbarHold::Armed.is_held());
        assert!(TaskbarHold::Minimized.is_held());
    }
}
