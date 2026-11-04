//! 稿件引擎：文本导入 → 段落/句子切分 → 归一化对齐序列。
//!
//! # 为什么要有「归一化对齐序列」这一层
//!
//! 语音识别的输出和写在纸上的稿子，在同一句话上会长得很不一样：
//!
//! | 稿件原文 | 语音识别输出 |
//! |---|---|
//! | `2026年第三季度，营收 50%` | `二零二六年第三季度 营收百分之五十` |
//! | `环比增长 3.5 倍` | `环比增长三点五倍` |
//! | `OKR 复盘（Q3）` | `欧克尔复盘 Q 三` |
//!
//! 直接拿原文去和识别文本做匹配，上面每一行都会失配。
//!
//! # 采取的归一化策略
//!
//! **两侧统一丢弃同一批「不可靠字符」**，剩下的序列再去对齐：
//!
//! * 空白、各类标点 —— 识别结果里标点位置本身就不稳定
//! * 阿拉伯数字与全角数字（`0-9`、`０-９`）
//!
//! 关键点是**两侧用同一张表**。稿件写 `50`、口述说「五十」时这两段仍然对不上，
//! 但 LCS 会跳过差异、靠周围的实词完成定位 —— 不需要去猜「2026」该读成
//! 「两千零二十六」还是「二零二六」，那个问题没有唯一答案，猜错反而引入新错位。
//!
//! # 为什么汉字数字不能一并丢弃
//!
//! 最初的实现把 `一二三…十百千万亿两` 也丢掉了，理由是"数字都不靠谱"。
//! **这是错的 —— 这些字同时也是普通用字。**
//!
//! 汇报稿里 `第一季度 / 第二季度 / 第三季度` 极其常见，丢掉数字后这三句话
//! 会全部退化成同一个 `季度增长`，定位时根本无法区分。此时光标不是"偏几个字"，
//! 而是会跳到完全错误的段落 —— 远比「50% 与百分之五十对不上」严重，
//! 后者只损失几个字的匹配率。
//!
//! 取舍依据是**误伤的代价**，不是规则本身的一致性：
//! 阿拉伯数字丢得起（语音侧本来就不输出它们），汉字数字丢不起。
//!
//! 剩下一个已知代价：纯数字的行（如孤零零一行 `2026`）归一化后为空、
//! 无法参与定位。这类内容通常是标题而非朗读内容，可以接受。

use serde::{Deserialize, Serialize};

/// 段落样式（**只影响显示，不影响文字与对位**）。
///
/// # 为什么样式是"段落的属性"而不是"文本里的标记"
///
/// 一旦样式写进文本（Markdown 的 `**`、或者自造的 `〔红〕`），显示文本就与
/// `text` 不再相等 —— 而逐字高亮靠 `offset_in_sentence` 的字符下标、点句靠
/// 累加 DOM 文本长度，两边都会整体错位。样式必须挂在**文本之外**。
///
/// # 为什么颜色是"预设索引"而不是 RGB
///
/// 自由取色在深色/浅色/磨砂三档主题下会撞上"红字在深色背景上看不见"这种
/// 无解组合。索引交给 CSS 去定义每个主题下的实际颜色，主题一切颜色跟着切。
/// 后续要加自定义颜色，再加一个 `custom: Option<[u8; 3]>` 覆盖索引即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ParaStyle {
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// 文字色：预设索引（0..`COLOR_PRESETS`），`None` = 跟随主题
    #[serde(default)]
    pub color: Option<u8>,
    /// 底色：预设索引（0..`BG_PRESETS`），`None` = 无底色
    #[serde(default)]
    pub bg: Option<u8>,
}

impl ParaStyle {
    /// 全空 = 没有样式。库里不存空样式，判定统一走这里。
    pub fn is_empty(&self) -> bool {
        !self.bold && !self.italic && self.color.is_none() && self.bg.is_none()
    }

    /// 夹取预设索引的范围 —— 前端传越界值时**收敛**而不是报错，
    /// 免得一个手滑的索引把整段渲染成透明。
    pub fn clamp(mut self) -> Self {
        self.color = self.color.map(|c| c.min(COLOR_PRESETS - 1));
        self.bg = self.bg.map(|b| b.min(BG_PRESETS - 1));
        self
    }
}

