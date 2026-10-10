//! Prompter —— 投屏隐身提词器。
//!
//! # 分层
//!
//! ```text
//!   commands / tray / hotkey   ← 前端、托盘、全局快捷键的入口
//!         │
//!      AppState         ← 稿件 + 对位器 + 识别引擎 + 隐身后端 + 稿件库 + 窗口状态
//!         │
//!   ┌─────┼─────┬──────────┬──────────┬───────────┬────────┐
//! stealth  script  align   asr      library      win      doc
//! （隐身）（稿件）（对位）（语音）（稿件库/纯）（几何/穿透，纯逻辑部分）（Office 解包/纯扫描）
//! ```
//!
//! 每一层都只依赖它下面的层，`stealth` / `script` / `align` / `library` / `doc`
//! 以及 `win` 里的 `geometry` / `click_through` **完全不认识 Tauri** ——
//! 它们只有纯数据和纯函数，可以单独拿出来跑单元测试（见各文件的 `tests`）。
//!
//! `doc` 里的 XML 扫描部分是**纯函数**（吃一段 XML 字符串、吐纯文本），
//! 只有解包那一步碰字节。所以"段落怎么分、实体怎么解"可以脱离真实的
//! Office 文件被完整验证 —— 而这两件事恰恰是最容易错的地方。
//!
//! 新模块必须守这条规矩。「穿透该不该切换」「窗口坐标该不该夹回主屏」
//! 这类判断正是最容易写错、又最不容易在真机上复现的地方，
//! 把判定留在纯函数里、把 API 调用留在这一层，是它们能被验证的唯一办法。

mod align;
mod asr;
mod commands;
mod doc;
mod library;
mod script;
mod state;
mod stealth;
mod tray;
mod win;

use crate::state::AppState;
use crate::win::geometry::Rect;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 全局快捷键能触发的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyAction {
    NextSentence,
    PrevSentence,
    ToggleStealth,
    ToggleVisibility,
    ToggleRecognition,
    ToggleCollapse,
    ToggleClickThrough,
}

