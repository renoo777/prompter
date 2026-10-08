import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent, ReactNode } from "react";

import { api } from "../lib/api";
import { findHits, groupHits, seKey, type Hit } from "../lib/find";
import { useApp } from "../lib/store";
import type { Paragraph } from "../types";

/**
 * 手动接管后「让位」多久（毫秒）。
 *
 * 4 秒是两件事的折中：
 *   * 太短 —— 用户还在往回翻看某一段，自动滚动就把画面拽走了，等于没接管；
 *   * 太长 —— 用户临时瞄一眼之后想恢复跟读，得干等它。
 * 这个值是**估值**，真机试过之后应当按手感调（和穿透的 `APPROACH_PAD` 同类）。
 */
const MANUAL_HOLD_MS = 4000;

/** 空态：没载入稿件时给一个能立刻上手的入口，而不是一片空白。 */
function EmptyState() {
  const { state, dispatch } = useApp();
  const { scriptExtsHint } = state;

  /** 打开系统文件对话框。扩展名清单**从后端来**，不在前端写死。 */
  async function openScript() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        filters: [{ name: "稿件", extensions: state.scriptExts }],
      });
      if (typeof picked !== "string") return;
      const s = await api.loadScript(picked);
      dispatch({ type: "SCRIPT", script: s });
    } catch {
      /* 用户在对话框里取消不是错误，静默返回 */
    }
  }

  return (
    <div className="empty">
      <div className="big">还没有载入稿件</div>
      <div className="hint">
        拖一个 <kbd>{scriptExtsHint}</kbd> 文件到窗口里
        <br />
        或者从下面选一种方式
      </div>
      <div className="btn-row" style={{ justifyContent: "center" }}>
        <button className="btn primary" onClick={() => void openScript()}>
          打开稿件
        </button>
        {/*
          粘贴是"从飞书/微信/网页弄一段过来"的主路径 —— 它不需要文件，
          也不需要任何授权，所以放在和「打开稿件」并列的位置，
          而不是藏进设置里。
        */}
        <button
          className="btn"
          onClick={() => dispatch({ type: "PASTE", open: true, mode: "new" })}
        >
          粘贴一段文字
        </button>
        <button
          className="btn"
          onClick={() => {
            void api
              .setScriptText(
                "各位好，今天我要汇报的是第三季度的业务复盘。\n\n" +
                  "先从整体大盘说起。本季度营收实现了稳步增长，环比提升明显，" +
                  "其中华东与华南两个大区贡献了主要增量。\n\n" +
                  "接下来是利润端。毛利率维持稳定，费用率有所下降，" +
                  "主要得益于上半年做的流程优化开始显效。\n\n" +
                  "最后说一下下季度的重点。我们会把资源进一步向高价值客户倾斜，" +
                  "同时完成新版本的上线准备。",
                "示例汇报稿",
              )
              .catch(() => {});
          }}
        >
          先试试示例稿
        </button>
      </div>
    </div>
  );
}

/**
 * 拖放覆盖层。
 *
 * 用 `position: fixed` 而不是放在稿件区里：拖放可以落在窗口任意位置，
 * 反馈也应该覆盖整窗。`pointer-events: none` 必须有 ——
 * 拖放是**操作系统**层面的事件（Tauri 在 Windows 上用 OLE drop target 独占），
 * 覆盖层一旦吃掉指针事件，落点判定就会出问题。
 */
function DragOverlay() {
  const { state } = useApp();
  if (!state.drag) return null;

  const names = state.drag.names;
  const many = names.length > 1;

  return (
    <div className={`drag-overlay${state.drag.ok ? " ok" : " bad"}`}>
      <div className="do-box">
        <div className="do-icon">{state.drag.ok ? "↓" : "!"}</div>
        <div className="do-title">
          {state.drag.ok ? "松手即载入" : "这个类型暂时不行"}
        </div>
        {names.length > 0 && (
          <div className="do-names">
            {names.join("、")}
            {many && " …"}
          </div>
        )}
        <div className="do-note">
          {state.drag.ok
            ? `支持 ${state.scriptExtsHint}，会自动识别 UTF-8 与 GBK`
            : `稿件支持 ${state.scriptExtsHint}。.doc/.ppt 老格式请先另存为 .docx/.pptx`}
        </div>
      </div>
    </div>
  );
}

