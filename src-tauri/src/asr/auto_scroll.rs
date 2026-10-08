//! 匀速自动滚动：**不采集音频**，按固定语速把光标推下去。
//!
//! # 为什么它值得是一个引擎，而不是"前端挂个定时器"
//!
//! 因为它要在**窗口不在前台**的时候照常推进 —— 而这正是提词器的常态：
//! 讲者在操作 PPT，提词层只是浮在最上面。浏览器对后台页面的定时器会降频到
//! 最低 1 秒一次（极端情况下直接冻结），前端 `setInterval` 在这个场景里会
//! 明显变慢、越讲越对不上，而且这种"变慢"没有任何报错，只是手感不对。
//! 放在后端的工作线程里就没有这个问题。顺带还省掉"每个字一次 IPC"的开销。
//!
//! # 它推进的是**位置**，不是文本
//!
//! 语音引擎的处境是"我知道听到了什么，但不知道这对应稿子里的哪里"，
//! 所以要靠 `Aligner` 做匹配。匀速滚动的处境正好相反：
//! **位置是它自己按时间算出来的**，确定无疑。
//! 所以它走 [`AsrUpdate::seek`] 那条路直接落位置，不经过文本匹配 ——
//! 把一个已知的答案拿去猜一遍，除了多一次猜错的机会，没有任何好处。
//!
//! # 它与「纯手动」的关系
//!
//! 两者都属于"不用语音"，区别只在**谁决定往下走**：时钟，还是人的手。
//! 所以设置页把它们并排成三个答案（语音跟随 / 匀速滚动 / 纯手动），
//! 而不是"自动滚动开关 + 一个叫引擎的下拉框" —— 后者会给出
//! 「自动滚动＝开，引擎＝本地离线」这种自相矛盾的组合。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{AsrEngine, AsrUpdate, UpdateSink};

/// 速度区间（字/分）。
///
/// 下限 60：再慢就是"一个字一个字往外蹦"，那种场景不如用「上一句/下一句」。
/// 上限 600：中文播报的极限大约在 400 上下，这里留足余量 ——
/// 再往上就不是"讲稿子"，是滑杆没控制住。
pub const WPM_RANGE: (f32, f32) = (60.0, 600.0);

/// 出厂速度。
///
/// 刻意取与「参考语速」同一个数：两根滑杆的出厂值一样，用户第一眼就明白
/// 它们说的是同一件事（"我大概讲多快"），只是各管一种推进方式。
pub const DEFAULT_WPM: f32 = 300.0;

/// 检查间隔。
///
/// ★ 它**不参与累积**：位置每次都由"基准时刻 + 总经过时间"现算，
/// 所以调度晚了一次不会让后面的位置整体慢下来（见 [`Rate::pos_at`]）。
/// 这个值只决定"多久看一眼、以及暂停最迟多久响应"。
const TICK: Duration = Duration::from_millis(100);

/// 把速度夹进合法区间。
pub fn clamp_wpm(wpm: f32) -> f32 {
    if wpm.is_finite() {
        wpm.clamp(WPM_RANGE.0, WPM_RANGE.1)
    } else {
        DEFAULT_WPM
    }
}

/// 推进基准 —— 整个自动滚动里唯一需要算对的东西。
///
/// 它被刻意做成**不含线程、不含 IO 的纯结构**：`pos_at` 是纯函数，
/// 单测里可以直接喂两个时间点核对"300 字/分跑 10 秒正好 50 字"。
/// 这类"按时间推进"的代码最容易出的错是**累积误差**（每次 `+= 1`，
/// 调度抖一下就越攒越多），把位置表示成"基准 + 经过时间 × 速度"就没得攒。
#[derive(Debug, Clone)]
pub struct Rate {
    /// 基准时刻的位置（字符索引）。
    pos0: usize,
    /// 基准时刻。显式传入而不是内部取 `now()`，好让单测控制时间。
    t0: Instant,
    /// 速度（字/分）。
    wpm: f32,
}

impl Rate {
    pub fn new(pos0: usize, t0: Instant, wpm: f32) -> Self {
        Self {
            pos0,
            t0,
            wpm: clamp_wpm(wpm),
        }
    }

    /// 这个时刻应该读到第几个字（可能是小数）。
    pub fn pos_at(&self, now: Instant) -> f64 {
        let secs = now.saturating_duration_since(self.t0).as_secs_f64();
        self.pos0 as f64 + secs * self.wpm as f64 / 60.0
    }

