//! 窗口几何：尺寸净化与「跑到屏幕外」自愈。
//!
//! 全是纯函数 —— 不碰 Tauri、不碰窗口，只做坐标与尺寸的算术。
//! 这样「没拔显示器时位置对不对」这种判断可以脱离真实屏幕跑单测，
//! 而多显示器、负坐标这些最容易写错的边界也就能被钉住。

/// 一块矩形（屏幕物理像素坐标系）。
///
/// 坐标为**桌面坐标系**，副屏在主屏左侧时为负 —— 这是刻意的，
/// 不是所有平台都把原点放在主屏左上角（X11 是"最左那块屏的左上角"）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }

    /// 向外扩张 `pad` 像素，四个方向都扩张。用于「接近区」判定。
    pub fn inflate(&self, pad: f64) -> Self {
        Self {
            x: self.x - pad,
            y: self.y - pad,
            w: self.w + pad * 2.0,
            h: self.h + pad * 2.0,
        }
    }

    /// 与另一矩形重叠区域的宽、高。
    pub fn overlap_size(&self, o: &Rect) -> (f64, f64) {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        (w.max(0.0), h.max(0.0))
    }
}

// ---------------------------------------------------------------------------
// 尺寸
// ---------------------------------------------------------------------------

/// 默认尺寸。**这不是拍脑袋**：字号 34px × 行高 1.7 ≈ 每行 58px，
/// 要看到"下几句"至少 4~5 行 ≈ 290px，加顶栏 36 + 底栏 28 ≈ 360px。
/// 再小就只能显示 2 行，读稿时会因为频繁滚动而晕。
/// 宽度 560 是照着「一行放得下 14~15 个 34px 汉字」定的。
pub const DEFAULT_W: u32 = 560;
pub const DEFAULT_H: u32 = 360;

/// 尺寸下限。低于这个值控制栏按钮就放不下了（R4a-3）。
pub const MIN_W: u32 = 420;
pub const MIN_H: u32 = 160;

/// 尺寸上限。超过它一定是手改配置或文件损坏，不是真实需求。
pub const MAX_W: u32 = 4000;
pub const MAX_H: u32 = 3000;

/// 窗口至少要有这么大一块面积落在某块屏幕里，才算"还看得见"。
pub const MIN_VISIBLE: f64 = 80.0;

/// 收起态的高度 —— 只留那一条控制栏。
///
/// ★★ 它必须 ≥ 栏高（CSS `--h-top` = 46px）＋ 窗口上下各 1px 的边框 = 48px。
///    定小了不会报任何错，只会静静地把栏的下沿裁掉一条 ——
///    而那条栏现在装着「展开」键，它是收起态下**唯一**能展开回来的入口
///    （上一版正因为"展开键长在被收起的栏里"而彻底点不到）。
///    所以这个数跟着 `--h-top` 一起改，别让它单独漂。
pub const COLLAPSED_H: u32 = 48;

/// 净化宽高。
///
/// 越界（`0`、`99999`）一律**回落默认值**，而不是"夹到边界"。
/// 理由：一个越界的值只可能来自手改配置或文件损坏，此时我们并不知道用户想要什么，
/// 回落到一个经过论证的已知良好值，比夹到一个任意的边界更安全
/// —— 夹到 `MIN_H=160` 会得到一个能开但几乎没法用的窗口（R4a-4 要避免的正是这个）。
pub fn sanitize_size(w: u32, h: u32) -> (u32, u32) {
    let w = if (MIN_W..=MAX_W).contains(&w) { w } else { DEFAULT_W };
    let h = if (MIN_H..=MAX_H).contains(&h) { h } else { DEFAULT_H };
    (w, h)
}

/// 窗口此刻算不算"在屏幕上"。
///
/// ★★ 判据必须是「可见 **且** 没有最小化」，只用 `is_visible()` 是错的：
///    Win32 的 `IsWindowVisible` 只反映 `WS_VISIBLE` 这个样式位，
///    **最小化的窗口它照样返回 true**。这条坑本项目踩过一次 ——
///    窗口被最小化后托盘菜单还写着「隐藏窗口」，点下去是 hide、再点 show
///    也回不到屏幕上，用户认为"软件没了"（见 `tray.rs` 里那段注释）。
///    对用户来说"最小化"和"隐藏"本来就是同一件事：都不在屏幕上。
///
/// 现在有三个调用点（托盘菜单文本、还原路径、快捷键显隐），所以抽成纯函数 ——
/// 三处各写一遍就是三份迟早会走偏的判断。
pub fn on_screen(visible: bool, minimized: bool) -> bool {
    visible && !minimized
}