/**
 * 把当前句渲染到「字」的粒度 —— 按歌词的卡拉OK范式：
 * **唱过的实、没唱的淡、正在唱的那个字最亮**。
 *
 * ★ 与上一版的区别在"基准色朝哪边"：
 *   上一版是「已读压暗」（整句亮、读过的降明度），那是"记录历史"的思路；
 *   歌词是「已唱实、未唱淡」（整句淡、唱过的提亮），那是"指示接下来读哪儿"的思路。
 *   提词器要的是后者 —— 讲者盯的始终是**还没念的那几个字**，
 *   余光需要一个"亮到哪儿了"的界线，而不是一串变灰的旧字。
 *
 * 三段各自的取色见 `styles.css` 的 `.sung / .singing / .unsung`。
 */
function renderSentence(text: string, offset: number) {
  const chars = Array.from(text);
  const n = Math.max(0, Math.min(offset, chars.length));

  return (
    <>
      <span className="sung">{chars.slice(0, n).join("")}</span>
      {n < chars.length && <span className="singing">{chars[n]}</span>}
      <span className="unsung">{chars.slice(n + 1).join("")}</span>
    </>
  );
}

/**
 * 在句子里把命中的字标出来。
 *
 * `from` / `to` 是相对 `text` 的**字符**下标（不是字节），与 `lib/find.ts`
 * 用同一套口径 —— 那里已保证下标一定落在 `text` 的字符范围内。
 */
function renderWithHits(text: string, hits: Hit[], activeOrdinal: number): ReactNode {
  const chars = Array.from(text);
  const out: ReactNode[] = [];
  let at = 0;

  for (const h of hits) {
    if (h.from > at) {
      out.push(<span key={`t${at}`}>{chars.slice(at, h.from).join("")}</span>);
    }
    out.push(
      <mark
        key={`h${h.ordinal}`}
        className={`hit${h.ordinal === activeOrdinal ? " active" : ""}`}
      >
        {chars.slice(h.from, h.to).join("")}
      </mark>,
    );
    at = h.to;
  }
  if (at < chars.length) {
    out.push(<span key={`t${at}`}>{chars.slice(at).join("")}</span>);
  }
  return out;
}

/**
 * 「把进度接到这一句」的**命中解析**。
 *
 * # 为什么不能只看 `e.target`
 *
 * 这一段原来挂在 `<span class="sentence">` 上。句子是 **inline** 元素，
 * 它的盒子只包住字形 —— 行距（`line-height` 默认 1.7，字号 34 时每行约 18px）
 * 和行尾的空白**都不属于它**。于是版面里有一大片点不动的区域：
 *
 *   * 行与行之间（行盒比字形高，多出来的部分没有元素）
 *   * 段落末尾那一行的右侧空白
 *   * 句子与句子之间的空格
 *
 * 用户看到的是"有时候点得上、有时候点不上" —— 而这条路的失败方式恰好最糟：
 * 它本来就是"点了就有反应"的单次动作，没反应就只能得出"这个功能坏了"。
 *
 * # 现在怎么算
 *
 * 处理器挪到**块级**的 `<p>` 上（块盒高度 = 各行行盒之和，行距落在行盒内），
 * 再用 `caretRangeFromPoint` 把坐标换成"这是段内第几个字"，最后按
 * `s.text` 的长度逐句累加找到那一句。
 * ★ 口径必须是 **UTF-16 码元**（`s.text.length` 就是），不能用 `Array.from`
 *   的码点口径 —— 两者在 emoji 上会差一位，而差一位就会跳错句。
 */
