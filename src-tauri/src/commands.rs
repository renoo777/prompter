//! Tauri 命令层：前端唯一能碰到的入口。
//!
//! 每个命令都返回 `Result<_, String>` —— 直接把错误文本给界面显示，
//! 而不是包装成错误码再让前端翻译。这个产品的用户是演讲者，
//! 在会议开始前 30 秒看到「模型文件缺失：...\tokens.txt」远比
//! 看到「E_MODEL_NOT_FOUND」有用。
//!
//! 一部分逻辑拆成了 `*_inner(&AppState)` 形式，因为**全局快捷键回调也要复用**：
//! 快捷键回调没有 Tauri 注入的 `State` / `WebviewWindow` 参数，
//! 只能直接拿 `&AppState`，硬套命令签名会写出一堆胶水。

use crate::asr;
use crate::library;
use crate::script;
use crate::state::{
    self, AppConfig, AppState, AsrStatusPayload, BootstrapPayload, CursorPayload, PlatformInfo,
    ScriptPayload,
};
use crate::stealth::{StealthMode, StealthReport};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

/// 托盘菜单里的"最近稿件"要跟着库变化刷新。放一个薄包装，
/// 让调用点读起来是"刷新托盘"而不是"调用 tray 模块"。
fn refresh_tray(state: &AppState) {
    crate::tray::refresh(state);
}

// ---------------------------------------------------------------------------
// 启动时的全量状态
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn bootstrap(state: State<AppState>) -> Result<BootstrapPayload, String> {
    let config = state.config.lock().map_err(|_| "配置锁异常")?.clone();

    // 把**已持久化的**跟读参数灌进引擎。
    //
    // ★ 这一步以前是缺的，症状很隐蔽：引擎由 `Aligner::new` 建成出厂默认，
    //   而用户改过的值躺在 config.json 里 —— 于是重启之后
    //   "设置页显示 0.5、引擎按 0.38 跑"，两个数都不报错，
    //   只有手感不对（而这恰恰是最难被归因的那类现象）。
    //   载入历史配置时本来就该把它们推到运行态，和 `import_bundle` 那条路径一样。
    //
    // 放在载入稿件之前：`set_script` 走的是 `reset_with`，它只换稿文、保留参数，
    // 所以两种顺序都对；写在前面是为了让"参数已就位"这个前提更明显。
    if let Ok(mut e) = state.engine.lock() {
        e.aligner.set_min_score(config.match_floor);
        e.aligner.set_jump_range(config.jump_range);
        // 打的是**引擎实际采用的**值，不是配置里的值 ——
        // 这两者曾经会漂（配置改了没灌进来），而漂了之后界面完全看不出来：
        // 设置页显示用户设的数，引擎跑的是出厂默认，两边都不报错。
        // 真机上"手感不对"的排查就是从这一行开始的。
        log::info!(
            "[bootstrap] 跟读参数：match_floor={:.2} jump_range={} 句",
            e.aligner.min_score(),
            e.aligner.jump_range()
        );
    }

    // 路标日志：前端**看到的**这两个开关值是后面所有动作的起点。
    // 排查"穿透/隐身点了没反应"时，第一件事就是确认前端到底拿到了什么。
    log::info!(
        "[bootstrap] 前端拿到：click_through={} stealth_on_start={} theme={:?}",
        config.click_through,
        config.stealth_on_start,
        config.theme
    );

    // 上次用的稿件还在的话直接载入 —— 演讲者不希望每次开会都重新选一次文件
    let script_payload = match config.last_script.clone() {
        Some(path) => match load_into_engine(&state, std::path::Path::new(&path)) {
            Ok(p) => p,
            Err(err) => {
                log::warn!("[script] 自动载入上次稿件失败：{err}");
                current_script_payload(&state)?
            }
        },
        None => current_script_payload(&state)?,
    };

    Ok(BootstrapPayload {
        config,
        script: script_payload,
        cursor: state.cursor_snapshot(),
        platform: platform_info(&state),
        stealth: None,
        models_dir: state.paths.models_dir.to_string_lossy().to_string(),
        found_models: state::scan_models(&state.paths.models_dir),
        asr_engine: Some(compute_asr_status(&state).engine),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        // 支持的稿件格式由**后端给出**，前端不再自己维护一份。
        // 前端那份一旦和后端分叉，表现是"文件对话框里选不到 .docx"
        // 或者"拖进来报不支持"—— 而这两处判断本来该是同一个答案。
        script_exts: crate::doc::SCRIPT_EXTS.iter().map(|s| s.to_string()).collect(),
        script_exts_hint: crate::doc::SCRIPT_EXTS_HINT.to_string(),
    })
}

fn platform_info(state: &AppState) -> PlatformInfo {
    use crate::win::platform::{self, TaskbarAction};
    PlatformInfo {
        os: platform::platform_key().to_string(),
        support: state.guard.support(),
        fallback_hint: state.guard.fallback_hint(),
        // ★ 文案与 `minimize_to_taskbar` 的分流**同一个来源**（`win::platform`），
        //   不是前端按 os 自己拼 —— 两边分头写就会漂成"按钮说一套、按下做另一套"。
        collapse_label: platform::collapse_entry_label().to_string(),
        collapse_hint: platform::collapse_entry_hint().to_string(),
        has_taskbar: platform::action() == TaskbarAction::MinimizeWithButton,
    }
}

fn current_script_payload(state: &AppState) -> Result<ScriptPayload, String> {
    let e = state.engine.lock().map_err(|_| "稿件锁异常".to_string())?;
    Ok(ScriptPayload::from(&e.script))
}

// ---------------------------------------------------------------------------
// 稿件
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_script_from_path(state: State<AppState>, path: String) -> Result<ScriptPayload, String> {
    let payload = load_into_engine(&state, std::path::Path::new(&path))?;

    // 记住路径，下次启动直接载入
    if let Ok(mut cfg) = state.config.lock() {
        cfg.last_script = Some(path.clone());
        let snapshot = cfg.clone();
        drop(cfg);
        if let Err(e) = state::save_config(&state.paths.config_file, &snapshot) {
            log::warn!("[config] 保存失败：{e}");
        }
    }

    // 进历史库。失败只记日志 —— 稿件已经能念了，历史是增强不是主路径。
    record_file_use(&state, &path, &payload.title, payload.total_chars);
    refresh_tray(&state);
    Ok(payload)
}

#[tauri::command]
pub fn set_script_text(
    state: State<AppState>,
    text: String,
    title: Option<String>,
) -> Result<ScriptPayload, String> {
    let s = script::build_script(&text, title);
    let payload = apply_script(&state, s)?;
    // 粘贴稿必须落盘副本 —— 它没有原文件可依，不落盘的话
    // 历史里会有一条点开是空的（比没有历史更糟）
    record_pasted_use(&state, &text, &payload.title, payload.total_chars);
    refresh_tray(&state);
    Ok(payload)
}

#[tauri::command]
pub fn clear_script(state: State<AppState>) -> Result<ScriptPayload, String> {
    apply_script(&state, script::build_script("", None))
}

/// 把一段文本当作「识别结果」喂给对位器。
///
/// 这个命令不是为了好玩 —— 它让整套跟随逻辑可以在**没有麦克风**的环境下被完整验证：
/// 在设置页粘一段稿件的片段，就能立刻看到光标是不是跳到了正确位置。
/// 排练新稿子、排查「为什么跟不上」时都用它。
#[tauri::command]
pub fn feed_hypothesis(state: State<AppState>, text: String) -> Result<CursorPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常")?;
    e.hyp = text.clone();
    let o = e.aligner.update(&text);
    Ok(e.cursor_payload(o.score, o.moved, o.miss_streak, o.realigned))
}