    pub fn wpm(&self) -> f32 {
        self.wpm
    }

    /// 换一个新速度，**且不让光标跳**。
    ///
    /// ★★ 这是运行中调速能不能用的关键。位置是
    /// `基准位置 + 已过时间 × 速度` 算出来的 —— 直接把 `wpm` 改掉，
    /// 等于把**过去那段时间**也按新速度重算了一遍：300→500 会让光标
    /// 瞬间往前窜一大段，用户看到的是"我调个速度，稿子跳了"。
    /// 所以先把当前位置固化下来当新基准，再换速度。
    pub fn rebase(&mut self, now: Instant, wpm: f32) {
        let here = self.pos_at(now).max(0.0).round() as usize;
        self.pos0 = here;
        self.t0 = now;
        self.wpm = clamp_wpm(wpm);
    }

    /// 换一个起点并重设时钟。`start` 时用。
    pub fn reset_to(&mut self, now: Instant, pos: usize) {
        self.pos0 = pos;
        self.t0 = now;
    }
}

/// 匀速滚动引擎。
pub struct AutoScrollAsr {
    running: bool,
    /// 稿件总字数 —— 推到这里就停。
    total: usize,
    rate: Arc<Mutex<Rate>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AutoScrollAsr {
    /// `start` 是**起点**（字符索引），通常就是当前光标 ——
    /// 「暂停一下、回头接着念」要的就是这个。
    pub fn new(total: usize, start: usize, wpm: f32) -> Self {
        let start = start.min(total);
        Self {
            running: false,
            total,
            rate: Arc::new(Mutex::new(Rate::new(start, Instant::now(), wpm))),
            // 初始为 true：线程还没起，"停"才是安全态。
            stop: Arc::new(AtomicBool::new(true)),
            handle: None,
        }
    }

    /// 运行中调速。位置不跳（见 [`Rate::rebase`]）。
    ///
    /// 取 `&self` 而不是 `&mut self`：它要能在 `Box<dyn AsrEngine>` 后面
    /// 被直接调用（那时拿到的是可变引用，但调用点上不需要再关心）。
    pub fn set_wpm(&self, wpm: f32) {
        if let Ok(mut r) = self.rate.lock() {
            r.rebase(Instant::now(), wpm);
        }
    }
}

impl AsrEngine for AutoScrollAsr {
    fn name(&self) -> &'static str {
        "匀速滚动"
    }

    fn is_running(&self) -> bool {
        self.running && self.handle.as_ref().map_or(false, |h| !h.is_finished())
    }

    fn start(&mut self, sink: UpdateSink) -> Result<(), String> {
        if self.total == 0 {
            return Err("稿件是空的，没有可以滚动的内容。".into());
        }

        {
            let mut r = self.rate.lock().map_err(|_| "速度锁异常".to_string())?;
            let here = r.pos_at(Instant::now()).max(0.0).round() as usize;
            // ★ 「讲完了再按一次开始」＝ 从头再来一遍。
            //   不这么处理的话，起点就落在末尾，用户按下开始会看到
            //   **什么都不动** —— 而那是"卡住了"的观感，不是"重播"的观感。
            //   正常的中途暂停/继续不受影响（位置在中间，走上面那个分支）。
            let from = if here + 1 >= self.total { 0 } else { here };
            // 时钟也要重置：引擎可能是几分钟前建的（改过配置、换过稿子、
            // 上一次讲完停在那里），不重置的话第一帧就会按"已经过去几分钟"
            // 直接跳到稿子中间。
            r.reset_to(Instant::now(), from);
        }

        let rate = self.rate.clone();
        let stop = self.stop.clone();
        let total = self.total;
        stop.store(false, Ordering::Relaxed);

        self.handle = Some(thread::spawn(move || {
            // 上一次推出去的位置。100ms 的检查间隔比"每字一格"密
            // （300 字/分时每字 200ms），不筛的话有一半的推送都在重复
            // 同一个位置 —— 那既浪费 CPU，也会让前端反复重算同一帧。
            let mut last = usize::MAX;
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let pos = match rate.lock() {
                    Ok(r) => r.pos_at(Instant::now()).max(0.0).floor() as usize,
                    // 锁中毒说明别处已经炸了，这里空转没有意义。
                    Err(_) => break,
                };
                let pos = pos.min(total);
                if pos != last {
                    last = pos;
                    let done = pos >= total;
                    sink(AsrUpdate::seek_to(pos));
                    if done {
                        break;
                    }
                }
                thread::sleep(TICK);
            }
        }));