const DRAG_SLOP = 6;

/** 视口坐标 → `root` 内的字符下标（UTF-16）。取不到就返回 `null`，由调用方放过。 */
function charIndexAt(x: number, y: number, root: HTMLElement): number | null {
  const d = document as Document & {
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
  };
  let node: Node | null = null;
  let off = 0;
  const range = d.caretRangeFromPoint?.(x, y);
  if (range) {
    node = range.startContainer;
    off = range.startOffset;
  } else {
    const pos = d.caretPositionFromPoint?.(x, y);
    if (pos) {
      node = pos.offsetNode;
      off = pos.offset;
    }
  }
  // 点到的东西不属于这一段的文本（例如落在外层容器上）—— 不猜。
  if (!node || node.nodeType !== Node.TEXT_NODE || !root.contains(node)) return null;

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let acc = 0;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n === node) return acc + off;
    acc += (n.textContent ?? "").length;
  }
  return null;
}

/**
 * 段内第几个字落在第几句。
 *
 * 前提：渲染出来的段落文本恰好是 `sentences.map(s => s.text).join("")`。
 * 三种渲染分支（`renderSentence` / `renderWithHits` / 原样）都只是把 `s.text`
 * 切进若干 `<span>`，一个字都不增删 —— 所以这个下标可以直接累加对上。
 */
function sentenceAtChar(par: Paragraph, off: number): number | null {
  let acc = 0;
  for (let i = 0; i < par.sentences.length; i++) {
    acc += par.sentences[i].text.length;
    if (off < acc) return i;
  }
  return par.sentences.length > 0 ? par.sentences.length - 1 : null;
}

/**
 * 一整个段落。
 *
 * ★ 「接进度」挂在这一层（段落），不挂到句子上去：句子是 inline 盒，
 *   只盖住字形，**行距那一片点不到**（见 `charIndexAt` 的长注释）。
 *   ⇒ 所以"能点的地方"和"点下去算数的地方"是同**一块**区域（整个 `<p>`）。
 *
 * ★ 第十一轮之前这里还带一个 `jumpHint`（悬停说明"单击／双击＝把进度接到
 *   这一句"）。气泡删掉之后 `jumpHint` 整条链路（prop → hook → 状态）
 *   也一并去掉了。**它承载的信息没有丢**，而且本来就有更早的出口：
 *   「单击还是双击才算」由 `quick_jump` 决定，而那个开关在设置里、
 *   开关下方那行 `.grp-s`（`SettingsDrawer` 的「快捷跳读」行）
 *   写的就是当前到底是单击还是双击生效。界面上的悬停说明反而是
 *   第二份说法 —— 两处说同一件事就有漂移的风险。
 */
