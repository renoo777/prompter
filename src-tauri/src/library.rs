//! 稿件库：最近用过的稿件索引 + 粘贴稿的正文副本。
//!
//! # 为什么这个模块不认识 Tauri
//!
//! 与 `script` / `align` / `stealth` 同级：这里只有数据结构和纯函数，
//! 不碰文件系统、不碰窗口、不碰事件。文件读写留在 `commands.rs`。
//! 这样「去重、淘汰、迁移」这些真正会出错的地方可以脱离整个应用单独跑单测。
//!
//! # 三个关键设计决策（都有代价，写在这里免得以后被"优化"掉）
//!
//! 1. **文件类稿件只存路径，不存正文副本。**
//!    用户的原文件是唯一真相源。若我们存副本，用户改了原文件而我们显示旧的 ——
//!    这种"两份真相"是提词器最不可接受的错误（照着念的是一段过期内容）。
//!    代价：列表里的字符数是**载入当时**的值，原文件改动后会过期。
//!
//! 2. **粘贴类稿件必须存副本。**
//!    它没有原文件可依。若不存，历史里会有条目点开是空的 —— 比没有历史更糟。
//!
//! 3. **`id` 的生成规则不对称。**
//!    文件类用**路径**的 hash（文件改内容仍算同一份稿）；粘贴类用**内容**的 hash
//!    （同一段文本粘两次应当合并成一条）。代价：文件改名/移动会产生新条目，
//!    可接受 —— 用户自己删旧的就是。

use serde::{Deserialize, Serialize};

/// 索引文件格式版本。将来若改结构，靠它做迁移而不是猜。
pub const LIBRARY_VERSION: u32 = 1;

/// 历史上限。会议场景实际用不到几十条，超过就淘汰最久未用的。
pub const MAX_ENTRIES: usize = 50;

/// 粘贴类稿件的正文副本放在 `<配置目录>/scripts/` 下。
pub const COPY_DIR: &str = "scripts";

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptSource {
    /// 来自磁盘上的一个文件 —— 只存路径。
    File,
    /// 粘贴进来的（或示例稿）—— 必须存副本。
    Pasted,
}

/// 历史列表里的一条。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptEntry {
    pub id: String,
    pub source: ScriptSource,
    /// 仅 file 类：用户原文件的绝对路径。
    #[serde(default)]
    pub path: Option<String>,
    /// 显示名 —— 文件名去扩展名，或粘贴时给的标题。
    pub title: String,
    /// 字符数。**载入当时**的快照，原文件改动后会过期。
    pub chars: usize,
    /// Unix 秒。排序依据。
    pub last_used: i64,
    #[serde(default)]
    pub use_count: u32,
    /// 仅 pasted 类：相对 `<配置目录>` 的副本路径，形如 `scripts/<id>.txt`。
    #[serde(default)]
    pub copy_rel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptLibrary {
    pub version: u32,
    pub entries: Vec<ScriptEntry>,
}