/// 快捷键 → 动作的映射表。
///
/// 需要共享成 `Arc<Mutex<..>>` 而不是直接捕获进回调，是因为**回调必须在插件
/// 构造时定下来，而配置要在 `setup` 里才读得到** —— 两者的时机对不上。
/// 用一个先空后填的表把时序解开。
type HotkeyMap = Arc<Mutex<Vec<(Shortcut, HotkeyAction)>>>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkeys: HotkeyMap = Arc::new(Mutex::new(Vec::new()));
    let hotkeys_for_handler = hotkeys.clone();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        /*
         * 应用内更新。装完之后同事不必再去 GitHub 手动下载安装包。
         *
         * ★ 公钥与更新清单地址**只在 `tauri.conf.json` 的 `plugins.updater`
         *   里写一份**，这里刻意不用 `.pubkey(...)` 再写一遍 —— 两处各写一份
         *   就会漂成"配置换了、代码里还是旧公钥"，而那种错**只在真机上、
         *   只在真有新版本时才暴露**（验签失败），本地怎么跑都是好的。
         *
         * ★ 验签是强制的、关不掉的：公钥换错 ⇒ 更新装不上，
         *   而不是"装上了但没验" —— 这是 Tauri 刻意的设计，不是可以调的选项。
         *
         * ★ 与 `tauri-plugin-process` 是一对：Windows 上 NSIS 安装器自己带
         *   `/R` 重启（见 updater 的 `nsis_restart_after_install_args`），
         *   而 macOS 上装完必须由前端调 `relaunch()`，否则跑的还是旧进程。
         */
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    // 只响应按下，忽略抬起 —— 否则一次按键会触发两次动作
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let action = hotkeys_for_handler
                        .lock()
                        .ok()
                        .and_then(|m| m.iter().find(|(s, _)| s == shortcut).map(|(_, a)| *a));
                    if let Some(a) = action {
                        dispatch(app, a);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = AppState::new(handle.clone());
            app.manage(state);

            // 回到上次的窗口尺寸与位置。
            // 放在 manage 之后：它要用状态里的配置。
            apply_saved_geometry(app);

            // 窗口事件：拖入文件、尺寸/位置变化。
            if let Some(win) = app.get_webview_window("main") {
                attach_window_events(app.handle().clone(), &win);
            }

            // 按配置里的磨砂档位挂系统材质（Acrylic / hudWindow）。
            // ★ 必须走启动路径：档位是持久化配置，只在 save_config 里挂的话，
            //   重启之后到用户第一次改设置之前，窗口都是没有材质的 ——
            //   这正是 bootstrap 漏灌跟读参数的那类 bug（运行态与配置两张皮）。
            {
                let state = app.state::<AppState>();
                let mode = state
                    .config
                    .lock()
                    .map(|c| c.glass_mode)
                    .unwrap_or_default();
                commands::apply_glass_mode(app.handle(), mode);
            }

            // 托盘。**它是穿透功能的安全前提**，不是可选装饰 ——
            // 见 `tray.rs` 开头。建不起来不挡启动，但要留下日志与界面提示。
            {
                let state = app.state::<AppState>();
                match tray::build(app.handle(), &state) {
                    Some(t) => {
                        if let Ok(mut slot) = state.tray.lock() {
                            *slot = Some(t);
                        }
                    }
                    None => log::warn!("[tray] 托盘不可用：穿透时的找回窗口只剩全局热键一条路"),
                }
            }

            {
                let state = app.state::<AppState>();
                if let Err(e) = bind_hotkeys(app, &hotkeys, &state) {
                    // 热键失败不能挡住启动：没有热键产品仍然可用（只是要手动点），
                    // 没有窗口就什么都做不了。
                    log::warn!("[hotkey] 注册失败：{e}");
                }
            }

            // 穿透判定的轮询线程 + 几何落盘线程
            let hub = app.state::<AppState>().hub.clone();
            win::spawn_click_through(app.handle().clone(), hub.clone());

            let saver_hub = hub.clone();
            let saver_app = app.handle().clone();
            win::spawn_geometry_saver(saver_hub, move |w, h, x, y| {
                persist_geometry(&saver_app, w, h, x, y);
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::load_script_from_path,
            commands::set_script_text,
            commands::clear_script,
            commands::feed_hypothesis,
            commands::nudge,
            commands::goto,
            commands::step_sentence,
            commands::rewind,
            commands::set_stealth,
            commands::stealth_support,
            commands::start_asr,
            commands::stop_asr,
            commands::asr_status,
            commands::mic_check,
            commands::save_config,
            commands::get_config,
            commands::rescan_models,
            commands::models_dir,
            commands::open_path,
            commands::open_url,
            commands::diagnostics,
            // 稿件库
            commands::list_scripts,
            commands::activate_script,
            commands::forget_script,
            commands::clear_script_history,
            commands::script_availability,
            // 窗口层
            commands::set_click_through,
            commands::report_interactive_zone,
            commands::frontend_beat,
            commands::set_collapsed,
            commands::is_collapsed,
            commands::set_window_locked,
            commands::set_always_on_top,
            commands::minimize_to_taskbar,
            // 读稿手感
            commands::set_match_floor,
            commands::match_floor,
            commands::set_jump_range,
            commands::jump_range,
            // 匀速滚动（推进方式三选一里的第三种）
            commands::set_auto_wpm,
            // 省事类
            commands::preflight,
            commands::export_bundle,
            commands::import_bundle,
        ])
        .run(tauri::generate_context!())
        .expect("Prompter 启动失败");
}

// ---------------------------------------------------------------------------
// 窗口几何
// ---------------------------------------------------------------------------

/// 读回上次的尺寸与位置并应用。
fn apply_saved_geometry(app: &tauri::App) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let state = app.state::<AppState>();
    let cfg = match state.config.lock() {
        Ok(c) => c.clone(),
        Err(_) => return,
    };

    let (w, h) = win::geometry::sanitize_size(cfg.window.width, cfg.window.height);
    if let Err(e) = win.set_size(tauri::LogicalSize::new(w as f64, h as f64)) {
        log::warn!("[window] 恢复尺寸失败：{e}");
    }

    if let (Some(x), Some(y)) = (cfg.window.x, cfg.window.y) {
        let monitors = monitor_rects(&win);
        let primary = primary_rect(&win, &monitors);
        let (nx, ny) = win::geometry::clamp_to_monitors(x, y, w, h, &monitors, primary);
        if (nx, ny) != (x, y) {
            log::info!("[window] 上次的位置 ({x},{y}) 已不在任何屏幕内，移回 ({nx},{ny})");
        }
        if let Err(e) = win.set_position(tauri::PhysicalPosition::new(nx, ny)) {
            log::warn!("[window] 恢复位置失败：{e}");
        }
    }

    // hub 要立刻拿到真实值，否则第一次收起/展开会用到默认尺寸
    state.hub.set_size(w, h);
    state.hub.set_scale(win.scale_factor().unwrap_or(1.0));
    if let Ok(p) = win.inner_position() {
        state.hub.set_origin(p.x as f64, p.y as f64);
    }
}