function Para({
  para,
  dist,
  paraIsRead,
  dimRead,
  cursor,
  curPara,
  curSent,
  hitsBySe,
  activeHit,
  activeOrdinal,
  quickJump,
  downRef,
  seekFromPoint,
}: {
  para: Paragraph;
  dist: number;
  paraIsRead: boolean;
  dimRead: boolean;
  cursor: { offset_in_sentence: number } | null | undefined;
  curPara: number;
  curSent: number;
  hitsBySe: Map<string, Hit[]>;
  activeHit: Hit | null;
  activeOrdinal: number;
  quickJump: boolean;
  /** 指针按下时的坐标。`useRef` 的 `.current` 是只读的，所以要 `MutableRefObject`。 */
  downRef: React.MutableRefObject<{ x: number; y: number } | null>;
  seekFromPoint: (e: ReactMouseEvent<HTMLParagraphElement>, p: Paragraph) => void;
}) {
  return (
    <p
      className={`para${paraIsRead && dimRead ? " read" : ""}`}
      style={{ "--dist": dist } as CSSProperties}
      onPointerDown={(e) => {
        downRef.current = { x: e.clientX, y: e.clientY };
      }}
      {...(quickJump
        ? { onClick: (e: ReactMouseEvent<HTMLParagraphElement>) => seekFromPoint(e, para) }
        : {
            onDoubleClick: (e: ReactMouseEvent<HTMLParagraphElement>) =>
              seekFromPoint(e, para),
          })}
    >
      {para.sentences.length === 0 ? (
        // 归一化后为空的段落（纯数字、纯符号）—— 显示原文但不参与定位
        <span>{para.text}</span>
      ) : (
        para.sentences.map((s) => {
          const isCurrent = para.index === curPara && s.index === curSent;
          const seHits = hitsBySe.get(seKey(para.index, s.index));
          const isActiveSe =
            !!activeHit &&
            activeHit.paragraph === para.index &&
            activeHit.sentence === s.index;

          return (
            <span
              key={s.index}
              {...(isCurrent ? { "data-current": "1" } : {})}
              data-sekey={seKey(para.index, s.index)}
              className={`sentence${isCurrent ? " current" : ""}${
                isActiveSe ? " hit-se" : ""
              }`}
            >
              {isCurrent
                ? // 读到的那个字优先于搜索高亮：两个高亮叠在同一处
                  // 会让"正在念哪个字"变得看不清，而那是更重要的信息
                  renderSentence(s.text, cursor?.offset_in_sentence ?? 0)
                : seHits
                  ? renderWithHits(s.text, seHits, activeOrdinal)
                  : s.text}
            </span>
          );
        })
      )}
    </p>
  );
}