/// 文字色预设数量（与 `styles.css` 的 `--mark-*` 一一对应）。
pub const COLOR_PRESETS: u8 = 4;
/// 底色预设数量（与 `styles.css` 的 `--markbg-*` 一一对应）。
pub const BG_PRESETS: u8 = 3;

/// 一个段落。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub index: u32,
    /// 原始文本，保留用户排版，直接用于显示。
    ///
    /// ★ 现在**会含 `\n`**（段内的硬换行）与行首空白（缩进）。
    ///   这两个都是"粘进来什么样就显示什么样"的依据，别在别处再 trim 一遍。
    pub text: String,
    /// 段内句子。
    pub sentences: Vec<Sentence>,
    /// 这一段前面有几个空行（最多 3）。前端换算成段前留白。
    ///
    /// ★ 空行原来是"纯粹的分段符"、不产生任何可见效果 —— 于是 34px 字号下
    ///   16px 的段距只有半行高，一段接一段看着像连着的一大块。
    ///   现在把它记下来交给排版，才有"分段"这件事。
    #[serde(default)]
    pub blank_before: u32,
    /// 是不是 Markdown 标题 / 结构行（`# 标题`）。
    #[serde(default)]
    pub heading: bool,
    /// 段落样式。`None` = 没有样式（绝大多数段落）。
    #[serde(default)]
    pub style: Option<ParaStyle>,
}

/// 一个句子（对位的显示粒度）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sentence {
    pub index: u32,
    /// 原始文本（含标点），用于显示。
    pub text: String,
    /// 在归一化全稿序列中的起止，左闭右开。
    pub align_start: u32,
    pub align_end: u32,
    /// 这一句前面有一个**硬换行**（原文里断行，但不是新段落）。
    ///
    /// ★ 为什么不把 `\n` 留在 `text` 里让 CSS 去换行：那样换行符会算进
    ///   句子字符下标，而 `offset_in_sentence` 是按"不含换行的文本"算的
    ///   ⇒ 点句定位会差位。换行交给渲染层插 `<br>`，字符口径不变。
    #[serde(default)]
    pub line_break_before: bool,
}

/// 归一化后的单个对齐单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignChar {
    /// 归一化后的字符。
    pub ch: char,
    pub paragraph: u32,
    pub sentence: u32,
    /// 该字符对应到所属句子**原始文本**里的第几个字符（按 `char` 计数）。
    ///
    /// 前端靠这个把「当前读到第 N 个对齐单元」换算成「句子内第几个字要高亮」，
    /// 不需要自己再做一遍归一化 —— 两边实现一旦分叉就会对不上。
    pub offset_in_sentence: u32,
}

/// 一份完整的稿件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    pub title: String,
    pub paragraphs: Vec<Paragraph>,
    /// 扁平化的归一化字符流，对齐算法唯一操作的数据。
    pub chars: Vec<AlignChar>,
}

// ---------------------------------------------------------------------------
// 字符分类表 —— 归一化两侧共用同一张，这是正确性的前提
// ---------------------------------------------------------------------------

/// 中文标点 —— 丢弃。识别结果里的标点位置本身就不稳定，留着只会污染匹配。
fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '（'
            | '）'
            | '《'
            | '》'
            | '【'
            | '】'
            | '「'
            | '」'
            | '『'
            | '』'
            | '〈'
            | '〉'
            | '…'
            | '—'
            | '·'
            | '～'
            | '〔'
            | '〕'
            | '〖'
            | '〗'
            | '　'
    )
}

/// 该字符是否应当从对齐序列里剔除。
fn is_droppable(c: char) -> bool {
    if c.is_whitespace() {
        return true;
    }
    if c.is_ascii_digit() {
        return true;
    }
    // 全角数字 ０-９
    if ('０'..='９').contains(&c) {
        return true;
    }
    if is_cjk_punct(c) {
        return true;
    }
    // ASCII 标点与符号
    if c.is_ascii_punctuation() {
        return true;
    }
    // 各类空白与不可见字符
    if c.is_control() {
        return true;
    }
    // 全角空格、不间断空格等
    if matches!(c, '\u{00A0}' | '\u{3000}' | '\u{200B}' | '\u{FEFF}') {
        return true;
    }
    // 货币与百分号类
    if matches!(c, '％' | '￥' | '＄' | '＃' | '＠' | '＆' | '＊' | '＋' | '＝' | '／' | '＼' | '｜' | '～' | '＾' | '｀') {
        return true;
    }
    false
}

