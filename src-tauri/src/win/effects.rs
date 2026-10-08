//! 「系统磨砂」档位在每个平台上挂什么材质 —— 纯数据 + 纯函数，可单测。
//!
//! # 为什么这层不认识 Tauri
//!
//! 与 `geometry` / `click_through` / `taskbar` / `platform` 同一纪律
//! （根 MEMORY 不可破 1）：**平台到材质的映射是一个判断**，判断就要能被
//! 没有窗口、没有 macOS 的环境钉住。这里只产"效果名 + 染色 + 圆角"，
//! 到 `WindowEffect` 枚举的换算在 `commands::apply_glass_mode`（碰 Tauri 的那层）。
//!
//! # 关于 macOS 的 Liquid Glass（2026-09-28 核实）
//!
//! 调研文档（`docs/磨砂玻璃-调研与方案.md`）里写的 `liquidGlass` 来自
//! Tauri **官方 JS API 文档** —— 它领先于本仓库锁定的版本：
//! `tauri-utils 2.9.3` 的 `WindowEffect` 枚举里**没有 LiquidGlass 这个变体**
//! （到 `Acrylic` 为止）。当前工具链在 macOS 上能给的最强材质就是
//! NSVisualEffectMaterial 一族，这里选 `hudWindow` —— 它本来就是
//! 苹果给浮窗/HUD 准备的材质（暗色半透明，behindWindow blending）。
//! 真机轮若要 Liquid Glass，先升 Tauri 或引 `window-vibrancy 0.8.1`，
//! 到时候改的还是这个文件，**别处不用动**。

/// 一档材质方案：效果名（运行时取当前系统支持的第一个）+ 染色 + 圆角。
#[derive(Debug, Clone, PartialEq)]
pub struct GlassPlan {
    /// 效果名，与 `tauri::utils::WindowEffect` 的 serde 名一致
    /// （`acrylic` / `hudWindow` 的 snake 写法在这里只是字符串，
    ///   换算枚举的那层负责对应）。
    pub names: &'static [&'static str],
    /// 染色 RGBA。Windows 上影响 Blur/Acrylic；macOS 上 hudWindow 不吃它，
    /// 留着是为了将来 Liquid Glass（tint）与统一语义。
    pub color: [u8; 4],
    /// 圆角。**macOS 必需**：窗口 `decorations:false`，系统不知道往哪磨圆。
    /// 数值与前端 `--r-outer` 对齐。
    pub radius: Option<f64>,
}

/// 染色统一取自深色 `--bg`（`14 17 22`）。
///
/// alpha 取 190 而不是 spike 时的 160：提词器上**稿子是命**，
/// Acrylic 失焦会退成这个颜色的实色（拍板②接受的系统行为），
/// 160 叠在白底 PPT 上是中灰，白字对比不够；190 才压得住。
pub const GLASS_TINT: [u8; 4] = [14, 17, 22, 190];

/// 「系统磨砂」在当前平台上挂什么。`None` ＝ 这个平台没有可挂的材质
/// （Linux：效果由 compositor 决定，官方明确不支持 —— 这时 `System` 档
/// 退化为"什么都不挂"，界面上的口径见设置页那行常驻小字）。
pub fn plan_for(os: &str) -> Option<GlassPlan> {
    match os {
        // Windows：要"磨窗口后面"**只有 Acrylic 一条路**。
        // Mica 采的是桌面壁纸（窗口后面放什么它都不看），对本需求是假药；
        // Blur 在新系统上已被 Acrylic 取代且同样有拖拽性能坑。
        "windows" => Some(GlassPlan {
            names: &["acrylic"],
            color: GLASS_TINT,
            radius: None,
        }),
        // macOS：当前锁定版本的 Tauri 只有 NSVisualEffectMaterial 一族
        // （见文件头关于 Liquid Glass 的核实）。hudWindow 是苹果给浮窗的材质。
        "macos" => Some(GlassPlan {
            names: &["hud_window"],
            color: GLASS_TINT,
            radius: Some(12.0),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 平台 ⇒ 材质的映射必须钉成断言：它是"哪个平台挂哪种效果"的唯一依据，
    /// 改错了（比如把 Mica 给回来）在开发机上**没有任何信号** ——
    /// Windows 开发机编不出 macOS 分支的错，界面也不会报错。
    #[test]
    fn windows_gets_acrylic_not_mica() {
        let plan = plan_for("windows").expect("Windows 必须有材质方案");
        assert_eq!(plan.names, &["acrylic"], "要磨窗口后面只有 Acrylic；Mica 采的是壁纸");
        assert_eq!(plan.radius, None, "radius 是 macOS 专属");
    }

    #[test]
    fn macos_gets_hud_window_with_radius() {
        let plan = plan_for("macos").expect("macOS 必须有材质方案");
        assert_eq!(plan.names, &["hud_window"]);
        // decorations:false ⇒ 圆角必须给，否则磨的是一块直角。
        assert!(plan.radius.unwrap_or(0.0) > 0.0, "macOS 上 radius 必需");
    }

    #[test]
    fn unsupported_platform_gets_nothing() {
        assert!(plan_for("linux").is_none(), "官方不支持 ⇒ 显式不挂，不硬凑");
        assert!(plan_for("freebsd").is_none());
    }
}
