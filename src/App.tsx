import { useEffect, useRef } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { PrompterView } from "./components/PrompterView";
import { ScriptHistory } from "./components/ScriptHistory";
import { SettingsDrawer } from "./components/SettingsDrawer";
import { FindBar } from "./components/FindBar";
import { PasteImport } from "./components/PasteImport";
import { TopBar } from "./components/TopBar";
import { UpdateBar } from "./components/UpdateBar";
import { api, events, errText } from "./lib/api";
import { applyLayoutVars, useApp, watchSystemTheme } from "./lib/store";
import { findUpdateQuietly } from "./lib/update";
import type { AppConfig } from "./types";

/** 这个元素此刻在窗口里占的那块矩形；**不该算可交互区**时返回 `null`。
 *
 *  ★★ 为什么不能只判 `offsetParent !== null` / `getClientRects().length > 0`：
 *     设置抽屉关着的时候是靠 `transform: translateX(101%)` 挪出窗口的，
 *     `offsetParent` 依然非空、`getBoundingClientRect()` 依然给出一块
 *     352×358 的矩形 —— 于是它会被当成一块「可交互区」报上去，
 *     把窗口右侧一整条误判成"可点"。
 *
 *     判据必须是**几何**的：矩形要真的和视口有交集。
 *     顺带把矩形裁到视口内 —— 后端按 CSS 像素 × 缩放还原成屏幕坐标，
 *     一块伸出窗口的矩形在那边是没有意义的。
 */
function visibleRectInViewport(el: HTMLElement): [number, number, number, number] | null {
  const cs = getComputedStyle(el);
  if (cs.display === "none" || cs.visibility === "hidden") return null;
  // `opacity: 0` 的元素仍然可点，但它已经不是"看得见的控制区"了；
  // 本项目里没有这种控制栏，出现即说明是过渡帧，忽略掉更稳。
  if (Number(cs.opacity) === 0) return null;

  const r = el.getBoundingClientRect();
  const left = Math.max(0, r.left);
  const top = Math.max(0, r.top);
  const right = Math.min(window.innerWidth, r.right);
  const bottom = Math.min(window.innerHeight, r.bottom);
  if (right - left <= 0 || bottom - top <= 0) return null;
  return [left, top, right - left, bottom - top];
}

/** 历史列表与可用性一起刷新。两件事总是一起发生，所以合成一个动作。 */
async function reloadScripts(dispatch: ReturnType<typeof useApp>["dispatch"]) {
  try {
    const [list, avail] = await Promise.all([
      api.listScripts(),
      api.scriptAvailability(),
    ]);
    dispatch({ type: "SCRIPTS", scripts: list });
    dispatch({
      type: "SCRIPT_AVAIL",
      avail: Object.fromEntries(avail),
    });
  } catch {
    /* 历史读不出来不该打扰用户 —— 它是增强功能，当前稿件照常可用 */
  }
}