pub(crate) fn load_into_engine(
    state: &AppState,
    path: &std::path::Path,
) -> Result<ScriptPayload, String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败：{e}"))?;
    let ext = crate::doc::ext_of(path);

    // .docx / .pptx 是 ZIP 包，**不能**当文本解码 —— 那样只会得到一堆二进制垃圾，
    // 而且不会报错。所以按扩展名分派，而不是"先试 UTF-8 失败再兜底"。
    let (text, encoding) = if crate::doc::is_rich_ext(&ext) {
        (crate::doc::extract_rich(&bytes, &ext)?, ext.clone())
    } else {
        let (t, enc) = script::decode_bytes(&bytes);
        (t, enc.to_string())
    };

    log::info!(
        "[script] 载入 {} （{encoding}，{} 字节）",
        path.display(),
        bytes.len()
    );

    let title = path.file_stem().and_then(|s| s.to_str()).map(String::from);
    let s = script::build_script(&text, title);

    // 空文件 / 只有空白 / 只有数字与符号的稿子：**拒绝载入**，
    // 而不是把当前稿件替换成一个空的。
    //
    // 这条守的是「载入路径必须全有或全无」：提词器最不能接受的失败
    // 就是"念到一半稿子没了"。判定放在 `apply_script` **之前**，
    // 所以任何一条失败路径上台上的稿件都一字未动。
    //
    // 判"空"判的是**抽出来的文本**而不是原始字节：.docx 的原始字节永远不为空，
    // 拿字节判会得出"有内容"的错误结论，然后载入一份空稿 —— 正是这里要防的事。
    if s.chars.is_empty() {
        return Err(if text.trim().is_empty() {
            "文件里没有可朗读的文字，当前稿件未改动。".to_string()
        } else {
            "这份稿件里没有可定位的文字（只有数字或符号），无法跟随朗读。当前稿件未改动。"
                .to_string()
        });
    }

    apply_script(state, s)
}

pub(crate) fn apply_script(state: &AppState, s: script::Script) -> Result<ScriptPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常".to_string())?;
    e.set_script(s);
    Ok(ScriptPayload::from(&e.script))
}

// ---------------------------------------------------------------------------
// 稿件库
// ---------------------------------------------------------------------------

/// 历史上限保护的对象：`last_script` 指向的那一条。
///
/// 它必须活下来 —— 下次启动要自动载入它，被淘汰掉等于让用户升级后
/// 丢掉"上次的稿子"。其余条目按最久未用淘汰。
fn protected_id(state: &AppState) -> Option<String> {
    state
        .config
        .lock()
        .ok()
        .and_then(|c| c.last_script.clone())
        .map(|p| library::file_id(&p))
}

/// 记一次文件类稿件使用。只记日志、不向上抛错 —— 历史是增强功能，
/// 它坏掉不该让"载入稿件"这个主路径失败。
pub(crate) fn record_file_use(state: &AppState, path: &str, title: &str, chars: usize) {
    let now = state::now_unix();
    let protected = protected_id(state);

    // 锁的范围只包住内存操作：落盘与删文件都在锁外做，
    // 否则一次慢磁盘 IO 会把整个历史列表（包括界面刷新）卡住。
    let (snapshot, evicted) = {
        let Ok(mut lib) = state.library.lock() else {
            log::warn!("[library] 锁异常，本次不记历史");
            return;
        };
        lib.upsert_file(path, title, chars, now);
        let evicted = lib.prune(library::MAX_ENTRIES, protected.as_deref());
        (lib.clone(), evicted)
    };

    drop_copies(state, &evicted);
    if let Err(e) = state::save_library(&state.paths, &snapshot) {
        log::warn!("[library] 索引保存失败：{e}");
    }
}

/// 记一次粘贴类稿件使用，并把正文写成副本。
pub(crate) fn record_pasted_use(state: &AppState, text: &str, title: &str, chars: usize) {
    let now = state::now_unix();
    let protected = protected_id(state);

    let (snapshot, copy_rel, evicted) = {
        let Ok(mut lib) = state.library.lock() else {
            log::warn!("[library] 锁异常，本次不记历史");
            return;
        };
        let id = lib.upsert_pasted(text, title, chars, now);
        let evicted = lib.prune(library::MAX_ENTRIES, protected.as_deref());
        (lib.clone(), library::copy_rel_of(&id), evicted)
    };

    // 副本先落盘、再写索引：反过来的话索引里会出现指向不存在文件的记录。
    // 注意副本**不在**被淘汰之列（刚 upsert 的那条是 last_used 最新的一条）。
    if let Err(e) = std::fs::create_dir_all(&state.paths.scripts_dir) {
        log::warn!("[library] 无法创建副本目录：{e}");
    } else {
        let p = state::copy_path(&state.paths, &copy_rel);
        if let Err(e) = std::fs::write(&p, text) {
            log::warn!("[library] 副本写入失败（{}）：{e}", p.display());
        }
    }

    drop_copies(state, &evicted);
    if let Err(e) = state::save_library(&state.paths, &snapshot) {
        log::warn!("[library] 索引保存失败：{e}");
    }
    refresh_tray(state);
}

/// 删掉一批副本文件。
///
/// ⚠️ 只删传进来的路径，而传进来的**只可能是副本** ——
/// `library.rs` 的 `remove` / `clear` / `prune` 在类型层面就只返回 `copy_rel`，
/// 用户原文件的路径根本不会出现在这里。这条不变量由单测钉住。
fn drop_copies(state: &AppState, rels: &[String]) {
    for rel in rels {
        let p = state::copy_path(&state.paths, rel);
        if let Err(e) = std::fs::remove_file(&p) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[library] 删除副本失败（{}）：{e}", p.display());
            }
        }
    }
}

/// 把当前库落盘 + 刷新托盘菜单。库内容变了之后统一走这里。
fn persist_library(state: &AppState) {
    let snapshot = match state.library.lock() {
        Ok(lib) => lib.clone(),
        Err(_) => return,
    };
    if let Err(e) = state::save_library(&state.paths, &snapshot) {
        log::warn!("[library] 索引保存失败：{e}");
    }
    refresh_tray(state);
}