impl Default for ScriptLibrary {
    fn default() -> Self {
        Self {
            version: LIBRARY_VERSION,
            entries: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 标识生成
// ---------------------------------------------------------------------------

/// FNV-1a 64 位。用它而不是 `DefaultHasher` 是为了**跨编译器版本稳定** ——
/// `DefaultHasher` 的输出没有稳定性保证，换个 Rust 版本算出来的 id 就变了，
/// 历史条目会全部对不上（表现是"列表里一堆点不开的旧条目"）。
pub fn hash_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 路径归一化：分隔符统一、去掉尾部分隔符，Windows 上再统一大小写。
///
/// 只在 Windows 下降大小写 —— 类 Unix 文件系统区分大小写，
/// 在那里降大小写会把 `A.txt` 和 `a.txt` 两份不同的稿子合并成一条。
pub fn normalize_path(p: &str) -> String {
    let mut s = p.trim().replace('/', std::path::MAIN_SEPARATOR_STR);
    while s.len() > 3 && s.ends_with(std::path::MAIN_SEPARATOR) {
        s.pop();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    s
}

/// 文件类条目的 id。
pub fn file_id(path: &str) -> String {
    format!("f{}", hash_hex(&normalize_path(path)))
}

/// 粘贴类条目的 id —— 按内容算，同一段文本粘两次会合并。
pub fn pasted_id(text: &str) -> String {
    format!("p{}", hash_hex(text))
}

/// 粘贴类稿件的副本文件名。
pub fn copy_name(id: &str) -> String {
    format!("{id}.txt")
}

/// 由 `copy_rel` 取文件名（删除副本时用）。
pub fn copy_rel_of(id: &str) -> String {
    format!("{COPY_DIR}/{name}", name = copy_name(id))
}

// ---------------------------------------------------------------------------
// 库操作
// ---------------------------------------------------------------------------

impl ScriptLibrary {
    /// 按最后使用时间倒序 —— 列表的显示顺序。
    pub fn sorted(&self) -> Vec<&ScriptEntry> {
        let mut v: Vec<&ScriptEntry> = self.entries.iter().collect();
        // 同一秒内的多条要有确定顺序，否则列表每次渲染都可能换位置
        v.sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.id.cmp(&b.id)));
        v
    }

    pub fn entry(&self, id: &str) -> Option<&ScriptEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// 记一次文件类稿件的使用。返回 (id, 是否新建)。
    ///
    /// 重复载入同一路径**不产生第二条**，只刷新时间与计数。
    pub fn upsert_file(
        &mut self,
        path: &str,
        title: &str,
        chars: usize,
        now: i64,
    ) -> (String, bool) {
        let id = file_id(path);
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.title = title.to_string();
            e.chars = chars;
            e.last_used = now;
            e.use_count = e.use_count.saturating_add(1);
            // 路径大小写可能变了（Windows），把规范化的那份存回去
            e.path = Some(path.to_string());
            return (id, false);
        }
        self.entries.push(ScriptEntry {
            id: id.clone(),
            source: ScriptSource::File,
            path: Some(path.to_string()),
            title: title.to_string(),
            chars,
            last_used: now,
            use_count: 1,
            copy_rel: None,
        });
        (id, true)
    }

    /// 记一次粘贴类稿件的使用。返回 (id, 是否需要写副本)。
    ///
    /// 每次都返回 `true` 让调用方重写副本 —— 内容相同意味着 hash 相同、
    /// 重写是幂等的；内容不同则 hash 不同，会是一条新条目。两种情况都对。
    pub fn upsert_pasted(&mut self, text: &str, title: &str, chars: usize, now: i64) -> String {
        let id = pasted_id(text);
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.title = title.to_string();
            e.chars = chars;
            e.last_used = now;
            e.use_count = e.use_count.saturating_add(1);
            return id;
        }
        self.entries.push(ScriptEntry {
            id: id.clone(),
            source: ScriptSource::Pasted,
            path: None,
            title: title.to_string(),
            chars,
            last_used: now,
            use_count: 1,
            copy_rel: Some(copy_rel_of(&id)),
        });
        id
    }

    /// 移除一条。返回**需要删除的副本相对路径**（文件类返回 None）。
    ///
    /// 只返回路径、不真的删文件 —— 删除是 IO 层的事，这里保持纯粹。
    pub fn remove(&mut self, id: &str) -> Option<String> {
        let idx = self.entries.iter().position(|e| e.id == id)?;
        let e = self.entries.remove(idx);
        e.copy_rel
    }

    /// 清空。返回所有需要删除的副本相对路径。
    ///
    /// ⚠️ 这里返回的**只是副本**。调用方绝不允许顺着 `path` 去删用户的原文件 ——
    /// 那是用户自己的稿子，不属于我们。单测 `clear_only_returns_copies` 钉住这条。
    pub fn clear(&mut self) -> Vec<String> {
        let out: Vec<String> = self.entries.iter().filter_map(|e| e.copy_rel.clone()).collect();
        self.entries.clear();
        out
    }