export function PrompterView() {
  const { state, dispatch } = useApp();
  const containerRef = useRef<HTMLDivElement>(null);

  const { script, cursor, config } = state;

  // 垂直锚点：当前句放在视口高度的百分之多少处。
  //
  // 之前硬编码 0.36，现在可调 —— 这条是整个排版工具箱里实用价值最高的一项：
  // 有人喜欢"念的那句在中间"，有人喜欢"偏上、下面多留些备读"。
  const anchor = config?.anchor ?? 0.36;

  // 平滑滚动（设置 › 跟读）。关掉＝每次跳句瞬间到位。
  // 有人嫌动画"眼睛要追一下"（尤其用快捷键连翻几句时），
  // 关掉后画面是「啪」地换一屏 —— 快，但没有过渡可循。
  const scrollBehavior: ScrollBehavior = (config?.smooth_scroll ?? true) ? "smooth" : "auto";

  // 快捷跳读（设置 › 跟读）。开＝单击正文任意一句就把进度接过去；
  // 关＝改成双击。注意**关掉不是禁用**：单击就不再有反应，
  // 所以那一句的 `title` 必须跟着换文案，否则用户只会得出"点不动了"。
  const quickJump = config?.quick_jump ?? true;

  // 按下点。用来把"点击"与"拖选"分开（见 `seekFromPoint`）。
  // 用 ref 不用 state：它一变就重渲染，而这是个纯机械的判断。
  const downRef = useRef<{ x: number; y: number } | null>(null);

  // ---- 稿内查找 ----
  //
  // 命中集合在这里再算一遍（查找栏里算的是同一份），两边共用 `findHits`，
  // 所以"计数"和"标出来的处数"不可能对不上。
  const findActive = state.findOpen && state.findQuery.trim().length > 0;
  const hits = useMemo(
    () => (findActive ? findHits(script, state.findQuery) : []),
    [findActive, script, state.findQuery],
  );
  const hitsBySe = useMemo(() => groupHits(hits), [hits]);
  const activeHit = hits[Math.min(state.findIndex, Math.max(0, hits.length - 1))] ?? null;
  const activeOrdinal = activeHit ? activeHit.ordinal : -1;

  // 滚到当前命中处。**不动朗读光标** —— 查找是"看"，不是"跳"。
  useEffect(() => {
    if (!activeHit) return;
    const container = containerRef.current;
    if (!container) return;
    const el = container.querySelector<HTMLElement>(
      `[data-sekey="${seKey(activeHit.paragraph, activeHit.sentence)}"]`,
    );
    if (!el) return;
    const cRect = container.getBoundingClientRect();
    const eRect = el.getBoundingClientRect();
    const target = container.scrollTop + (eRect.top - cRect.top) - container.clientHeight * anchor;
    container.scrollTo({ top: Math.max(0, target), behavior: scrollBehavior });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeOrdinal, anchor, state.collapsed, scrollBehavior]);

  // ---- 滚轮临时接管 ----
  //
  // 场景：讲到一半想往回看一眼某个数字。这时候如果自动滚动还在跟着语音跑，
  // 画面会被立刻拽回当前句 —— 用户看到的是"滚不动"，而这个功能就废了。
  //
  // 做法是**让位**：滚轮一动就暂停自动滚动 4 秒，停手后无缝接回当前句。
  // 用 ref 存截止时间而不是 state，是为了让滚轮处理函数不必每帧重建
  // —— wheel 是高频事件，给它挂一个随 state 变化的闭包会明显掉帧。
  const manualUntilRef = useRef(0);
  const [manual, setManual] = useState(false);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onWheel = () => {
      manualUntilRef.current = Date.now() + MANUAL_HOLD_MS;
      setManual(true);
    };
    // passive: true —— 我们只观察、不拦截。拦截滚轮会让触控板惯性滚动失效。
    container.addEventListener("wheel", onWheel, { passive: true });
    return () => container.removeEventListener("wheel", onWheel);
  }, [script?.title]);

  // 到点了就把接管状态摘掉。轮询而不是 setTimeout：
  // 用户连续滚动会不断把截止时间往后推，setTimeout 会被反复重建，
  // 250ms 一次的检查反而更省也更准。
  useEffect(() => {
    if (!manual) return;
    const t = window.setInterval(() => {
      if (Date.now() >= manualUntilRef.current) setManual(false);
    }, 250);
    return () => window.clearInterval(t);
  }, [manual]);

  // 自动滚动：只在「换句」时触发，不是每个字都滚。
  // 按字符滚动会让画面持续微动，长时间盯着会晕。
  //
  // 依赖里带上 `collapsed`：收起会把稿件区压成 0 高，展开时必须**重新对位**，
  // 否则用户看到的是稿子开头而不是刚才念到的地方。
  //
  // 依赖里带上 `manual`：接管结束时（true → false）这个 effect 会再跑一次，
  // 于是画面**自动滑回**当前句 —— 这就是"无缝接回"，不需要额外写一个恢复逻辑。
  //
  // 查找打开时**不自动滚**：否则用户刚翻到某处，语音一推光标又把画面拽走了。
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    if (state.collapsed) return;
    if (manual) return;
    if (findActive) return;
    const el = container.querySelector<HTMLElement>("[data-current='1']");
    if (!el) return;

    const cRect = container.getBoundingClientRect();
    const eRect = el.getBoundingClientRect();
    const target = container.scrollTop + (eRect.top - cRect.top) - container.clientHeight * anchor;
    container.scrollTo({ top: Math.max(0, target), behavior: scrollBehavior });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cursor?.paragraph, cursor?.sentence, state.collapsed, anchor, manual, findActive, scrollBehavior]);

  if (!script || script.total_chars === 0) {
    return (
      <>
        <div className="viewport">
          <EmptyState />
        </div>
        <DragOverlay />
      </>
    );
  }

  const curPara = cursor?.paragraph ?? -1;
  const curSent = cursor?.sentence ?? -1;

  /**
   * 把进度接到某一句。
   *
   * **失败要让用户看见** —— 这里原来是 `.catch(() => {})`，跳转失败时点了没反应，
   * 用户只能得出"这个点不了了"的结论（本项目踩过同型的坑：可交互区上报被静默吞掉，
   * 看起来像"开关坏了"）。
   */
  function seekTo(alignStart: number) {
    void api
      .goto(alignStart)
      .then((c) => dispatch({ type: "CURSOR", cursor: c }))
      .catch(() =>
        dispatch({ type: "ERROR", message: "没能把进度接到这一句，请重试" }),
      );
  }

  /**
   * 点正文 ⇒ 接进度。落点由**坐标**解析，不由 `e.target` 解析（理由见 `charIndexAt`）。
   *
   * ★ 拖选必须放过：正文是可选中复制的。没有这条，用户"选一段来复制"会顺手
   *   把朗读进度跳到他划过去的那个字上 —— 一个他完全没打算做的动作。
   *   判据用位移而不是"有没有选区"：双击路径本来就会选中一个词，
   *   按选区判断会把双击跳读整条路堵死。
   */
  function seekFromPoint(e: ReactMouseEvent<HTMLParagraphElement>, par: Paragraph) {
    const down = downRef.current;
    downRef.current = null;
    if (down && Math.hypot(e.clientX - down.x, e.clientY - down.y) > DRAG_SLOP) return;

    const off = charIndexAt(e.clientX, e.clientY, e.currentTarget);
    if (off == null) return;
    const at = sentenceAtChar(par, off);
    const s = at == null ? null : par.sentences[at];
    if (s) seekTo(s.align_start);
  }

  return (
    <>
      {/*
        这里**刻意没有** `onDragOver`。
        以前有一行 `onDragOver={(e) => e.preventDefault()}` —— 那是 HTML5 拖放的
        前半段，而 `dragDropEnabled` 默认 true 时 Tauri 会劫持拖放（Windows 上是
        OLE drop target 独占），浏览器的 drop 根本不会触发。留着那行只会让下一个
        读代码的人以为"拖放已经接好了"。真正接拖放在 Rust 侧（见 lib.rs::on_drag）。
      */}
      <div
        ref={containerRef}
        className={`viewport${config?.mirror ? " mirror" : ""}${quickJump ? " quick" : ""}`}
      >
        {script.paragraphs.map((p) => {
          const paraIsRead = p.index < curPara;
          /*
            歌词式的层次：**离当前段落越远，字号越小、越淡**。
            用 `--dist` 交给 CSS 去算，而不是在这里直接算字号 ——
            具体取值属于排版，集中在 styles.css 一处才好调
            （本项目定式：凡两处都要用的值，只能有一个源）。

            上限取 3：更远的段落已经不重要，继续变小变淡只会让"扫一眼下面"
            更难；而且封顶之后，滚动过程中远处的段落不会持续重排。

            ★ 层次做在**段落**上而不是句子上的原因：句子是 inline 元素，
              改它的字号会改变行高 ⇒ 每次换句都让整段重排、画面跟着抖。
              段落是块级，只有跨段落时才会动一次（而且有 transition 兜着）。
          */
          const dist = curPara < 0 ? 0 : Math.min(Math.abs(p.index - curPara), 3);
          return (
            <Para
              key={p.index}
              para={p}
              dist={dist}
              paraIsRead={paraIsRead}
              dimRead={!!config?.dim_read}
              cursor={cursor}
              curPara={curPara}
              curSent={curSent}
              hitsBySe={hitsBySe}
              activeHit={activeHit}
              activeOrdinal={activeOrdinal}
              quickJump={quickJump}
              downRef={downRef}
              seekFromPoint={seekFromPoint}
            />
          );
        })}
      </div>

      {/*
        接管中的提示。**必须显示** —— 否则用户会以为"跟读失灵了"，
        而它其实是我们主动让位。少这一个条，"手动接管"就变成了一个
        让人困惑的故障。
      */}
      {manual && (
        <div className="manual-chip" data-interactive>
          手动浏览中 · 跟读暂停，停手后自动接回
        </div>
      )}

      <DragOverlay />
    </>
  );
}
