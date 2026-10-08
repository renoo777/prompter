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

/// 一个段落。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub index: u32,
    /// 原始文本，保留用户排版，直接用于显示。
    pub text: String,
    /// 段内句子。
    pub sentences: Vec<Sentence>,
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
fn is_sentence_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '；' | '…' | '!' | '?' | ';' | '\n')
}

/// 判断一行是否是 Markdown 标题 / 分隔线 —— 这类行单独成段且不参与朗读定位。
fn looks_like_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("---") || t.starts_with("===")
}

/// 从纯文本构建稿件。`title` 留空时自动取首行。
pub fn build_script(raw: &str, title: Option<String>) -> Script {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut chars: Vec<AlignChar> = Vec::new();

    // 段落切分：空行分段；Markdown 标题行强制单独成段。
    let mut buf = String::new();
    let mut blocks: Vec<String> = Vec::new();

    // 闭包本身不被重新赋值（可变状态都走参数传入），所以不需要 `mut`
    let flush = |buf: &mut String, blocks: &mut Vec<String>| {
        let t = buf.trim();
        if !t.is_empty() {
            blocks.push(t.to_string());
        }
        buf.clear();
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut buf, &mut blocks);
        } else if looks_like_heading(trimmed) {
            flush(&mut buf, &mut blocks);
            blocks.push(trimmed.to_string());
        } else {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(trimmed);
        }
    }
    flush(&mut buf, &mut blocks);

    for (pi, block) in blocks.iter().enumerate() {
        let paragraph_index = pi as u32;
        let mut sentences: Vec<Sentence> = Vec::new();

        // 段内切句：在句末标点处断开，标点归入前一句。
        let chars_of_block: Vec<char> = block.chars().collect();
        let mut sent_start = 0usize;

        let push_sentence = |sent_start: usize,
                                 sent_end: usize,
                                 sentences: &mut Vec<Sentence>,
                                 chars: &mut Vec<AlignChar>| {
            if sent_start >= sent_end {
                return;
            }
            let text: String = chars_of_block[sent_start..sent_end].iter().collect();
            let align_start = chars.len() as u32;

            // 逐字符归一化，并记录它在本句原始文本里的位置
            for (local_off, ch) in chars_of_block[sent_start..sent_end].iter().enumerate() {
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
                text: text.trim().to_string(),
                align_start,
                align_end,
            });
        };

        for (i, c) in chars_of_block.iter().enumerate() {
            if is_sentence_end(*c) {
                push_sentence(sent_start, i + 1, &mut sentences, &mut chars);
                sent_start = i + 1;
            }
        }
        // 收尾：最后一段没有句末标点的内容
        push_sentence(sent_start, chars_of_block.len(), &mut sentences, &mut chars);

        // 整段归一化后为空（例如纯数字或纯符号）时也保留段落文本，
        // 让用户能看见，只是它不参与定位。
        paragraphs.push(Paragraph {
            index: paragraph_index,
            text: block.clone(),
            sentences,
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
    fn consecutive_lines_merge_into_one_paragraph() {
        let s = build_script("第一行\n第二行\n\n另起一段", None);
        assert_eq!(s.paragraphs.len(), 2);
        assert!(s.paragraphs[0].text.contains("第一行"));
        assert!(s.paragraphs[0].text.contains("第二行"));
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
