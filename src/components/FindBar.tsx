import { useEffect, useMemo, useRef } from "react";

import { api, errText } from "../lib/api";
import { findHits } from "../lib/find";
import { useApp } from "../lib/store";

/**
 * 稿内查找。
 *
 * # 为什么「查找」默认**不动**朗读进度
 *
 * 会前翻稿子找"Q&A 那一页"时，如果每按一次下一处都把朗读光标搬过去，
 * 用户找完一圈之后就已经不在原来念的地方了 —— 找位置这个动作本身
 * 把位置弄丢了。所以查找只做"滚过去 + 标出来"。
 *
 * 真要从此处开始念，有独立的「从这处开始读」按钮，那一下才动光标。
 * 把两个意图分开，用户不需要猜"我刚才那下算不算跳转"。
 *
 * # 为什么计数要和稿件区共用 `findHits`
 *
 * 见 `lib/find.ts` 的模块注释 —— 两处各算一遍必然分叉。
 */
export function FindBar() {
  const { state, dispatch } = useApp();
  const { findOpen, findQuery, findIndex, script } = state;
  const inputRef = useRef<HTMLInputElement>(null);

  const hits = useMemo(() => findHits(script, findQuery), [script, findQuery]);
  const total = hits.length;
  const cur = total === 0 ? -1 : Math.min(findIndex, total - 1);
  const active = cur >= 0 ? hits[cur] : null;

  useEffect(() => {
    if (findOpen) {
      // 打开就聚焦并全选 —— 接着改关键词是最常见的下一步
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [findOpen]);

  // ★★ 注意：`if (!findOpen) return null;` 是**提前返回** —— 将来往这个
  //   组件里加任何 hook，都必须写在它**之前**（React 规则 #310，踩过一次）。
  if (!findOpen) return null;

  function go(delta: number) {
    if (total === 0) return;
    // 环形翻页：找到最后一处再按下一处回到第一处，比"到头就没反应"好判断
    const next = (cur + delta + total) % total;
    dispatch({ type: "FIND_INDEX", index: next });
  }

  /** 只把**朗读光标**搬到当前命中处（查找本身不动光标，见组件注释）。 */
  async function readFromHere() {
    if (!active) return;
    try {
      const c = await api.goto(active.align_start);
      dispatch({ type: "CURSOR", cursor: c });
      dispatch({ type: "FIND", open: false });
    } catch (e) {
      dispatch({ type: "ERROR", message: errText(e) });
    }
  }

  return (
    <div className="findbar" data-interactive>
      <span className="fb-icon">⌕</span>
      <input
        ref={inputRef}
        className="fb-input"
        value={findQuery}
        placeholder="在稿件里找…"
        spellCheck={false}
        onChange={(e) => dispatch({ type: "FIND_QUERY", query: e.target.value })}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            go(e.shiftKey ? -1 : 1);
          } else if (e.key === "Escape") {
            e.preventDefault();
            dispatch({ type: "FIND", open: false });
          }
        }}
      />

      {/*
        计数必须**始终**显示，包括"0 处"。
        空着的话用户分不清"没找到"和"还没输入" —— 这是查找框最常见的困惑。
      */}
      <span className={`fb-count${total === 0 ? " none" : ""}`}>
        {findQuery.trim().length === 0
          ? ""
          : total === 0
            ? "没有匹配"
            : `${cur + 1} / ${total}`}
      </span>

      <button className="icon-btn sm" disabled={total === 0} onClick={() => go(-1)}>
        ↑
      </button>
      <button className="icon-btn sm" disabled={total === 0} onClick={() => go(1)}>
        ↓
      </button>

      <button className="btn" disabled={!active} onClick={() => void readFromHere()}>
        从这处开始读
      </button>

      <button
        className="icon-btn sm"
        onClick={() => dispatch({ type: "FIND", open: false })}
      >
        ✕
      </button>
    </div>
  );
}