/// 历史列表（按最后使用时间倒序）。
#[tauri::command]
pub fn list_scripts(state: State<AppState>) -> Vec<library::ScriptEntry> {
    match state.library.lock() {
        Ok(lib) => lib.sorted().into_iter().cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// 从历史里切回一份稿件。
#[tauri::command]
pub fn activate_script(state: State<AppState>, id: String) -> Result<ScriptPayload, String> {
    activate_script_inner(&state, &id)
}

/// 切稿的内部实现。**拆出来是因为托盘菜单也要用它** ——
/// 菜单回调没有 Tauri 注入的 `State` 参数，硬套命令签名只会写出更多胶水。
pub(crate) fn activate_script_inner(state: &AppState, id: &str) -> Result<ScriptPayload, String> {
    let entry = {
        let lib = state.library.lock().map_err(|_| "稿件库锁异常".to_string())?;
        lib.entry(id).cloned().ok_or_else(|| "这一条已不在历史里".to_string())?
    };

    let payload = match entry.source {
        library::ScriptSource::File => {
            let path = entry
                .path
                .clone()
                .ok_or_else(|| "这条历史记录没有文件路径".to_string())?;
            if !std::path::Path::new(&path).is_file() {
                // 文件可能只是插着移动硬盘 / 网络盘没挂上，所以**不自动移除条目**，
                // 只把情况说清楚
                return Err(format!("文件已不在原位置：{path}"));
            }
            let p = load_into_engine(state, std::path::Path::new(&path))?;
            if let Ok(mut cfg) = state.config.lock() {
                cfg.last_script = Some(path.clone());
                let snap = cfg.clone();
                drop(cfg);
                let _ = state::save_config(&state.paths.config_file, &snap);
            }
            p
        }
        library::ScriptSource::Pasted => {
            let rel = entry
                .copy_rel
                .clone()
                .ok_or_else(|| "这条历史记录缺少正文副本".to_string())?;
            let p = state::copy_path(&state.paths, &rel);
            let bytes = std::fs::read(&p).map_err(|e| format!("读取副本失败：{e}"))?;
            let (text, _) = script::decode_bytes(&bytes);
            let s = script::build_script(&text, Some(entry.title.clone()));
            apply_script(state, s)?
        }
    };

    // 刷新使用时间与计数
    {
        let now = state::now_unix();
        if let Ok(mut lib) = state.library.lock() {
            match entry.source {
                library::ScriptSource::File => {
                    if let Some(p) = entry.path.as_deref() {
                        lib.upsert_file(p, &payload.title, payload.total_chars, now);
                    }
                }
                library::ScriptSource::Pasted => {
                    if let Some(rel) = entry.copy_rel.as_deref() {
                        let p = state::copy_path(&state.paths, rel);
                        if let Ok(bytes) = std::fs::read(&p) {
                            let (text, _) = script::decode_bytes(&bytes);
                            lib.upsert_pasted(&text, &payload.title, payload.total_chars, now);
                        }
                    }
                }
            }
        }
    }
    persist_library(state);

    Ok(payload)
}

/// 从历史里移除一条（同时删掉它的正文副本）。
#[tauri::command]
pub fn forget_script(state: State<AppState>, id: String) -> Result<(), String> {
    let rel = {
        let mut lib = state.library.lock().map_err(|_| "稿件库锁异常".to_string())?;
        lib.remove(&id)
    };
    if let Some(rel) = rel {
        drop_copies(&state, &[rel]);
    }
    persist_library(&state);
    Ok(())
}

/// 清空历史。
///
/// 删掉的**只有副本**。用户的原文件一个都不动 —— 这是本产品最不能出错的一条：
/// 一个"清空历史"按钮如果顺手删了用户的稿子，是不可原谅的。
#[tauri::command]
pub fn clear_script_history(state: State<AppState>) -> Result<(), String> {
    let rels = {
        let mut lib = state.library.lock().map_err(|_| "稿件库锁异常".to_string())?;
        lib.clear()
    };
    drop_copies(&state, &rels);
    persist_library(&state);
    Ok(())
}

/// 检查一条历史记录对应的文件是否还在（列表里用来标灰）。
#[tauri::command]
pub fn script_availability(state: State<AppState>) -> Vec<(String, bool)> {
    let Ok(lib) = state.library.lock() else {
        return Vec::new();
    };
    lib.entries
        .iter()
        .map(|e| {
            let ok = match e.source {
                library::ScriptSource::File => e
                    .path
                    .as_deref()
                    .map(|p| std::path::Path::new(p).is_file())
                    .unwrap_or(false),
                library::ScriptSource::Pasted => e
                    .copy_rel
                    .as_deref()
                    .map(|r| state::copy_path(&state.paths, r).is_file())
                    .unwrap_or(false),
            };
            (e.id.clone(), ok)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 光标控制
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn nudge(state: State<AppState>, delta: i64) -> Result<CursorPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常")?;
    e.aligner.nudge(delta as isize);
    Ok(e.cursor_payload(0.0, true, 0, false))
}

#[tauri::command]
pub fn goto(state: State<AppState>, pos: usize) -> Result<CursorPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常")?;
    e.aligner.set_cursor(pos);
    Ok(e.cursor_payload(0.0, true, 0, false))
}

#[tauri::command]
pub fn step_sentence(state: State<AppState>, forward: bool) -> Result<CursorPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常")?;
    let cur = e.aligner.cursor();
    let target = if forward {
        next_sentence_start(&e.script, cur)
    } else {
        prev_sentence_start(&e.script, cur)
    };
    if let Some(t) = target {
        e.aligner.set_cursor(t);
    }
    Ok(e.cursor_payload(0.0, target.is_some(), 0, false))
}

/// 光标回到开头 —— 排练时最常用的操作。
#[tauri::command]
pub fn rewind(state: State<AppState>) -> Result<CursorPayload, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常")?;
    e.aligner.set_cursor(0);
    Ok(e.cursor_payload(0.0, true, 0, false))
}

/// 找到光标之后的第一个句首。
///
/// 遍历顺序依赖「段落的句子按 `align_start` 全局递增」这个不变量 ——
/// 它由 `script::build_script` 保证（按顺序构建、顺序追加字符）。
pub(crate) fn next_sentence_start(script: &script::Script, cur: usize) -> Option<usize> {
    for p in &script.paragraphs {
        for s in &p.sentences {
            // 跳过归一化后为空的句子（纯数字/纯符号），它们没有可停靠的位置
            if s.align_end <= s.align_start {
                continue;
            }
            let start = s.align_start as usize;
            if start > cur {
                return Some(start);
            }
        }
    }
    None
}

/// 找到光标之前的最后一个句首。
pub(crate) fn prev_sentence_start(script: &script::Script, cur: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for p in &script.paragraphs {
        for s in &p.sentences {
            if s.align_end <= s.align_start {
                continue;
            }
            let start = s.align_start as usize;
            if start < cur {
                best = Some(start);
            } else {
                return best;
            }
        }
    }
    best
}

// ---------------------------------------------------------------------------
// 隐身
// ---------------------------------------------------------------------------

/// 取得当前窗口的原生句柄。
///
/// 两个平台的语义不同，但上层不需要知道：
///   * Windows —— `HWND`
///   * macOS   —— `NSView*`（不是 `NSWindow*`！）
///
/// ★★ macOS 这条要特别小心：`raw_window_handle` 给的是 **view**，
/// 而 AppKit 里绝大多数窗口级选择子（`setSharingType:` `windowNumber` …）
/// 都属于 **`NSWindow`**。对 view 发这些选择子会 unrecognized selector，
/// 在 `objc_msgSend` 的 C ABI 下**直接终止进程**（不是 panic，拦不住）。
/// ⇒ 所以升层 **不是**在这里做，而是由真正的调用方 `stealth::macos` 内部
/// 用 `[view window]` 统一处理（见 `stealth::macos::view_window`），
/// 这样上层拿到的句柄语义在各平台一致，且不会有人漏做这一步。
pub(crate) fn native_handle(
    window: &tauri::WebviewWindow,
) -> Result<crate::stealth::NativeHandle, String> {
    // tauri 没有重新导出 raw-window-handle，用自己的直接依赖。
    // 版本必须与 tauri 内部一致（都是 0.6），否则 trait 对不上。
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window
        .window_handle()
        .map_err(|e| format!("无法取得窗口句柄：{e}"))?;

    match handle.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(w) => Ok(w.hwnd.get() as *mut std::ffi::c_void),
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(w) => Ok(w.ns_view.as_ptr()),
        _ => Err("当前平台暂不支持窗口级别的捕获保护。".to_string()),
    }
}

#[tauri::command]
pub fn set_stealth(
    state: State<AppState>,
    window: tauri::WebviewWindow,
    enabled: bool,
) -> Result<StealthReport, String> {
    let mode = if enabled {
        StealthMode::Hidden
    } else {
        StealthMode::Visible
    };
    let handle = native_handle(&window)?;
    let report = state
        .guard
        .apply(handle, mode)
        .map_err(|e| e.to_string())?;
    if report.applied {
        if let Ok(mut m) = state.stealth_mode.lock() {
            *m = mode;
        }
    }
    Ok(report)
}

