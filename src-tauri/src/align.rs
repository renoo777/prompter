//! 语音对位引擎：把「识别到的文本」映射成「稿件里的位置」。
//!
//! # 问题定义
//!
//! 输入是语音识别的**增量假设**（一句话会随着识别推进不断被修正），
//! 输出是稿件里「已经读到哪了」的一个位置。
//!
//! # 为什么不用「逐词编辑距离」这类常见做法
//!
//! 因为 ASR 的输出有两个和文本 diff 完全不同的特性：
//!
//! 1. **会回头改**。同一句话在一秒内可能输出成 `第三季度营收` → `第三季度营收增长` →
//!    `第三季营收增长`。纯编辑距离会把这些修正当成真实位置变化，光标就会来回抖。
//! 2. **长度不对等**。识别输出里的数字、标点、口语填充词（嗯、那个）都不在稿子里。
//!
//! 所以这里的做法是：
//!
//! * 只拿识别结果**末尾**一小段（`probe`）去匹配 —— 末尾才是"刚刚说到的内容"，
//!   前面那部分上一轮已经用过了；
//! * 在稿件的一个**受限窗口**内滑动搜索（默认只允许往后 240 字、往前 40 字），
//!   天然抑制"跳到后面某个相似段落"这类误判；
//! * 用 **LCS 比率**而不是编辑距离打分 —— LCS 对"多出来的字符"和"少掉的字符"
//!   同样宽容，正适合处理填充词和数字差异；
//! * 加一道**单调性约束**：位置不可倒退超过窗口下界，且单次推进有上限。
//!
//! # 冷启动与重新对齐
//!
//! 连续多次匹配失败（用户中途跳了一段、或者换了个地方开始讲）时，
//! 会自动放开窗口做一次**全稿搜索**，找到最可能的落点后重新锚定。

use crate::script;

/// 对位参数。默认值针对中文汇报语速（约 200~260 字/分钟）调过。
#[derive(Debug, Clone, Copy)]
pub struct AlignConfig {
    /// 每次匹配取识别结果末尾多少个字符作为探针。
    ///
    /// 太小（<8）会频繁误匹配短词；太大（>30）会拖慢响应，且用户改口时反应迟钝。
    pub probe_len: usize,
    /// 允许向前搜索的字数 —— 决定"跟得上说多快"。
    pub forward: usize,
    /// 允许向后搜索的字数 —— 决定"能容忍多大的识别回退"。
    pub backward: usize,
    /// 用户设定的「跳读范围」（句）。
    ///
    /// 这是**真相**，`forward` / `backward` 是由它派生出来的（见 [`jump_window`]）。
    /// 之所以两份都留着而不是"每次用时现算"：搜索路径（`update_normalized`）
    /// 在 ASR 回调里每几百毫秒跑一次，那里不该出现任何一次乘法之外的东西；
    /// 而两个字段同进同出（只由 [`Aligner::set_jump_range`] 写），不会漂。
    pub jump_sentences: u32,
    /// 最低匹配得分，低于此值视为"没对上"，不移动光标。
    ///
    /// 0.38 取得偏松，原因：稿件和口述在数字上**必然**有差异
    /// （`50%` vs `百分之五十`、`3.5倍` vs `三点五倍`），而汉字数字不能归一化掉
    /// （见 `script` 模块顶部关于「第一季度 / 第二季度」的说明）。
    /// 这类差异会实打实地拉低得分，阈值定高会把正常句子误判成"对不上"。
    ///
    /// 放宽的风险由另外三重机制兜住：**受限窗口**（只往前 240 字）、
    /// **位置惩罚**（离锚点越远扣分越多）、**单调约束**。
    /// 三者叠加后，低分误匹配几乎不可能跳到远处 —— 想跳远就得先扣掉足够多的分。
    pub min_score: f32,
    /// 平滑系数：新位置 = 锚点 + (目标 - 锚点) × alpha。
    /// 1.0 = 不平滑（光标跟着识别跳），0.5 = 明显滞后。提词器要"跟得住"，取偏大值。
    pub alpha: f32,
    /// 连续失败多少次后放开窗口做全稿搜索。
    pub realign_after_misses: u32,
}

/// `AlignConfig::min_score` 的默认值，也是设置里「跟读灵敏度」滑杆的出厂位置。
///
/// 抽成常量是为了让**设置页、配置默认值、引擎默认值**三处引用同一个数 ——
/// 之前这个数分散在两处字面量里，改一处忘一处就会出现
/// 「设置显示 0.38、引擎实际用 0.35」这种查不出来的偏差。
pub const DEFAULT_MIN_SCORE: f32 = 0.38;

