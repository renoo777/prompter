//! 窗口层：尺寸/位置记忆、收起态、鼠标穿透。
//!
//! 目录结构按「纯逻辑」与「碰 Tauri」分开：
//!
//! * [`geometry`] —— 尺寸净化、可见区夹取、在屏判据，纯函数，可单测
//! * [`click_through`] —— 穿透判定状态机，纯函数，可单测
//! * [`taskbar`] —— 「收进任务栏」的两步握手状态机，纯函数，可单测
//! * [`platform`] —— 「收进任务栏」在各平台**落点不同**的判定，纯函数，可单测
//! * 本文件 —— 把上面四者接到真实窗口上：一个轮询线程 + 一个落盘线程
//!
//! 这样做的原因和 `stealth` / `script` / `align` 一样：
//! **能单测的部分绝不留在这层**。穿透与多屏坐标是最容易写错、
//! 又最不容易在真机上反复复现的地方。
//!
//! ★ `platform` 是 2026-09-27（macOS 移植轮）加进来的，它守的是另一类错：
//! Windows 上「收进任务栏」有任务栏按钮可点，macOS 上**没有这个东西**。
//! 照搬过去就是"窗口缩下去、没有任何入口回来"。判定放在纯函数里，
//! 才能在没有 Mac 的机器上被钉住。

pub mod click_through;
pub mod effects;
pub mod geometry;
pub mod platform;
pub mod taskbar;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use geometry::Rect;

// ---------------------------------------------------------------------------
// 共享状态
// ---------------------------------------------------------------------------

/// 窗口层的共享状态。
///
/// 三处会同时碰它，所以全部字段都是原子的或加了锁：
/// * 前端命令（上报可交互区、开关穿透、收起/展开）
/// * 窗口事件回调（位置/尺寸变了、DPI 变了）
/// * 后台线程（轮询光标、落盘几何）
pub struct WindowHub {
    /// 收起态。窗口只剩一条控制栏。
    pub collapsed: AtomicBool,
    /// 内容区左上角，屏幕物理坐标。
    origin: Mutex<(f64, f64)>,
    /// 内容区尺寸，逻辑像素。
    size: Mutex<(u32, u32)>,
    /// 窗口 DPI 缩放系数。CSS 像素 × 它 = 物理像素。
    scale: Mutex<f64>,
    /// 前端上报的可交互区（CSS 像素，相对内容区左上角）。
    ///
    /// 由前端算而不是后端算，是因为控制区长什么样只有 DOM 知道 ——
    /// 设置抽屉打开时、历史浮层展开时、收起态下，可交互区的形状都不一样。
    ///
    /// ★★ 是**一组**矩形，不是一个。理由见 `click_through::next` 的文档：
    /// 控制栏有上下两条，合并成一个包围盒会得到整窗，穿透就再也不触发了。
    zones_css: Mutex<Option<Vec<Rect>>>,
    /// 是否已经收到过一次可交互区。只用来做「等太久就报一次」的日志去重。
    zones_seen: AtomicBool,
    /// 上报的可交互区**当前是否可用**（一组有效的矩形）。
    ///
    /// 与 `zones_seen` 不同：上报上来了、但被判为不可信（`sanitize_zones` 返回 `None`）时，
    /// `seen = true` 而 `ok = false`。界面上的「穿透中」就是按它判的 —— 见 `ct_working`。
    zones_ok: AtomicBool,
    /// 上一次**已经记进日志**的可交互区描述。
    ///
    /// 前端每 400ms 上报一次，但形状其实只在用户操作后才变。
    /// 直接每次上报都打日志会把日志冲成一片噪声，而穿透恰恰要靠日志分岔 ——
    /// 所以这里做去重：只在**形状变了**的时候才落一行。
    zones_logged: Mutex<String>,
    /// 用户是否开启了穿透。
    ct_enabled: AtomicBool,
    /// 窗口**当前**是否真的是穿透态。与 `ct_enabled` 分开：
    /// 开关是用户的意图，这个是实际生效的状态，两者在切换瞬间会不一致。
    ct_now: AtomicBool,
    /// 几何有变化、等待落盘。
    geometry_dirty: AtomicBool,
}

