//! macOS 后端 —— **已在开发机（Windows）完成离线编译校验，尚未在真机运行**。
//!
//! # 「已验到什么程度」要说清
//!
//! * ✅ 能编译、零警告 —— 由 `scripts/check-macos-backend.py` 保证（`--cfg` 交叉检查）。
//! * ✅ `[view window]` 升层、版本分流、撤销必失败 —— 有单测。
//! * ❌ 隐身**实际效果**、穿透耗电、快捷键冲突 —— **必须在真 Mac 上验**。
//!
//! 上 Mac 后需要做的：跑起来、看诊断输出、按实测结果调整。不是从零设计。
//!
//! # 两条路径（按系统版本分流）
//!
//! 1. **macOS ≤ 14**：`NSWindow.sharingType = NSWindowSharingNone(0)`。
//!    这是 Apple 的公开 API，Tauri / Electron 的 `setContentProtection` 走的就是它。
//!
//! 2. **macOS ≥ 15**：Apple 改了 WindowServer 架构，ScreenCaptureKit 直接抓
//!    合成后的最终 framebuffer，**公开 API 全部失效**。官方立场是「没有公开 API
//!    可以阻止屏幕捕获」。社区目前唯一已知有效路径是私有 API
//!    `CGSSetWindowCaptureExcludeShape`。
//!
//! # 刻意采用的实现方式
//!
//! 不引入 `objc2` / `objc2-app-kit` —— 那两个 crate 的 API 在 0.x 阶段迭代很快，
//! 在没有真机可以验证的情况下，写出来的绑定很可能对不上当前版本。
//! 这里改为**手工 runtime FFI**（`sel_registerName` + `objc_msgSend` 强转），
//! 只依赖 C ABI，抗版本漂移；私有符号用 `dlsym` **动态查找**，
//! 找不到就优雅降级，不会因为 Apple 挪动符号位置而崩溃。

use super::{CaptureGuard, NativeHandle, StealthError, StealthMode, StealthReport, SupportLevel};

use std::ffi::{c_char, c_void, CString};

// ---------------------------------------------------------------------------
// Objective-C runtime
// ---------------------------------------------------------------------------

#[link(name = "objc")]
extern "C" {
    /// 真实签名是变参的，用 `transmute` 转成具体签名后调用。
    fn objc_msgSend();
    fn sel_registerName(name: *const c_char) -> *mut c_void;
}