    /// 淘汰到 `limit` 条以内，返回被淘汰条目的副本路径。
    ///
    /// `protected` 指向的那条**永不被淘汰** —— 它是"上次用的稿子"，
    /// 下次启动要自动载入。把它淘汰掉等于让用户升级后丢掉上次的稿。
    pub fn prune(&mut self, limit: usize, protected: Option<&str>) -> Vec<String> {
        let mut removed = Vec::new();
        while self.entries.len() > limit {
            // 找 last_used 最旧且不是 protected 的那条
            let victim = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| Some(e.id.as_str()) != protected)
                .min_by(|(_, a), (_, b)| a.last_used.cmp(&b.last_used).then_with(|| a.id.cmp(&b.id)))
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    let e = self.entries.remove(i);
                    if let Some(c) = e.copy_rel {
                        removed.push(c);
                    }
                }
                // 剩下的全是 protected：limit 小于受保护条目数时停手，
                // 宁可超限也不能把"上次的稿子"删了。
                None => break,
            }
        }
        removed
    }

    /// 从旧配置的 `last_script` 无损迁移。
    ///
    /// 老用户升级后"上次的稿子"必须还在 —— 直接删掉 `last_script` 字段会让这个
    /// 用户可见行为**静默消失**（`serde(default)` 不会报错，只会悄悄少一个功能）。
    /// 返回是否真的新建了条目。
    pub fn migrate_file(&mut self, path: &str, title: &str, now: i64) -> bool {
        let (_, created) = self.upsert_file(path, title, 0, now);
        created
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_with(n: usize) -> ScriptLibrary {
        let mut l = ScriptLibrary::default();
        for i in 0..n {
            l.upsert_file(&format!("C:/docs/s{i}.txt"), &format!("s{i}"), 100 + i, 1000 + i as i64);
        }
        l
    }

    #[test]
    fn hash_is_stable_and_hex() {
        // 钉住具体值：一旦有人换成 DefaultHasher，这条会红，
        // 从而挡住"用户历史条目全部失效"这种静默回归
        assert_eq!(hash_hex(""), "cbf29ce484222325");
        assert_eq!(hash_hex("abc"), "e71fa2190541574b");
        assert_eq!(hash_hex("abc").len(), 16);
    }

    #[test]
    fn file_id_ignores_separator_style_and_trailing_slash() {
        let a = file_id("C:/docs/report.txt");
        let b = file_id("C:\\docs\\report.txt");
        let c = file_id("C:/docs/report.txt/");
        assert_eq!(a, b, "同一文件用正/反斜杠写必须是同一条");
        assert_eq!(a, c, "末尾多一个分隔符不该变成新条目");
    }

    #[test]
    fn file_id_is_case_insensitive_only_on_windows() {
        let a = file_id("C:/Docs/Report.txt");
        let b = file_id("C:/docs/report.txt");
        if cfg!(windows) {
            assert_eq!(a, b, "Windows 路径不区分大小写，否则同一文件会有两条历史");
        } else {
            assert_ne!(a, b, "类 Unix 区分大小写，降大小写会把两份不同的稿合并");
        }
    }

    #[test]
    fn dedupe_file_updates_instead_of_appending() {
        let mut l = ScriptLibrary::default();
        for i in 0..3 {
            l.upsert_file("C:/a.txt", "a", 42, 100 + i);
        }
        assert_eq!(l.entries.len(), 1, "同一路径载入 3 次仍应只有一条");
        assert_eq!(l.entries[0].use_count, 3);
        assert_eq!(l.entries[0].last_used, 102);
        assert_eq!(l.entries[0].source, ScriptSource::File);
        assert!(l.entries[0].copy_rel.is_none(), "文件类不存副本");
    }

    #[test]
    fn dedupe_pasted_by_content() {
        let mut l = ScriptLibrary::default();
        l.upsert_pasted("各位好", "粘贴的稿件", 3, 10);
        l.upsert_pasted("各位好", "粘贴的稿件", 3, 20);
        l.upsert_pasted("各位好，今天", "粘贴的稿件", 6, 30);
        assert_eq!(l.entries.len(), 2, "同内容合并，内容变了才是新条目");
        assert_eq!(l.entries.len(), l.entries.iter().filter(|e| e.copy_rel.is_some()).count());
        assert_eq!(l.sorted()[0].chars, 6, "最新的排最前");
    }

    #[test]
    fn remove_returns_copy_only_for_pasted() {
        let mut l = ScriptLibrary::default();
        l.upsert_file("C:/a.txt", "a", 10, 1);
        let pid = l.upsert_pasted("正文", "粘贴", 2, 2);

        let fid = file_id("C:/a.txt");
        assert_eq!(l.remove(&fid), None, "文件类没有副本可删");
        assert_eq!(l.entries.len(), 1);

        assert_eq!(l.remove(&pid), Some(copy_rel_of(&pid)));
        assert!(l.entries.is_empty());
        assert_eq!(l.remove("不存在"), None, "删不存在的条目不该 panic");
    }

    #[test]
    fn clear_only_returns_copies() {
        let mut l = ScriptLibrary::default();
        l.upsert_file("C:/重要稿子.txt", "重要稿子", 10, 1);
        let pid = l.upsert_pasted("正文", "粘贴", 2, 2);

        let outs = l.clear();
        assert_eq!(outs, vec![copy_rel_of(&pid)], "只返回副本，绝不包含用户原文件");
        assert!(!outs.iter().any(|p| p.contains("重要稿子")), "用户原文件一个都不能动");
        assert!(l.entries.is_empty());
    }

    #[test]
    fn prune_drops_oldest_and_keeps_protected() {
        let mut l = lib_with(5);
        let protected = file_id("C:/docs/s0.txt"); // 最旧的那条
        let removed = l.prune(3, Some(&protected));

        assert_eq!(l.entries.len(), 3);
        assert!(l.entry(&protected).is_some(), "protected 必须留下");
        // 淘汰的是次旧的 s1、s2
        assert!(l.entry(&file_id("C:/docs/s1.txt")).is_none());
        assert!(l.entry(&file_id("C:/docs/s2.txt")).is_none());
        assert!(removed.is_empty(), "这几条都是文件类，没有副本要删");
    }

    #[test]
    fn prune_returns_copy_paths_of_evicted_pasted_entries() {
        let mut l = ScriptLibrary::default();
        let old = l.upsert_pasted("旧的", "旧的", 2, 1);
        l.upsert_file("C:/b.txt", "b", 2, 2);
        l.upsert_file("C:/c.txt", "c", 2, 3);

        let removed = l.prune(2, None);
        assert_eq!(removed, vec![copy_rel_of(&old)], "被淘汰的粘贴稿要连副本一起回收");
        assert_eq!(l.entries.len(), 2);
    }

    #[test]
    fn prune_keeps_protected_even_when_limit_is_too_small() {
        let mut l = lib_with(3);
        let protected = file_id("C:/docs/s0.txt");
        // limit=0 但只有一条 protected —— 结果必然超限，这是刻意的：
        // 宁可列表超限，也不能把"上次的稿子"删掉
        let removed = l.prune(0, Some(&protected));
        assert_eq!(l.entries.len(), 1);
        assert!(l.entry(&protected).is_some());
        assert!(removed.is_empty());
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut l = ScriptLibrary::default();
        assert!(l.migrate_file("C:/last.txt", "last", 500), "首次迁移应当新建条目");
        assert!(!l.migrate_file("C:/last.txt", "last", 501), "再跑一次不该产生第二条");
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].use_count, 2);
    }

    #[test]
    fn sorted_is_recent_first_and_deterministic_within_a_second() {
        let mut l = ScriptLibrary::default();
        l.upsert_file("C:/a.txt", "a", 1, 100);
        l.upsert_file("C:/b.txt", "b", 1, 100); // 同一秒
        l.upsert_file("C:/c.txt", "c", 1, 200);

        let order = l.sorted().iter().map(|e| e.title.clone()).collect::<Vec<_>>();
        assert_eq!(order[0], "c");
        // 同秒的两条按 id 定序 —— 保证每次渲染顺序一致
        let again = l.sorted().iter().map(|e| e.title.clone()).collect::<Vec<_>>();
        assert_eq!(order, again);
    }

    #[test]
    fn library_round_trips_through_json() {
        let mut l = ScriptLibrary::default();
        l.upsert_file("C:/a.txt", "a", 10, 1);
        l.upsert_pasted("正文", "粘贴", 2, 2);
        let json = serde_json::to_string(&l).unwrap();
        let back: ScriptLibrary = serde_json::from_str(&json).unwrap();
        assert_eq!(l, back);
    }

    #[test]
    fn empty_object_deserializes_to_empty_library() {
        // 索引文件被写空（`{}`）时也要能起来，不能因为缺字段就整个崩掉
        let l: ScriptLibrary = serde_json::from_str("{}").unwrap();
        assert_eq!(l.version, LIBRARY_VERSION);
        assert!(l.entries.is_empty());
    }
}