impl WindowHub {
    pub fn new() -> Self {
        Self {
            collapsed: AtomicBool::new(false),
            origin: Mutex::new((0.0, 0.0)),
            size: Mutex::new((geometry::DEFAULT_W, geometry::DEFAULT_H)),
            scale: Mutex::new(1.0),
            zones_css: Mutex::new(None),
            zones_seen: AtomicBool::new(false),
            zones_ok: AtomicBool::new(false),
            zones_logged: Mutex::new(String::new()),
            ct_enabled: AtomicBool::new(false),
            ct_now: AtomicBool::new(false),
            geometry_dirty: AtomicBool::new(false),
        }
    }

    pub fn set_origin(&self, x: f64, y: f64) {
        if let Ok(mut o) = self.origin.lock() {
            *o = (x, y);
        }
    }

    pub fn origin(&self) -> (f64, f64) {
        self.origin.lock().map(|o| *o).unwrap_or((0.0, 0.0))
    }

    pub fn set_size(&self, w: u32, h: u32) {
        if let Ok(mut s) = self.size.lock() {
            *s = (w, h);
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size.lock().map(|s| *s).unwrap_or((geometry::DEFAULT_W, geometry::DEFAULT_H))
    }

    pub fn set_scale(&self, s: f64) {
        if s > 0.0 {
            if let Ok(mut v) = self.scale.lock() {
                *v = s;
            }
        }
    }

    pub fn scale(&self) -> f64 {
        self.scale.lock().map(|s| *s).unwrap_or(1.0)
    }

    /// 前端上报可交互区。`None` 表示"整窗都算可交互"，
    /// 这时穿透会被立刻关掉 —— 这是**安全兜底**：
    /// 前端还没来得及测量时，宁可让窗口可点，也不能让它变成一个点不动的幽灵。
    pub fn set_zones_css(&self, z: Option<Vec<Rect>>) {
        self.zones_seen.store(true, Ordering::Relaxed);
        self.zones_ok.store(z.is_some(), Ordering::Relaxed);
        if let Ok(mut v) = self.zones_css.lock() {
            *v = z;
        }
    }

    /// 是否已经收到过前端上报（不管是有效的还是 `None`）。
    pub fn zones_seen(&self) -> bool {
        self.zones_seen.load(Ordering::Relaxed)
    }

    /// 上一次已经记进日志的可交互区描述。用于「只在形状变化时落一行」的去重。
    pub fn zones_logged(&self) -> String {
        self.zones_logged.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_zones_logged(&self, s: String) {
        if let Ok(mut v) = self.zones_logged.lock() {
            *v = s;
        }
    }

    /// 可交互区换算到屏幕物理坐标。前端没上报过时返回 `None`。
    pub fn zones_physical(&self) -> Option<Vec<Rect>> {
        let guard = self.zones_css.lock().ok()?;
        let z = guard.as_ref()?.clone();
        let (ox, oy) = self.origin();
        let s = self.scale();
        Some(
            z.into_iter()
                .map(|r| Rect::new(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s))
                .collect(),
        )
    }

    /// 自检用：把当前的可交互区描述成一行字。
    ///
    /// 存在的理由：穿透失效时，"有没有收到可交互区"是第一现场证据，
    /// 而它在界面上完全看不出来。用户报"穿透没反应"时，
    /// 这一行能立刻把"前端没上报"和"后端没切"分开。
    pub fn zones_summary(&self) -> String {
        let Ok(z) = self.zones_css.lock() else {
            return "锁异常".into();
        };
        match &*z {
            None => "未收到（按整窗可交互处理，窗口保持可点）".into(),
            Some(v) => {
                let s = self.scale();
                let (ox, oy) = self.origin();
                let parts: Vec<String> = v
                    .iter()
                    .map(|r| {
                        format!(
                            "{:.0},{:.0} {:.0}×{:.0}",
                            ox + r.x * s,
                            oy + r.y * s,
                            r.w * s,
                            r.h * s
                        )
                    })
                    .collect();
                format!("{} 块 @ {}", parts.len(), parts.join(" | "))
            }
        }
    }

    pub fn ct_enabled(&self) -> bool {
        self.ct_enabled.load(Ordering::Relaxed)
    }

    pub fn ct_now(&self) -> bool {
        self.ct_now.load(Ordering::Relaxed)
    }

    /// 穿透是否**已经在工作**：开关开着，而且拿到了可信的可交互区。
    ///
    /// ★★ 界面上的「穿透中／开启中」按这个判，**不要用 `ct_now`**：
    ///   `ct_now` 表达的是"此刻光标是不是落在稿件区里"，鼠标一动就翻 ——
    ///   拿它驱动状态灯会闪成一片，而且分不清"功能没生效"和"光标恰好在栏上"。
    ///   "开关已开 + 有可交互区"才是"这功能真的跑起来了"。
    pub fn ct_working(&self) -> bool {
        self.ct_enabled() && self.zones_ok.load(Ordering::Relaxed)
    }

    pub fn is_collapsed(&self) -> bool {
        self.collapsed.load(Ordering::Relaxed)
    }

    pub fn set_collapsed(&self, v: bool) {
        self.collapsed.store(v, Ordering::Relaxed);
    }

    pub fn mark_geometry_dirty(&self) {
        self.geometry_dirty.store(true, Ordering::Relaxed);
    }

    fn take_dirty(&self) -> bool {
        self.geometry_dirty.swap(false, Ordering::Relaxed)
    }
}

impl Default for WindowHub {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 穿透开关
// ---------------------------------------------------------------------------

/// 开关鼠标穿透。
///
/// 关闭时**立刻**把窗口恢复成可点，不等轮询线程的下一拍 ——
/// 用户按下"关闭穿透"却还要等 40ms 才点得动，是很糟糕的手感；
/// 而且如果轮询线程因为任何原因卡住，这里就是唯一的兜底。
pub fn set_click_through(
    window: &WebviewWindow,
    hub: &WindowHub,
    enabled: bool,
) -> Result<(), String> {
    hub.ct_enabled.store(enabled, Ordering::Relaxed);
    if !enabled {
        window
            .set_ignore_cursor_events(false)
            .map_err(|e| format!("恢复窗口可点失败：{e}"))?;
        hub.ct_now.store(false, Ordering::Relaxed);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 轮询线程
// ---------------------------------------------------------------------------

/// 启动光标轮询线程。
///
/// 只在用户开启穿透时做事；没开启时每 120ms 空转一次（代价可忽略），
/// 这样开关穿透不需要动态起停线程。
pub fn spawn_click_through(app: AppHandle, hub: Arc<WindowHub>) {
    std::thread::spawn(move || {
        // ★ 路标日志。穿透是"点了没反应"最难自查的功能：开关、线程、
        //   可交互区三者任缺一个，界面上看到的现象完全一样。
        //   每个里程碑各留一行，用户报"穿透没反应"时看一眼日志就知道断在哪。
        log::info!(
            "[click-through] 轮询线程已启动（周期 {}ms）",
            click_through::TICK_MS
        );

        let mut state = click_through::ZoneState::default();
        let mut tick: u64 = 0;
        // 「还没收到可交互区」这个理由只报一次，免得每 120ms 刷一行日志。
        let mut warned_no_zone = false;
        // 「开关打开后第一次开始判定」也只报一次（关掉再开可再报一次）。
        let mut armed_logged = false;
        // 上一次已经上报给界面的「穿透在工作吗」。**只在变化时发。**
        // ★ 绝不能改成上报 `ct_now`：那是随光标每 40ms 翻的值，
        //   会变成一场事件风暴，界面上的状态灯也会跟着闪。
        let mut reported_working: Option<bool> = None;

        loop {
            std::thread::sleep(Duration::from_millis(click_through::TICK_MS));
            tick = tick.wrapping_add(1);

            let Some(window) = app.get_webview_window("main") else {
                continue;
            };

            // 把「穿透是否已经在工作」同步给界面（**变化时才发**）。
            //
            // ★ 这是「开启中 → 穿透中」那个状态灯唯一的数据来源：前端点一下只能
            //   知道"命令调成功了"，不知道"轮询线程真的拿到可交互区了"。
            //   而后者才是用户能感知的"生效"。
            //   放在这里而不是 `set_click_through` 里，是因为生效时机不受那次调用
            //   控制 —— 可交互区可能几十毫秒后才到，甚至要等到用户挪一下鼠标。
            let working = hub.ct_working();
            if reported_working != Some(working) {
                reported_working = Some(working);
                let _ = app.emit("prompter://click-through", working);
            }

            if !hub.ct_enabled() {
                // 停用时保证窗口最终回到可点态（正常路径上 set_click_through 已经做过了，
                // 这里是防止"直接改标志位"之类绕过路径留下一扇点不动的窗）
                if hub.ct_now.swap(false, Ordering::Relaxed) {
                    let _ = window.set_ignore_cursor_events(false);
                }
                state = click_through::ZoneState::default();
                warned_no_zone = false;
                armed_logged = false;
                std::thread::sleep(Duration::from_millis(80));
                continue;
            }

            if !armed_logged {
                armed_logged = true;
                log::info!(
                    "[click-through] 开关已打开，开始判定（可交互区：{}）",
                    hub.zones_summary()
                );
            }

            // 位置与缩放每 10 拍（约 400ms）刷新一次。
            // 拖动窗口时会连续收到 Moved 事件，由那里即时更新，所以不会滞后。
            if tick % 10 == 0 {
                if let Ok(p) = window.inner_position() {
                    hub.set_origin(p.x as f64, p.y as f64);
                }
                if let Ok(s) = window.scale_factor() {
                    hub.set_scale(s);
                }
            }

            let Ok(cursor) = window.cursor_position() else {
                continue;
            };

            // ★★ 没收（或收到不可信的）可交互区 ⇒ 按「整窗可交互」处理，
            //    也就是**强制回到可点态**。
            //
            //    这里以前是 `continue`，即"维持现状"—— 但文档里写的是
            //    "此时穿透会被立刻关掉"。两者差别很大：维持现状意味着
            //    如果此刻恰好是穿透态，窗口会**永远**保持穿透，
            //    用户点不到任何东西，而这个状态在界面上没有任何提示。
            //    （真机症状：穿透开关打开后完全没有反应，因为上报链路
            //      一旦没走通，这一拍就永远被跳过。）
            let Some(zones) = hub.zones_physical() else {
                if hub.ct_now.swap(false, Ordering::Relaxed) {
                    let _ = window.set_ignore_cursor_events(false);
                }
                state = click_through::ZoneState::default();
                if !warned_no_zone {
                    warned_no_zone = true;
                    log::info!(
                        "[click-through] 尚未收到可交互区（已上报过 {}）⇒ 窗口保持可点、暂不切换",
                        hub.zones_seen()
                    );
                }
                continue;
            };
            warned_no_zone = false;

            let (next_state, change) = click_through::next(&zones, (cursor.x, cursor.y), state);
            state = next_state;

            if let Some(through) = change {
                match window.set_ignore_cursor_events(through) {
                    Ok(()) => {
                        hub.ct_now.store(through, Ordering::Relaxed);
                    }
                    Err(e) => {
                        // 切换失败必须显式退回可点，绝不能留下一个点不动的窗口
                        log::warn!("[click-through] 切换失败，已回退：{e}");
                        let _ = window.set_ignore_cursor_events(false);
                        hub.ct_now.store(false, Ordering::Relaxed);
                        hub.ct_enabled.store(false, Ordering::Relaxed);
                        let _ = app.emit_to(
                            "main",
                            "prompter://warning",
                            "鼠标穿透切换失败，已自动关闭。窗口仍可正常操作。",
                        );
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 几何落盘线程
// ---------------------------------------------------------------------------

/// 几何变化的落盘线程。
///
/// 拖动窗口时 `Resized` / `Moved` 会每秒触发几十次，每次都写配置文件
/// 既浪费磁盘又会让配置写入互相竞争。这里统一攒起来，
/// 变化停止 700ms 后才真正落盘。
///
/// 用线程而不是定时器：Tauri 侧没有跨平台的"可取消定时器"，
/// 而且这个循环足够简单，不值得引入一个调度框架。
pub fn spawn_geometry_saver<F>(hub: Arc<WindowHub>, mut persist: F)
where
    F: FnMut(u32, u32, i32, i32) + Send + 'static,
{
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(700));
        if !hub.take_dirty() {
            continue;
        }
        let (w, h) = hub.size();
        let (x, y) = hub.origin();
        persist(w, h, x.round() as i32, y.round() as i32);
    });
}
