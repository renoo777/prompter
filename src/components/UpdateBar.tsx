import { useApp } from "../lib/store";
import { dismissUpdate, installPendingUpdate } from "../lib/update";

/**
 * 更新提示条 —— 挂在顶栏下方的一条细横条。
 *
 * # 为什么它是一条**真的占位**的横条，而不是浮在稿子上
 *
 * 浮层（像查找栏那样 `position: absolute`）会**盖住正文开头**。
 * 查找是"用户主动唤出、用完就收"的，盖一下无所谓；而更新提示是**常驻**的
 * （用户可能整场会都不理它）—— 常驻的东西盖住稿子，正是本项目最忌讳的一条：
 * 提词器上，任何挡住正在念的字的装饰都是净损失。
 *
 * ⇒ 所以它进正常文档流：在时正文从它下面开始，不在时（绝大多数时候）
 *   整个 DOM 里没有这个节点，版面与从前逐像素一致。
 *
 * # 三个形态共用一条
 *
 *   available   新版本 x.y.z 可用      [更新] [稍后]
 *   busy        正在下载 42%           （底边一条进度线）
 *   失败        更新失败：<原因>        [重试] [稍后]
 *
 * ★ 「稍后」＝**放弃这一次**（`dismissUpdate()` 会把手里的句柄关掉），
 *   不是"收起来但留着"。理由：留着就意味着一个后端资源挂到进程结束，
 *   而它其实没有任何用处 —— 用户下次想知道，重新查一遍是 0.2 秒的事，
 *   还能顺便拿到更新的版本号（清单随时可能又变了）。
 *
 * ★ 界面**不显示"下载了多少 MB"**：这个窗口是给讲台上的人看的，
 *   他关心的是"能不能现在装",不是字节数。要的就是一眼。
 */
export function UpdateBar() {
  const { state, dispatch } = useApp();
  const { update, updateBusy, updateProgress, updateError } = state;

  // 没有待安装的更新 ⇒ 这个组件**不渲染任何东西**（连空壳都不留）。
  // 判据只看 `update`：手动检查失败那种"没有更新可装"的错误属于设置页那一行，
  // 不该在这里长出一条针对"不存在的更新"的提示条。
  if (!update) return null;

  /*
   * 「更新」与「稍后」都走 `lib/update.ts` 里那两个共用函数 ——
   * 设置页里那一行调的是**同一个** `installPendingUpdate` / `dismissUpdate`。
   * 两处各写一遍的话，迟早漂成"提示条上点得动、设置里点了没反应"。
   */
  const start = () => void installPendingUpdate(dispatch);
  const later = () => void dismissUpdate(dispatch);

  /** 下载中的百分比文字。总数未知时**不编**，只说"下载中"。 */
  const pctText =
    updateProgress === null ? null : `${Math.round(updateProgress * 100)}%`;

  return (
    <div className="update-bar" data-interactive>
      <span className="ub-msg">
        {updateError ? (
          <>
            <span className="ub-strong">更新失败</span>
            {`：${updateError}`}
          </>
        ) : updateBusy ? (
          pctText === null ? (
            "正在下载更新…"
          ) : (
            `正在下载更新 ${pctText}`
          )
        ) : (
          <>
            <span className="ub-strong">新版本 {update.version} 可用</span>
            {`（当前 ${update.currentVersion}）`}
          </>
        )}
      </span>

      <span className="ub-actions">
        {updateBusy ? null : (
          <button className="ub-btn primary" onClick={start}>
            {updateError ? "重试" : "更新"}
          </button>
        )}
        <button className="ub-btn" onClick={later}>
          稍后
        </button>
      </span>

      {/*
        进度线画在整条的底边上（一条 2px 的填充）。
        ★ 它是**唯一**在动的东西，而且只动 `width` —— 这条条子在场时
          用户的注意力应该在稿子上，不该被一个跳动的数字抢走。
      */}
      {updateBusy && (
        <div className="ub-track" aria-hidden="true">
          <div
            className={`ub-fill${updateProgress === null ? " unknown" : ""}`}
            style={
              updateProgress === null ? undefined : { width: `${updateProgress * 100}%` }
            }
          />
        </div>
      )}
    </div>
  );
}