/// 全角 ASCII（`Ａ`-`Ｚ`、`ａ`-`ｚ`）转半角。
fn fullwidth_to_ascii(c: char) -> char {
    let cp = c as u32;
    // 全角区间 FF01..FF5E 与半角 21..7E 一一对应
    if (0xFF01..=0xFF5E).contains(&cp) {
        char::from_u32(cp - 0xFEE0).unwrap_or(c)
    } else {
        c
    }
}

/// 单字符归一化。返回 `None` 表示该字符不进入对齐序列。
fn normalize_char(c: char) -> Option<char> {
    if is_droppable(c) {
        return None;
    }
    let c = fullwidth_to_ascii(c);
    // 再判一次：全角数字/标点转半角后可能变成可丢字符
    if is_droppable(c) {
        return None;
    }
    // 拉丁字母统一小写（中文与日文汉字不受影响）
    Some(c.to_ascii_lowercase())
}

/// 把一段文本归一化成字符序列。
pub fn normalize(text: &str) -> Vec<char> {
    text.chars().filter_map(normalize_char).collect()
}

// ---------------------------------------------------------------------------
// 文本解码
// ---------------------------------------------------------------------------

/// 解码任意来源的稿件字节。
///
/// 优先按 UTF-8 严格解析；失败则按 GBK 解码 —— 国内大量存量 `.txt` 是 GBK，
/// 用 UTF-8 硬解会得到一片乱码，而乱码不会报错，只会在对齐时表现为"完全对不上"，
/// 极难排查。所以这里宁可多花一次解码。
pub fn decode_bytes(bytes: &[u8]) -> (String, &'static str) {
    // 去掉 UTF-8 BOM，否则第一个字符会带上来变成不可见脏字符
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);

    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "utf-8");
    }
    let (s, _, had_errors) = encoding_rs::GBK.decode(bytes);
    if had_errors {
        // GBK 也解不干净时，用有损 UTF-8 兜底，保证至少能显示
        (String::from_utf8_lossy(bytes).into_owned(), "utf-8-lossy")
    } else {
        (s.into_owned(), "gbk")
    }
}

// ---------------------------------------------------------------------------
// 切分
// ---------------------------------------------------------------------------

/// 句末标点 —— 切句依据。
///
/// ★ 硬换行（`\n`）**不在**这张表里，它由调用方单独当"行的边界"处理。
///   混进来的话换行符会跟着进句子文本：渲染时多一个空行、字符下标也把
///   换行算进去，而 `offset_in_sentence` 是按显示文本算的 ⇒ 点句差一位。
fn is_sentence_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '；' | '…' | '!' | '?' | ';')
}

/// Markdown 标题 → 去掉标记后的文本；分隔线（`---` / `===`）→ 空串。
///
/// ★ 返回 `Some("")` 时调用方要**把这一行整块丢掉**：分隔线归一化后没有一个
///   可对位的字符，留着只会在正文里显示一排横线。
/// ★ 标题的 `#` 也必须在这里去掉 —— 原来只让它单独成段、`#` 却留在 `text` 里，
///   屏幕上就会念出并显示「# 季度复盘」。
fn heading_of(line: &str) -> Option<String> {
    let t = line.trim_end();
    let lead = t.trim_start();
    if lead.starts_with('#') {
        return Some(lead.trim_start_matches('#').trim().to_string());
    }
    if lead.starts_with("---") || lead.starts_with("===") {
        return Some(String::new());
    }
    None
}

