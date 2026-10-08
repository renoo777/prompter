//! 兜底后端：当前平台没有可用方案时使用。
//!
//! 覆盖 Linux（Wayland 无通用方案，X11 亦无可靠公开 API）以及未来任何
//! 未实现后端的平台。**不返回错误而是返回「不可用」的能力声明** ——
//! 前端的隐身开关会变成禁用态并显示替代方案，而不是让用户点了没反应。

// 在 Windows/macOS 上这个模块会被编译进来但永远不会被构造
// （`stealth::backend()` 走 cfg 分发）。它是有意保留的跨平台实现，不是遗留代码，
// 所以显式关掉 dead_code 检查。
#![allow(dead_code)]

use super::{CaptureGuard, NativeHandle, StealthError, StealthMode, StealthReport, SupportLevel};

pub struct UnsupportedGuard;

impl UnsupportedGuard {
    pub fn new() -> Self {
        Self
    }
}

impl CaptureGuard for UnsupportedGuard {
    fn support(&self) -> SupportLevel {
        SupportLevel::Unavailable(format!(
            "{} 暂无窗口级捕获保护方案：Wayland 不允许应用排除自身，X11 也无可靠的公开 API。",
            std::env::consts::OS
        ))
    }

    fn fallback_hint(&self) -> Option<String> {
        Some(
            "建议使用「第二设备」模式：稿件跑在手机/iPad 上，通过局域网与电脑同步，\
             屏幕上不存在提词器窗口，物理上就不会被捕获。"
                .to_string(),
        )
    }

    fn apply(
        &self,
        _handle: NativeHandle,
        _mode: StealthMode,
    ) -> Result<StealthReport, StealthError> {
        let support = self.support();
        let note = match &support {
            SupportLevel::Unavailable(n) => n.clone(),
            _ => String::new(),
        };
        Err(StealthError::Unsupported(note))
    }
}