#[tauri::command]
pub fn stealth_support(state: State<AppState>) -> PlatformInfo {
    platform_info(&state)
}

// ---------------------------------------------------------------------------
// 识别
// ---------------------------------------------------------------------------

pub(crate) fn start_asr_inner(state: &AppState) -> Result<AsrStatusPayload, String> {
    let (cfg, auto_wpm) = {
        let c = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        (c.asr.clone(), c.auto_wpm)
    };

    // 匀速滚动要知道稿子多长、从哪个字接着走 —— 它的位置就是按时间对稿子长度
    // 算出来的。语音引擎忽略这份上下文（它们自己从麦克风拿文本）。
    //
    // ★ 起点取**当前光标**而不是 0：「暂停一下、回头接着念」要求按继续时从原地
    //   接上，而不是回到开头。光标已经停在末尾的情况由引擎自己处置
    //   （那等于"讲完了再按一次"，应当重头来 —— 见 `AutoScrollAsr::start`）。
    let seed = {
        let e = state.engine.lock().map_err(|_| "稿件锁异常".to_string())?;
        asr::EngineSeed {
            total: e.aligner.total(),
            start: e.aligner.cursor(),
            wpm: auto_wpm as f32,
        }
    };

    // 先停掉可能还在跑的旧引擎，避免两路音频同时抢麦克风
    if let Ok(mut slot) = state.asr.lock() {
        if let Some(mut old) = slot.take() {
            old.stop();
        }
    }

    let mut engine = asr::create(cfg.engine, &cfg, &seed)?;
    let name = engine.name().to_string();
    engine.start(state.make_sink())?;
    let running = engine.is_running();
    log::info!("[asr] 启动「{name}」（engine={:?}）", cfg.engine);

    if let Ok(mut slot) = state.asr.lock() {
        *slot = Some(engine);
    }

    Ok(AsrStatusPayload {
        running,
        engine: name.clone(),
        detail: format!("{name} 已启动"),
    })
}

pub(crate) fn stop_asr_inner(state: &AppState) -> Result<AsrStatusPayload, String> {
    let mut slot = state.asr.lock().map_err(|_| "识别锁异常".to_string())?;
    let name = slot
        .as_ref()
        .map(|e| e.name().to_string())
        .unwrap_or_else(|| "识别".to_string());
    if let Some(mut e) = slot.take() {
        e.stop();
    }
    Ok(AsrStatusPayload {
        running: false,
        engine: name.clone(),
        detail: format!("{name} 已停止"),
    })
}

fn compute_asr_status(state: &AppState) -> AsrStatusPayload {
    match state.asr.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(e) => AsrStatusPayload {
                running: e.is_running(),
                engine: e.name().to_string(),
                detail: String::new(),
            },
            None => AsrStatusPayload {
                running: false,
                engine: "未启动".into(),
                detail: String::new(),
            },
        },
        Err(_) => AsrStatusPayload {
            running: false,
            engine: "未知".into(),
            detail: "识别状态读取失败".into(),
        },
    }
}

#[tauri::command]
pub fn start_asr(state: State<AppState>) -> Result<AsrStatusPayload, String> {
    start_asr_inner(&state)
}

#[tauri::command]
pub fn stop_asr(state: State<AppState>) -> Result<AsrStatusPayload, String> {
    stop_asr_inner(&state)
}

#[tauri::command]
pub fn asr_status(state: State<AppState>) -> AsrStatusPayload {
    compute_asr_status(&state)
}

/// 麦克风自检：录一小段，回答「这个设备到底有没有在送声音」。
///
/// 做成后端命令而不是让前端去猜，是因为「判断」必须拿到**原始采样** ——
/// 只有后端有。前端能看到的只有识别文本，而设备送静音时文本就是空的，
/// 跟「用户没说话」长得一模一样，无从区分。
///
/// 会阻塞 `seconds` 秒（默认 2.5 秒）。前端按异步 await 处理即可，
/// 不要放在会卡住 UI 的地方。
#[tauri::command]
pub fn mic_check(seconds: Option<f64>) -> Result<asr::MicProbe, String> {
    #[cfg(feature = "local-asr")]
    {
        asr::mic::probe_microphone(seconds.unwrap_or(asr::mic::DEFAULT_PROBE_SECONDS))
    }
    #[cfg(not(feature = "local-asr"))]
    {
        let _ = seconds;
        Err("当前构建未包含本地识别能力，因此没有麦克风采集通道可供自检。".to_string())
    }
}

// ---------------------------------------------------------------------------
// 配置与文件系统
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn save_config(state: State<AppState>, mut config: AppConfig) -> Result<(), String> {
    /*
     * ★★ 后端拥有的字段一律以**内存态**为准，不采纳前端这份副本（理由见
     *    `state::merge_backend_owned`）。配置是整份写的，而前端手上这份是
     *    **启动那一刻**的快照：它并不知道窗口被拖到了多大、上次载的是哪篇稿子、
     *    穿透/置顶是不是刚被打开。直接写它，等于每改一个设置就把那些值
     *    一起退回启动时 —— 用户对这件事只有一种描述：「我设的东西没记住」。
     */
    // 磨砂档位在合并前后各取一次：变了就要立刻换窗体材质。
    // 它是**前端拥有**的字段（设置页三档行写它，后端从不写），
    // 所以 `merge_backend_owned` 不会碰它。
    let (prev_glass_mode, new_glass_mode) = {
        let current = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        let prev = current.glass_mode;
        state::merge_backend_owned(&mut config, &current);
        (prev, config.glass_mode)
    };

    state::save_config(&state.paths.config_file, &config)?;
    // 配置里的「跟读灵敏度」要落到正在跑的对位器上。
    // 不在这里同步的话，用户拖完滑杆、重启应用之前的这一段时间里，
    // 界面显示的是新值、引擎用的是旧值 —— 而这种偏差用户只会
    // 总结成"这个灵敏度没用"。
    if let Ok(mut e) = state.engine.lock() {
        e.aligner.set_min_score(config.match_floor);
    }
    if let Ok(mut c) = state.config.lock() {
        *c = config;
    }
    // 档位变了 → 立刻换窗体材质。挂/卸都是这一个函数（卸 = set_effects(None)），
    // 没有"CSS 关了但系统材质还挂着"的中间态。
    if new_glass_mode != prev_glass_mode {
        apply_glass_mode(&state.app, new_glass_mode);
    }
    Ok(())
}

/// 按磨砂档位挂/卸**系统级窗口材质**（Windows Acrylic / macOS hudWindow）。
///
/// # 为什么挂在这里（save_config + 启动），而不是独立命令
///
/// 档位是 `AppConfig` 的字段，写它的唯一路径就是设置页 → 防抖 → `save_config`
/// （不可破 2：前端不直接 invoke 窗口操作）。在这条唯一路径上检测变化，
/// 就不需要第二个命令、第二种时序。
///
/// # 平台映射在哪
///
/// 「哪个平台挂什么」是纯判断，在 `win::effects::plan_for`（可单测）；
/// 这里只做它 → Tauri 类型的换算与调用。
pub(crate) fn apply_glass_mode(app: &AppHandle, mode: state::GlassMode) {
    use tauri::utils::config::{Color, WindowEffectsConfig};
    use tauri::utils::WindowEffect;

    let plan = match mode {
        state::GlassMode::System => crate::win::effects::plan_for(std::env::consts::OS),
        // Off / Css：不挂系统材质。`None` ⇒ set_effects(None)，把 Acrylic 真正卸下
        // —— P0-6：「关」必须连窗体合成一起退，不是只关 CSS 那一层。
        _ => None,
    };

    let effects = plan.map(|p| WindowEffectsConfig {
        effects: p
            .names
            .iter()
            .filter_map(|n| match *n {
                "acrylic" => Some(WindowEffect::Acrylic),
                "hud_window" => Some(WindowEffect::HudWindow),
                _ => None,
            })
            .collect(),
        state: None,
        radius: p.radius,
        color: Some(Color(p.color[0], p.color[1], p.color[2], p.color[3])),
    });

    let Some(win) = app.get_webview_window("main") else {
        log::warn!("[glass] 找不到主窗口，材质未应用");
        return;
    };
    match win.set_effects(effects) {
        Ok(()) => log::info!("[glass] 档位 → {mode:?}（材质已按平台方案应用/卸下）"),
        // 显式失败：挂不上要说出来，不能"界面显示系统磨砂、窗口其实是实底"。
        Err(e) => log::warn!("[glass] 设置窗口材质失败（{e}）——档位 {mode:?} 未生效"),
    }
}