/// 把一「块」里的多行接起来。
///
/// ★ 用一个 `\n` 接，**不再用一个空格拼平** —— 这是「结构化分段」那条需求
///   的核心：粘进来几行，屏幕上就该是几行。空格拼接会把换行彻底抹掉，
///   五行话挤成一行，这是"挤成一团"最直接的成因。
/// ★ 行首空白**保留**（那是缩进），只砍行尾 —— 行尾空白在任何排版里都看不见，
///   留着只会在 `white-space: pre-wrap` 下把折行位置推歪。
fn join_lines(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 把块内 `[start, end)` 这一片字符收成一句。
///
/// `brk` —— 这一句前面有一个硬换行（段内换行，不是新段落）。
fn push_sentence(
    block: &[char],
    start: usize,
    end: usize,
    brk: bool,
    paragraph_index: u32,
    sentences: &mut Vec<Sentence>,
    chars: &mut Vec<AlignChar>,
) {
    // 行尾空白（含换行）砍掉；**行首空白保留** —— 那正是缩进
    let mut stop = end.min(block.len());
    while stop > start && block[stop - 1].is_whitespace() {
        stop -= 1;
    }
    if stop <= start {
        return;
    }
    let core = &block[start..stop];
    let text: String = core.iter().collect();
    let align_start = chars.len() as u32;

    // 逐字符归一化。`offset_in_sentence` 直接取**这句显示文本**里的下标 ——
    // 两者必须同一口径，否则前端逐字高亮会整体偏一位。
    for (local_off, ch) in core.iter().enumerate() {
        if let Some(n) = normalize_char(*ch) {
            chars.push(AlignChar {
                ch: n,
                paragraph: paragraph_index,
                sentence: sentences.len() as u32,
                offset_in_sentence: local_off as u32,
            });
        }
    }
    let align_end = chars.len() as u32;

    sentences.push(Sentence {
        index: sentences.len() as u32,
        text,
        align_start,
        align_end,
        // 段的第一句前面没有"上一行"，不该画换行
        line_break_before: brk && !sentences.is_empty(),
    });
}

/// 从纯文本构建稿件。`title` 留空时自动取首行。
pub fn build_script(raw: &str, title: Option<String>) -> Script {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut chars: Vec<AlignChar> = Vec::new();

    // ---- 1) 切块：空行是块边界；**块内保留硬换行与行首缩进** ----
    //
    // 每块带上「前面有几个空行」—— 那是段前留白的唯一依据。
    // ★ 上一版这里是把连续非空行用一个空格拼进同一个 `String`，换行与缩进
    //   同时消失。用户看到的"整篇挤成一大段"就是从这一行开始的。
    let mut blocks: Vec<(String, u32, bool)> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut before: u32 = 0;

    for line in raw.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                blocks.push((join_lines(&cur), before, false));
                cur.clear();
                before = 0;
            }
            before += 1;
            continue;
        }
        if let Some(h) = heading_of(line) {
            if !cur.is_empty() {
                blocks.push((join_lines(&cur), before, false));
                cur.clear();
                before = 0;
            }
            // 分隔线（`---`）剥出来是空串 —— 它不该在正文里留一排横线
            if !h.is_empty() {
                blocks.push((h, before, true));
            }
            before = 0;
            continue;
        }
        cur.push(line);
    }
    if !cur.is_empty() {
        blocks.push((join_lines(&cur), before, false));
    }

    // ---- 2) 段内切句：句末标点断开；硬换行只断行、不断段 ----
    for (pi, (block, blank_before, heading)) in blocks.iter().enumerate() {
        let paragraph_index = pi as u32;
        let mut sentences: Vec<Sentence> = Vec::new();
        let chars_of_block: Vec<char> = block.chars().collect();

        let mut sent_start = 0usize;
        // 下一句前面有没有硬换行。块的第一句一定没有（没有"上一行"）。
        let mut brk = false;

        for (i, c) in chars_of_block.iter().enumerate() {
            if *c == '\n' {
                // 换行只结句，**不进句子文本** —— 交给前端画 `<br>`
                push_sentence(
                    &chars_of_block, sent_start, i, brk,
                    paragraph_index, &mut sentences, &mut chars,
                );
                sent_start = i + 1;
                brk = true;
            } else if is_sentence_end(*c) {
                // 标点归前一句
                push_sentence(
                    &chars_of_block, sent_start, i + 1, brk,
                    paragraph_index, &mut sentences, &mut chars,
                );
                sent_start = i + 1;
                brk = false;
            }
        }
        // 收尾：最后一段没有句末标点的内容
        push_sentence(
            &chars_of_block, sent_start, chars_of_block.len(), brk,
            paragraph_index, &mut sentences, &mut chars,
        );

        // 整段归一化后为空（例如纯数字或纯符号）时也保留段落文本，
        // 让用户能看见，只是它不参与定位。
        paragraphs.push(Paragraph {
            index: paragraph_index,
            text: block.clone(),
            sentences,
            // 首段前面的空行是"文件开头的空行"，不是段距 —— 不给留白
            blank_before: if pi == 0 { 0 } else { (*blank_before).min(3) },
            heading: *heading,
            // 样式不存在文本里，由调用方按库里的记录挂上（见 commands::attach_styles）
            style: None,
        });
    }

    let auto_title = paragraphs
        .first()
        .map(|p| {
            let t = p.text.trim_start_matches('#').trim();
            let head: String = t.chars().take(24).collect();
            head
        })
        .unwrap_or_else(|| "未命名稿件".to_string());

    Script {
        title: title.filter(|t| !t.trim().is_empty()).unwrap_or(auto_title),
        paragraphs,
        chars,
    }
}