fn monitor_rects(win: &tauri::WebviewWindow) -> Vec<Rect> {
    win.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            Rect::new(p.x as f64, p.y as f64, s.width as f64, s.height as f64)
        })
        .collect()
}

fn primary_rect(win: &tauri::WebviewWindow, fallback: &[Rect]) -> Rect {
    win.primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            Rect::new(p.x as f64, p.y as f64, s.width as f64, s.height as f64)
        })
        .or_else(|| fallback.first().copied())
        .unwrap_or_else(|| Rect::new(0.0, 0.0, 1920.0, 1080.0))
}

/// 把几何写回配置。由落盘线程在变化停止后调用。
fn persist_geometry(app: &AppHandle, w: u32, h: u32, x: i32, y: i32) {
    let state = app.state::<AppState>();
    let snapshot = {
        let Ok(mut cfg) = state.config.lock() else {
            return;
        };
        cfg.window.width = w;
        cfg.window.height = h;
        cfg.window.x = Some(x);
        cfg.window.y = Some(y);
        cfg.clone()
    };
    if let Err(e) = state::save_config(&state.paths.config_file, &snapshot) {
        log::warn!("[window] 几何保存失败：{e}");
    }
}

// ---------------------------------------------------------------------------
// 窗口事件
// ---------------------------------------------------------------------------

fn attach_window_events(app: AppHandle, win: &tauri::WebviewWindow) {
    let app_for_events = app.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::DragDrop(ev) => on_drag(&app_for_events, ev),

        WindowEvent::Resized(size) => {
            let state = app_for_events.state::<AppState>();
            let Some(w) = app_for_events.get_webview_window("main") else {
                return;
            };
            if let Ok(s) = w.scale_factor() {
                state.hub.set_scale(s);
            }
            settle_taskbar(&state, &w);

            // ★ 最小化 / 隐藏时**不能**记几何：最小化会送来一次 `WM_SIZE`，
            //   此时宽高报成 `0×0`，记下去会被 `sanitize_size` 判为越界、
            //   回落成 `560×360` 默认值 —— 用户"收起来一次"就把拖好的窗口
            //   大小弄丢了，而且全程没有任何提示。
            //   收起态的理由不同（收起高度是临时的，不能覆盖展开尺寸），
            //   两者收进同一个纯函数，让"该不该记"只剩一处判断。
            if win::geometry::should_capture_geometry(
                w.is_visible().unwrap_or(false),
                w.is_minimized().unwrap_or(false),
                state.hub.is_collapsed(),
            ) {
                let scale = state.hub.scale();
                state.hub.set_size(
                    (size.width as f64 / scale).round() as u32,
                    (size.height as f64 / scale).round() as u32,
                );
                state.hub.mark_geometry_dirty();
            }
        }

        WindowEvent::Moved(pos) => {
            let state = app_for_events.state::<AppState>();
            let Some(w) = app_for_events.get_webview_window("main") else {
                return;
            };
            settle_taskbar(&state, &w);

            // 最小化时位置报的是 `-32000,-32000`，记下去下次会开在屏幕外。
            if win::geometry::should_capture_geometry(
                w.is_visible().unwrap_or(false),
                w.is_minimized().unwrap_or(false),
                state.hub.is_collapsed(),
            ) {
                state.hub.set_origin(pos.x as f64, pos.y as f64);
                state.hub.mark_geometry_dirty();
            }
        }

        // 还原不一定送来 Resized —— 用户点任务栏按钮时系统可能只补一条焦点事件。
        // 多一个触发点没有副作用：状态机在 `Armed` 与 `None` 两步都是空的。
        WindowEvent::Focused(_) => {
            let state = app_for_events.state::<AppState>();
            if let Some(w) = app_for_events.get_webview_window("main") {
                settle_taskbar(&state, &w);
            }
        }

        _ => {}
    });
}