        self.running = true;
        log::info!(
            "[auto] 匀速滚动已启动：{} 字/分，稿长 {} 字",
            self.rate.lock().map(|r| r.wpm()).unwrap_or(0.0).round(),
            self.total
        );
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            // join 而不是放任它自己收尾：这个函数返回之后，
            // `stop_asr` 就认为引擎已经停了，而线程若还在推位置，
            // 光标会在"已停止"之后继续走一小段。最多等一个 TICK。
            let _ = h.join();
        }
        self.running = false;
    }

    /// 运行中调速。语音引擎没有"速度"这个旋钮，所以 trait 上的默认实现
    /// 是空操作 —— 调用方不必先判断"当前是不是自动滚动"。
    fn set_rate(&mut self, wpm: f32) {
        self.set_wpm(wpm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 位置必须随**真实经过时间**走，而不是"每 tick 加一格"。
    ///
    /// 钉的是累积误差：按 tick 累加的实现，只要调度晚一次，
    /// 后面的位置就整体偏了，而且再也追不回来。
    #[test]
    fn rate_advances_by_elapsed_time_not_by_ticks() {
        let t0 = Instant::now();
        let r = Rate::new(0, t0, 300.0);

        // 300 字/分 = 5 字/秒。
        assert!((r.pos_at(t0) - 0.0).abs() < 1e-9);
        assert!((r.pos_at(t0 + Duration::from_secs(1)) - 5.0).abs() < 1e-6);
        assert!((r.pos_at(t0 + Duration::from_secs(10)) - 50.0).abs() < 1e-6);
        // 隔很久才来看一眼（模拟调度被拖长），位置照样是对的。
        assert!((r.pos_at(t0 + Duration::from_secs(120)) - 600.0).abs() < 1e-6);
    }

    /// ★★ 调速不能让光标跳 —— 这是运行中调速可用不可用的分界。
    #[test]
    fn rebase_keeps_the_cursor_where_it_was() {
        let t0 = Instant::now();
        let mut r = Rate::new(0, t0, 300.0);

        let now = t0 + Duration::from_secs(10);
        let before = r.pos_at(now);
        assert!((before - 50.0).abs() < 1e-6);

        // 提速一倍：过去那 10 秒绝不能被按新速度重算（那会变成 100）。
        r.rebase(now, 600.0);
        assert!(
            (r.pos_at(now) - before).abs() < 1.0,
            "调速瞬间位置应该基本不动，实测 {} → {}",
            before,
            r.pos_at(now)
        );

        // 换速度之后按**新速度**继续走：再过 10 秒是 50 + 100。
        assert!((r.pos_at(now + Duration::from_secs(10)) - before - 100.0).abs() < 1.0);
    }

    #[test]
    fn wpm_is_clamped_instead_of_trusted() {
        assert_eq!(clamp_wpm(0.0), WPM_RANGE.0);
        assert_eq!(clamp_wpm(-5.0), WPM_RANGE.0);
        assert_eq!(clamp_wpm(99999.0), WPM_RANGE.1);
        // 非有限值不能让滑杆把引擎带进 NaN，退到出厂值。
        assert_eq!(clamp_wpm(f32::NAN), DEFAULT_WPM);
        assert_eq!(clamp_wpm(f32::INFINITY), DEFAULT_WPM);

        let t0 = Instant::now();
        let mut r = Rate::new(0, t0, 1.0);
        assert_eq!(r.wpm(), WPM_RANGE.0);
        r.rebase(t0, 1e9);
        assert_eq!(r.wpm(), WPM_RANGE.1);
    }

    /// 起点越界要收敛在稿内，不能让光标落在稿子外面。
    #[test]
    fn start_position_is_clamped_to_script() {
        let e = AutoScrollAsr::new(100, 999, 300.0);
        // 起点被夹到末尾 ⇒ `start` 会把它当成"讲完了再来一遍"，从头开始。
        assert!(e.total == 100);
        let r = e.rate.lock().unwrap();
        assert!(r.pos0 <= 100);
    }
}