/// 置顶开关。**窗口属性与配置一起改**，而且只有这一处写。
///
/// # 为什么必须合成一个命令
///
/// 它有两个调用者：「开始提词 / 收摊」和设置里那个开关。两边各写一份的话
/// 迟早出现"界面显示置顶、窗口其实没置顶"——而那种偏差在会议里才会显形
/// （窗口被别的窗口压住），排查时又没有任何线索指向它。
///
/// 之前它是**纯运行时属性**（前端 `getCurrentWindow().setAlwaysOnTop()`，
/// 不落配置），于是每次开机都要重新点一次。用户的说法是"设置不记住"。
///
/// 返回**实际生效后的状态**，不是"用户点了什么" —— 与 `set_click_through`
/// 同一条纪律：界面显示的是窗口现在是什么样。
#[tauri::command]
pub fn set_always_on_top(
    state: State<AppState>,
    window: tauri::WebviewWindow,
    on: bool,
) -> Result<bool, String> {
    window
        .set_always_on_top(on)
        .map_err(|e| format!("置顶没设上：{e}"))?;

    if let Ok(mut cfg) = state.config.lock() {
        cfg.always_on_top = on;
        let snap = cfg.clone();
        drop(cfg);
        // 写失败要说出来：这一条是"用户以为设好了、其实下次打开又没了"。
        if let Err(e) = state::save_config(&state.paths.config_file, &snap) {
            log::warn!("[config] 置顶落盘失败：{e}");
        }
    }
    // 读回来，而不是把入参原样回给前端 —— 两个值不一致时以窗口为准。
    Ok(window.is_always_on_top().unwrap_or(on))
}

