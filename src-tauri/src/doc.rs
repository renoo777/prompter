//! 富文本文档导入：`.docx` / `.pptx` → 纯文本。
//!
//! # 为什么这个模块必须存在
//!
//! 提词器的真实使用场景里，讲稿绝大多数**不是** `.txt`：
//! 汇报稿是 Word，路演讲稿躺在 PPT 的备注页里。
//! 只能拖 `.txt` 的话，用户每次都要先"另存为纯文本"再导 —— 一步多余的手工操作，
//! 而这正是提词器该替他省掉的那一步。
//!
//! # 为什么不引 XML 库
//!
//! `.docx` / `.pptx` 都是 ZIP 包着一堆 XML。抽正文只需要"认出哪些元素是正文文本、
//! 哪些是段落边界"—— 而 Word / PowerPoint 生成的 XML 结构高度规整：
//! 没有 DTD、没有 CDATA、正文里的 `<` `&` 一定被转义过。
//!
//! 所以这里用手写扫描器 + `zip` 解包，**不引 XML 解析库**，理由有两条：
//!
//! 1. 解析行为完全在我们掌控内。XML 库为了通用性会接受各种畸形输入，
//!    而这里宁可"遇到畸形就少抓几个字"，也不要一个第三方库决定我们的容错边界。
//! 2. 手写扫描器**可以脱离 ZIP 单独单测** —— 传一段 XML 字符串进去就能验，
//!    不需要在测试里造一个真的 Office 文件（虽然也造了，见文末测试）。
//!
//! # 已知取舍
//!
//! * 只抽正文/备注的文本，**不保留**加粗、颜色、批注、修订痕迹。
//!   提词器只需要"念什么"，样式是干扰项。
//! * `.doc` / `.ppt` **老二进制格式不支持** —— 那不是 ZIP，是 OLE 复合文档，
//!   解析成本远高于收益。报错里会明确让用户"另存为 .docx"。
//! * 表格里的文字会被抽出来（`<w:tbl>` 里也是 `<w:p>`），顺序是文档顺序。
//!   这符合直觉：表格里的话也是要念的。

use std::io::Read;
use std::path::Path;

/// 需要先解包再抽正文的扩展名。
pub const RICH_EXTS: [&str; 2] = ["docx", "pptx"];

/// 稿件文件对话框与拖放共用的"能不能载入"清单。
///
/// **只有这一份**。之前判断散在 `lib.rs::is_supported` 与前端 dialog filter
/// 两处，加一个扩展名要改两个地方，漏掉一处就会出现"对话框里选不到、
/// 拖进来却能载入"这种自相矛盾的状态。
///
/// 没被 `RICH_EXTS` 覆盖的那些就是纯文本（走 `script::decode_bytes`，
/// 含 GBK 兜底）—— 不再单独维护一份纯文本清单，免得第三处要同步。
pub const SCRIPT_EXTS: [&str; 5] = ["txt", "md", "markdown", "docx", "pptx"];

/// 给用户看的扩展名清单（报错文案用，避免文案和清单分叉）。
pub const SCRIPT_EXTS_HINT: &str = ".txt / .md / .docx / .pptx";

/// 取小写扩展名；没有扩展名时返回空串。
pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn is_supported_ext(ext: &str) -> bool {
    SCRIPT_EXTS.contains(&ext)
}

pub fn is_rich_ext(ext: &str) -> bool {
    RICH_EXTS.contains(&ext)
}

// ---------------------------------------------------------------------------
// XML → 文本（纯函数，可脱离 ZIP 单测）
// ---------------------------------------------------------------------------

/// 取限定名的本地部分：`w:t` → `t`，`t` → `t`。
fn local_name(qname: &str) -> &str {
    match qname.rsplit_once(':') {
        Some((_, local)) => local,
        None => qname,
    }
}