export default function App() {
  const { state, dispatch } = useApp();

  /**
   * 防抖窗口里"还没写出去"的那一份配置。
   *
   * 存下来只为了一件事：卸载前把它补发掉（见下面那个只在卸载时跑一次的
   * effect）。没有它，"改完设置马上关窗"那一下的改动就永久丢了。
   */
  const pendingCfgRef = useRef<AppConfig | null>(null);

  // ---- 订阅后端事件 ----
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const pending = [
        events.onCursor((c) => dispatch({ type: "CURSOR", cursor: c })),
        events.onAsr((a) => dispatch({ type: "ASR", text: a.text, level: a.level })),
        events.onStealth((r) => dispatch({ type: "STEALTH", report: r })),
        events.onAsrStatus((s) => {
          dispatch({ type: "ASR_STATUS", status: s });
          // 新一轮识别开始时清掉上一轮的警告。用户多半正是看到警告之后
          // 去处理了麦克风再重开的 —— 这时候还挂着旧警告只会让人以为没生效。
          if (s.running) dispatch({ type: "WARNING", warning: null });
        }),
        events.onWarning((msg) =>
          // 后端这条通道目前只有麦克风在用（多为「设备在送数字静音」）。
          //
          // ★ 标签只留**是什么**（「麦克风异常」），不带「· 点击自检」——
          //   可点这件事由状态槽末尾那个 `›` 表达（见 Transport），
          //   而状态槽是顶栏里**唯一**一个"文字先说是什么"的地方：
          //   它旁边就是标题和进度条，多一个四字后缀就要从它们嘴里抢字。
          dispatch({
            type: "WARNING",
            warning: { label: "麦克风异常", detail: msg },
          }),
        ),
        events.onError((msg) => dispatch({ type: "ERROR", message: msg })),

        // 拖放与托盘切稿走的都是这条路 —— 它们不经过 invoke，没有返回值
        events.onScript((p) => {
          dispatch({ type: "SCRIPT", script: p });
          void reloadScripts(dispatch);
        }),
        events.onDrag((h) =>
          dispatch({ type: "DRAG", hint: h.kind === "enter" ? h : null }),
        ),
        events.onCollapsed((c) => dispatch({ type: "COLLAPSED", collapsed: c })),
        events.onClickThrough((active) => dispatch({ type: "CT", active })),
        events.onNotice((msg) => dispatch({ type: "NOTICE", message: msg })),
      ];

      const resolved = await Promise.all(pending);
      // StrictMode 下 effect 会跑两遍：第一遍的 listen 可能还没来得及返回
      // 就被 cleanup 了。这里补一次注销，否则会留下一组野监听器，
      // 表现为每次光标更新都触发两遍。
      if (cancelled) {
        resolved.forEach((f) => f());
        return;
      }
      unlisteners.push(...resolved);
    })();

    return () => {
      cancelled = true;
      unlisteners.forEach((f) => f());
    };
  }, [dispatch]);

  // ---- 启动：拉全量状态，并按配置开启隐身 ----
  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const b = await api.bootstrap();
        if (cancelled) return;
        void api.beat(`bootstrap 返回：click_through=${b.config.click_through}`).catch(() => {});

        dispatch({
          type: "BOOTSTRAP",
          payload: {
            config: b.config,
            script: b.script,
            cursor: b.cursor,
            platform: b.platform,
            modelsDir: b.models_dir,
            foundModels: b.found_models,
            appVersion: b.app_version,
            scriptExts: b.script_exts,
            scriptExtsHint: b.script_exts_hint,
            // 意图先行：配置说"启动即隐身"，界面就先落到「开启中」，
            // 等下面那次 setStealth 回来再收敛成「投屏隐身」。
            // 这样启动瞬间也不会有一段"看起来没开"的窗口期。
            stealthIntent: b.config.stealth_on_start ? "on" : "off",
            stealthBusy: b.config.stealth_on_start,
            // 穿透同理（意图先行）：配置里开着，界面就先落到「开启中」。
            // 它会等后端真正进入工作态（开关 + 可交互区都到位）才收敛成绿色 ——
            // 中间那段不到一秒的等待，如果显示的是"没开"，用户会以为配置没生效。
            ctPending: b.config.click_through,
            asrStatus: {
              running: false,
              engine: b.asr_engine ?? "未启动",
              detail: "",
            },
          },
        });

        void reloadScripts(dispatch);
        void api.beat("BOOTSTRAP 已 dispatch").catch(() => {});

        // 隐身必须在窗口真正创建之后才生效，所以放在这里而不是 Rust 的 setup 里。
        if (b.config.stealth_on_start) {
          try {
            const report = await api.setStealth(true);
            if (!cancelled) dispatch({ type: "STEALTH", report });
            void api.beat("隐身已开启").catch(() => {});
          } catch (e) {
            // 隐身失败不该挡住启动 —— 稿件仍然可用，只是需要退到兜底方案。
            //
            // ★ 这里**必须把 busy 落下来**：`stealthBusy` 是「开启中」动画的唯一开关，
            //   启动时它是 true（意图先行）。如果这条异常路径不清它，pill 会永远停在
            //   转圈的「开启中」—— 一个"看起来还在忙、其实早就完了"的状态，
            //   比直接报错还糟。落到 intent=on + busy=false，pill 就显示琥珀色
            //   的「未生效」，那才是实话。
            if (!cancelled) {
              dispatch({ type: "STEALTH_INTENT", intent: "on", busy: false });
              dispatch({ type: "ERROR", message: errText(e) });
            }
            void api.beat(`隐身失败：${errText(e)}`).catch(() => {});
          }
        }

        // 穿透同理：窗口必须在、可交互区必须已经上报，否则可能得到一个
        // 点不动的窗口。后端的兜底是"没收到可交互区就永不切换"，
        // 所以这里即便早于上报也不会出事。
        if (b.config.click_through) {
          try {
            const active = await api.setClickThrough(true);
            if (!cancelled) dispatch({ type: "CT", active });
            void api.beat("穿透已开启").catch(() => {});
          } catch (e) {
            if (!cancelled) {
              dispatch({ type: "CT_PENDING", pending: false });
              dispatch({ type: "ERROR", message: errText(e) });
            }
            void api.beat(`穿透失败：${errText(e)}`).catch(() => {});
          }
        }

        // 置顶同理：配置说"开机就置顶"，那就置顶 —— 这个产品是浮在 PPT 上的
        // 提词层，每次开机都要手动点一下最烦人。失败只打点，不打断启动：
        // 置顶没设上不影响稿件与隐身，只是窗口可能被压住。
        if (b.config.always_on_top) {
          void api
            .setAlwaysOnTop(true)
            .catch((e) => void api.beat(`置顶恢复失败：${errText(e)}`).catch(() => {}));
        }
      } catch (e) {
        dispatch({ type: "ERROR", message: errText(e) });
        void api.beat(`bootstrap 抛错：${errText(e)}`).catch(() => {});
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [dispatch]);

  // ---- 启动时查一次有没有新版本 ----
  //
  // ★ 为什么延后 3 秒：它和 bootstrap 抢的是同一条网络、同一个主线程。
  //   刚开窗那几百毫秒里用户要的是"稿子出现"，一次 HTTP 往返插在那儿
  //   只会把"能开始念"的时刻往后推。3 秒之后窗口已经稳定、稿件也在了。
  //
  // ★ 为什么走**静默**那条（`findUpdateQuietly`，不是 `findUpdate`）：
  //   这一次是我们自己发起的，用户没问过。断网、还没发过 Release、
  //   清单地址临时 404 —— 都会让它抛，但它们没有一条是用户此刻该知道的事。
  //   真正的"查不动"由设置里那颗「检查更新」如实报告 —— 在那里用户是主动问的，
  //   沉默才是最糟的回答。
  useEffect(() => {
    let cancelled = false;
    const t = window.setTimeout(() => {
      void (async () => {
        const found = await findUpdateQuietly();
        if (cancelled || !found) return;
        dispatch({ type: "UPDATE", patch: { update: found } });
      })();
    }, 3000);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [dispatch]);

  // ---- 版面参数落到 CSS 变量 ----
  useEffect(() => {
    applyLayoutVars(state.config);
  }, [state.config]);

  // ---- 主题跟随系统时，系统换了要跟着变 ----
  useEffect(() => {
    if (state.config?.theme !== "system") return;
    return watchSystemTheme(() => applyLayoutVars(state.config));
  }, [state.config]);

  // ---- 配置改动落盘（防抖，避免拖滑条时每帧写一次文件）----
  //
  // ★★ 清理时**只清定时器、不做写**：effect 每次依赖变化都会重跑，
  //    清理里顺手写一下等于把防抖整个废掉（拖滑杆会变成每步一次写盘）。
  //    补发那次"还没写出去"的值交给下面那个只在卸载时跑一次的 effect。
  useEffect(() => {
    if (!state.ready || !state.config) return;
    pendingCfgRef.current = state.config;
    const t = window.setTimeout(() => {
      pendingCfgRef.current = null;
      void api.saveConfig(state.config!).catch(() => {
        /* 配置保存失败不打扰用户，下次改动会再试 */
      });
    }, 400);
    return () => window.clearTimeout(t);
  }, [state.config, state.ready]);

  // ---- 卸载前补发防抖窗口里那一份 ----
  //
  // 不加这一条，用户"改完设置马上关窗"那一下的改动会**永远不落盘**
  // （定时器被清掉就没了），下次打开只看到旧值 —— 又是一条"设置没记住"，
  // 而"改完就走"恰恰是最自然的动作。
  useEffect(() => {
    return () => {
      const cfg = pendingCfgRef.current;
      pendingCfgRef.current = null;
      if (cfg) void api.saveConfig(cfg).catch(() => {});
    };
  }, []);

  // ---- 上报「可交互区」给后端 ----
  //
  // 穿透态下前端收不到 mousemove（事件被穿到下层窗口去了），所以
  // "鼠标在不在控制区里"只能由后端轮询判断；而"控制区长什么样"只有 DOM 知道
  // —— 设置抽屉打开时、历史浮层展开时、收起态下都不一样。
  // 两边各出一半信息，汇合点在 win/click_through.rs。
  //
  // ★★ 上报的是**一组矩形**，不是它们的包围盒。
  //    展开态下控制栏有上下两条，包围盒恰好覆盖整窗 ——
  //    那会让"在控制区外"永远不成立，穿透一次都不触发。
  //    这个坑是真机上量出来的，不是推演，所以这里刻意用数组。
  useEffect(() => {
    if (!state.ready) return;

    // 上报失败只提示一次。★ 静默失败是穿透最难查的一类问题：
    // 后端的兜底是"没收可交互区就永不切换"，于是现象只剩"开关点了没反应"，
    // 而界面上没有任何线索指向"上报链路断了"。这条提示就是那个线索。
    let warned = false;
    let beated = false;

    const report = () => {
      const rects: [number, number, number, number][] = [];
      for (const el of document.querySelectorAll<HTMLElement>("[data-interactive]")) {
        const r = visibleRectInViewport(el);
        if (r) rects.push(r);
      }

      // 第一次上报打个点：日志里有没有它，直接区分"前端上报链路没走通"
      // 和"上报了但形状不对" —— 这两种情况的修法完全不同。
      if (!beated) {
        beated = true;
        void api.beat(`首次上报可交互区：${rects.length} 块`).catch(() => {});
      }

      // 一个可交互区都没有 ⇒ 告诉后端"整窗都可以点"，
      // 后端会据此把穿透关掉。宁可让窗口挡一下，也不能让它点不动。
      const sent = rects.length === 0 ? api.reportZone(null) : api.reportZone(rects);
      void sent.catch((e) => {
        if (warned) return;
        warned = true;
        void api.beat(`可交互区上报失败：${errText(e)}`).catch(() => {});
        dispatch({
          type: "WARNING",
          warning: {
            label: "穿透未能生效",
            detail:
              `鼠标穿透没能生效：可交互区上报失败（${errText(e)}）。\n` +
              "点击打开设置，看自检里的「鼠标穿透」那一行。",
          },
        });
      });
    };

    report();
    // 400ms 一次的轮询够了：这些区域只在用户操作后变化，
    // 而真正的"光标是否在区内"判断在后端以 40ms 跑。
    const timer = window.setInterval(report, 400);
    window.addEventListener("resize", report);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("resize", report);
    };
  }, [
    state.ready,
    state.settingsOpen,
    state.historyOpen,
    state.collapsed,
    state.config?.padding_x,
    state.findOpen,
    state.pasteOpen,
  ]);

  // ---- 一次性提示自动消失 ----
  useEffect(() => {
    if (!state.notice) return;
    const t = window.setTimeout(() => dispatch({ type: "NOTICE", message: null }), 4000);
    return () => window.clearTimeout(t);
  }, [state.notice, dispatch]);

  // ---- 兜底：Ctrl/Cmd + 方向键也能翻句（不依赖全局热键是否注册成功）----
  const settingsOpenRef = useRef(state.settingsOpen);
  settingsOpenRef.current = state.settingsOpen;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        void api.stepSentence(true).then((c) => dispatch({ type: "CURSOR", cursor: c }));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        void api.stepSentence(false).then((c) => dispatch({ type: "CURSOR", cursor: c }));
      } else if (e.key === ",") {
        e.preventDefault();
        dispatch({ type: "SETTINGS", open: !settingsOpenRef.current });
      } else if (e.key === "k") {
        // 历史浮层也有键盘入口 —— 会前 60 秒切稿时手不用离开键盘
        e.preventDefault();
        dispatch({ type: "HISTORY", open: !state.historyOpen });
      } else if (e.key === "f") {
        // Ctrl+F 查找。**在稿子内部找**，不是浏览器搜索 ——
        // 不拦下来的话 WebView 会弹出它自己的查找框，而它搜的是整个 DOM
        // （连设置面板里的"字号"都能搜到），完全不是用户想要的。
        e.preventDefault();
        dispatch({ type: "FIND", open: !state.findOpen });
      } else if (e.key === "e") {
        e.preventDefault();
        dispatch({
          type: "PASTE",
          open: true,
          mode:
            state.script && state.script.total_chars > 0 ? "edit" : "new",
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dispatch, state.historyOpen, state.findOpen, state.script]);

  // ---- 收起态下打开抽屉 / 历史浮层 / 查找 / 粘贴 ⇒ 自动展开 ----
  //
  // 收起后窗口只有 40px 高，抽屉和浮层会被窗口边界裁掉，按钮点得到但内容看不见
  // —— 那是比"打不开"更让人困惑的状态。所以这些面板一打开就展开窗口。
  useEffect(() => {
    if (!state.collapsed) return;
    if (
      !state.settingsOpen &&
      !state.historyOpen &&
      !state.findOpen &&
      !state.pasteOpen
    ) {
      return;
    }
    void api
      .setCollapsed(false)
      .then(() => dispatch({ type: "COLLAPSED", collapsed: false }))
      .catch(() => {});
  }, [
    state.collapsed,
    state.settingsOpen,
    state.historyOpen,
    state.findOpen,
    state.pasteOpen,
    dispatch,
  ]);

  // 「固定控制栏」的形态由 CSS 类切换（见 styles.css 的同名小节）。
  // 钉住 = 浮在正文之上；不钉住 = 占独立一行。两种都不会随正文滚走。
  const barInline = state.config && !state.config.bar_pinned;

  // 「正在提词」的全局开关。
  //
  // ★ 为什么用 `asrStatus.running`（识别在跑）而不是只看 `state.cursor`：
  //   这一位驱动的是"要不要关掉装饰性动效"—— 判据必须是**"屏幕正在被当作稿子读"**
  //   这个事实本身。识别在跑就说明用户正对着稿子念。
  //
  // ★ 与 `collapsed` 并存时以 `collapsed` 的观感为准（收起态整窗就是一条栏），
  //   两者不冲突：收起来的时候本来也没什么装饰性动效可关。
  const prompting = !!state.asrStatus?.running;

  return (
    <div
      className={`app${state.collapsed ? " collapsed" : ""}${
        state.collapsing ? " collapsing" : ""
      }${barInline ? " bar-inline" : ""}${prompting ? " prompting" : ""}`}
      /*
        收起态整窗就是一条控制栏 ⇒ 整窗都算「可交互区」。
        这样光标在那条栏上时可点，离开后穿透照常生效 ——
        一条 40px 的细条既不挡看也不挡点，正是收起模式想要的效果。

        展开态只把那条栏标为可交互（见 TopBar / Transport），
        稿件区刻意不标 —— 那一片就是"点得到下面"的那一片。
      */
      {...(state.collapsed ? { "data-interactive": "" } : {})}
    >
      {/*
        唯一的栏。★ 它同时是顶栏、状态栏和播放控制 —— 上一版分开在窗口
        上下两端的两条栏已经合成这一条（见 TopBar / Transport 的组件注释）。
        底栏取消之后，正文因此多出 82px（274px 窗口里的 30%）。
      */}
      <TopBar />
      {/*
        更新提示条。★ 它在**正常文档流**里（不是浮层），所以在的时候
        正文从它下面开始 —— 常驻的东西绝不盖住正在念的字。
        没有待安装的更新时它渲染 `null`，整个节点都不存在。
      */}
      <UpdateBar />
      {/*
        稿件区始终挂载，收起时由 CSS 收成 0 高 —— 不卸载的原因：
        卸载会丢掉滚动位置，展开后用户会看到稿子开头而不是刚才念到的地方。
      */}
      <PrompterView />
      <FindBar />
      <ScriptHistory />
      <SettingsDrawer />
      <PasteImport />

      {state.notice && (
        <div className="toast" onClick={() => dispatch({ type: "NOTICE", message: null })}>
          {state.notice}
        </div>
      )}
    </div>
  );
}
