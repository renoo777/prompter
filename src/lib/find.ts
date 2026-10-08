// 稿内搜索：把「查什么」变成「命中在哪」的纯函数。
//
// 独立成模块而不是写在组件里，是因为**有两处要问同一个问题**：
// 查找栏要数出「第 3 / 共 12 处」并翻页，稿件区要把命中的字标出来。
// 两边各写一遍匹配逻辑，迟早会在"大小写怎么算""空查询怎么办"上分叉 ——
// 表现是"翻页数着有 12 处，画面上只标出 11 处"，很难查。

import type { ScriptPayload } from "../types";

export interface Hit {
  /** 在**全部命中**里的序号，查找栏的「第几处」就是它。 */
  ordinal: number;
  paragraph: number;
  sentence: number;
  /** 该句在归一化序列里的起点。要真从这儿开始念时传给后端的就是它。 */
  align_start: number;
  /** 命中范围，相对 `Sentence.text` 的字符（非字节）下标，左闭右开。 */
  from: number;
  to: number;
  /** 该句原文，用于可能的结果列表。 */
  text: string;
}

/** 空查询一律返回空数组 —— 而不是"匹配所有"。 */
export function findHits(script: ScriptPayload | null, query: string): Hit[] {
  const q = query.trim();
  if (!script || q.length === 0) return [];

  const hits: Hit[] = [];
  let ordinal = 0;

  for (const p of script.paragraphs) {
    for (const s of p.sentences) {
      // 一律小写比较：中文不受影响，英文则符合"搜 okr 也能找到 OKR"的直觉。
      // 用 indexOf 而不是正则，是为了让 `(` `[` `.` 这类字符不必转义 ——
      // 搜稿子不该需要先学会正则。
      //
      // 但 `toLowerCase()` **不保证长度不变**（例如 'İ'.toLowerCase() 会变成两个字符）。
      // 长度一变，命中下标就不再对应 `s.text` 的字符位置，高亮会整体错位。
      // 所以这里加一道守卫：长度不一致的句子退回**区分大小写**匹配 ——
      // 宁可少匹配几个字母，也不能把高亮画到别的字上。
      const lower = s.text.toLowerCase();
      const same = lower.length === s.text.length;
      const hay = same ? lower : s.text;
      const needle = same ? q.toLowerCase() : q;

      let at = hay.indexOf(needle);
      while (at !== -1) {
        hits.push({
          ordinal: ordinal++,
          paragraph: p.index,
          sentence: s.index,
          align_start: s.align_start,
          from: at,
          to: at + needle.length,
          text: s.text,
        });
        // 从命中之后继续找，避免"aaa"里搜"aa"时原地打转
        at = hay.indexOf(needle, at + needle.length);
      }
    }
  }
  return hits;
}

/** 按「段:句」聚合，供稿件区渲染高亮用。 */
export function groupHits(hits: Hit[]): Map<string, Hit[]> {
  const m = new Map<string, Hit[]>();
  for (const h of hits) {
    const k = `${h.paragraph}:${h.sentence}`;
    const list = m.get(k);
    if (list) list.push(h);
    else m.set(k, [h]);
  }
  return m;
}

/** 同一句在稿件区里的稳定键。 */
export function seKey(paragraph: number, sentence: number): string {
  return `${paragraph}:${sentence}`;
}