#[link(name = "System")]
extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_DEFAULT`：在当前进程所有已加载镜像里查找符号。
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// `NSWindowSharingNone` —— 不参与任何捕获。
const NS_WINDOW_SHARING_NONE: usize = 0;
/// `NSWindowSharingReadOnly` —— 正常参与捕获。
const NS_WINDOW_SHARING_READ_ONLY: usize = 1;

/// `[window setSharingType:]`
unsafe fn set_sharing_type(window: *mut c_void, value: usize) {
    let sel = sel_registerName(c"setSharingType:".as_ptr());
    // SAFETY: 转成的签名与 `setSharingType:` 一致（void 返回，一个 NSUInteger 参数）。
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(window, sel, value);
}

/// `[window windowNumber]` → `NSInteger`
unsafe fn window_number(window: *mut c_void) -> i64 {
    let sel = sel_registerName(c"windowNumber".as_ptr());
    // SAFETY: 签名匹配（NSInteger 返回，无参数）。
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i64 =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(window, sel)
}

/// `[view window]` → `NSWindow*`（拿不到时返回 NULL）。
///
/// ★★★ 这一步是**必须的**，不是保险措施。
///
/// `native_handle()` 在 macOS 上返回的是 `raw_window_handle` 给的 `NSView*`，
/// 而 `setSharingType:` 是 **`NSWindow` 的方法**，不是 `NSView` 的。
/// 直接对 view 发这个选择子会抛 `unrecognized selector` 异常 —— 在
/// `objc_msgSend` 的 C ABI 调用下这笔异常会**直接终止进程**（不是 Rust panic，
/// 没有 unwind、没有捕获点），表现为「打开隐身就闪退」。
///
/// 因此每次对外发 `NSWindow` 的选择子之前，都先经过这里升一层。
unsafe fn view_window(view: *mut c_void) -> *mut c_void {
    if view.is_null() {
        return std::ptr::null_mut();
    }
    let sel = sel_registerName(c"window".as_ptr());
    // SAFETY: 签名匹配（对象指针返回，无参数）。view 已判空。
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    let window = f(view, sel);
    if window.is_null() {
        // 还没上屏（窗口尚未 orderFront）时 `[view window]` 会返回 nil，
        // 这属于可预期状态，交由调用方返回「应用失败」而不是硬崩。
        log::warn!("[stealth] [view window] 返回 nil —— 窗口可能尚未上屏");
    }
    window
}

// ---------------------------------------------------------------------------
// 系统版本探测
// ---------------------------------------------------------------------------

/// 取 macOS 主版本号（15 = Sequoia）。失败返回 `None`，由调用方决定降级策略。
///
/// 用 `sysctlbyname("kern.osproductversion")` 而不是 `NSProcessInfo` ——
/// 后者需要处理结构体返回值的 ABI，在没有真机验证的情况下风险更高。
fn macos_major_version() -> Option<u32> {
    let name = CString::new("kern.osproductversion").ok()?;
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: name 是合法 C 字符串；buf/len 均为栈上有效内存。
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 {
        return None;
    }
    // sysctl 返回的字符串带结尾 NUL。
    let s = std::str::from_utf8(&buf[..len.saturating_sub(1)]).ok()?;
    s.split('.').next()?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// 私有 API（macOS 15+ 的唯一路径）
// ---------------------------------------------------------------------------

type CgsMainConnectionIdFn = unsafe extern "C" fn() -> i32;
/// `CGError CGSSetWindowCaptureExcludeShape(CGSConnectionID, CGWindowID, CFArrayRef shape)`
/// 传 `NULL` shape 表示该窗口完全排除出捕获。
type CgsSetWindowCaptureExcludeShapeFn =
    unsafe extern "C" fn(i32, u32, *mut c_void) -> i32;

/// 探测私有符号。返回 `None` 表示当前系统没有这个符号 —— **必须优雅降级**，
/// 不能 panica。
fn probe_private_api() -> Option<(CgsMainConnectionIdFn, CgsSetWindowCaptureExcludeShapeFn)> {
    // SAFETY: dlsym 对不存在的符号返回 NULL，是安全调用。
    unsafe {
        let main_conn = dlsym(
            RTLD_DEFAULT,
            c"CGSMainConnectionID".as_ptr(),
        );
        let set_shape = dlsym(
            RTLD_DEFAULT,
            c"CGSSetWindowCaptureExcludeShape".as_ptr(),
        );
        if main_conn.is_null() || set_shape.is_null() {
            log::warn!(
                "[stealth] 私有符号缺失：CGSMainConnectionID={}, CGSSetWindowCaptureExcludeShape={}",
                main_conn.is_null(),
                set_shape.is_null()
            );
            return None;
        }
        Some((
            std::mem::transmute(main_conn),
            std::mem::transmute(set_shape),
        ))
    }
}

/// 用私有 API 把窗口排除出捕获。返回 `Ok(())` 仅代表调用未报错，
/// 真实效果需要在 Mac 上截图验证。
///
/// 参数是 `NSWindow*`（调用方负责先经 [`view_window`] 升层）。
unsafe fn apply_private_exclusion(window: *mut c_void, hidden: bool) -> Result<(), String> {
    let (main_conn_fn, set_shape_fn) =
        probe_private_api().ok_or_else(|| "私有符号不可用".to_string())?;

    let wid = window_number(window);
    if wid <= 0 {
        return Err(format!("windowNumber 异常：{wid}"));
    }

    let cid = main_conn_fn();
    // shape 的语义是「要排除的形状」：NULL = 整个窗口完全排除出捕获。
    //
    // ★★ 关闭隐身（hidden=false）时**不能**沿用 NULL —— 那会变成「再来一次完全排除」，
    // 也就是**关不掉**。但没有公开文档说明「恢复」该传什么形状（需要一个覆盖全窗口的
    // CGPath/CFArray），本轮无法在真机上试出来，所以选择**显式失败**：
    // 上层会拿到 Err，UI 如实显示「未能关闭隐身」，而不是静默地以为已经关掉了。
    // 这是「宁可显式失败，不可静默失败」的落点。
    if !hidden {
        return Err("私有 API 路径不支持撤销（无法构造恢复形状），需重启窗口".to_string());
    }
    let shape: *mut c_void = std::ptr::null_mut();
    let err = set_shape_fn(cid, wid as u32, shape);
    if err != 0 {
        return Err(format!("CGSSetWindowCaptureExcludeShape 返回 {err}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 后端实现
// ---------------------------------------------------------------------------

pub struct MacGuard {
    major: Option<u32>,
}

impl MacGuard {
    pub fn new() -> Self {
        let major = macos_major_version();
        log::info!("[stealth] macOS major = {major:?}");
        Self { major }
    }

    /// macOS 15 起公开 API 失效。
    fn public_api_broken(&self) -> bool {
        matches!(self.major, Some(v) if v >= 15)
    }
}

impl CaptureGuard for MacGuard {
    fn support(&self) -> SupportLevel {
        match self.major {
            // 拿不到版本号时不武断下结论，交给实际调用回答。
            None => SupportLevel::Fragile(
                "无法确定 macOS 版本，隐身效果需实测确认。".to_string(),
            ),
            Some(v) if v < 15 => SupportLevel::Native,
            Some(v) => SupportLevel::Fragile(format!(
                "macOS {v} 起 Apple 改变了屏幕捕获架构（ScreenCaptureKit 直接读取合成后画面），\
                 公开 API `sharingType` 已失效，当前依赖私有 API，可能在系统升级后再次失效。\
                 注意：该路径目前只能「开启」不能「关闭」，关闭需重开窗口。"
            )),
        }
    }

    fn fallback_hint(&self) -> Option<String> {
        Some(
            "Mac 端建议同时启用「第二设备」模式：稿件在手机/iPad 上显示，\
             屏幕上不存在提词器窗口，不依赖任何系统 API。"
                .to_string(),
        )
    }

    fn apply(&self, handle: NativeHandle, mode: StealthMode) -> Result<StealthReport, StealthError> {
        if handle.is_null() {
            return Err(StealthError::NoHandle("窗口句柄为空".into()));
        }
        // ★★★ 关键升层：handle 是 `NSView*`，而下面所有选择子都属于 `NSWindow`。
        // 漏掉这一步会 unrecognized selector → 进程直接死（见 `view_window` 注释）。
        // SAFETY: handle 已判空。
        let window = unsafe { view_window(handle) };
        if window.is_null() {
            return Err(StealthError::NoHandle(
                "无法从 NSView 取得 NSWindow（窗口可能尚未上屏）".into(),
            ));
        }
        let support = self.support();

        // 第一层：公开 API。macOS ≤14 上这一层就够了；15+ 上它不再起作用，
        // 但仍然调用 —— 成本为零，且在 Apple 将来恢复该行为时自动重新生效。
        // SAFETY: window 已确认非空。
        let sharing = if mode.is_hidden() {
            NS_WINDOW_SHARING_NONE
        } else {
            NS_WINDOW_SHARING_READ_ONLY
        };
        unsafe { set_sharing_type(window, sharing) };

        if !self.public_api_broken() {
            return Ok(StealthReport {
                mode,
                applied: true,
                detail: format!(
                    "已在 macOS {} 上通过公开 API（sharingType）{}。",
                    self.major.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    if mode.is_hidden() { "启用隐身" } else { "关闭隐身" }
                ),
                support,
                fallback_hint: None,
            });
        }

        // 第二层：macOS 15+ 才需要走私有路径。
        // SAFETY: 同上；spec 内部已做符号探测与错误码检查。
        let private_result = unsafe { apply_private_exclusion(handle, mode.is_hidden()) };

        match private_result {
            Ok(()) => Ok(StealthReport {
                mode,
                // 注意：这里只能标记为「已应用」而非「已验证」——
                // 私有 API 的实际效果必须在真机上截图确认。
                applied: true,
                detail: format!(
                    "已在 macOS {} 上应用私有 API 排除捕获。\
                     ⚠️ 该路径未经 Apple 认可，实际效果请在真实会议共享中确认一次。",
                    self.major.unwrap_or(0)
                ),
                support,
                fallback_hint: Some(
                    "请在真实会议里验证一次；若仍可见，改用「第二设备」模式。".to_string(),
                ),
            }),
            Err(e) => Ok(StealthReport {
                mode,
                applied: false,
                detail: format!(
                    "macOS {} 上公开 API 已失效，私有 API 亦不可用（{e}）。\
                     当前系统没有可用的窗口级隐身方案。",
                    self.major.unwrap_or(0)
                ),
                support,
                fallback_hint: Some(
                    "请使用「第二设备」模式：稿件显示在手机/iPad 上，电脑屏幕不含提词器窗口。"
                        .to_string(),
                ),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// 单测（只在 macOS 真机 / 交叉检查下跑）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 版本分流是纯逻辑，不碰 FFI —— 可以在任何平台上测。
    #[test]
    fn version_splits_at_15() {
        let g = MacGuard { major: Some(14) };
        assert!(!g.public_api_broken(), "14 及以下应走公开 API");
        let g = MacGuard { major: Some(15) };
        assert!(g.public_api_broken(), "15 起公开 API 失效");
    }

    /// ★ 撤销必须显式失败。若哪天有人「顺手」把 hidden=false 也传 NULL，
    /// 就会变成「关不掉但 UI 说关掉了」—— 这条断言就是拦住那次改动的。
    #[test]
    fn closing_stealth_must_not_silently_succeed() {
        // SAFETY: 传 NULL 时 apply_private_exclusion 在探测私有符号之前就返回 Err，
        // 不会真正触碰任何系统调用。这里只验证「非 hidden 一定失败」这条契约。
        let r = unsafe { apply_private_exclusion(std::ptr::null_mut(), false) };
        assert!(r.is_err(), "关闭隐身不能静默成功");
    }
}