/// 收进任务栏。**缩下去，但任务栏留一枚按钮。**
///
/// # 为什么不是 `hide()`
///
/// 原来这里是 hide()（连按钮文案都写的是「收进托盘」）。问题是三件事叠在一起：
///
/// 1. `hide()` 之后**任务栏按钮也跟着消失** —— 那是 Windows 的行为，
///    窗口不可见就不占任务栏；
/// 2. 托盘图标在 Windows 11 上**默认被折进 `^` 溢出区**（实测本机注册表里
///    我们的图标 `IsPromoted=0`），用户不点那个小箭头根本看不见它；
/// 3. 于是"收起来"之后，屏幕上**没有任何可见入口** —— 用户的原话是
///    「没有办法收起到这里（任务栏）」。而当时那条提示还写着"点托盘图标"，
///    等于**指了一个他看不见的东西**。
///
/// 所以改成 minimize 语义：把窗口缩下去，并保证任务栏上有它的按钮。
/// 用户点任务栏按钮还原 —— 这是所有 Windows 软件都一样的那套肌肉记忆。
///
/// # ★★ macOS 上这条路**不存在**，必须换落点
///
/// 上面整套（`set_skip_taskbar(false)` + 最小化 + 三态握手）的前提是
/// **"任务栏上能长出一枚可点的按钮"**。macOS 没有这个东西：
/// Dock 图标只在 `activationPolicy = .regular` 时出现，而本应用是
/// `.accessory` 语义（`skipTaskbar: true`），且该策略**只能冷启动时确定**——
/// 启动后再改，Dock 不会重排。所以照搬的结局是"窗口缩下去、没有任何入口回来"，
/// 即本项目最不能接受的**静默失败**。
///
/// ⇒ 在不支持的平台上退化成「收起」：窗口压成一条细栏，不碰任何系统 API。
///    它比 Windows 那条路弱（还占着 46px），但**可见、可逆、一定能回来**。
///    判定见 [`win::platform`]，那里有单测钉着"绝不静默消失"。
///
/// # 为什么顺序是「先放按钮、再最小化」
///
/// `set_skip_taskbar(false)` 在 Windows 上走 `ITaskbarList::AddTab`
/// （见 tao 的 `platform_impl/windows/window.rs`），它是给**已存在的窗口**
/// 补一个任务栏按钮。窗口已经最小化之后再补，任务栏不一定立刻重排；
/// 先补、再最小化，则最小化那一刻按钮已经在位。
///
/// # 与 `taskbar_hold` 的关系
///
/// 这里只负责"关掉 skip_taskbar"，**回收**（还原成不占任务栏）交给窗口事件
/// —— 因为还原的触发点在用户手里（点任务栏按钮），后端只能从
/// "窗口不再最小化了"这个事件里知道。标志位就是这两半之间的握手。
#[tauri::command]
pub fn minimize_to_taskbar(
    state: State<AppState>,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    // ★ 平台分流放在最前面：macOS 上**一个系统 API 都不碰**，
    //   直接走收起。见本函数文档里那段关于 Dock 的说明。
    if crate::win::platform::action() == crate::win::platform::TaskbarAction::CollapseInstead {
        log::info!("[taskbar] 当前平台无任务栏按钮，退化为「收起」");
        set_collapsed_inner(&state, &window, true)?;
        return Ok(());
    }

    window
        .set_skip_taskbar(false)
        .map_err(|e| format!("任务栏按钮没放出来：{e}"))?;
    // ★ 先记标志再最小化：最小化会立刻触发 Resized 回调，
    //   那个回调要靠这个标志判断"该不该回收"。反过来的话，
    //   第一拍事件会看到一个还没置位的标志，按钮就会留着不走。
    state.arm_taskbar();
    window.minimize().map_err(|e| {
        // 最小化失败要把按钮还回去，否则任务栏会白留一个点了没反应的按钮。
        if state.clear_taskbar_hold() {
            let _ = window.set_skip_taskbar(true);
        }
        format!("收进任务栏失败：{e}")
    })?;
    log::info!("[taskbar] 已收进任务栏（任务栏按钮已放出，窗口已最小化）");
    Ok(())
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> Result<AppConfig, String> {
    state
        .config
        .lock()
        .map(|c| c.clone())
        .map_err(|_| "配置锁异常".to_string())
}

#[tauri::command]
pub fn rescan_models(state: State<AppState>) -> Vec<asr::ModelPaths> {
    state::scan_models(&state.paths.models_dir)
}

#[tauri::command]
pub fn models_dir(state: State<AppState>) -> String {
    state.paths.models_dir.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// 跟读灵敏度（读稿手感类）
// ---------------------------------------------------------------------------

/// 改匹配下限，并**立即**作用于正在跑的对位器。
///
/// 拖滑杆时每次 `onChange` 都会调到这里，所以它必须便宜 ——
/// 不重建引擎、不清稿件，只改一个 f32 加清零失败计数。
/// 返回**夹取后**的实际值，让界面显示的和引擎用的是同一个数
/// （否则用户会看到「滑到 0.2 了但感觉没变」这类无从判断的现象）。
#[tauri::command]
pub fn set_match_floor(state: State<AppState>, value: f32) -> Result<f32, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常".to_string())?;
    e.aligner.set_min_score(value);
    Ok(e.aligner.min_score())
}

/// 读回当前生效的匹配下限 —— 界面初始化的唯一来源，
/// 不靠前端自己记（前端记的会和引擎里的漂开）。
#[tauri::command]
pub fn match_floor(state: State<AppState>) -> f32 {
    state
        .engine
        .lock()
        .map(|e| e.aligner.min_score())
        .unwrap_or(crate::align::DEFAULT_MIN_SCORE)
}

/// 改「跳读范围」（单位：句），并**立即**作用于正在跑的对位器。
///
/// 与 `set_match_floor` 同形：拖滑杆时每次 `onChange` 都调到这里，
/// 所以只改两个 usize 加清零失败计数，不重建引擎、不清稿件。
/// 返回**夹取后**的值，让界面显示的和引擎用的是同一个数。
#[tauri::command]
pub fn set_jump_range(state: State<AppState>, value: u32) -> Result<u32, String> {
    let mut e = state.engine.lock().map_err(|_| "稿件锁异常".to_string())?;
    Ok(e.aligner.set_jump_range(value))
}

/// 读回当前生效的跳读范围（句）。
#[tauri::command]
pub fn jump_range(state: State<AppState>) -> u32 {
    state
        .engine
        .lock()
        .map(|e| e.aligner.jump_range())
        .unwrap_or(crate::align::DEFAULT_JUMP_RANGE)
}

/// 改匀速滚动的速度（字/分），并**立即**作用于正在跑的那台引擎。
///
/// 返回夹取后的实际值 —— 界面显示的和引擎用的是同一个数。
///
/// ★ 与 `set_match_floor` / `set_jump_range` 有一处**刻意的不同**：
///   那两个只推引擎，这一个还要写一份进配置。因为速度不只是"当前生效值"，
///   它同时是**建引擎时的参数** —— 不写这一份的话，"拖完速度马上点开始"
///   会用 400ms 前那次防抖保存的旧值启动，而此后没有一处会把它推回来
///   （引擎已经跑起来了，前端以为早就同步过了）。
///   表现就是"设置里写着 400，稿子按 300 在滚"，两个数都不报错。
#[tauri::command]
pub fn set_auto_wpm(state: State<AppState>, value: u32) -> Result<u32, String> {
    let v = crate::asr::auto_scroll::clamp_wpm(value as f32).round() as u32;

    if let Ok(mut c) = state.config.lock() {
        c.auto_wpm = v;
    }

    // 推给正在跑的引擎。当前若不是匀速滚动，`set_rate` 是 trait 上的
    // 默认空实现 —— 所以这里不必先判断"现在是哪种推进方式"，
    // 那种判断散在几处，迟早会有一处忘了跟着改。
    if let Ok(mut slot) = state.asr.lock() {
        if let Some(e) = slot.as_mut() {
            e.set_rate(v as f32);
        }
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// 会前自检（省事类）
// ---------------------------------------------------------------------------

/// 自检的一行结论。
///
/// `level` 只有三档，**每一档都对应界面上一种确定的显示**：
/// `ok` 绿、`warn` 黄（能开讲但有事要留意）、`bad` 红（现在开讲会出事）。
/// 不让后端返回自由文本级别，是为了避免前端"猜"该怎么显示 ——
/// 猜错的结果是把「麦克风没声音」显示成绿色。
#[derive(Debug, Clone, Serialize)]
pub struct PreflightRow {
    /// 项目名（稿件 / 投屏隐身 / 麦克风 / 语音模型）。
    pub key: String,
    /// 一句人话结论。
    pub verdict: String,
    pub level: String,
    /// 出问题时**具体怎么办**。`ok` 时为空串。
    pub hint: String,
}

fn pf(key: &str, verdict: impl Into<String>, level: &str, hint: impl Into<String>) -> PreflightRow {
    PreflightRow {
        key: key.to_string(),
        verdict: verdict.into(),
        level: level.to_string(),
        hint: hint.into(),
    }
}

/// 会前自检 —— 四行结论，扫一眼就知道现在能不能开讲。
///
/// 它**刻意包含一次真实录音**（约 1.2 秒）。理由：「设备在送数字静音」
/// 这件事只看配置永远查不出来，而它恰恰是最常见的"点了开始却没反应"
/// （见 README 里那次真实故障）。用户主动点自检时等一秒多是可接受的成本；
/// 把这一秒省掉，代价是用户在真实会议上才发现没声音。
#[tauri::command]
pub fn preflight(state: State<AppState>) -> Vec<PreflightRow> {
    let mut rows: Vec<PreflightRow> = Vec::new();

    // 当前的**推进方式**要不要麦克风 —— 后面③④两项靠它决定该不该报。
    // ★ 匀速滚动与纯手动都不采集声音，那时候报一句"麦克风没声音"是纯假警报，
    //   而假消息比不提示更坏：它把人引到一个跟当前问题无关的地方去查。
    let (wants_mic, drive_label) = state
        .config
        .lock()
        .map(|c| (c.asr.engine.uses_mic(), c.asr.engine.label()))
        .unwrap_or((false, "未知"));

    // ① 稿件 —— 没稿子的话后面几项都没有意义，所以放第一位
    let (chars, title) = state
        .engine
        .lock()
        .map(|e| (e.script.chars.len(), e.script.title.clone()))
        .unwrap_or((0, String::new()));
    if chars > 0 {
        rows.push(pf(
            "稿件",
            format!("已载入「{title}」· 可定位 {chars} 字"),
            "ok",
            "",
        ));
    } else {
        rows.push(pf(
            "稿件",
            "还没有载入稿件",
            "bad",
            format!(
                "把 {} 拖进窗口，点顶栏的 ✎ 粘贴一段，或按 Ctrl+K 从历史里选一篇。",
                crate::doc::SCRIPT_EXTS_HINT
            ),
        ));
    }

    // ② 投屏隐身 —— 这是本产品的立身之本，没生效就等于没装
    let support = state.guard.support();
    let hidden = state
        .stealth_mode
        .lock()
        .map(|m| m.is_hidden())
        .unwrap_or(false);
    match (&support, hidden) {
        (crate::stealth::SupportLevel::Unavailable(note), _) => rows.push(pf(
            "投屏隐身",
            format!("本平台没有可用方案：{note}"),
            "bad",
            state
                .guard
                .fallback_hint()
                .unwrap_or_else(|| "共享时请选「只共享 PPT 窗口」。".to_string()),
        )),
        (crate::stealth::SupportLevel::Fragile(note), true) => rows.push(pf(
            "投屏隐身",
            "已开启，但本平台走的是脆弱路径（可能随系统升级失效）",
            "warn",
            note.clone(),
        )),
        (_, true) => rows.push(pf("投屏隐身", "已生效，共享画面里看不到本窗口", "ok", "")),
        (_, false) => rows.push(pf(
            "投屏隐身",
            "未开启 —— 本窗口会出现在共享画面里",
            "bad",
            "按 Ctrl+Alt+S 开启，或点顶栏左边的状态胶囊。",
        )),
    }

    // ③④ 只在**语音跟随**下才有意义。不采集声音时直接给出两行"跳过"就返回 ——
    // 比在下面每一处都加条件要清楚得多，也顺带省掉了那 1.2 秒的真实录音。
    if !wants_mic {
        rows.push(pf(
            "麦克风",
            format!("当前是「{drive_label}」，不采集声音 —— 这一项跳过"),
            "ok",
            "换成「语音跟随」之后，这里才会真去录一段。",
        ));
        rows.push(pf(
            "语音模型",
            "同上，当前方式用不到",
            "ok",
            "",
        ));
        return rows;
    }

    // ③ 麦克风 —— 真录一小段。这是唯一能查出「数字静音」的办法
    #[cfg(feature = "local-asr")]
    {
        match asr::mic::probe_microphone(1.2) {
            Ok(p) => {
                let ok = matches!(p.verdict, asr::MicVerdict::Ok);
                rows.push(pf(
                    "麦克风",
                    p.detail.clone(),
                    if ok { "ok" } else { "bad" },
                    if ok {
                        ""
                    } else {
                        "先看键盘上有没有按到静音键（这类静音在系统设置里查不出来）；\
                         再打开设置 → 麦克风自检看具体原因。"
                    },
                ));
            }
            Err(e) => rows.push(pf(
                "麦克风",
                format!("打不开采集设备：{e}"),
                "bad",
                "设备可能被别的程序独占，或系统麦克风权限被拒。",
            )),
        }
    }
    #[cfg(not(feature = "local-asr"))]
    {
        rows.push(pf(
            "麦克风",
            "当前构建不含本地识别，没有采集通道",
            "warn",
            "这个构建只能手动翻页；需要跟读请用带 local-asr 的构建。",
        ));
    }

    // ④ 语音模型 —— 只在走本地识别时才重要
    #[cfg(feature = "local-asr")]
    {
        let models = state::scan_models(&state.paths.models_dir);
        match models.iter().find(|m| m.validate().is_empty()) {
            Some(m) => {
                let label = if m.label.is_empty() {
                    "（未命名）".to_string()
                } else {
                    m.label.clone()
                };
                rows.push(pf("语音模型", format!("已就绪：{label}"), "ok", ""));
            }
            None if models.is_empty() => rows.push(pf(
                "语音模型",
                "一个模型都没有",
                "bad",
                "运行 python scripts\\fetch-model.py 下载模型。",
            )),
            None => rows.push(pf(
                "语音模型",
                format!("有 {} 个模型目录，但文件不完整", models.len()),
                "bad",
                "重新跑一次 python scripts\\fetch-model.py（支持断点续传）。",
            )),
        }
    }

    rows
}

// ---------------------------------------------------------------------------
// 配置导入导出（省事类）
// ---------------------------------------------------------------------------

/// 迁移包。`kind` 是给人工看的 —— 用户可能直接编辑这个文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub kind: String,
    pub version: u32,
    pub exported_at: i64,
    pub config: AppConfig,
    pub library: library::ScriptLibrary,
    /// `相对路径 -> 正文`，**仅粘贴类**稿件。
    ///
    /// 为什么必须把正文带上：文件类历史项存的是**绝对路径**，
    /// 换机器后那些路径必然全部失效 —— 只导索引的话，
    /// 用户在新机器上拿到的是一张"全部找不到"的列表，等于没导。
    /// 粘贴类的正文本来就在我们的数据目录里（它没有别的落点），
    /// 带上它才是真正可迁移的那部分。
    #[serde(default)]
    pub bodies: std::collections::BTreeMap<String, String>,
}

/// 把当前配置与稿件库导出成一个 JSON 文件。
///
/// 返回写出的字节数 —— 界面用它做"确实写出去了"的凭据。
/// 只报成功不报大小的话，写了个 0 字节文件用户也看不出来。
#[tauri::command]
pub fn export_bundle(state: State<AppState>, path: String) -> Result<u64, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "配置锁异常".to_string())?
        .clone();
    let lib = state
        .library
        .lock()
        .map_err(|_| "稿件库锁异常".to_string())?
        .clone();

    let mut bodies = std::collections::BTreeMap::new();
    for e in &lib.entries {
        if let Some(rel) = &e.copy_rel {
            let p = state::copy_path(&state.paths, rel);
            if let Ok(text) = std::fs::read_to_string(&p) {
                bodies.insert(rel.clone(), text);
            }
        }
    }

    let bundle = Bundle {
        kind: "prompter-config-bundle".to_string(),
        version: 1,
        exported_at: state::now_unix(),
        config,
        library: lib,
        bodies,
    };

    let text = serde_json::to_string_pretty(&bundle).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("写入失败：{e}"))?;
    Ok(text.len() as u64)
}