/// 这一拍要不要把窗口几何记进"用户记住的尺寸/位置"。
///
/// 最小化会触发一次 `WM_SIZE`，此时宽高报成 `0×0`。若照记不误，
/// `sanitize_size` 会把 `0×0` 判为越界、**回落成 `560×360` 默认值** ——
/// 于是用户"收起来一次"就把精心拖出来的窗口尺寸弄丢了，而且全程没有任何提示。
/// 隐藏态（`on_screen == false`）同理不记。
///
/// 收起态本来在调用点就有独立分支（收起高度是临时的，不能覆盖展开尺寸），
/// 这里一并收进来，让"该不该记几何"只剩一处判断。
pub fn should_capture_geometry(visible: bool, minimized: bool, collapsed: bool) -> bool {
    on_screen(visible, minimized) && !collapsed
}

/// 把位置夹回可见区。
///
/// 典型触发场景：上次窗口在副屏上，这次副屏拔了 —— 位置还留着，
/// 但窗口已经不在任何一块屏幕里，用户会以为"程序没启动"。
///
/// `monitors` 是当前所有屏幕；`primary` 是主屏（回落目标）。
/// 判定用「重叠区域的宽和高都 ≥ MIN_VISIBLE」，而不是"左上角在屏内"——
/// 后者在窗口大部分移出屏幕右侧时会误判为可见，而用户其实什么都点不到。
pub fn clamp_to_monitors(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    monitors: &[Rect],
    primary: Rect,
) -> (i32, i32) {
    let win = Rect::new(x as f64, y as f64, w as f64, h as f64);

    let visible = monitors.iter().any(|m| {
        let (ow, oh) = win.overlap_size(m);
        ow >= MIN_VISIBLE && oh >= MIN_VISIBLE
    });
    if visible {
        return (x, y);
    }

    // 回落到主屏左上角，留 48px 边距 —— 别贴着屏幕边缘，遮住标题栏不好拖。
    //
    // 注意「窗口比屏幕还大」这个分支：此时 `primary.w - w` 是负数，
    // 如果无条件往内收就会把窗口推到屏幕外的负坐标去（用户照样什么都看不到）。
    // 放不下时保持 48px 边距、只保证「左上角在屏内」，那已经是能给出的最好结果。
    let margin = 48.0;
    let axis = |origin: f64, extent: f64, size: f64| -> i32 {
        let start = origin + margin;
        if size <= extent {
            // 放得下：边距可能把窗口顶出右/下边界，那就贴边收进来
            start.min(origin + extent - size).round() as i32
        } else {
            // 放不下：至少让左上角可见
            start.round() as i32
        }
    };
    (
        axis(primary.x, primary.w, w as f64),
        axis(primary.y, primary.h, h as f64),
    )
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn primary() -> Rect {
        Rect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    #[test]
    fn default_size_fits_the_documented_budget() {
        assert_eq!(DEFAULT_W, 560);
        assert_eq!(DEFAULT_H, 360);
        // 占 1080p 屏高不超过 35%（PRD 指标）
        assert!((DEFAULT_H as f64 / 1080.0) <= 0.35);
    }

    #[test]
    fn sanitize_keeps_in_range_values() {
        assert_eq!(sanitize_size(560, 360), (560, 360));
        assert_eq!(sanitize_size(MIN_W, MIN_H), (MIN_W, MIN_H));
        assert_eq!(sanitize_size(MAX_W, MAX_H), (MAX_W, MAX_H));
    }

    #[test]
    fn sanitize_falls_back_to_default_for_garbage() {
        assert_eq!(sanitize_size(0, 0), (DEFAULT_W, DEFAULT_H));
        assert_eq!(sanitize_size(99999, 99999), (DEFAULT_W, DEFAULT_H));
        assert_eq!(sanitize_size(10, 20), (DEFAULT_W, DEFAULT_H));
    }

    #[test]
    fn sanitize_handles_axes_independently() {
        // 宽度坏了不该把好高度一起扔掉
        assert_eq!(sanitize_size(0, 500), (DEFAULT_W, 500));
        assert_eq!(sanitize_size(800, 99999), (800, DEFAULT_H));
    }

    #[test]
    fn position_fully_on_primary_is_kept() {
        assert_eq!(clamp_to_monitors(100, 100, 560, 360, &[primary()], primary()), (100, 100));
    }

    #[test]
    fn negative_coords_on_secondary_left_are_kept() {
        // 副屏在主屏左侧：桌面坐标是负的，这是**合法**位置，必须原样保留。
        // 把负坐标一律当成"跑出屏幕"会导致副屏在主屏左侧的用户每次重启窗口都被拽回来。
        let left = Rect::new(-1920.0, 0.0, 1920.0, 1080.0);
        let (x, y) = clamp_to_monitors(-1600, 200, 560, 360, &[left, primary()], primary());
        assert_eq!((x, y), (-1600, 200));
    }

    #[test]
    fn unplugged_monitor_position_falls_back_to_primary() {
        // 上次在副屏 (2000, 100)，现在只剩主屏 —— 必须拉回主屏可见区
        let (x, y) = clamp_to_monitors(2000, 100, 560, 360, &[primary()], primary());
        assert_eq!((x, y), (48, 48));

        let win = Rect::new(x as f64, y as f64, 560.0, 360.0);
        let (ow, oh) = win.overlap_size(&primary());
        assert!(ow >= MIN_VISIBLE && oh >= MIN_VISIBLE, "回落之后必须真的看得见");
    }

    #[test]
    fn far_off_screen_position_falls_back() {
        assert_eq!(clamp_to_monitors(-9999, -9999, 560, 360, &[primary()], primary()), (48, 48));
        assert_eq!(clamp_to_monitors(50000, 50000, 560, 360, &[primary()], primary()), (48, 48));
    }

    #[test]
    fn tiny_corner_sliver_counts_as_invisible() {
        // 只有 10px 露在屏幕内 —— 用户其实点不到，应当回落
        let x = 1920 - 10;
        assert_eq!(clamp_to_monitors(x, 100, 560, 360, &[primary()], primary()), (48, 48));
    }

    #[test]
    fn enough_corner_visible_is_kept() {
        // 露出 100x100，够点到了 —— 保留用户原来的位置
        let x = 1920 - 100;
        let (rx, ry) = clamp_to_monitors(x, 100, 560, 360, &[primary()], primary());
        assert_eq!((rx, ry), (x, 100));
    }

    #[test]
    fn fallback_never_exceeds_primary_bounds() {
        // 一块很小的屏幕：窗口比它还大，回落也不能算出屏幕外的坐标
        let small = Rect::new(0.0, 0.0, 400.0, 300.0);
        let (x, y) = clamp_to_monitors(9999, 9999, 560, 360, &[small], small);
        assert!(x <= 400 && y <= 300, "回落结果不能跑到屏幕外：({x},{y})");
        assert_eq!((x, y), (48, 48));
    }

    #[test]
    fn rect_geometry_helpers() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains((10.0, 20.0)));
        assert!(r.contains((109.9, 69.9)));
        assert!(!r.contains((110.0, 20.0)), "右边界是开区间");
        assert!(!r.contains((10.0, 70.0)));

        let i = r.inflate(5.0);
        assert_eq!((i.x, i.y, i.w, i.h), (5.0, 15.0, 110.0, 60.0));

        // r: x 10..110 / y 20..70；o: x 50..150 / y 40..90
        // → 重叠 x 50..110（宽 60）、y 40..70（高 30）
        assert_eq!(r.overlap_size(&Rect::new(50.0, 40.0, 100.0, 50.0)), (60.0, 30.0));
        assert_eq!(r.overlap_size(&Rect::new(500.0, 500.0, 10.0, 10.0)), (0.0, 0.0));
        // 完全包含：重叠区就是被包含者本身
        assert_eq!(r.overlap_size(&Rect::new(20.0, 30.0, 10.0, 10.0)), (10.0, 10.0));
    }

    #[test]
    fn minimized_window_is_not_on_screen() {
        assert!(on_screen(true, false));
        // ★ 这条是重点：最小化的窗口 `is_visible()` 仍然返回 true。
        //   只用可见位判，就会对着一个不在屏幕上的窗口说"它在这儿"。
        assert!(!on_screen(true, true));
        assert!(!on_screen(false, false));
        assert!(!on_screen(false, true));
    }

    #[test]
    fn geometry_is_not_captured_while_minimized_hidden_or_collapsed() {
        assert!(should_capture_geometry(true, false, false));
        // 最小化会送来一次 0×0 的 WM_SIZE；记下去会被 sanitize_size 判越界、
        // 回落成 560×360，等于悄悄抹掉用户拖出来的尺寸。
        assert!(!should_capture_geometry(true, true, false));
        assert!(!should_capture_geometry(false, false, false));
        assert!(!should_capture_geometry(true, false, true));
    }
}