/// 解码 XML 实体。
///
/// 只认 5 个具名实体 + 十进制/十六进制数字引用。Word 的输出里不会出现
/// 自定义实体（没有 DTD），所以不需要更复杂的处理。
fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        // 实体最短 `&lt;`，最长 `&#x1F600;`。超过 12 个字符还没闭合的
        // `;` 一定不是实体（多半是正文里孤零零一个 &）—— 当字面量处理。
        match after.find(';') {
            Some(j) if j <= 12 => {
                let ent = &after[1..j];
                let rep = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => {
                        if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                        } else if let Some(dec) = ent.strip_prefix('#') {
                            dec.parse::<u32>().ok().and_then(char::from_u32)
                        } else {
                            None
                        }
                    }
                };
                match rep {
                    Some(c) => {
                        out.push(c);
                        rest = &after[j + 1..];
                    }
                    // 认不出来的实体原样保留 —— 宁可显示成 `&nbsp;`
                    // 也不要凭空吞掉一个字
                    None => {
                        out.push('&');
                        rest = &after[1..];
                    }
                }
            }
            _ => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// 从一段 Office XML 里抽纯文本。
///
/// * `text_tags` —— 本地名命中这些元素的，其**紧跟标签之后**的文本节点被采集。
///   docx 是 `t`（`<w:t>`），pptx 也是 `t`（`<a:t>`）。
/// * `break_tags` —— 本地名命中这些元素的，**闭合处**产出一个换行（段落边界）；
///   若是自闭合写法（`<w:br/>`、`<a:br/>`），开标签处即产出换行。
/// * `skip_tags` —— 在其内部**完全不采集**文本。用来丢掉 PPT 备注页里的
///   "幻灯片编号"字段（`<a:fld type="slidenum">` 里也会有一个 `<a:t>1</a:t>`，
///   不排掉的话每页会多出一个孤零零的数字行）。
///
/// # 扫描方式
///
/// 按 `<` 切开。切出来的每一段形如 `w:t>文字` —— 前半是标签（不含 `<`），
/// 后半是它的尾随文本。能这么切的前提是正文里的 `<` 已被转义成 `&lt;`
/// （Office 生成时一定会转），所以文本节点里不会再出现 `<`。
pub fn extract_from_xml(xml: &str, text_tags: &[&str], break_tags: &[&str], skip_tags: &[&str]) -> String {
    let mut out = String::new();
    let mut capturing = 0usize;
    let mut skipping = 0usize;
    let mut first = true;

    for chunk in xml.split('<') {
        if first {
            // 第一个 `<` 之前的裸文本。Office 文件里不会出现，忽略。
            first = false;
            continue;
        }
        let Some((tag, tail)) = chunk.split_once('>') else {
            continue; // 被截断的最后一段
        };
        let tag = tag.trim();
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');

        let bare = tag.trim_start_matches('/');
        let bare = bare.strip_suffix('/').unwrap_or(bare);
        let name = local_name(bare.split_whitespace().next().unwrap_or(""));

        if closing {
            if skip_tags.contains(&name) {
                skipping = skipping.saturating_sub(1);
            }
            if text_tags.contains(&name) {
                capturing = capturing.saturating_sub(1);
            }
            if break_tags.contains(&name) {
                out.push('\n');
            }
        } else if self_closing {
            if break_tags.contains(&name) {
                out.push('\n');
            }
        } else {
            if skip_tags.contains(&name) {
                skipping += 1;
            }
            if text_tags.contains(&name) && skipping == 0 {
                capturing += 1;
            }
        }

        if capturing > 0 && skipping == 0 {
            out.push_str(&unescape_xml(tail));
        }
    }

    out
}

/// docx 正文 → 文本。段落边界是 `</w:p>`，`<w:br/>` 是段内换行。
pub fn docx_text(xml: &str) -> String {
    // skip 为空：`<w:instrText>`（域代码）的本地名是 `instrText` 而非 `t`，
    // 天然不会被采集；`<w:fldSimple>` 里的缓存结果是真文本，应当保留。
    extract_from_xml(xml, &["t"], &["p", "br"], &[])
}

