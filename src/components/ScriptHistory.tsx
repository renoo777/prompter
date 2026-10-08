import { useEffect, useRef, useState } from "react";

import { api, errText } from "../lib/api";
import { useApp } from "../lib/store";
import { relTime, type ScriptEntry } from "../types";

/**
 * 历史列表里的一行。
 *
 * ★ 拆成组件的理由（第十一轮改写）：原来它单成组件是因为 `useHint` 不能在
 *   `scripts.map()` 里调用（hook 数量会随条目数变）。气泡删掉之后那个约束
 *   没了，但**仍然**留成组件：内联展开会让 `map` 那段多三层缩进，
 *   而列表项的字段（`ok` / 两个回调）是有名字的，包起来读得更清楚。
 *
 * ★ 原来挂在 `hintMain` 上的「文件已不在原位置 —— 可能是移动盘没挂上」
 *   没有丢：前一句就是下面 `.hi-meta` 里已有的「· 文件已不在原位置」，
 *   后一句在组件头注释里。行的真正用途（点它＝换稿）由它在「稿件」面板
 *   里这个位置本身说明。
 */
function HistoryItem({
  entry,
  ok,
  onActivate,
  onForget,
}: {
  entry: ScriptEntry;
  ok: boolean;
  onActivate: () => void;
  onForget: () => void;
}) {
  return (
    <div className={`history-item${ok ? "" : " missing"}`}>
      <button className="history-main" onClick={onActivate}>
        <span className="hi-title">{entry.title}</span>
        <span className="hi-meta">
          {entry.source === "pasted" ? "粘贴" : "文件"} · {relTime(entry.last_used)}
          {entry.use_count > 1 && ` · 用过 ${entry.use_count} 次`}
          {!ok && " · 文件已不在原位置"}
        </span>
      </button>
      <button className="icon-btn" onClick={onForget}>
        ✕
      </button>
    </div>
  );
}

/**
 * 稿件历史浮层。
 *
 * # 为什么放在顶栏下方而不是设置抽屉里
 *
 * 会前 60 秒切稿是**最高频**的动作。放抽屉里要多两步（打开抽屉 → 找到列表），
 * 而浮层是"点一下标题就出来了"。抽屉留给"配置一次就不动"的东西。
 *
 * # 为什么条目可以标灰但不会自动消失
 *
 * 原文件可能只是插着的移动硬盘 / 网络盘没挂上。自动移除会让用户
 * "插上硬盘回来发现历史没了"。所以只标灰 + 说明，移除必须由用户自己做。
 */