/// 窗口事件里顺手推进「收进任务栏」的握手（状态机见 [`win::taskbar`]）。
///
/// 为什么必须挂在这里：**还原的动作在用户手里**（点任务栏按钮），
/// 后端唯一能知道的途径就是"窗口不再最小化了"。没人通知我们就没人收回按钮，
/// 于是窗口回到屏幕上之后任务栏会一直挂着它 —— 与"平时不占任务栏"相反。
fn settle_taskbar(state: &AppState, win: &tauri::WebviewWindow) {
    if state.advance_taskbar(win.is_minimized().unwrap_or(false))
        == win::taskbar::TaskbarStep::Release
    {
        match win.set_skip_taskbar(true) {
            Ok(()) => log::info!("[taskbar] 窗口已回到屏幕上，任务栏按钮已收回（恢复「平时不占任务栏」）"),
            Err(e) => log::warn!("[taskbar] 收回任务栏按钮失败：{e}"),
        }
    }
}

/// 走「显式还原」这条路（托盘菜单 / 全局热键）时，把任务栏占用一并了结。
///
/// 与 [`settle_taskbar`] 的区别：那条路是靠"窗口不再最小化"推断出来的，
/// 这条是我们**自己**刚把窗口弄回屏幕上，所以直接了结，不用等事件。
pub(crate) fn finish_taskbar_hold(state: &AppState, win: &tauri::WebviewWindow) {
    if state.clear_taskbar_hold() {
        // 失败不致命：最坏情况是任务栏多留一个按钮，而窗口已经回来了。
        if let Err(e) = win.set_skip_taskbar(true) {
            log::warn!("[taskbar] 还原时收回任务栏按钮失败：{e}");
        }
    }
}

/// 拖放进入 / 离开 / 落下。
///
/// # 为什么在 Rust 侧接而不是前端接
///
/// `dragDropEnabled` 默认 `true` 时 Tauri 会**劫持**拖放（Windows 上是 OLE
/// drop target 独占），前端 HTML5 的 `drop` 事件永远不触发。要拿到**文件路径**
/// 就必须保持这个默认值、并在 Rust 侧处理事件 —— 关掉它只能拿到 `File` 对象，
/// 没有路径，而稿件历史是以路径为键的（见 `library.rs`）。
fn on_drag(app: &AppHandle, ev: &tauri::DragDropEvent) {
    match ev {
        tauri::DragDropEvent::Enter { paths, .. } => {
            let names: Vec<String> = paths
                .iter()
                .take(3)
                .map(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("文件")
                        .to_string()
                })
                .collect();
            let ok = paths.first().map(is_supported).unwrap_or(false);
            let _ = app.emit(
                "prompter://drag",
                DragHint {
                    kind: "enter".into(),
                    names,
                    ok,
                },
            );
        }
        tauri::DragDropEvent::Over { .. } => {}
        tauri::DragDropEvent::Drop { paths, .. } => {
            let _ = app.emit(
                "prompter://drag",
                DragHint {
                    kind: "leave".into(),
                    names: Vec::new(),
                    ok: false,
                },
            );
            handle_drop(app, paths);
        }
        tauri::DragDropEvent::Leave => {
            let _ = app.emit(
                "prompter://drag",
                DragHint {
                    kind: "leave".into(),
                    names: Vec::new(),
                    ok: false,
                },
            );
        }
        // `DragDropEvent` 是 #[non_exhaustive] 的：Tauri 将来可能加新变体。
        // 这里必须留通配分支而不是 `unreachable!()` —— 一个未知的拖放状态
        // 最坏的结果是覆盖层没更新，绝不该让整个应用崩掉。
        _ => {}
    }
}

#[derive(serde::Serialize, Clone)]
struct DragHint {
    /// `enter` | `leave`
    kind: String,
    names: Vec<String>,
    /// 拖入的东西**看起来**能不能载入。只用于决定覆盖层的颜色与文案，
    /// 真正能不能载入由 `handle_drop` 判定 —— 两处判断同一个函数，不会分叉。
    ok: bool,
}

/// 拖进来的东西能不能载入。
///
/// 判定唯一来源是 `doc::SCRIPT_EXTS` —— 前端文件对话框的 filter 也从同一份清单来，
/// 所以不会出现"对话框里选不到、拖进来却能载入"。
///
/// 只做扩展名判断，**不去嗅探内容**：嗅探需要先读文件，而拖入一个几百 MB 的
/// 视频时那一下读取是明显卡顿。真正的校验在 `load_into_engine`，失败会明确报错。
fn is_supported(p: &PathBuf) -> bool {
    crate::doc::is_supported_ext(&crate::doc::ext_of(p))
}