/// 从迁移包恢复配置与稿件库。返回**导入后**的配置，前端据此重刷界面。
///
/// 刻意不做"先备份当前配置"这种半自动的事：用户点导入时心里想的就是覆盖。
/// 真正需要保护的是**写坏**（文件截断/JSON 损坏），所以先整体解析、
/// 全部成功之后才落盘 —— 解析失败时磁盘上什么都不会被改动。
#[tauri::command]
pub fn import_bundle(state: State<AppState>, path: String) -> Result<AppConfig, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读不到文件：{e}"))?;
    let bundle: Bundle =
        serde_json::from_str(&text).map_err(|e| format!("不是有效的迁移包：{e}"))?;

    if bundle.kind != "prompter-config-bundle" {
        return Err(format!(
            "这个文件的类型是「{}」，不是 Prompter 迁移包。",
            bundle.kind
        ));
    }

    // 先把粘贴类的正文写回磁盘，再写索引 —— 顺序反过来的话，
    // 中途失败会留下"索引指向一个不存在的正文"的状态。
    for (rel, body) in &bundle.bodies {
        let p = state::copy_path(&state.paths, rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
        }
        std::fs::write(&p, body.as_bytes()).map_err(|e| format!("写稿件副本失败：{e}"))?;
    }

    state::save_library(&state.paths, &bundle.library)?;
    if let Ok(mut lib) = state.library.lock() {
        *lib = bundle.library.clone();
    }

    state::save_config(&state.paths.config_file, &bundle.config)?;
    if let Ok(mut e) = state.engine.lock() {
        e.aligner.set_min_score(bundle.config.match_floor);
        e.aligner.set_jump_range(bundle.config.jump_range);
    }
    if let Ok(mut c) = state.config.lock() {
        *c = bundle.config.clone();
    }

    Ok(bundle.config)
}

/// 用系统默认程序打开一个路径（模型目录、导出的稿件等）。
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在：{path}"));
    }

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&path).spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&path).spawn();

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();

    result.map(|_| ()).map_err(|e| format!("无法打开：{e}"))
}

// ---------------------------------------------------------------------------
// 窗口层：穿透 / 收起 / 位置锁定
// ---------------------------------------------------------------------------

/// 开关鼠标穿透。
///
/// 返回**切换后是否真的处于穿透态**。前端据此把状态显示出来 ——
/// 显示的不是"用户点了什么"，而是"窗口现在是什么"。
#[tauri::command]
pub fn set_click_through(
    state: State<AppState>,
    window: tauri::WebviewWindow,
    enabled: bool,
) -> Result<bool, String> {
    set_click_through_inner(&state, &window, enabled)
}

/// 内部实现。**拆出来是因为全局热键也要用它** ——
/// 热键回调没有 Tauri 注入的 `State` / `WebviewWindow` 参数。
pub(crate) fn set_click_through_inner(
    state: &AppState,
    window: &tauri::WebviewWindow,
    enabled: bool,
) -> Result<bool, String> {
    crate::win::set_click_through(window, &state.hub, enabled)?;
    // 路标日志：这一行有没有出现，直接区分"前端/热键没调到"和"调到了但没生效"。
    log::info!(
        "[click-through] 开关 → {}（实际穿透={}）",
        if enabled { "开" } else { "关" },
        state.hub.ct_now()
    );

    // 同步到配置，下次启动仍然生效
    if let Ok(mut cfg) = state.config.lock() {
        cfg.click_through = enabled;
        let snap = cfg.clone();
        drop(cfg);
        let _ = state::save_config(&state.paths.config_file, &snap);
    }
    refresh_tray(state);
    // 返回给前端的是「**已经在工作**吗」（开关 + 可交互区都到位），不是 `ct_now`。
    // `ct_now` 表达的是"此刻光标在不在稿件区"，会随鼠标翻 ——
    // 拿它当返回值，前端的状态灯会闪，而且分不清"功能没生效"和"光标恰好在栏上"。
    Ok(state.hub.ct_working())
}

