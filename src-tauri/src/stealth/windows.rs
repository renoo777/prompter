//! Windows 后端：`SetWindowDisplayAffinity`。
//!
//! 手写 FFI 而非引入 `windows-sys` ：本层只需要 3 个函数（`SetWindowDisplayAffinity`
//! / `GetWindowDisplayAffinity` / `RtlGetVersion`），手写声明可以：
//!   1. 完全避开 `windows` crate 的 feature 组合与版本漂移；
//!   2. 让「无第三方依赖」这条约束在这一层继续成立，便于将来单独抽出复用。
//!
//! 前一轮的验证 Demo 已经用同样的声明在真实飞书会议里跑通，这里沿用。

use super::{CaptureGuard, NativeHandle, StealthError, StealthMode, StealthReport, SupportLevel};

use std::ffi::c_void;

type Hwnd = *mut c_void;
type Bool = i32;

/// 关闭保护（窗口正常参与捕获）。
const WDA_NONE: u32 = 0x0000_0000;
/// 窗口从捕获结果中排除。Windows 10 版本 2004（build 19041）起可用。
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
/// 旧常量：窗口渲染为黑块。**不用它** —— 投屏时观众会看到一个突兀的黑框。
#[allow(dead_code)]
const WDA_MONITOR: u32 = 0x0000_0001;

#[link(name = "user32")]
extern "system" {
    fn SetWindowDisplayAffinity(hwnd: Hwnd, affinity: u32) -> Bool;
    fn GetWindowDisplayAffinity(hwnd: Hwnd, affinity: *mut u32) -> Bool;
}

/// `RtlGetVersion` 的返回结构（只用到前几个字段）。
///
/// 不挂 `#[derive(Default)]` —— `[u16; 128]` 没实现 `Default`
/// （标准库只为长度 ≤ 32 的数组实现了它），手写一份反而更直白。
#[repr(C)]
struct RtlOsVersionInfoW {
    os_version_info_size: u32,
    major_version: u32,
    minor_version: u32,
    build_number: u32,
    platform_id: u32,
    csd_version: [u16; 128],
}

impl Default for RtlOsVersionInfoW {
    fn default() -> Self {
        Self {
            os_version_info_size: 0,
            major_version: 0,
            minor_version: 0,
            build_number: 0,
            platform_id: 0,
            csd_version: [0u16; 128],
        }
    }
}

// 不用 GetVersionExW —— 它在没有 manifest 声明时会谎报成 Windows 8。
// RtlGetVersion 是 ntdll 的底层实现，永远说真话。
#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut RtlOsVersionInfoW) -> i32;
}

/// 取当前系统 build 号，失败返回 0。
fn os_build_number() -> u32 {
    let mut info = RtlOsVersionInfoW::default();
    info.os_version_info_size = std::mem::size_of::<RtlOsVersionInfoW>() as u32;
    // SAFETY: 传入了正确大小、且完整初始化的结构体指针。
    let status = unsafe { RtlGetVersion(&mut info) };
    if status == 0 {
        info.build_number
    } else {
        0
    }
}

/// `WDA_EXCLUDEFROMCAPTURE` 要求的最低 build 号（Windows 10 2004 / 20H1）。
const MIN_BUILD_FOR_EXCLUDE: u32 = 19041;

pub struct WindowsGuard {
    build: u32,
}

impl WindowsGuard {
    pub fn new() -> Self {
        let build = os_build_number();
        log::info!("[stealth] Windows build = {build}");
        Self { build }
    }
}

impl CaptureGuard for WindowsGuard {
    fn support(&self) -> SupportLevel {
        // build 号取不到时（极罕见）不武断判死，让实际调用去回答。
        if self.build != 0 && self.build < MIN_BUILD_FOR_EXCLUDE {
            return SupportLevel::Unavailable(format!(
                "当前 Windows build {} 低于 {}，系统不支持 WDA_EXCLUDEFROMCAPTURE",
                self.build, MIN_BUILD_FOR_EXCLUDE
            ));
        }
        SupportLevel::Native
    }

    fn fallback_hint(&self) -> Option<String> {
        // 原生可用时也给出兜底说明 —— 它同时是「双保险」建议：
        // 万一某些会议客户端走的是非常规捕获通道，只共享指定窗口仍然兜得住。
        Some(
            "若个别会议客户端仍能看到稿件，请在共享时选择「仅共享选中窗口」而非整个屏幕。"
                .to_string(),
        )
    }

    fn apply(&self, handle: NativeHandle, mode: StealthMode) -> Result<StealthReport, StealthError> {
        if handle.is_null() {
            return Err(StealthError::NoHandle("HWND 为空".into()));
        }
        let hwnd = handle as Hwnd;

        let support = self.support();
        if let SupportLevel::Unavailable(ref note) = support {
            return Err(StealthError::Unsupported(note.clone()));
        }

        let target = match mode {
            StealthMode::Hidden => WDA_EXCLUDEFROMCAPTURE,
            StealthMode::Visible => WDA_NONE,
        };

        // SAFETY: hwnd 已判空；affinity 传的是本文件定义的合法常量。
        let ok = unsafe { SetWindowDisplayAffinity(hwnd, target) };
        if ok == 0 {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            return Ok(StealthReport {
                mode,
                applied: false,
                detail: format!(
                    "SetWindowDisplayAffinity 调用失败（错误码 {code}）。\
                     常见原因：窗口尚未创建完成、或系统版本低于 Windows 10 2004。"
                ),
                support,
                fallback_hint: Some(
                    "请改用「仅共享选中窗口」，或在稿件窗口上右键选择「不参与共享」。".to_string(),
                ),
            });
        }

        // 回读确认 —— 这是本层最关键的一步。
        // 该 API 在部分环境下「返回成功但实际未生效」，不回读等于把风险留给用户在会议上发现。
        let mut current: u32 = 0;
        // SAFETY: current 是栈上有效变量，指针有效。
        let read_ok = unsafe { GetWindowDisplayAffinity(hwnd, &mut current) };
        let confirmed = read_ok != 0 && current == target;

        if !confirmed {
            return Ok(StealthReport {
                mode,
                applied: false,
                detail: format!(
                    "系统接受了请求但回读确认失败（期望 {target:#06x}，实际 {current:#06x}）。\
                     该状态下稿件仍可能出现在共享画面里。"
                ),
                support,
                fallback_hint: Some(
                    "请在共享时选择「仅共享选中窗口」，并把稿件窗口保持在共享窗口之外。"
                        .to_string(),
                ),
            });
        }

        Ok(StealthReport {
            mode,
            applied: true,
            detail: if mode.is_hidden() {
                format!("已启用投屏隐身（WDA_EXCLUDEFROMCAPTURE，当前 {current:#06x}），截图/录屏/会议共享均捕获不到本窗口。")
            } else {
                "已关闭投屏隐身，本窗口现在会出现在共享画面中。".to_string()
            },
            support,
            fallback_hint: None,
        })
    }
}