/// 把全稿归一化成一条扁平字符序列 —— 给对齐算法用的热路径表示。
pub fn flat_chars(script: &Script) -> Vec<char> {
    script.chars.iter().map(|c| c.ch).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_only_arabic_digits() {
        // 稿件侧：阿拉伯数字被丢掉，汉字数字**保留**
        let a = normalize("2026年第三季度，营收50%");
        assert_eq!(a.iter().collect::<String>(), "年第三季度营收");

        // 语音侧：全是汉字，只丢标点，数字读法完整保留
        let b = normalize("二零二六年第三季度营收百分之五十");
        assert_eq!(
            b.iter().collect::<String>(),
            "二零二六年第三季度营收百分之五十"
        );

        // 关键：两侧共有的那一段必须逐字对齐 —— 定位依赖的正是这段
        let a_common: String = a.iter().take(5).collect();
        let b_common: String = b.iter().skip(4).take(5).collect();
        assert_eq!(a_common, "年第三季度");
        assert_eq!(a_common, b_common);
    }

    #[test]
    fn keeps_cjk_digits() {
        // 汉字数字必须保留，否则「第一季度 / 第二季度 / 第三季度」会全部退化成
        // 同一个序列，对位时就无法区分。
        let s: String = normalize("第一季度第二季度第三季度").iter().collect();
        assert_eq!(s, "第一季度第二季度第三季度");
    }

    #[test]
    fn drops_arabic_and_fullwidth_digits() {
        assert_eq!(normalize("一共 12 个").iter().collect::<String>(), "一共个");
        assert_eq!(normalize("１２３件").iter().collect::<String>(), "件");
    }

    #[test]
    fn normalize_handles_fullwidth_latin() {
        assert_eq!(normalize("ＡＢＣ").iter().collect::<String>(), "abc");
    }

    #[test]
    fn split_sentences_by_cjk_punct() {
        let s = build_script("第一句。第二句！第三句没有标点", None);
        assert_eq!(s.paragraphs.len(), 1);
        assert_eq!(s.paragraphs[0].sentences.len(), 3);
    }

    #[test]
    fn digits_only_line_keeps_text_but_no_align_units() {
        // 注意用空行分隔 —— 连续的两行会被合并成一个段落（稿件里硬换行很常见，
        // 一律当成同段处理才符合"一段话读下来"的直觉）。
        let s = build_script("2026\n\n正文内容在这里", None);
        assert_eq!(s.paragraphs.len(), 2);
        // 段落和句子都会保留（用户得能看见这行标题），
        // 但它的对齐区间是空的 —— 归一化后没有可用字符，不参与定位。
        assert_eq!(s.paragraphs[0].sentences.len(), 1);
        let only = &s.paragraphs[0].sentences[0];
        assert_eq!(only.text, "2026");
        assert_eq!(only.align_start, only.align_end);
        assert!(!s.chars.is_empty());
    }

    #[test]
    fn consecutive_lines_stay_separate_lines_inside_one_paragraph() {
        // ★ 这一条守的是「结构化分段」的地基。
        //   上一版把连续非空行用一个空格拼成一行，用户粘五句话进来就变成
        //   一大段 —— 这是"挤成一团"最直接的成因。现在块内保留硬换行：
        //   同一个段落（空行才算分段），但行与行之间必须记得住"这里断过行"。
        let s = build_script("第一行\n第二行\n\n另起一段", None);
        assert_eq!(s.paragraphs.len(), 2, "空行才是段落边界");
        assert_eq!(s.paragraphs[0].text, "第一行\n第二行");
        assert_eq!(s.paragraphs[0].sentences.len(), 2);
        assert!(
            !s.paragraphs[0].sentences[0].line_break_before,
            "段的第一句前面没有上一行，不该画换行"
        );
        assert!(
            s.paragraphs[0].sentences[1].line_break_before,
            "第二行前面有一个硬换行，必须标出来"
        );
        // 换行符不能留在句子文本里 —— 否则字符下标会把换行算进去，点句差一位
        assert_eq!(s.paragraphs[0].sentences[0].text, "第一行");
        assert_eq!(s.paragraphs[0].sentences[1].text, "第二行");
    }

    #[test]
    fn blank_lines_become_paragraph_spacing() {
        // 空行不产生可见留白的时候，段落之间只有 16px，看着还是连着的
        let s = build_script("甲段\n\n\n乙段", None);
        assert_eq!(s.paragraphs.len(), 2);
        assert_eq!(s.paragraphs[1].blank_before, 2, "两个空行要记成 2");
        // 文件开头的空行不是段距
        let s2 = build_script("\n\n甲段", None);
        assert_eq!(s2.paragraphs[0].blank_before, 0);
        // 封顶 3 —— 粘一份中间夹了十行空行的稿子，不该出现半屏空白
        let s3 = build_script("甲\n\n\n\n\n\n乙", None);
        assert_eq!(s3.paragraphs[1].blank_before, 3);
    }

    #[test]
    fn markdown_heading_loses_its_hashes() {
        // 原来 `#` 只被用来"单独成段"，字符本身留在了 text 里 ⇒ 屏幕上显示「# 季度复盘」
        let s = build_script("# 季度复盘\n\n正文内容", None);
        assert!(s.paragraphs[0].heading);
        assert_eq!(s.paragraphs[0].text, "季度复盘");
        assert!(!s.paragraphs[1].heading);
        // 标题的自动取题不该带 `#`
        assert_eq!(s.title, "季度复盘");
    }

    #[test]
    fn separator_lines_are_dropped() {
        // `---` 归一化后没有任何可对位字符，留着只会在正文里显示一排横线
        let s = build_script("上段\n\n---\n\n下段", None);
        assert_eq!(s.paragraphs.len(), 2);
        assert!(!s.paragraphs.iter().any(|p| p.text.contains("---")));
    }

    #[test]
    fn indentation_is_kept_and_offsets_match_the_displayed_text() {
        // 行首空白是缩进，必须留；而 `offset_in_sentence` 必须按**显示文本**算
        // （含那几个空格），否则逐字高亮会整体偏一位。
        let s = build_script("  营收增长。", None);
        let p = &s.paragraphs[0];
        assert_eq!(p.text, "  营收增长。");
        let sent = &p.sentences[0];
        assert_eq!(sent.text, "  营收增长。");
        let first = &s.chars[sent.align_start as usize];
        assert_eq!(first.ch, '营');
        assert_eq!(first.offset_in_sentence, 2, "前面两个空格也要算进去");
    }

    #[test]
    fn sentence_texts_concatenate_to_the_paragraph_text_minus_newlines() {
        // ★ 这是"点句 / 逐字高亮"的**契约**，不是巧合：
        //   前端 `charIndexAt` 按 DOM 里的文本节点累加字符下标去找句子，
        //   而 DOM 文本恰好等于"各句文本相接"（换行由 `<br>` 画，不产生文本节点）。
        //   任何一句多吃或少吃一个字符（典型：把换行算进某一句、或悄悄 trim 掉
        //   行首缩进），下标就整体错位 —— 症状是"点了这一句跳到上一句"，
        //   而且只在有换行/缩进的稿子上出现，极难复现。
        for raw in [
            "纯一行。",
            "第一行\n第二行。\n第三行",
            "  缩进行一。\n  缩进行二。",
            "# 标题\n\n正文一。正文二！\n\n\n再一段",
        ] {
            let s = build_script(raw, None);
            for p in &s.paragraphs {
                let joined: String = p.sentences.iter().map(|x| x.text.as_str()).collect();
                assert_eq!(
                    joined,
                    p.text.replace('\n', ""),
                    "段落文本与句子拼接不一致：{raw:?}"
                );
            }
        }
    }

    #[test]
    fn offset_maps_back_to_original_sentence() {
        let s = build_script("营收50%增长。", None);
        let sent = &s.paragraphs[0].sentences[0];
        assert_eq!(sent.text, "营收50%增长。");
        // 首个对齐单元应指向「营」（原文第 0 个字符）
        let first = &s.chars[sent.align_start as usize];
        assert_eq!(first.ch, '营');
        assert_eq!(first.offset_in_sentence, 0);
        // 「增」在原文里是第 5 个字符（营收50%增）
        let zeng = s.chars.iter().find(|c| c.ch == '增').unwrap();
        assert_eq!(zeng.offset_in_sentence, 5);
    }
}
