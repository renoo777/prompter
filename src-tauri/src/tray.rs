//! 系统托盘。
//!
//! # 为什么托盘是 P0 而不是"锦上添花"
//!
//! 三件事叠在一起，会让窗口**彻底找不回来**：
//!
//! 1. `skipTaskbar: true` —— 任务栏里没有它；
//! 2. 鼠标穿透（R4c）—— 点它就跟点空气一样，事件被穿到下层了；
//! 3. 透明度可以调到很低 —— 视觉上也找不到。
//!
//! 剩下的唯一退路是 `Ctrl+Alt+H` 这个热键。而热键可能注册失败
//! （被别的软件占用是很常见的）。所以托盘不是"多一个入口"，
//! 它是**穿透功能的安全前提** —— 没有它，穿透就是一个可能把用户
//! 关在门外的功能。
//!
//! # ★ 2026-09-27 修正第 1 条的口径（它以前是恒定前提，现在不是）
//!
//! 上面第 1 条写于"窗口永远不占任务栏"的年代。现在「收进任务栏」这条路
//! （见 `commands::minimize_to_taskbar` / `win::taskbar`）会在**收起的那段时间**
//! 把任务栏按钮放出来，回来后再收回 ⇒ `skipTaskbar` 是**动态的**。
//!
//! 但**这不削弱托盘的地位**：任务栏按钮只在窗口**已最小化**时才存在，
//! 而穿透是窗口**仍在屏幕上、只不过点不中**的状态 —— 那种状态下没有任务栏按钮。
//! ⇒ 穿透的退路依旧只有「托盘」和 `Ctrl+Alt+H` 两条，托盘仍是穿透的安全前提。
//! 两条路谁也不能替代谁：收起来该走任务栏（用户第一直觉），点不中该走托盘/热键。

use crate::library::ScriptSource;
use crate::state::AppState;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "prompter-tray";

/// 最近稿件在菜单里的条数。再多菜单会长得不像话。
const RECENT_IN_MENU: usize = 5;

/// 菜单项的 id 前缀。
const ID_WINDOW: &str = "win";
const ID_CLICK_THROUGH: &str = "ct";
const ID_QUIT: &str = "quit";
const ID_SCRIPT_PREFIX: &str = "s:";

/// 建托盘。**失败不返回 Err 中断启动**，只记日志 ——
/// 没有托盘的提词器仍然完全可用，只是穿透时少一条退路。
pub fn build(app: &AppHandle, state: &AppState) -> Option<TrayIcon> {
    let menu = match build_menu(app, state) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("[tray] 菜单构建失败：{e}");
            return None;
        }
    };

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Prompter 提词器")
        .on_menu_event(|app, event| on_menu(app, event.id().0.as_str()));

    // 图标直接用打包时嵌进来的那个（`icons/icon.ico`，由 tauri-codegen 转成 RGBA）。
    // 不用 `include_image!` 是为了不额外开 image-png / image-ico 特性。
    if let Some(icon) = app.default_window_icon().cloned().map(|i| i.to_owned()) {
        builder = builder.icon(icon);
    }

    match builder.build(app) {
        Ok(t) => {
            log::info!("[tray] 已创建");
            Some(t)
        }
        Err(e) => {
            log::warn!("[tray] 创建失败：{e}");
            None
        }
    }
}

