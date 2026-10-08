//! 鼠标穿透的**判定**部分 —— 纯状态机，不碰窗口 API。
//!
//! # 为什么判定必须抽出来
//!
//! 穿透态下前端收不到 `mousemove`（事件已经被穿到下层窗口去了），
//! 所以"现在鼠标在不在控制区里"只能在**后端**判断。而后端判断要么轮询、
//! 要么上全局鼠标钩子 —— 两条路都很难在真实环境里反复验证。
//!
//! 把"该不该切换"抽成纯函数之后：轮询周期、边界像素、负坐标、防抖
//! 全都能用单测钉住，留在 `mod.rs` 里的只剩「读一次光标位置 → 调一次 API」。
//!
//! # 迟滞（hysteresis）的两条规则
//!
//! 朴素做法是「光标在控制区 → 可点；否则 → 穿透」。它在真实使用里必然出问题：
//!
//! 1. **点不到按钮**：光标从稿件区快速移向控制栏时，轮询还没跟上，
//!    用户已经按下去了 —— 那一下点会穿透到下层应用（可能点掉了 PPT 的放映）。
//! 2. **反复闪烁**：光标停在控制区边界上时，穿透状态每帧翻转，
//!    `WS_EX_TRANSPARENT` 的频繁切换会让画面抖。
//!
//! 所以用 `APPROACH_PAD` + `DWELL_TICKS` 两个参数做迟滞：
//!
//! * 进入**放大后**的控制区 ⇒ **立刻**变可点（宁可提前一点，也别漏掉点击）
//! * 离开放大后的控制区 ⇒ 还要连续 `DWELL_TICKS` 次都在外面，才切成穿透
//!   （避免擦边抖动）
//!
//! 代价是控制区外一圈 `APPROACH_PAD` 宽的环里点击不会穿透。
//! 这是**有意为之**的取舍：那圈里误判成"可点"只是浪费一次点击，
//! 误判成"穿透"却可能真的误操作到桌面上别的东西。

use super::geometry::Rect;

/// 「接近环」宽度（屏幕物理像素）。
///
/// 40ms 一个 tick 时，鼠标快速移动（约 1000 px/s）在这段时间里走 40px，
/// 所以 24px 的提前量大致能覆盖"手比轮询快"的常见情形。
/// 调大 → 更好点，但能穿透的区域变小。
pub const APPROACH_PAD: f64 = 24.0;

/// 离开接近环后还要连续几次在外面，才切成穿透。
///
/// 3 个 tick ≈ 120ms。这个量级足以吸收手抖与边界擦过，
/// 又短到用户"把鼠标甩到桌面上"时感知不到延迟。
pub const DWELL_TICKS: u32 = 3;

/// 轮询周期（毫秒）。
///
/// 40ms 是"人眼几乎不可感知 + 一次 `GetCursorPos` 级别的开销可以忽略"之间的折中。
/// 起测值来自方案里的 60ms，实测后收紧到 40ms 以改善"点不到按钮"的窗口期。
pub const TICK_MS: u64 = 40;

/// 判定所需的状态。只有一个 bool 和一个计数器 —— 刻意不做成结构体套结构体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZoneState {
    /// 窗口当前是否处于穿透态。
    pub through: bool,
    /// 已经连续多少次判定为"在接近环之外"。
    pub outside_ticks: u32,
}

/// 判定一次。返回 `Some(新状态)` 表示**需要调用窗口 API**，`None` 表示维持现状。
///
/// 返回值刻意用 `Option<bool>` 而不是 `bool`：调用方只在 `Some` 时去碰窗口，
/// 这样「没有变化就不调 API」这件事是**类型层面保证**的，
/// 而不是靠调用方记得加 `if`。频繁切 `WS_EX_TRANSPARENT` 正是要避免的事。
///
/// # 为什么收的是**一组**矩形，而不是一个
///
/// ★★ 这里踩过一次，值得写下来：控制栏不是一块，是**上下两块**
/// （顶栏 + 底栏）。前端如果把它们合并成一个**包围盒**再上报，
/// 得到的矩形恰好覆盖整窗 —— 于是在"控制区外"这件事**永远不成立**，
/// 穿透一次都不会触发。界面上完全看不出来，只有拿 hit-test 去量才发现。
/// 所以这里的入参是切片，语义是「光标落在其中**任意一块**里都算可点」。
pub fn next(zones: &[Rect], cursor: (f64, f64), state: ZoneState) -> (ZoneState, Option<bool>) {
    // 处在任意一块的接近环内（含控制区本身）都算"靠近"
    let near = zones
        .iter()
        .any(|z| z.contains(cursor) || z.inflate(APPROACH_PAD).contains(cursor));

    if near {
        // 已经可点就什么都不做 —— 不要把同一个 false 反复写下去
        let change = if state.through { Some(false) } else { None };
        return (
            ZoneState {
                through: false,
                outside_ticks: 0,
            },
            change,
        );
    }

    let outside_ticks = state.outside_ticks.saturating_add(1);
    if !state.through && outside_ticks >= DWELL_TICKS {
        (
            ZoneState {
                through: true,
                outside_ticks,
            },
            Some(true),
        )
    } else {
        (
            ZoneState {
                through: state.through,
                outside_ticks,
            },
            None,
        )
    }
}