export function ScriptHistory() {
  const { state, dispatch } = useApp();
  const { historyOpen, scripts, scriptAvail } = state;
  const [confirmClear, setConfirmClear] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  // 点空白处关闭
  useEffect(() => {
    if (!historyOpen) return;
    const onDown = (e: MouseEvent) => {
      /*
       * ★★ 必须放过**打开它的那颗按钮**，否则"再点一下收起来"永远做不到。
       *
       * 事件顺序是 mousedown → mouseup → click，而 React 的 onClick 拿的是
       * **本次渲染时**的闭包：
       *   ① mousedown 在这里把 `historyOpen` 置 false（面板关了）；
       *   ② React 在同一个任务之后重渲染，按钮的 onClick 变成"当前是关着的"；
       *   ③ click 到了，它把 false 取反 ⇒ **又打开**。
       * 表现就是"点一下开、再点一下还是开" —— 用户看到的关闭发生在 mousedown
       * 与 click 之间那一瞬，肉眼追不上。
       *
       * 用 `data-panel-toggle` 标记而不是"比对某个 ref"：将来给别的面板加外点关闭时，
       * 同一个坑会原样再来一遍，标记是**能复用**的那一半。
       */
      const t = e.target as Element | null;
      if (t && typeof t.closest === "function" && t.closest("[data-panel-toggle]")) return;
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) {
        dispatch({ type: "HISTORY", open: false });
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dispatch({ type: "HISTORY", open: false });
    };
    // 用 mousedown 而不是 click：click 会在"点标题按钮"的同一拍触发，
    // 结果浮层刚打开就被自己关掉
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [historyOpen, dispatch]);

  // 三秒内没有再次点击就撤销"确认清空"状态
  useEffect(() => {
    if (!confirmClear) return;
    const t = window.setTimeout(() => setConfirmClear(false), 3000);
    return () => window.clearTimeout(t);
  }, [confirmClear]);

  // ★★ 注意：`if (!historyOpen) return null;` 是**提前返回** —— 将来往这个
  //   组件里加任何 hook，都必须写在它**之前**（React 规则 #310，踩过一次）。
  if (!historyOpen) return null;

  async function activate(e: ScriptEntry) {
    try {
      const s = await api.activateScript(e.id);
      dispatch({ type: "SCRIPT", script: s });
      // 不弹"已切到「X」（N 字）"：顶栏标题与正文在你点完的那一刻就换掉了，
      // 那是比一句提示更直接的证据 —— 而且提示会自己消失，界面变化不会。
      dispatch({ type: "HISTORY", open: false });
      const list = await api.listScripts();
      dispatch({ type: "SCRIPTS", scripts: list });
    } catch (err) {
      // 文件不在了这类失败必须说出来 —— 点了没反应是最糟的
      dispatch({ type: "ERROR", message: errText(err) });
    }
  }

  async function forget(id: string) {
    try {
      await api.forgetScript(id);
      const [list, avail] = await Promise.all([
        api.listScripts(),
        api.scriptAvailability(),
      ]);
      dispatch({ type: "SCRIPTS", scripts: list });
      dispatch({ type: "SCRIPT_AVAIL", avail: Object.fromEntries(avail) });
      // 不弹"已从历史移除「X」——原文件没有被删除"：
      // 列表里那一条当场消失就是回执；而"原文件没被删"这件事，
      // 面板底部那行常驻说明一直写着 —— 比事后补一句更早，也更不容易被错过。
    } catch (e) {
      dispatch({ type: "ERROR", message: errText(e) });
    }
  }

  /**
   * 打开文件。
   *
   * ★ 本轮改版把「打开文件 / 粘贴 / 最近」三个入口合并进了顶栏那一个「稿件」按钮
   *   —— 它们服务的是同一件事（换稿），只是来源不同。所以这两个动作搬到了这个面板里，
   *   恰好也回到了它们**语义上**该在的地方：换稿面板。
   *
   * 扩展名清单从 `state.scriptExts` 取，**不在前端再写一份** ——
   * 曾经前端硬编码 `["txt","md"]`，后端加了新格式之后表现为
   * "拖进去能载入、但从「打开」里选不到"。
   */
  async function pickFile() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        filters: [{ name: "稿件", extensions: state.scriptExts }],
      });
      if (typeof picked !== "string") return;
      const s = await api.loadScript(picked);
      dispatch({ type: "SCRIPT", script: s });
      dispatch({ type: "HISTORY", open: false });
      const list = await api.listScripts();
      dispatch({ type: "SCRIPTS", scripts: list });
    } catch (e) {
      dispatch({ type: "ERROR", message: errText(e) });
    }
  }

  async function clearAll() {
    if (!confirmClear) {
      setConfirmClear(true);
      return;
    }
    setConfirmClear(false);
    try {
      await api.clearHistory();
      dispatch({ type: "SCRIPTS", scripts: [] });
      dispatch({ type: "SCRIPT_AVAIL", avail: {} });
      // 不弹"历史已清空，你的原始文件一个都没被删除"。
      // 这件事**面板底部那行常驻说明已经写着**（见 history-foot 的 note）——
      // 而且它出现在你按下按钮**之前**，比事后补一句更早、更不容易被错过。
      // 一次性弹窗讲一件常驻的事，本来就多此一举。
    } catch (e) {
      dispatch({ type: "ERROR", message: errText(e) });
    }
  }

  return (
    <div className="history" ref={boxRef} data-interactive>
      <div className="history-head">
        <span>稿件</span>
        <span className="count">{scripts.length}</span>
      </div>

      {/*
        换稿三件事里"新来源"的两件。第三件「最近」就是下面那个列表。
        ★ 放在**头部**而不是底部：这个面板的首要用途是换稿，
          而"从文件打开"和"粘贴一段"是换稿里最先想到的两条路，
          不该要用户先滚过一屏历史才找得到。
      */}
      <div className="history-act">
        <button className="btn primary" onClick={() => void pickFile()}>
          打开文件
        </button>
        <button
          className="btn"
          onClick={() => {
            dispatch({ type: "HISTORY", open: false });
            dispatch({ type: "PASTE", open: true, mode: "new" });
          }}
        >
          粘贴一段
        </button>
      </div>

      {scripts.length === 0 ? (
        <div className="history-empty">
          还没有用过的稿件。
          <br />
          拖一个 {state.scriptExtsHint} 文件进来，或者点上面的「打开文件」，之后就会出现在这里。
        </div>
      ) : (
        <div className="history-list">
          {scripts.map((e) => {
            const ok = scriptAvail[e.id] !== false;
            return (
                <HistoryItem
                  key={e.id}
                  entry={e}
                  ok={ok}
                  onActivate={() => void activate(e)}
                  onForget={() => void forget(e.id)}
                />
            );
          })}
        </div>
      )}

      {scripts.length > 0 && (
        <div className="history-foot">
          <button
            className={`btn${confirmClear ? " primary" : ""}`}
            onClick={() => void clearAll()}
          >
            {confirmClear ? "再点一次确认清空" : "清空历史"}
          </button>
          <span className="note">只删历史与副本，不动你的原文件</span>
        </div>
      )}
    </div>
  );
}
