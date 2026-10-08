import { useEffect, useRef, useState } from "react";

import { api, errText } from "../lib/api";
import { useApp } from "../lib/store";
import type { ScriptPayload } from "../types";

/**
 * 把稿件还原成可编辑的纯文本。
 *
 * 段与段之间**空一行** —— `build_script` 正是按空行分段的。
 * 不用空行的话，"打开编辑 → 直接保存"会把所有段落粘成一段，
 * 于是排版全变、段落导航（上一段/下一段）也失效。
 */
function scriptToText(s: ScriptPayload | null): string {
  if (!s) return "";
  return s.paragraphs.map((p) => p.text).join("\n\n");
}

/**
 * 粘贴 / 编辑稿件。
 *
 * # 为什么这两个功能共用一个浮层
 *
 * "从别处弄一段文字进来"和"改一改当前这段"对用户是**同一件事的两种起点**：
 * 都是在一块大文本框里敲字，保存后得到一份可朗读的稿子。
 * 分成两个界面会让"粘贴完发现想改两个字"变成关掉再打开另一个。
 *
 * # 为什么编辑走的是「另存为一份新副本」
 *
 * 直接写回原文件是不可接受的：用户可能只是想为这次演讲调整措辞，
 * 不该顺手改掉硬盘上那份 Word。所以编辑保存走 `set_script_text`，
 * 落一份历史副本，**原文件一字不动**。界面上明确写出来，
 * 不靠用户自己推测。
 */
export function PasteImport() {
  const { state, dispatch } = useApp();
  const { pasteOpen, pasteMode } = state;

  const [text, setText] = useState("");
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const areaRef = useRef<HTMLTextAreaElement>(null);

  // 打开时决定初值：新稿留空，编辑则预填当前正文
  useEffect(() => {
    if (!pasteOpen) return;
    if (pasteMode === "edit") {
      setText(scriptToText(state.script));
      setTitle(state.script?.title ?? "");
    } else {
      setText("");
      setTitle("");
    }
    setMsg(null);
    // 只在"打开"这一拍初始化。把 state.script 放进依赖会让用户
    // 输入到一半时被重新预填、光标跳到末尾。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pasteOpen, pasteMode]);

  useEffect(() => {
    if (pasteOpen) areaRef.current?.focus();
  }, [pasteOpen]);

  useEffect(() => {
    if (!pasteOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dispatch({ type: "PASTE", open: false });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pasteOpen, dispatch]);

  // ★★ 注意：`if (!pasteOpen) return null;` 是**提前返回** —— 将来往这个
  //   组件里加任何 hook，都必须写在它**之前**（React 规则 #310，踩过一次）。
  if (!pasteOpen) return null;

  /**
   * 读剪贴板。
   *
   * 读不到时**必须给出替代路径**：WebView 里读剪贴板需要权限，被拒是常态，
   * 而"点了没反应"会让用户以为是程序坏了 —— 实际上 Ctrl+V 一直是可用的。
   */
  async function fromClipboard() {
    try {
      const t = await navigator.clipboard.readText();
      if (!t.trim()) {
        setMsg("剪贴板里没有文字。");
        return;
      }
      setText(t);
      setMsg(null);
    } catch {
      setMsg("这个环境不允许程序直接读剪贴板 —— 按 Ctrl+V 粘到上面的框里就行。");
    }
  }

  async function save() {
    if (!text.trim()) {
      setMsg("内容是空的，没有可朗读的文字。当前稿件未改动。");
      return;
    }
    setBusy(true);
    try {
      const s = await api.setScriptText(text, title.trim() || undefined);
      dispatch({ type: "SCRIPT", script: s });
      dispatch({ type: "PASTE", open: false });

      // 历史里多了一条，列表要刷新 —— 不刷的话用户关掉浮层去看历史
      // 会发现"刚存的稿子不在里面"
      try {
        const [list, avail] = await Promise.all([
          api.listScripts(),
          api.scriptAvailability(),
        ]);
        dispatch({ type: "SCRIPTS", scripts: list });
        dispatch({ type: "SCRIPT_AVAIL", avail: Object.fromEntries(avail) });
      } catch {
        /* 历史是增强功能，刷新失败不打扰用户 */
      }

      dispatch({
        type: "NOTICE",
        // 收成一句回执。原来那句尾缀着「· 原文件未改动，历史里多了一份副本」，
        // 但这两件事浮层里**早就说过了**（输入框下方那行说明 + 按钮上的「保存副本」），
        // 而且是说在你按下之前 —— 事后重复一遍只是让你多读十几个字。
        message:
          pasteMode === "edit"
            ? `已保存副本「${s.title}」（${s.total_chars} 字）`
            : `已载入「${s.title}」（${s.total_chars} 字）`,
      });
    } catch (e) {
      setMsg(errText(e));
    } finally {
      setBusy(false);
    }
  }

  const chars = Array.from(text).length;

  return (
    <div className="modal-scrim" data-interactive onMouseDown={(e) => {
      // 只有点在蒙层本身上才关 —— 点在浮层内部不该算"点空白处"
      if (e.target === e.currentTarget) dispatch({ type: "PASTE", open: false });
    }}>
      <div className="pastebox">
        <div className="pb-head">
          <span className="pb-title">
            {pasteMode === "edit" ? "编辑当前稿件" : "粘贴一段文字当稿件"}
          </span>
          <button
            className="icon-btn sm"
            onClick={() => dispatch({ type: "PASTE", open: false })}
          >
            ✕
          </button>
        </div>

        <div className="pb-hint">
          {pasteMode === "edit"
            ? "改完保存会生成一份新副本，你的原文件不会被改动。段落之间用空行分开。"
            : "从飞书文档、微信、网页、Word 里直接复制过来都行。段落之间用空行分开。"}
        </div>

        <div className="pb-row">
          <label className="pb-label" htmlFor="pb-title">
            标题
          </label>
          <input
            id="pb-title"
            className="pb-input"
            value={title}
            placeholder={pasteMode === "edit" ? "不改就沿用原标题" : "留空则自动取首行"}
            onChange={(e) => setTitle(e.target.value)}
          />
        </div>

        <textarea
          ref={areaRef}
          className="pb-area"
          value={text}
          spellCheck={false}
          placeholder={
            pasteMode === "edit"
              ? ""
              : `在这里粘贴讲稿…\n\n若不介意，也可以直接拖一个 ${state.scriptExtsHint} 文件到窗口里。`
          }
          onChange={(e) => {
            setText(e.target.value);
            if (msg) setMsg(null);
          }}
        />

        <div className="pb-foot">
          <button className="btn" onClick={() => void fromClipboard()}>
            读剪贴板
          </button>
          <span className="pb-meta">{chars.toLocaleString("zh-CN")} 字</span>
          <span className="spacer" />
          {msg && <span className="pb-msg">{msg}</span>}
          <button
            className="btn"
            disabled={busy}
            onClick={() => dispatch({ type: "PASTE", open: false })}
          >
            取消
          </button>
          <button className="btn primary" disabled={busy} onClick={() => void save()}>
            {busy ? "保存中…" : pasteMode === "edit" ? "保存副本并载入" : "载入这份稿子"}
          </button>
        </div>
      </div>
    </div>
  );
}
