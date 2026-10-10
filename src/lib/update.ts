// 应用内更新的**唯一**出口（含编排）—— 组件里不碰 updater 插件。
//
// 组件里不直接 `import { check } from "@tauri-apps/plugin-updater"`
// —— 与「不直接 invoke」是同一条纪律。更新这条链路上，"什么时候检查、
// 失败算不算错、装完怎么重启"全是判断；散到组件里迟早漂成两份。
// 而且它现在有**两个入口**（顶栏下方那条提示条、设置里那一行），
// 两处各写一遍必然漂成"提示条能装、设置里点了没反应"。
//
// ★ 为什么是 `lib/update.ts` 而不是塞进 `lib/api.ts`：
//   `api.ts` 是一张**无状态的命令对照表**（函数名 → invoke 名），
//   而更新必须**留住那个 `Update` 句柄** —— `check()` 拿到的对象同时握着
//   "下载哪个包、装它"的全部信息（它绑着一个后端资源），中间隔一次
//   渲染就丢了。`lib/` 下已有的 `prompting.ts` / `collapse.ts` 也是这个形状：
//   一个流程一个模块，自己管住那些跨调用的中间物。
//
// ★ 验签是强制的、关不掉的（Tauri 的设计）：公钥在 `tauri.conf.json` 的
//   `plugins.updater.pubkey` 里。公钥对不上 ⇒ 更新**装不上**，
//   而不是"装上了但没验"。

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

import { errText } from "./api";
import type { Action } from "./store";

type Dispatch = (a: Action) => void;

/** 发现的新版本。 */
export interface FoundUpdate {
  /** 更新清单里公布的版本号。 */
  version: string;
  /** 当前正在跑的版本号。 */
  currentVersion: string;
}

/**
 * 待安装的更新句柄，**刻意留在模块里、不给组件**。
 *
 * 原因：它绑着一个后端资源（`rid`）。组件一旦拿到手，就得回答
 * "我什么时候 close 它" —— 而那件事只有这里知道（被新一次检查替换掉、
 * 或者用户点了稍后）。留在模块里，"谁拥有它"就只有一个答案。
 */
let pending: Update | null = null;

/** 放掉手里的句柄。重复调用无副作用。 */
async function closePending(): Promise<void> {
  const u = pending;
  pending = null;
  if (u) {
    try {
      await u.close();
    } catch {
      /* 关不掉不影响任何事：最坏是后端多留一份已下载的字节，
         而它会在下一次检查时被替换掉。 */
    }
  }
}

/**
 * 静默检查（**启动时**走这条）。任何失败都当作"没有更新"。
 *
 * ★ 为什么这里可以吞掉错误，而下面 `checkForUpdate` 不行：
 *   启动检查是**我们自己**发起的，用户没问过。断网、还没发过 Release、
 *   清单地址临时 404 —— 这些都会让 `check()` 抛，但它们没有一条是
 *   用户此刻需要知道的事。把网络错误糊到开机画面上，正是本项目最忌讳的
 *   "用不重要的失败打扰正在干活的人"。
 */
export async function findUpdateQuietly(): Promise<FoundUpdate | null> {
  try {
    const u = await check();
    await closePending();
    if (!u) return null;
    pending = u;
    return { version: u.version, currentVersion: u.currentVersion };
  } catch {
    await closePending();
    return null;
  }
}

/**
 * 用户主动按了「检查更新」。
 *
 * ★ 与启动那次的分叉点是刻意的：用户按下去的那一刻，"有没有新版本"
 *   就是他等的答案，此时**沉默是最糟的回答**（他分不清"已是最新"
 *   和"检查根本没跑成"）。所以三种结果各有各的出口：
 *
 *     查到新版本 → 落进状态 ⇒ 顶栏下方那条提示条自己长出来，这里不必再说；
 *     已是最新   → 一次性提示（toast）—— 它是一次性的回答，不该常驻；
 *     查不动     → 一次性提示带上原因。**不落 `updateError`**：那个字段
 *                  是给提示条用的（"有一条更新但装不上"），而此刻
 *                  根本不存在可装的更新，落进去只会让状态自相矛盾。
 */
export async function checkForUpdate(dispatch: Dispatch): Promise<void> {
  try {
    const found = await findUpdateQuietly();
    if (found) {
      dispatch({ type: "UPDATE", patch: { update: found, updateError: null } });
      return;
    }
    // 顺手把可能残留的旧提示条收掉：清单随时可能变（Release 被撤回、
    // 或者我们自己刚发了一版把上一个换掉了），查到"没有更新"就该相信这次的结果。
    await closePending();
    dispatch({ type: "UPDATE", patch: { update: null, updateError: null } });
    dispatch({ type: "NOTICE", message: "已是最新版本" });
  } catch (e) {
    dispatch({ type: "NOTICE", message: `检查更新失败：${errText(e)}` });
  }
}

/**
 * 下载并安装手里那个更新。**两个入口共用这一个**（提示条那颗「更新」、
 * 设置里那颗「更新」）。
 *
 * ★ 进度落进全局状态而不是组件本地：Windows 上安装器一起，进程就被接管，
 *   组件可能连一次重渲染都来不及 —— 状态是给"装之前的最后一眼"用的。
 */
export async function installPendingUpdate(dispatch: Dispatch): Promise<void> {
  const u = pending;
  if (!u) {
    dispatch({
      type: "UPDATE",
      patch: { updateBusy: false, updateError: "没有待安装的更新，请重新检查。" },
    });
    return;
  }

  dispatch({
    type: "UPDATE",
    patch: { updateBusy: true, updateProgress: null, updateError: null },
  });

  let got = 0;
  let total: number | null = null;

  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") {
        total = ev.data.contentLength ?? null;
      } else if (ev.event === "Progress") {
        got += ev.data.chunkLength;
        // 总数未知时给 `null`：界面据此只说"正在下载"。
        // **绝不编一个假百分比** —— 它会在 100% 处停住，看起来就是卡死。
        dispatch({
          type: "UPDATE",
          patch: {
            updateProgress: total && total > 0 ? Math.min(1, got / total) : null,
          },
        });
      }
    });

    // ★ 这一行是给 **macOS** 的：
    //   Windows 上 `downloadAndInstall` 起完 NSIS 安装器就**直接退出进程**
    //   （安装器自己带 `/R` 重启，见 Rust 侧 updater 的
    //   `nsis_restart_after_install_args`），所以这行到不了。
    //   macOS 上"安装"只是把 `.app` 换掉，**不重启跑的还是旧进程** ——
    //   不调它，用户会看到"更新成功"然后又打开一模一样的旧版本。
    dispatch({ type: "UPDATE", patch: { updateBusy: false, updateProgress: null } });
    await relaunch();
  } catch (e) {
    dispatch({
      type: "UPDATE",
      patch: { updateBusy: false, updateProgress: null, updateError: errText(e) },
    });
  }
}

/**
 * 放弃这次更新（「稍后」）。
 *
 * ★ 语义是**放弃这一次**，不是"收起来但留着"：留着就是一个后端资源挂到
 *   进程结束，而它其实没有用处 —— 用户下次想知道，重新查一遍是 0.2 秒的事，
 *   还能顺便拿到更新的版本号（清单随时可能又变了）。
 */
export async function dismissUpdate(dispatch: Dispatch): Promise<void> {
  await closePending();
  dispatch({
    type: "UPDATE",
    patch: { update: null, updateBusy: false, updateProgress: null, updateError: null },
  });
}