/// 按当前状态重新生成菜单。
///
/// 每次库内容变化、窗口显隐、穿透开关之后都调它。
/// 整份重建而不是逐项改文本：菜单项的文本与可用性会一起变，
/// 重建只有一处逻辑，比维护五六个 `set_text` / `set_enabled` 更难写错。
fn build_menu(app: &AppHandle, state: &AppState) -> tauri::Result<Menu<tauri::Wry>> {
    // ★ 不能只看 `is_visible()`：**最小化的窗口它仍然返回 true**
    //   （Win32 的 IsWindowVisible 只反映 WS_VISIBLE 这个样式位，不看窗口实际在不在屏幕上）。
    //   这个坑真踩过：窗口被最小化后菜单写着「隐藏窗口」，点下去是 hide、
    //   再点 show 也回不到屏幕上 —— 用户认为"软件没了"。
    //   对用户来说"最小化"和"隐藏"本来就是同一件事：都不在屏幕上。
    //
    //   ★ 2026-09-27 起这条判据抽成了纯函数 `win::geometry::on_screen`：
    //     现在有三个调用点（这里、快捷键显隐、窗口事件的几何保护），
    //     三处各写一遍就是三份迟早走偏的判断。
    let on_screen = app
        .get_webview_window("main")
        .map(|w| {
            crate::win::geometry::on_screen(
                w.is_visible().unwrap_or(true),
                w.is_minimized().unwrap_or(false),
            )
        })
        .unwrap_or(true);

    let win_item = MenuItem::with_id(
        app,
        ID_WINDOW,
        if on_screen { "隐藏窗口" } else { "显示窗口" },
        true,
        None::<&str>,
    )?;

    // 「关闭鼠标穿透」在没开穿透时是灰的 —— 而不是藏起来。
    // 灰着能让用户知道"这个功能存在、现在不需要关"，藏起来就什么都看不出来。
    let ct_on = state.hub.ct_enabled();
    let ct_item = MenuItem::with_id(
        app,
        ID_CLICK_THROUGH,
        if ct_on { "关闭鼠标穿透" } else { "鼠标穿透：未开启" },
        ct_on,
        None::<&str>,
    )?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let header = MenuItem::with_id(app, "recent-header", "最近稿件", false, None::<&str>)?;

    let recent: Vec<(String, String)> = match state.library.lock() {
        Ok(lib) => lib
            .sorted()
            .into_iter()
            .take(RECENT_IN_MENU)
            .map(|e| {
                let mark = if e.source == ScriptSource::Pasted { "· 粘贴" } else { "" };
                (e.id.clone(), format!("{}{}", truncate(&e.title, 22), mark))
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    let mut items: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = Vec::new();
    items.push(Box::new(win_item));
    items.push(Box::new(ct_item));
    items.push(Box::new(sep1));
    items.push(Box::new(header));
    if recent.is_empty() {
        items.push(Box::new(MenuItem::with_id(
            app,
            "recent-empty",
            "（还没有用过的稿件）",
            false,
            None::<&str>,
        )?));
    } else {
        for (id, text) in recent {
            items.push(Box::new(MenuItem::with_id(
                app,
                format!("{ID_SCRIPT_PREFIX}{id}"),
                text,
                true,
                None::<&str>,
            )?));
        }
    }
    let sep2 = PredefinedMenuItem::separator(app)?;
    items.push(Box::new(sep2));
    items.push(Box::new(MenuItem::with_id(app, ID_QUIT, "退出", true, None::<&str>)?));

    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

/// 在**主线程**上刷新托盘菜单。
///
/// 必须回主线程：菜单与托盘图标都是平台 UI 对象。Tauri 的命令处理
/// 跑在异步运行时线程上，直接在那儿改菜单在 Windows 上能过、
/// 在 macOS 上会炸 —— 这种"只在某个平台炸"的 bug 最难查，干脆一开始就走正路。
pub(crate) fn refresh(state: &AppState) {
    let app = state.app.clone();
    let app_in = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let state = app_in.state::<AppState>();
        let Some(tray) = state.tray.lock().ok().and_then(|t| t.clone()) else {
            return;
        };
        match build_menu(&app_in, &state) {
            Ok(menu) => {
                if let Err(e) = tray.set_menu(Some(menu)) {
                    log::warn!("[tray] 菜单更新失败：{e}");
                }
            }
            Err(e) => log::warn!("[tray] 菜单重建失败：{e}"),
        }
    }) {
        log::warn!("[tray] 无法调度到主线程：{e}");
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let mut out: String = chars.into_iter().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn on_menu(app: &AppHandle, id: &str) {
    // 托盘菜单项的动作全部直接操作窗口/状态，不复用命令层 ——
    // 命令层的签名要求 Tauri 注入 `State`，在菜单回调里没有那些上下文。
    if let Some(script_id) = id.strip_prefix(ID_SCRIPT_PREFIX) {
        activate_from_tray(app, script_id);
        return;
    }

    match id {
        ID_WINDOW => {
            let Some(win) = app.get_webview_window("main") else {
                return;
            };
            // 判据与菜单文本一致（见 `build_menu`）：**最小化也算不在屏幕上**。
            // 还原必须是三步 `unminimize → show → set_focus`：
            //   * 少了 `unminimize`，最小化的窗口只会"被显示"成最小化状态，
            //     用户看到的仍是没反应 —— 这正是上一版救不回来的原因；
            //   * 少了 `set_focus`，窗口会出现在别的窗口底下（它是 alwaysOnTop，
            //     但焦点不在它身上时，用户第一眼未必注意到）。
            let on_screen = crate::win::geometry::on_screen(
                win.is_visible().unwrap_or(true),
                win.is_minimized().unwrap_or(false),
            );
            if on_screen {
                let _ = win.hide();
            } else {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
                // 从"不在屏幕上"回到屏幕上：如果它是被「收进任务栏」收起来的，
                // 那枚任务栏按钮该收回去了（平时这窗口不占任务栏）。
                crate::finish_taskbar_hold(&app.state::<AppState>(), &win);
            }
            let _ = app.emit("prompter://visibility", !on_screen);
            // 菜单文本要跟着翻面（「隐藏窗口」↔「显示窗口」）。
            // 原先这里漏了刷新 —— 点完菜单文字不变，看着像没生效。
            refresh(&app.state::<AppState>());
        }

        ID_CLICK_THROUGH => {
            let state = app.state::<AppState>();
            if let Some(win) = app.get_webview_window("main") {
                if let Err(e) = crate::win::set_click_through(&win, &state.hub, false) {
                    log::warn!("[tray] 关闭穿透失败：{e}");
                }
            }
            let _ = app.emit("prompter://click-through", false);
            refresh(&app.state::<AppState>());
        }

        ID_QUIT => {
            app.exit(0);
        }

        _ => {}
    }
}

/// 从托盘切稿。等价于点历史列表里的一项。
fn activate_from_tray(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match crate::commands::activate_script_inner(&state, id) {
        Ok(payload) => {
            let _ = app.emit("prompter://script", payload);
            let _ = app.emit_to("main", "prompter://notice", "已从托盘切换稿件");
        }
        Err(e) => {
            // 托盘切稿失败必须让用户看见 —— 点了菜单却什么都没发生是最糟的
            let _ = app.emit_to("main", "prompter://error", e);
        }
    }
    refresh(&state);
}