/// 前端进度打点。
///
/// ★ 存在的理由：release 产物**没有 devtools**，前端的异常和"卡在第几步"从外面完全看不见。
///   日志里有没有 `[bootstrap]` 只能区分"页面跑没跑"，分不出"跑到一半停了" ——
///   而这两种情况的排查方向完全相反（前者看 webview/产物，后者看前端逻辑）。
///   因此前端在启动链路的每个关键节点各打一点，日志里就能直接看出断在哪。
///
/// 刻意做成"无返回值、不参与业务"的命令：打点本身失败不该影响任何功能。
#[tauri::command]
pub fn frontend_beat(step: String) {
    log::info!("[frontend] {step}");
}

/// 前端上报「可交互区」矩形（CSS 像素，相对内容区左上角）。
///
/// 为什么由前端算：控制区长什么样只有 DOM 知道 —— 设置抽屉打开时、
/// 历史浮层展开时、收起态下，可交互区的形状都不一样。
/// 后端只负责把 CSS 坐标换算成屏幕物理坐标，然后判断光标在不在里面。
///
/// 传 `None`（或一份**不可信**的数据）表示"整窗都算可交互"，
/// 此时穿透会被关掉 —— 这是安全兜底：宁可让窗口可点，
/// 也不能让它变成一个点不动的幽灵。
///
/// ★★ 收的是**一组**矩形。展开态下控制栏有上下两条，
/// 前端若把它们并成一个包围盒再传过来，那个盒子就是整窗，
/// 穿透将永远不触发（真机踩过，见 `win/click_through.rs` 的回归用例）。
#[tauri::command]
pub fn report_interactive_zone(state: State<AppState>, rects: Option<Vec<[f64; 4]>>) {
    let first = !state.hub.zones_seen();
    let cleaned = rects.as_deref().and_then(crate::win::click_through::sanitize_zones);
    if cleaned.is_none() && rects.as_ref().is_some_and(|r| !r.is_empty()) {
        // 传上来了东西但被拒 ⇒ 这是**异常**，必须留下痕迹。
        // 静默丢掉的话，表现就是"穿透开关点了没反应"，最难查的那一类。
        log::warn!(
            "[click-through] 可交互区上报被拒（{} 块）⇒ 按整窗可交互处理",
            rects.as_ref().map(|r| r.len()).unwrap_or(0)
        );
    }
    state.hub.set_zones_css(cleaned);
    // ★ 路标日志：这条是"前端到底有没有把可交互区送过来"的唯一直接证据。
    //   穿透失效时第一现场就靠它分岔 ——
    //   没这行 ⇒ 链路断在前端（报错被吞、命令没注册）；
    //   有这行但形状不对 ⇒ 断在 DOM 标了哪些 `data-interactive`。
    //   只在**形状变化**时记，所以稳态下不会刷屏（400ms 一次的轮询不产生日志）。
    if first || state.hub.zones_summary() != state.hub.zones_logged() {
        let s = state.hub.zones_summary();
        log::info!("[click-through] 可交互区已更新 → {s}");
        state.hub.set_zones_logged(s);
    }
}

/// 收起 / 展开。
///
/// 只改窗口高度，不动宽度 —— 收起后控制栏还是那么宽，
/// 突然变窄会让用户找不到刚才那个按钮在哪。
#[tauri::command]
pub fn set_collapsed(
    state: State<AppState>,
    window: tauri::WebviewWindow,
    collapsed: bool,
) -> Result<(), String> {
    set_collapsed_inner(&state, &window, collapsed)
}

/// 内部实现。热键回调与命令层共用（与 `set_click_through_inner` 同理）。
pub(crate) fn set_collapsed_inner(
    state: &AppState,
    window: &tauri::WebviewWindow,
    collapsed: bool,
) -> Result<(), String> {
    let hub = &state.hub;
    if collapsed == hub.is_collapsed() {
        return Ok(());
    }

    let (w, h) = crate::win::geometry::sanitize_size(hub.size().0, hub.size().1);
    let target_h = if collapsed {
        crate::win::geometry::COLLAPSED_H
    } else {
        h
    };

    // 先置标志再改尺寸：`Resized` 事件是异步来的，标志先到位，
    // 事件处理时才知道"这一下是收起造成的，不该覆盖记忆中的展开尺寸"。
    hub.set_collapsed(collapsed);
    window
        .set_size(tauri::LogicalSize::new(w as f64, target_h as f64))
        .map_err(|e| format!("调整窗口高度失败：{e}"))
}

#[tauri::command]
pub fn is_collapsed(state: State<AppState>) -> bool {
    state.hub.is_collapsed()
}

/// 位置锁定（防误拖）。锁上之后顶栏不再作为拖动区域。
#[tauri::command]
pub fn set_window_locked(state: State<AppState>, locked: bool) -> Result<(), String> {
    let snapshot = {
        let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        cfg.window.locked = locked;
        cfg.clone()
    };
    state::save_config(&state.paths.config_file, &snapshot)
}

// ---------------------------------------------------------------------------
// 自检
// ---------------------------------------------------------------------------

/// 自检信息 —— 设置页里展示，出问题时用户可以直接把这段发出来。
#[tauri::command]
pub fn diagnostics(state: State<AppState>, window: tauri::WebviewWindow) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    out.push(("平台".into(), crate::stealth::platform_name().to_string()));
    out.push(("隐身能力".into(), format!("{:?}", state.guard.support())));
    out.push((
        "窗口句柄".into(),
        match native_handle(&window) {
            Ok(h) => format!("{h:p}"),
            Err(e) => format!("取得失败：{e}"),
        },
    ));
    // 穿透是"点了没反应"最难自查的一项：开关、实际生效、可交互区
    // 三者可能各不相同，而界面上只看得到一个胶囊。放在自检里，
    // 用户报问题时这一行就能把"前端没上报"和"后端没切"分开。
    out.push((
        "鼠标穿透".into(),
        format!(
            "开关={} 实际={} 可交互区={}",
            if state.hub.ct_enabled() { "开" } else { "关" },
            if state.hub.ct_now() { "开" } else { "关" },
            state.hub.zones_summary(),
        ),
    ));
    out.push((
        "本地识别".into(),
        if cfg!(feature = "local-asr") {
            "已编译进本版本".into()
        } else {
            "本版本未包含".to_string()
        },
    ));
    out.push((
        "模型目录".into(),
        state.paths.models_dir.to_string_lossy().to_string(),
    ));
    out.push((
        "已发现模型".into(),
        format!("{} 个", state::scan_models(&state.paths.models_dir).len()),
    ));

    if let Ok(cfg) = state.config.lock() {
        // 标签是「推进方式」而不是「识别引擎」：这一行的值现在也可能是
        // 「匀速滚动」或「纯手动」—— 那时根本没有识别在跑，
        // 顶着"识别引擎"的名头会让人去查一个不存在的故障。
        out.push(("推进方式".into(), cfg.asr.engine.label().to_string()));
        if let Some(m) = &cfg.asr.local_model {
            out.push(("当前模型".into(), m.label.clone()));
            let missing = m.validate();
            out.push((
                "模型文件".into(),
                if missing.is_empty() {
                    "完整".into()
                } else {
                    format!("缺失 {}", missing.join(" / "))
                },
            ));
        }
    }
    out
}