/// pptx 单页（备注页或幻灯片）→ 文本。
pub fn pptx_page_text(xml: &str) -> String {
    // `<a:fld type="slidenum">` 放的是"第几页"这种自动编号 —— 不是讲稿，
    // 排掉它，否则每页正文前面都会多出一个孤立的数字。
    extract_from_xml(xml, &["t"], &["p", "br"], &["fld"])
}

/// 去掉多余空行（抽出来的正文常见连续空段落）。
fn tidy(s: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            // 连续空行最多保留一个 —— 空行是分段依据，全删会粘连段落，
            // 全留则会得到一屏空行
            if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                continue;
            }
            lines.push("");
        } else {
            lines.push(t);
        }
    }
    lines.join("\n").trim().to_string()
}

// ---------------------------------------------------------------------------
// ZIP 解包
// ---------------------------------------------------------------------------

/// 打开 ZIP 包。**这是"格式对不对"的唯一判据。**
///
/// 单列成一个函数，是为了让"不是 Office 文件"和"是 Office 文件但里面没有正文"
/// 这两种失败能被分开报出来。它们对用户意味着完全不同的下一步：
/// 前者是"你把 .doc 改了个名 / 文件坏了"，后者是"这份文档没写字"。
/// 混成一句话的话，用户会去文档里翻半天，而问题根本不在那儿。
fn open_zip(bytes: &[u8]) -> Result<zip::ZipArchive<std::io::Cursor<&[u8]>>, String> {
    zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("不是有效的 Office 文件（{e}）"))
}

fn read_entry(bytes: &[u8], name: &str) -> Result<String, String> {
    let mut zip = open_zip(bytes)?;
    let mut f = zip.by_name(name).map_err(|_| format!("包里找不到 {name}"))?;
    let mut s = String::new();
    f.read_to_string(&mut s)
        .map_err(|e| format!("读取 {name} 失败：{e}"))?;
    Ok(s)
}

fn entry_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut zip = open_zip(bytes)?;
    let mut v = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| format!("读包内条目失败：{e}"))?;
        v.push(f.name().to_string());
    }
    Ok(v)
}

/// 匹配 `{dir}/{prefix}N.xml`，取出 N。
fn slide_index(name: &str, dir: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(dir)?.strip_prefix('/')?;
    let num = rest.strip_prefix(prefix)?.strip_suffix(".xml")?;
    num.parse::<u32>().ok()
}

fn pptx_text(bytes: &[u8]) -> Result<String, String> {
    let names = entry_names(bytes)?;

    let mut notes: Vec<u32> = names
        .iter()
        .filter_map(|n| slide_index(n, "ppt/notesSlides", "notesSlide"))
        .collect();
    notes.sort_unstable();

    let mut slides: Vec<u32> = names
        .iter()
        .filter_map(|n| slide_index(n, "ppt/slides", "slide"))
        .collect();
    slides.sort_unstable();

    // **备注优先**。PPT 正文往往只有标题几个字，讲稿在备注里 ——
    // 提词器要念的是备注。只有在完全没有备注时才退到正文。
    let (dir, prefix, list, label) = if !notes.is_empty() {
        ("ppt/notesSlides", "notesSlide", notes, "备注")
    } else if !slides.is_empty() {
        ("ppt/slides", "slide", slides, "正文")
    } else {
        return Err("这个 .pptx 里既没有备注页也没有幻灯片，提不出文字。".to_string());
    };

    // 按页码升序拼接 —— 讲稿的顺序就是放映顺序
    let mut parts: Vec<String> = Vec::new();
    for n in list {
        let xml = read_entry(bytes, &format!("{dir}/{prefix}{n}.xml"))?;
        let t = tidy(&pptx_page_text(&xml));
        if !t.is_empty() {
            parts.push(t);
        }
    }

    if parts.is_empty() {
        return Err(format!(
            "这个 .pptx 的{label}里没有文字（备注是空的？）。\
             在 PowerPoint 里用「视图 → 备注」补上讲稿，或者直接导出成 .docx / .txt。"
        ));
    }
    Ok(parts.join("\n\n"))
}