/// 一次上报里最多接受多少块矩形。
///
/// 超过它基本只有一种来路：DOM 里某层容器被整片标上了 `data-interactive`。
/// 那种情况下我们没法对"可交互区到底在哪"做出可靠判断，
/// 于是**宁可退回"整窗可点"** —— 让窗口挡一下视线是可接受的，
/// 让它变成一个点不动的幽灵不是。
pub const MAX_ZONES: usize = 32;

/// 把前端上报的一批矩形净化为可判定的列表。
///
/// 返回 `None` 表示**这份上报不可信**，调用方应退回「整窗可交互」（即保持可点）。
/// 判定"不可信"而不是"跳过坏的那几块"，也是同一个取向：
/// 一旦混进一个 `NaN`，`contains` 会全判 false ⇒ 整窗都被判成"控制区外"
/// ⇒ 窗口立刻变成穿透态点不动。这种数据错误必须在**进判定之前**拦住。
pub fn sanitize_zones(raw: &[[f64; 4]]) -> Option<Vec<Rect>> {
    if raw.is_empty() || raw.len() > MAX_ZONES {
        return None;
    }
    let mut out = Vec::with_capacity(raw.len());
    for a in raw {
        if !a.iter().all(|v| v.is_finite()) {
            return None;
        }
        // 零尺寸/负尺寸的块不可能来自 getBoundingClientRect（前端已经滤过），
        // 出现即说明数据来路有问题 —— 同样按不可信处理。
        if a[2] <= 0.0 || a[3] <= 0.0 {
            return None;
        }
        out.push(Rect::new(a[0], a[1], a[2], a[3]));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 展开态：**上下两条**控制栏（顶栏 + 底栏），中间那片是"点得到下面"的稿件区。
    ///
    /// 这个形状是全部用例的共同前提，因为它就是真机上的形状 ——
    /// 而"把两块合成一块"正是踩过的坑（见 `next` 的文档）。
    fn zones() -> Vec<Rect> {
        vec![
            Rect::new(0.0, 0.0, 560.0, 40.0),   // 顶栏
            Rect::new(0.0, 320.0, 560.0, 40.0), // 底栏
        ]
    }

    /// 只有一条整宽控制栏的形状（收起态 / 单栏布局）。
    fn one_bar() -> Vec<Rect> {
        vec![Rect::new(0.0, 0.0, 560.0, 40.0)]
    }

    /// 从窗口内部一点（远离控制栏）连续跑 n 次判定，返回触发过的状态序列。
    fn run(start: ZoneState, cursor: (f64, f64), n: u32) -> (ZoneState, Vec<bool>) {
        let mut st = start;
        let mut fired = Vec::new();
        for _ in 0..n {
            let (s, v) = next(&zones(), cursor, st);
            st = s;
            if let Some(x) = v {
                fired.push(x);
            }
        }
        (st, fired)
    }

    #[test]
    fn cursor_on_control_bar_is_immediately_clickable() {
        let st = ZoneState {
            through: true,
            outside_ticks: 7,
        };
        let (s, fired) = next(&zones(), (200.0, 20.0), st);
        assert_eq!(fired, Some(false), "进入控制区必须立刻恢复可点，不能等");
        assert!(!s.through);
        assert_eq!(s.outside_ticks, 0);
    }

    #[test]
    fn entering_approach_ring_also_restores_clickable() {
        // 控制栏正上方 20px（在 24px 的接近环内）—— 提前量生效
        let st = ZoneState {
            through: true,
            outside_ticks: 0,
        };
        let (_, fired) = next(&zones(), (200.0, 60.0), st);
        assert_eq!(fired, Some(false), "接近环内就该提前变可点，这是给慢速轮询留的余量");
    }

    #[test]
    fn far_away_needs_dwell_before_going_through() {
        let far = (200.0, 500.0);
        let st = ZoneState::default();

        // 前 DWELL_TICKS-1 次都不该触发
        let (s, fired) = run(st, far, DWELL_TICKS - 1);
        assert!(fired.is_empty(), "还没到停留阈值就不该切，否则擦边会抖");
        assert!(!s.through);

        let (s, fired) = next(&zones(), far, s);
        assert_eq!(fired, Some(true), "第 DWELL_TICKS 次才切穿透");
        assert!(s.through);
    }

    #[test]
    fn once_through_it_stops_repeating_the_call() {
        let far = (200.0, 500.0);
        let (s, fired) = run(ZoneState::default(), far, DWELL_TICKS);
        assert_eq!(fired, vec![true]);

        // 已经是穿透态，接下来多少次都不该再调 API
        let (_, fired) = run(s, far, 20);
        assert!(fired.is_empty(), "没有变化就不要反复写 WS_EX_TRANSPARENT");
    }

    #[test]
    fn a_brief_pass_through_the_ring_does_not_flip_state() {
        let far = (200.0, 500.0);
        let mut st = ZoneState::default();

        // 攒两次"在外面"
        (st, _) = next(&zones(), far, st);
        (st, _) = next(&zones(), far, st);

        // 光标擦过接近环
        let (s, fired) = next(&zones(), (200.0, 60.0), st);
        assert_eq!(fired, None, "本来就可点，擦过不该产生任何调用");
        assert_eq!(s.outside_ticks, 0, "计数器必须归零");

        // 再出去一次 —— 因为归零了，不该马上切穿透
        let (_, fired) = next(&zones(), far, s);
        assert!(fired.is_none());
    }

    #[test]
    fn leaving_while_through_does_not_re_trigger() {
        let far = (200.0, 500.0);
        let st = ZoneState {
            through: true,
            outside_ticks: 99,
        };
        let (s, fired) = next(&zones(), far, st);
        assert!(fired.is_none());
        assert!(s.through);
    }

    #[test]
    fn works_with_negative_coordinates_on_a_left_side_monitor() {
        // 副屏在主屏左侧：控制栏的屏幕坐标是负的。
        // 这里用整块副屏当"控制区"（收起态就是这种形态），验证坐标换算不受原点影响。
        let secondary = vec![Rect::new(-1920.0, 0.0, 1920.0, 40.0)];

        let (s, fired) = next(&secondary, (-960.0, 20.0), ZoneState::default());
        assert_eq!(fired, None, "在区内、本来就可点 → 无需调用");
        assert!(!s.through);

        let st = ZoneState {
            through: true,
            outside_ticks: 0,
        };
        let (_, fired) = next(&secondary, (-960.0, 20.0), st);
        assert_eq!(fired, Some(false), "负坐标的区同样要能恢复可点");
    }

    #[test]
    fn boundary_pixels_belong_to_the_clickable_side() {
        // 左闭右开：右边界那一个像素算"区外"。判定方向必须是
        // 「宁可判成区外一点点，也不要多占一块可点区域」——
        // 因为多占的代价是穿透区域缩小，而缩小是安全的那个方向。
        let (s, _) = next(&zones(), (560.0, 20.0), ZoneState::default());
        assert!(!s.through, "还没到停留阈值，仍是可点");
        // 但它落在接近环里（560+24 内），所以计数器应当归零
        assert_eq!(s.outside_ticks, 0);
    }

    #[test]
    fn collapsed_window_uses_the_whole_window_as_zone() {
        // 收起态整窗就是一条控制栏，所以整窗都可点 → 稳定态永远不该切穿透
        let whole = one_bar();

        let mut st = ZoneState::default();
        for _ in 0..50 {
            let (s, v) = next(&whole, (280.0, 20.0), st);
            st = s;
            assert!(v.is_none(), "收起态整窗可点，不该有任何切换");
        }
        assert!(!st.through);

        // 但「万一当前是穿透态」（例如刚切进收起态、状态还没收敛），
        // 光标落在窗内必须恢复一次可点 —— 这是唯一允许的切换。
        let (s, v) = next(
            &whole,
            (280.0, 20.0),
            ZoneState {
                through: true,
                outside_ticks: 0,
            },
        );
        assert_eq!(v, Some(false), "进收起态后必须能恢复可点");
        assert!(!s.through);
    }

    // -----------------------------------------------------------------------
    // ★ 回归：这一组是为了「上下两条控制栏」那个真机事故固化下来的
    // -----------------------------------------------------------------------

    #[test]
    fn two_bars_do_not_make_the_whole_window_clickable() {
        // 展开态的真实形状：顶栏 + 底栏，中间是稿件区。
        // 只要把两块**并成一个包围盒**，这个包围盒就恰好是整窗 ——
        // 于是"在控制区外"永远不成立、穿透一次都不触发。
        // 这条用例就是钉死这件事：中间的稿件区**必须**判成区外。
        let middle = (280.0, 200.0);

        let (s, fired) = run(ZoneState::default(), middle, DWELL_TICKS);
        assert_eq!(
            fired,
            vec![true],
            "稿件区在两条栏中间：不能被任一栏的包围盒吃掉，必须能切穿透"
        );
        assert!(s.through);
    }

    #[test]
    fn union_of_the_two_bars_would_be_the_whole_window() {
        // 把「为什么不能合并」写成一条断言，而不是只写在注释里：
        // 两块合并出来的包围盒确实覆盖了窗口中部 —— 证明上面那条用例
        // 不是假想的风险，而是这个 bug 的精确刻画。
        let z = zones();
        let union = {
            let x1 = z.iter().map(|r| r.x).fold(f64::INFINITY, f64::min);
            let y1 = z.iter().map(|r| r.y).fold(f64::INFINITY, f64::min);
            let x2 = z.iter().map(|r| r.x + r.w).fold(f64::NEG_INFINITY, f64::max);
            let y2 = z.iter().map(|r| r.y + r.h).fold(f64::NEG_INFINITY, f64::max);
            Rect::new(x1, y1, x2 - x1, y2 - y1)
        };
        assert!(
            union.contains((280.0, 200.0)),
            "合并后的包围盒会覆盖稿件区 —— 这正是真机上穿透失效的原因"
        );
        // 而且合并版会把窗口底部那一整条也变成"可点"（本该是穿透区）
        assert!(union.contains((280.0, 330.0)));
    }

    #[test]
    fn cursor_on_either_of_the_two_bars_restores_clickable() {
        // 顶栏和底栏都必须是可点的。只认其中一条，用户就会发现
        // "底栏那几个按钮点不动" —— 而且只在穿透态下出现，极难复现。
        let z = zones();
        for (label, p) in [("顶栏", (280.0, 20.0)), ("底栏", (280.0, 340.0))] {
            let st = ZoneState {
                through: true,
                outside_ticks: 9,
            };
            let (s, fired) = next(&z, p, st);
            assert_eq!(fired, Some(false), "{label}上必须能立刻恢复可点");
            assert!(!s.through);
            assert_eq!(s.outside_ticks, 0, "{label}上计数器要归零");
        }
    }

    #[test]
    fn leaving_both_bars_eventually_goes_through() {
        // 从顶栏慢慢挪到稿件区：攒够停留次数后切穿透。
        let z = zones();
        let mut st = ZoneState::default();
        let mut fired = Vec::new();
        for i in 0..(DWELL_TICKS + 3) {
            // 从顶栏下沿往下走，逐步离开接近环
            let y = 40.0 + 10.0 * i as f64;
            let (s, v) = next(&z, (280.0, y), st);
            st = s;
            if let Some(x) = v {
                fired.push(x);
            }
        }
        assert_eq!(fired, vec![true], "离开两条栏之后应当切穿透，且只切一次");
        assert!(st.through);
    }

    #[test]
    fn dirty_reports_are_rejected_so_the_window_can_never_lock_up() {
        // NaN 混进来会让 contains 全判 false ⇒ 整窗被判成"区外" ⇒
        // 窗口立刻变穿透、点不动。必须在进判定之前就整份丢掉。
        assert!(sanitize_zones(&[[0.0, 0.0, f64::NAN, 40.0]]).is_none());
        assert!(sanitize_zones(&[[f64::INFINITY, 0.0, 10.0, 40.0]]).is_none());
        // 零/负尺寸不可能来自 getBoundingClientRect
        assert!(sanitize_zones(&[[0.0, 0.0, 0.0, 40.0]]).is_none());
        assert!(sanitize_zones(&[[0.0, 0.0, 10.0, -1.0]]).is_none());
        // 空上报 ⇒ 不可信（调用方按"整窗可点"处理）
        assert!(sanitize_zones(&[]).is_none());
        // 块数爆掉 ⇒ 同样不可信，而不是"取前 32 块"
        let many: Vec<[f64; 4]> = (0..(MAX_ZONES + 1))
            .map(|i| [i as f64, 0.0, 10.0, 10.0])
            .collect();
        assert!(sanitize_zones(&many).is_none());

        // 正常上报要原样通过
        let good = sanitize_zones(&[[0.0, 0.0, 560.0, 48.0], [0.0, 330.0, 560.0, 30.0]]);
        let good = good.expect("正常数据不该被拒");
        assert_eq!(good.len(), 2);
        assert_eq!(good[1].y, 330.0);
    }
}