fn handle_drop(app: &AppHandle, paths: &[PathBuf]) {
    let state = app.state::<AppState>();
    let warn = |msg: String| {
        let _ = app.emit_to("main", "prompter://warning", msg);
    };

    let Some(first) = paths.first() else {
        return;
    };

    // 多文件：取第一个，但**明确说出来** —— 静默忽略是"承诺了做不到"之外的
    // 另一种糟糕失败：用户以为载入了，其实不是他想要的那份
    if paths.len() > 1 {
        warn(format!(
            "一次只能载入一份稿件，已载入第一份（共拖入 {} 个文件）。",
            paths.len()
        ));
    }

    if !is_supported(first) {
        let ext = first
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("（无扩展名）")
            .to_string();
        // 报错里必须给出**下一步能做什么**，而不只是"不支持"。
        // 补上 .doc/.ppt 的出路 —— 那两个是老二进制格式，不是 ZIP，
        // 解析它等于写一个 OLE 复合文档解析器，收益远不及"让用户另存一次"。
        warn(format!(
            "暂不支持 .{ext} 文件 —— 稿件目前支持 {}。\n\
             如果是 .doc / .ppt 老格式，请先在 Word / PowerPoint 里「另存为」.docx / .pptx。\n\
             （图片要能提词必须先做 OCR，那是另一项能力，本版本不做。当前稿件未改动。）",
            crate::doc::SCRIPT_EXTS_HINT
        ));
        return;
    }

    match commands::load_into_engine(&state, first) {
        Ok(payload) => {
            let path = first.to_string_lossy().to_string();
            if let Ok(mut cfg) = state.config.lock() {
                cfg.last_script = Some(path.clone());
                let snap = cfg.clone();
                drop(cfg);
                let _ = state::save_config(&state.paths.config_file, &snap);
            }
            commands::record_file_use(&state, &path, &payload.title, payload.total_chars);
            tray::refresh(&state);

            // 前端主动更新稿件（拖放不经过 invoke，没有返回值可用）
            let _ = app.emit_to("main", "prompter://script", payload);
        }
        Err(e) => {
            // 载入失败时当前稿件**一字未动**（`load_into_engine` 只在最后一步替换）
            let _ = app.emit_to("main", "prompter://error", format!("无法载入拖入的文件：{e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// 热键
// ---------------------------------------------------------------------------

fn bind_hotkeys(app: &tauri::App, map: &HotkeyMap, state: &AppState) -> Result<(), String> {
    let hk = state
        .config
        .lock()
        .map_err(|_| "配置锁异常".to_string())?
        .hotkeys
        .clone();

    let gs = app.global_shortcut();
    // 先全部注销，避免重复绑定（例如将来做"改完热键立即生效"时会再次走到这里）
    let _ = gs.unregister_all();

    let entries: [(&String, HotkeyAction); 7] = [
        (&hk.next_sentence, HotkeyAction::NextSentence),
        (&hk.prev_sentence, HotkeyAction::PrevSentence),
        (&hk.toggle_stealth, HotkeyAction::ToggleStealth),
        (&hk.toggle_visibility, HotkeyAction::ToggleVisibility),
        (&hk.toggle_recognition, HotkeyAction::ToggleRecognition),
        (&hk.toggle_collapse, HotkeyAction::ToggleCollapse),
        (&hk.toggle_click_through, HotkeyAction::ToggleClickThrough),
    ];

    let mut bound = map.lock().map_err(|_| "热键表锁异常".to_string())?;
    bound.clear();

    for (spec, action) in entries {
        match Shortcut::from_str(spec) {
            Ok(sc) => match gs.register(sc.clone()) {
                Ok(()) => bound.push((sc, action)),
                Err(e) => log::warn!("[hotkey] 注册 {spec} 失败：{e}"),
            },
            Err(e) => log::warn!("[hotkey] 解析 {spec} 失败：{e}"),
        }
    }
    log::info!("[hotkey] 已注册 {} 个快捷键", bound.len());
    Ok(())
}

/// 执行热键动作。
///
/// 这里刻意**不复用命令层的函数**，而是直接操作 state ——
/// 命令层的签名要求 `State<'_, T>` 和 `WebviewWindow` 这些由 Tauri 注入的参数，
/// 在快捷键回调里没有这些上下文，硬套只会写出更多胶水。
fn dispatch(app: &AppHandle, action: HotkeyAction) {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!("[hotkey] 找不到主窗口，动作被忽略");
        return;
    };
    let state = app.state::<AppState>();

    match action {
        HotkeyAction::NextSentence | HotkeyAction::PrevSentence => {
            let forward = action == HotkeyAction::NextSentence;
            let payload = {
                let Ok(mut e) = state.engine.lock() else { return };
                let cur = e.aligner.cursor();
                let target = if forward {
                    commands::next_sentence_start(&e.script, cur)
                } else {
                    commands::prev_sentence_start(&e.script, cur)
                };
                if let Some(t) = target {
                    e.aligner.set_cursor(t);
                }
                e.cursor_payload(0.0, target.is_some(), 0, false)
            };
            let _ = app.emit("prompter://cursor", payload);
        }

        HotkeyAction::ToggleStealth => {
            let next = state
                .stealth_mode
                .lock()
                .map(|m| m.toggled())
                .unwrap_or(crate::stealth::StealthMode::Hidden);

            match commands::native_handle(&window) {
                Ok(h) => match state.guard.apply(h, next) {
                    Ok(report) => {
                        if report.applied {
                            if let Ok(mut m) = state.stealth_mode.lock() {
                                *m = next;
                            }
                        }
                        let _ = app.emit("prompter://stealth", report);
                    }
                    Err(e) => {
                        let _ = app.emit("prompter://error", e.to_string());
                    }
                },
                Err(e) => {
                    let _ = app.emit("prompter://error", e);
                }
            }
        }

        HotkeyAction::ToggleVisibility => {
            // ★ 判据是「可见 **且** 没有最小化」，不能只看 `is_visible()`：
            //   最小化的窗口它照样返回 true。只看可见位的话，一个被"收进任务栏"
            //   的窗口会被当成还在屏幕上，于是去 hide() 一个已经最小化的窗口 ——
            //   用户按了快捷键，什么都没发生。
            let on_screen = win::geometry::on_screen(
                window.is_visible().unwrap_or(true),
                window.is_minimized().unwrap_or(false),
            );
            if on_screen {
                let _ = window.hide();
            } else {
                // 三步还原：unminimize → show → set_focus。少了 `unminimize`，
                // 最小化的窗口只会"被显示"成最小化状态，用户看到的仍是没反应。
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                // 从"不在屏幕上"回来 ⇒ 任务栏那枚按钮该收回去了。
                finish_taskbar_hold(&state, &window);
            }
            let _ = app.emit("prompter://visibility", !on_screen);
        }

        HotkeyAction::ToggleRecognition => {
            let running = state
                .asr
                .lock()
                .map(|s| s.as_ref().map(|e| e.is_running()).unwrap_or(false))
                .unwrap_or(false);

            let result = if running {
                commands::stop_asr_inner(&state)
            } else {
                commands::start_asr_inner(&state)
            };

            match result {
                Ok(status) => {
                    let _ = app.emit("prompter://asr-status", status);
                }
                Err(e) => {
                    let _ = app.emit("prompter://error", e);
                }
            }
        }

        HotkeyAction::ToggleCollapse => {
            let collapsed = !state.hub.is_collapsed();
            match commands::set_collapsed_inner(&state, &window, collapsed) {
                Ok(()) => {
                    let _ = app.emit("prompter://collapsed", collapsed);
                }
                Err(e) => {
                    let _ = app.emit("prompter://error", e);
                }
            }
        }

        HotkeyAction::ToggleClickThrough => {
            let next = !state.hub.ct_enabled();
            match commands::set_click_through_inner(&state, &window, next) {
                // 上报「穿透是否已经在工作」（开关开着 + 可交互区到位），
                // 与轮询线程共用同一个判据 —— 界面上的状态灯只认这一个语义。
                // ★ 不报 `ct_now`：它表达的是"此刻光标在不在稿件区"，随时会翻。
                Ok(_) => {
                    let _ = app.emit("prompter://click-through", state.hub.ct_working());
                }
                Err(e) => {
                    let _ = app.emit("prompter://error", e);
                }
            }
        }
    }
}