/// 跟读灵敏度滑杆的取值区间。
///
/// 下限 0.20：再低就几乎"什么都能匹配上"，光标会乱跳；
/// 上限 0.60：再高连标准普通话都会被判成"没对上"，等于关掉了跟读。
pub const MIN_SCORE_RANGE: (f32, f32) = (0.20, 0.60);

/// 「跳读范围」滑杆的出厂位置（单位：句）。
///
/// 20 句 × [`CHARS_PER_SENTENCE`] = **240 字，正好是 `forward` 的历史默认值**
/// （240 / 40 与 `AlignConfig::default()` 里那对数字逐字相同）。
/// 也就是说这条滑杆一出生就落在"引擎本来就在用的那个数"上：
/// 把它接出来只是让用户**能改**，不是换了一套行为 ——
/// 出厂状态下跟读手感与此前完全一致。
pub const DEFAULT_JUMP_RANGE: u32 = 20;

/// 「跳读范围」滑杆的取值区间（单位：句）。
///
/// 下限 1 句（12 字）：再小就等于"只认眼前这几个字"，
/// 用户稍微跳一段就会落到全稿搜索的兜底路径上（`realign_after_misses`），
/// 表现是"一跳就乱"。
/// 上限 40 句（480 字）：再大跟全稿搜索没什么区别，滑杆就失去意义了。
pub const JUMP_RANGE_RANGE: (u32, u32) = (1, 40);

/// 换算用的"一句多少字"。
///
/// 用一个**常量**而不是"当前稿件的平均句长"，是为了让这条滑杆的手感
/// 不随稿件变化：同一份设置在任何稿子上都应对应同样的行为。
/// 若按稿件现算，换一篇稿子用户会发现"同样是 20，这篇跟得紧那篇跟得松"，
/// 而屏幕上没有任何东西能解释这个差别 —— 那比不给这个滑杆更糟。
///
/// 12 取自中文口播句的常见长度中值（短句 8~10、长句 15~20）。
pub const CHARS_PER_SENTENCE: usize = 12;

/// 把「跳读范围（句）」换算成对位引擎的搜索窗口（向前 / 向后，单位：字）。
///
/// 抽成纯函数的理由很实际：这条换算一旦错，症状是"跟读偶尔跳到很远的地方"，
/// 在真机上既难复现、更难归因 —— 用单测钉死比现场猜便宜得多。
pub fn jump_window(sentences: u32) -> (usize, usize) {
    let n = sentences.clamp(JUMP_RANGE_RANGE.0, JUMP_RANGE_RANGE.1) as usize;
    let forward = n * CHARS_PER_SENTENCE;
    // 向后只留向前量的 1/6，与默认的 240:40 同配比 ——
    // 识别回退的幅度本就远小于前跳（口误、重复半句 vs 整段跳过）。
    // 兜底给一句的量：`forward / 6` 在 1 句时会算成 2 字，太紧。
    let backward = (forward / 6).max(CHARS_PER_SENTENCE);
    (forward, backward)
}

impl Default for AlignConfig {
    fn default() -> Self {
        // 窗口从「跳读范围」推出来 —— 出厂值 20 句恰好等于历史的 240/40 字，
        // 所以这两行不改变任何既有行为，只是把那个数**收到一处**。
        let (forward, backward) = jump_window(DEFAULT_JUMP_RANGE);
        Self {
            probe_len: 16,
            forward,
            backward,
            jump_sentences: DEFAULT_JUMP_RANGE,
            min_score: DEFAULT_MIN_SCORE,
            alpha: 0.7,
            realign_after_misses: 6,
        }
    }
}

/// 一次对齐的结果。
#[derive(Debug, Clone, Copy)]
pub struct AlignOutcome {
    /// 稿件归一化序列里的新位置（左闭右开，指向"下一个待读字符"）。
    ///
    /// 与 `Aligner::cursor()` 同值。上层目前统一从 `Aligner` 取，
    /// 这里保留一份是为了让这个返回值**自解释** —— 调用方不需要为了拿到位置
    /// 再去持有一个 `Aligner` 的引用。
    #[allow(dead_code)]
    pub cursor: usize,
    /// 本次是否真的移动了。
    pub moved: bool,
    /// 本次匹配得分 0..1。
    pub score: f32,
    /// 连续未匹配次数 —— 前端用它提示"对不上了，请手动校准"。
    pub miss_streak: u32,
    /// 本次是否触发了全稿重对齐。
    pub realigned: bool,
}