/// 从 `.docx` / `.pptx` 的**原始字节**里抽纯文本。
///
/// 返回 `Err` 时调用方必须**保持当前稿件不变** —— 这条与 `load_into_engine`
/// 的"全有或全无"是同一件事。
pub fn extract_rich(bytes: &[u8], ext: &str) -> Result<String, String> {
    // 先单独判一次"这是不是一个 ZIP"。不先做这一步的话，一个把 .doc 改名成
    // .docx 的文件会被报成"找不到正文"—— 而用户真正的问题在扩展名上，
    // 那句提示只会把他引到错误的方向去。
    open_zip(bytes)?;

    match ext {
        "docx" => {
            // 到这里 ZIP 一定是好的，所以 read_entry 的失败只剩"没这个条目"
            let xml = read_entry(bytes, "word/document.xml").map_err(|_| {
                "这个 .docx 里找不到正文（word/document.xml）。\
                 若它是 Word 加密文档、或者是 .doc 改了个扩展名，\
                 请先用 Word 打开并「另存为 → .docx」。当前稿件未改动。"
                    .to_string()
            })?;
            let t = tidy(&docx_text(&xml));
            if t.is_empty() {
                return Err(
                    "这份 Word 里没有提取到文字（可能是纯图片/扫描件）。\
                     图片要能提词必须先做 OCR，那是另一项能力，本版本不做。当前稿件未改动。"
                        .to_string(),
                );
            }
            Ok(t)
        }
        "pptx" => {
            let t = pptx_text(bytes)?;
            Ok(t)
        }
        other => Err(format!("不支持的富文本类型 .{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn docx_paragraphs_become_lines() {
        // 真实 docx 里 </w:p> 是两个标签：`/w:r` 与 `/w:p`，扫描器只看本地名
        let xml = "<w:document><w:body>\
            <w:p><w:r><w:t>第一段</w:t></w:r></w:p>\
            <w:p><w:r><w:t>第二段</w:t></w:r><w:r><w:t>的后半句</w:t></w:r></w:p>\
            </w:body></w:document>";
        assert_eq!(docx_text(xml), "第一段\n第二段的后半句\n");
    }

    #[test]
    fn docx_run_split_inside_a_paragraph_stays_on_one_line() {
        // Word 会把一句话切成多个 run（改一次格式就多一个）。
        // 同一段里的多个 <w:t> 必须拼在**同一行**，否则每个 run 都会变成一段。
        let xml = "<w:p><w:r><w:t>营收</w:t></w:r><w:r><w:t>增长</w:t></w:r><w:r><w:t>明显</w:t></w:r></w:p>";
        assert_eq!(docx_text(xml), "营收增长明显\n");
    }

    #[test]
    fn docx_br_is_a_line_break_but_stays_its_own_paragraph() {
        let xml = "<w:p><w:r><w:t>上句</w:t><w:br/><w:t>下句</w:t></w:r></w:p>";
        assert_eq!(docx_text(xml), "上句\n下句\n");
    }

    #[test]
    fn entities_are_decoded() {
        let xml = "<w:p><w:r><w:t>R&amp;D 投入 &lt;10% “A&amp;B”&#65;</w:t></w:r></w:p>";
        // &#65; = 'A'
        assert!(docx_text(xml).contains("R&D 投入 <10% “A&B”A"));
    }

    #[test]
    fn unescape_leaves_unknown_entities_alone() {
        // 认不出来的实体既不能吞掉、也不能崩 —— 原样留着
        assert_eq!(unescape_xml("a&nbsp;b"), "a&nbsp;b");
        // 孤零零一个 & 后面一大段没有分号：当字面量
        assert_eq!(unescape_xml("Tom & Jerry 很好看"), "Tom & Jerry 很好看");
    }

    #[test]
    fn pptx_skips_the_slide_number_field() {
        // 备注页第一个 shape 是"幻灯片编号"字段 —— 必须排掉，
        // 否则每页开头都会多一个孤立的数字行
        let xml = "<p:sp><p:txBody>\
            <a:p><a:fld id=\"{1}\" type=\"slidenum\"><a:t>3</a:t></a:fld></a:p>\
            <a:p><a:r><a:t>各位好，本页讲成本。</a:t></a:r></a:p>\
            </p:txBody></p:sp>";
        assert_eq!(pptx_page_text(xml), "\n各位好，本页讲成本。\n");
        assert!(!pptx_page_text(xml).contains('3'));
    }

    #[test]
    fn slide_index_only_accepts_plain_numbers() {
        assert_eq!(slide_index("ppt/notesSlides/notesSlide12.xml", "ppt/notesSlides", "notesSlide"), Some(12));
        assert_eq!(slide_index("ppt/slides/slide1.xml", "ppt/slides", "slide"), Some(1));
        // 这些都不该被当成分页文件
        assert_eq!(slide_index("ppt/notesSlides/_rels/notesSlide1.xml.rels", "ppt/notesSlides", "notesSlide"), None);
        assert_eq!(slide_index("ppt/slides/slideLayout1.xml", "ppt/slides", "slide"), None);
        assert_eq!(slide_index("word/document.xml", "ppt/slides", "slide"), None);
    }

    #[test]
    fn tidy_collapses_runs_of_blank_lines() {
        assert_eq!(tidy("甲\n\n\n\n乙\n"), "甲\n\n乙");
        assert_eq!(tidy("\n\n甲\n\n"), "甲");
    }

    /// 造一个最小的 .docx（就是个 zip）并验证端到端抽取。
    #[test]
    fn extracts_a_real_zip_packaged_docx() {
        let xml = "<w:document><w:body>\
            <w:p><w:r><w:t>季度复盘</w:t></w:r></w:p>\
            <w:p><w:r><w:t>营收环比提升。</w:t></w:r></w:p>\
            </w:body></w:document>";

        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        z.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(xml.as_bytes()).unwrap();
        let bytes = z.finish().unwrap().into_inner();

        let text = extract_rich(&bytes, "docx").unwrap();
        assert_eq!(text, "季度复盘\n营收环比提升。");
    }

    #[test]
    fn rejects_a_docx_without_a_body() {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        z.start_file("word/theme/theme1.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"<a/>").unwrap();
        let bytes = z.finish().unwrap().into_inner();

        let err = extract_rich(&bytes, "docx").unwrap_err();
        assert!(err.contains("找不到正文"), "报错该说清原因，实际：{err}");
        assert!(err.contains("当前稿件未改动"), "必须承诺不动当前稿件：{err}");
    }

    #[test]
    fn pptx_prefers_notes_over_slides() {
        let notes = "<p:sp><p:txBody><a:p><a:r><a:t>备注里的讲稿</a:t></a:r></a:p></p:txBody></p:sp>";
        let slide = "<p:sp><p:txBody><a:p><a:r><a:t>幻灯片标题</a:t></a:r></a:p></p:txBody></p:sp>";

        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        z.start_file("ppt/slides/slide1.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(slide.as_bytes()).unwrap();
        z.start_file("ppt/notesSlides/notesSlide1.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(notes.as_bytes()).unwrap();
        let bytes = z.finish().unwrap().into_inner();

        let text = extract_rich(&bytes, "pptx").unwrap();
        assert_eq!(text, "备注里的讲稿");
        assert!(!text.contains("幻灯片标题"), "有备注时不该退到正文");
    }

    #[test]
    fn pptx_falls_back_to_slides_when_there_are_no_notes() {
        let slide = "<p:sp><p:txBody><a:p><a:r><a:t>只有正文</a:t></a:r></a:p></p:txBody></p:sp>";
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        z.start_file("ppt/slides/slide2.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(slide.as_bytes()).unwrap();
        let bytes = z.finish().unwrap().into_inner();

        assert_eq!(extract_rich(&bytes, "pptx").unwrap(), "只有正文");
    }

    #[test]
    fn pptx_pages_are_joined_in_numeric_order() {
        let page = |t: &str| format!("<p:sp><p:txBody><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp>");
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        // 故意乱序写入：10 排在 2 前面（字符串序会这样排）
        for (n, t) in [(10u32, "第十页"), (2, "第二页"), (1, "第一页")] {
            z.start_file(
                format!("ppt/notesSlides/notesSlide{n}.xml"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(page(t).as_bytes()).unwrap();
        }
        let bytes = z.finish().unwrap().into_inner();

        // 必须是**数字序**：第一页 → 第二页 → 第十页。
        // 若按字符串排序会得到 第一页 → 第十页 → 第二页，讲稿顺序就乱了。
        assert_eq!(
            extract_rich(&bytes, "pptx").unwrap(),
            "第一页\n\n第二页\n\n第十页"
        );
    }

    #[test]
    fn garbage_bytes_report_a_readable_error() {
        let err = extract_rich(b"this is not a zip at all", "docx").unwrap_err();
        assert!(err.contains("不是有效的 Office 文件"), "实际：{err}");
        // 关键：不能说成"找不到正文"。把 .doc 改名成 .docx 是最常见的误用，
        // 报错必须指向真正的问题（格式不对），否则用户会去文档里翻半天。
        assert!(
            !err.contains("找不到正文"),
            "格式错被报成了内容错，会把用户引到错误的方向：{err}"
        );
    }

    #[test]
    fn ext_helpers_are_consistent() {
        assert_eq!(ext_of(Path::new("C:/a/b/稿件.DOCX")), "docx");
        assert_eq!(ext_of(Path::new("C:/a/b/无扩展名")), "");
        assert!(is_rich_ext("docx"));
        assert!(is_rich_ext("pptx"));
        assert!(!is_rich_ext("txt"));
        // 清单里的每一个都必须被认作可载入 —— 这条防的是"清单加了名字、
        // is_supported_ext 忘了同步"
        for e in SCRIPT_EXTS {
            assert!(is_supported_ext(e), "{e} 在清单里却不被支持");
        }
        assert!(!is_supported_ext("doc"));
        assert!(!is_supported_ext("pdf"));
    }

    /// `SCRIPT_EXTS` = 富文本 ∪ 纯文本，两边不能有交叠、不能有遗漏。
    ///
    /// 这条守的是分派逻辑：`load_into_engine` 用 `is_rich_ext` 决定
    /// "解包抽文本"还是"当字节解码"。若某个扩展名同时在两张表里，
    /// 走哪条路就取决于判断顺序 —— 这种"改一行代码就换条路"的隐式依赖
    /// 必须在这里被拦住。
    #[test]
    fn script_exts_is_exactly_rich_plus_plain() {
        let rich: Vec<&str> = RICH_EXTS.to_vec();
        let plain: Vec<&str> = SCRIPT_EXTS
            .iter()
            .copied()
            .filter(|e| !rich.contains(e))
            .collect();

        // 没有交叠
        for e in &rich {
            assert!(
                !plain.contains(e),
                "{e} 既被当成富文本又被当成纯文本"
            );
        }
        // 合起来正好是全集，且不含重复项
        let mut all: Vec<&str> = rich.iter().copied().chain(plain.iter().copied()).collect();
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n, "SCRIPT_EXTS 里有重复项");
        assert_eq!(all.len(), SCRIPT_EXTS.len());
        // 纯文本那一侧至少要有 txt —— 否则"拖 .txt"这条最基本的路会断掉
        assert!(plain.contains(&"txt"));
    }
}