pub struct Aligner {
    hay: Vec<char>,
    anchor: usize,
    miss_streak: u32,
    config: AlignConfig,
}

impl Aligner {
    pub fn new(hay: Vec<char>) -> Self {
        Self::with_config(hay, AlignConfig::default())
    }

    pub fn with_config(hay: Vec<char>, config: AlignConfig) -> Self {
        Self {
            hay,
            anchor: 0,
            miss_streak: 0,
            config,
        }
    }

    /// 换稿时重建。
    pub fn reset_with(&mut self, hay: Vec<char>) {
        self.hay = hay;
        self.anchor = 0;
        self.miss_streak = 0;
    }

    /// 改「跟读灵敏度」。设置页拖动滑杆时实时生效，不用重建引擎。
    ///
    /// 顺手把 `miss_streak` 清零：用户刚调灵敏度，多半正是因为
    /// "刚才一直没对上"，留着旧计数会让它在下一个 tick 直接触发全稿搜索 ——
    /// 用户看到的是"我一调它就更乱了"。
    pub fn set_min_score(&mut self, v: f32) {
        self.config.min_score = v.clamp(MIN_SCORE_RANGE.0, MIN_SCORE_RANGE.1);
        self.miss_streak = 0;
    }

    pub fn min_score(&self) -> f32 {
        self.config.min_score
    }

    /// 改「跳读范围」（单位：句）。设置页拖动滑杆时实时生效，不用重建引擎。
    ///
    /// 与 `set_min_score` 同样清零 `miss_streak`，理由也一样：
    /// 用户调它多半正因为"刚才跟丢了"，留着旧计数会让它在下一个 tick
    /// 直接触发全稿搜索 —— 用户看到的是"我一调它就更乱了"。
    ///
    /// 返回**夹取后**的实际值：界面显示的和引擎用的是同一个数。
    pub fn set_jump_range(&mut self, sentences: u32) -> u32 {
        let n = sentences.clamp(JUMP_RANGE_RANGE.0, JUMP_RANGE_RANGE.1);
        let (fwd, bwd) = jump_window(n);
        self.config.jump_sentences = n;
        self.config.forward = fwd;
        self.config.backward = bwd;
        self.miss_streak = 0;
        n
    }

    /// 当前生效的跳读范围（句）。
    pub fn jump_range(&self) -> u32 {
        self.config.jump_sentences
    }

    pub fn cursor(&self) -> usize {
        self.anchor
    }

    pub fn total(&self) -> usize {
        self.hay.len()
    }

    /// 进度 0..1。
    pub fn progress(&self) -> f32 {
        if self.hay.is_empty() {
            0.0
        } else {
            (self.anchor as f32 / self.hay.len() as f32).clamp(0.0, 1.0)
        }
    }

    /// 手动跳转到指定位置 —— 快捷键翻页、点击句子、拖动进度条都走这里。
    pub fn set_cursor(&mut self, pos: usize) {
        self.anchor = pos.min(self.hay.len());
        self.miss_streak = 0;
    }

    /// 相对移动 —— 对应「下一句 / 上一句」这类操作的单位是句子，
    /// 由上层换算成字符偏移后调用这里。
    pub fn nudge(&mut self, delta: isize) {
        let next = self.anchor as isize + delta;
        self.set_cursor(next.max(0) as usize);
    }

    /// 摄入一次识别结果。
    pub fn update(&mut self, hyp_raw: &str) -> AlignOutcome {
        let h = script::normalize(hyp_raw);
        self.update_normalized(&h)
    }

    /// 摄入已归一化的识别结果（热路径，避免重复归一化）。
    pub fn update_normalized(&mut self, h: &[char]) -> AlignOutcome {
        if h.is_empty() || self.hay.is_empty() {
            return self.outcome(false, 0.0, false);
        }

        let probe_len = self.config.probe_len.min(h.len());
        let probe = &h[h.len() - probe_len..];

        // 常规搜索：围绕锚点的受限窗口
        let lo = self.anchor.saturating_sub(self.config.backward);
        let hi = (self.anchor + self.config.forward).min(self.hay.len());
        let (best_start, best_score) = self.search_in(lo, hi, probe);

        // 窗口内找不到 → 连续失败计数；达到阈值就放开全稿重对齐
        if best_score < self.config.min_score {
            self.miss_streak += 1;
            if self.miss_streak >= self.config.realign_after_misses {
                let (s2, sc2) = self.search_in(0, self.hay.len(), probe);
                if sc2 >= self.config.min_score {
                    self.miss_streak = 0;
                    let target = s2 + probe_len;
                    self.anchor = target.min(self.hay.len());
                    return self.outcome(true, sc2, true);
                }
            }
            return self.outcome(false, best_score, false);
        }

        self.miss_streak = 0;
        let target = (best_start + probe_len).min(self.hay.len());

        let alpha = self.config.alpha.clamp(0.05, 1.0);
        let next = if target > self.anchor {
            let step = ((target - self.anchor) as f32 * alpha).round() as usize;
            // 至少推进一格，否则低 alpha 下会永远追不上
            self.anchor + step.max(1)
        } else {
            target
        };

        let next = next.min(hi.max(self.anchor)).min(self.hay.len());
        let moved = next != self.anchor;
        self.anchor = next;
        self.outcome(moved, best_score, false)
    }

    /// 在 `[lo, hi)` 内滑动探针，返回 (最佳起点, 得分)。
    ///
    /// 得分相同的情况下偏向**离锚点更近**的位置 —— 稿件里重复出现的词太多了
    /// （「我们」「增长」「第三季度」），不加这个偏好就会往后乱跳。
    fn search_in(&self, lo: usize, hi: usize, probe: &[char]) -> (usize, f32) {
        if probe.is_empty() || lo >= hi {
            return (self.anchor, 0.0);
        }
        let n = probe.len();
        // 允许窗口比探针长几个字符，容忍插入导致的错位
        let slack = 4usize;
        let mut best = (lo.min(self.anchor), 0.0f32);

        let mut s = lo;
        while s < hi {
            let end = (s + n + slack).min(self.hay.len());
            if end <= s {
                break;
            }
            let window = &self.hay[s..end];
            let score = lcs_ratio(window, probe);

            // 起点首字相符时给一点奖励。
            //
            // 没有它会出现一类很隐蔽的错位：稿件里若有多处相似句式
            // （「第一季度增长百分之十五第二季度增长百分之二十…」），
            // 窗口从中间位置切起时 LCS 同样能算出满分 —— 因为 probe 成了
            // 该窗口的子序列，起点漂到哪儿都能把字凑齐。表现就是光标
            // 稳定地偏早几个字，而且在句式重复越多的稿件里偏得越厉害。
            //
            // 用**软奖励**而不是硬性要求首字相等：识别错首字时仍能靠 LCS 兜住，
            // 硬过滤则会让那一整句直接定位失败。
            let head_bonus = if window.first() == probe.first() {
                0.10
            } else {
                0.0
            };

            // 位置惩罚：每远离锚点 1 字扣 0.0004，240 字窗口内最多约 0.1
            let dist = (s as isize - self.anchor as isize).unsigned_abs() as f32;
            let adjusted = score + head_bonus - dist * 0.0004;

            if adjusted > best.1 {
                best = (s, adjusted);
            }
            s += 1;
        }
        best
    }

    fn outcome(&self, moved: bool, score: f32, realigned: bool) -> AlignOutcome {
        AlignOutcome {
            cursor: self.anchor,
            moved,
            score,
            miss_streak: self.miss_streak,
            realigned,
        }
    }
}

/// 最长公共子序列长度 —— 滚动数组实现，空间 O(min(n,m))。
fn lcs_len(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut prev = vec![0usize; short.len() + 1];
    let mut cur = vec![0usize; short.len() + 1];

    for &lc in long {
        for (j, &sc) in short.iter().enumerate() {
            cur[j + 1] = if sc == lc {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[short.len()]
}

/// LCS 相对 `needle` 长度的比率 0..1。
///
/// 分母是 `needle`，所以它的语义是「**needle 有多少比例出现在 hay 里**」。
/// 需要「识别文本保留了基线的几成」时就这样用：`lcs_ratio(&实测, &基线)`。
///
/// 设为 `pub(crate)` 是为了让采集链路的测试复用同一套口径 ——
/// 测试里另写一份相似度算法，就会出现「测试说 90%、对位引擎却判失配」的分裂。
pub(crate) fn lcs_ratio(hay: &[char], needle: &[char]) -> f32 {
    if needle.is_empty() {
        return 0.0;
    }
    (lcs_len(hay, needle) as f32 / needle.len() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hay(s: &str) -> Vec<char> {
        script::normalize(s)
    }

    #[test]
    fn tracks_forward_progress() {
        let text = "各位好今天我要汇报的是第三季度的业务复盘情况整体来看营收实现了稳步增长";
        let mut a = Aligner::new(hay(text));
        let o1 = a.update("各位好今天我要汇报的是");
        assert!(o1.moved, "首句应能定位");
        assert!(o1.score > 0.9, "首句分应很高，实际 {}", o1.score);
        let p1 = a.cursor();

        let o2 = a.update("今天我要汇报的是第三季度的业务复盘");
        assert!(o2.moved);
        assert!(a.cursor() > p1, "光标应向前推进");
        assert!(o2.score > 0.9);
    }

    #[test]
    fn ignores_asr_backtracking() {
        let text = "整体来看营收实现了稳步增长利润端表现同样稳健";
        let mut a = Aligner::new(hay(text));
        a.update("整体来看营收实现了稳步增长");
        let p = a.cursor();
        // ASR 把「增长」改口成「增」—— 属于修正，不应让光标大幅回退
        let o = a.update("整体来看营收实现了稳步增");
        assert!(a.cursor() + 3 >= p, "小幅修正不应大幅回退，{} → {}", p, a.cursor());
        assert!(o.score > 0.8);
    }

    #[test]
    fn rejects_unrelated_speech() {
        let text = "整体来看营收实现了稳步增长利润端表现同样稳健";
        let mut a = Aligner::new(hay(text));
        a.update("整体来看营收实现了稳步增长");
        let p = a.cursor();
        // 完全无关的内容（例如旁边有人插话）
        let o = a.update("今天天气不错适合出门散步");
        assert!(!o.moved, "无关内容不应移动光标");
        assert_eq!(a.cursor(), p);
    }

    #[test]
    fn tolerates_digit_differences() {
        // 稿件写阿拉伯数字，语音输出汉字数字。
        // 归一化只丢阿拉伯数字，所以「50%」与「百分之五十」这两段仍然对不上 ——
        // 但只要周围实词能匹配上，整体就应当能定位并推进。
        let text = "2026年第三季度营收50%环比增长3.5倍";
        let mut a = Aligner::new(hay(text));
        let hyp = "二零二六年第三季度营收百分之五十环比增长三点五倍";

        let o = a.update(hyp);
        assert!(o.moved, "数字表述差异不应导致完全失配");
        assert!(o.score > 0.35, "得分应高于阈值，实际 {}", o.score);

        // 平滑推进意味着单次更新不会一步到底。这里连续更新，
        // 模拟真实情况 —— ASR 每 80ms 推一次、probe 持续后移，最终应收敛到末尾。
        for _ in 0..8 {
            a.update(hyp);
        }
        assert!(
            a.progress() > 0.95,
            "连续更新后应基本读完，进度 {}",
            a.progress()
        );
    }

    #[test]
    fn keeps_cjk_digits_distinguishable() {
        // 这是汉字数字**不能**被归一化掉的原因。
        // 若丢掉数字，这三句会退化成完全一样的序列，光标会跳到错误的那一季度。
        let text = "第一季度增长百分之十五第二季度增长百分之二十第三季度增长百分之二十五";
        let h = hay(text);
        assert!(
            h.contains(&'二') && h.contains(&'三'),
            "汉字数字必须保留在归一化序列里"
        );

        let mut a = Aligner::new(h);
        // 连续更新几次，让平滑推进收敛到目标位置
        for _ in 0..5 {
            a.update("第二季度增长百分之二十");
        }

        let cursor = a.cursor();
        // 归一化序列的构成（阿拉伯数字已丢、汉字数字保留）：
        //   [0..11)  第一季度增长百分之十五
        //   [11..21) 第二季度增长百分之二十   ← 光标应落在这里
        //   [21..34) 第三季度增长百分之二十五
        assert!(
            (11..=22).contains(&cursor),
            "光标应落在第二季度区间、不能越过第三季度，实际 {cursor}"
        );
    }

    #[test]
    fn realigns_after_repeated_misses() {
        // 稿件必须比 forward 窗口（240 字）更长，否则窗口本来就覆盖全稿，
        // 根本测不出"放开窗口搜索"这件事。
        let head = "前面这一段是开场白主要是介绍背景和整体情况".repeat(12);
        let tail = "现在我们从后半段继续讲具体的执行细节和落地安排";
        let text = format!("{head}{tail}");

        let mut a = Aligner::new(hay(&text));
        assert!(a.total() > 240, "测试稿件需要超过窗口长度，实际 {}", a.total());

        let hyp = "从后半段继续讲具体的执行细节和落地安排";
        let mut realigned = false;
        for _ in 0..12 {
            let o = a.update(hyp);
            if o.realigned {
                realigned = true;
                assert!(o.score > 0.5, "重对齐得分应较高，实际 {}", o.score);
                break;
            }
        }
        assert!(realigned, "应在连续失败后触发全稿重对齐");
        assert!(
            a.progress() > 0.8,
            "重对齐后光标应落在稿件后段，实际进度 {}",
            a.progress()
        );
    }

    #[test]
    fn lcs_ratio_sanity() {
        let a: Vec<char> = "abcdef".chars().collect();
        let b: Vec<char> = "abcxyz".chars().collect();
        assert!((lcs_ratio(&a, &b) - 0.5).abs() < 1e-6);
    }

    // ---- 跳读范围（设置页那条滑杆）----------------------------------------

    #[test]
    fn jump_window_preserves_legacy_defaults() {
        // 这条是"接出来不等于改行为"的证据：出厂态 20 句必须换算成
        // 引擎历史上一直在用的 240 / 40，一个字都不能差。
        assert_eq!(jump_window(DEFAULT_JUMP_RANGE), (240, 40));
        let d = AlignConfig::default();
        assert_eq!((d.forward, d.backward), (240, 40));
        assert_eq!(d.jump_sentences, DEFAULT_JUMP_RANGE);
    }

    #[test]
    fn jump_window_clamps_instead_of_truncating() {
        // 两端越界都要夹取到闭区间内，且**不回绕**（u32 直接乘会溢出）。
        assert_eq!(jump_window(0), jump_window(JUMP_RANGE_RANGE.0));
        assert_eq!(jump_window(u32::MAX), jump_window(JUMP_RANGE_RANGE.1));
        assert_eq!(jump_window(1000).0, 40 * CHARS_PER_SENTENCE);
        // 向后至少留一句的量，不能因除法变成 2 字
        assert_eq!(jump_window(1).1, CHARS_PER_SENTENCE);
    }

    #[test]
    fn jump_range_actually_narrows_the_search() {
        // 行为验证：光有换算函数不算数，得证明**窗口真的变了**。
        // 症状对齐真实故障 —— 把范围调太小，用户主动跳一段之后跟读就跟不上了。
        //
        // ★ 构造时要注意窗口的真实语义：`hi` 限制的是**匹配起点的上界**，
        //   窗口本身还会从那个起点再往后伸出 `probe_len` 个字。
        //   所以"能跳多远" ≈ forward + probe_len，而不是 forward ——
        //   按 forward 直接摆目标会得到一个恰好能匹配上的假结果（本轮真踩）。
        let head = "前面这一段是开场白主要是介绍背景和整体情况".repeat(3); // 63 字
        let tail = "现在我们从后半段继续讲具体的执行细节和落地安排"; // 从 63 字处开始
        let text = format!("{head}{tail}");

        let mut narrow = Aligner::new(hay(&text));
        assert_eq!(narrow.set_jump_range(1), 1);
        let o = narrow.update(tail);
        assert!(
            !o.moved,
            "1 句（起点最多到 12 字）不该够到 63 字开外，得分 {}",
            o.score
        );

        let mut wide = Aligner::new(hay(&text));
        assert_eq!(wide.set_jump_range(20), 20);
        let o2 = wide.update(tail);
        assert!(
            o2.moved,
            "20 句（起点可到 240 字）覆盖全稿，应当能接上，得分 {}",
            o2.score
        );
        assert!(o2.score > 0.8, "接上时应是高置信匹配，实际 {}", o2.score);
    }

    #[test]
    fn set_jump_range_clears_miss_streak() {
        // 调滑杆的时机多半是"刚才一直没对上"。留着旧计数，它会在下一个 tick
        // 直接触发全稿搜索 —— 用户看到的是"我一调它反而更乱了"。
        let mut a = Aligner::new(hay("前面是一段毫不相干的内容用来制造失败"));
        for _ in 0..4 {
            a.update("完全对不上的另一句话");
        }
        assert!(a.miss_streak > 0);
        a.set_jump_range(5);
        assert_eq!(a.miss_streak, 0, "改跳读范围应当把失败计数清零");
    }
}
