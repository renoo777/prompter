/**
 * 「跟读颜色」色板。
 *
 * 为什么归**界面**持有而不是后端：同一个色号在亮色主题下需要往深里压一档才看得清，
 * 那是纯视觉的调整 —— 后端存的是"用户挑了哪一个"这个意图（`config.read_color`
 * 里是一个 key），而不是一个 RGB 数值。这样以后调色板不会动到任何老配置。
 *
 * 颜色写成 `"r g b"` 三元组而不是 `#hex`：正文里的明读指示要的是**同一个色号的
 * 三种用法** —— 当前句底色 6%、左侧竖条 100%、正在念的那个字 100% ——
 * 三元组可以直接喂给 `rgb(var(--read) / .06)`，hex 则要前端自己拆通道。
 * （这也是项目里所有颜色的既有写法，见 styles.css 顶部的变量区。）
 */
export type ReadColor = {
  key: string;
  name: string;
  /** 深色主题下的色值。 */
  rgb: string;
  /**
   * 浅色主题下的**同色号、压深一档**的色值。
   *
   * ★ 必须有两套：这几个色是按深色背景挑的（饱和度高、明度也高），
   *   直接放到浅底上，做细线（2px 下划线、6% 底色）时几乎看不见。
   *   这与 `--ok` 的做法一致（深色 `52 211 153` → 浅色 `5 150 105`）。
   *   ★ 而它不能靠 CSS 主题块覆盖：`--read` 是**内联**写在根元素上的
   *   （见 `applyLayoutVars`），内联的优先级高于主题选择器里的规则 ——
   *   所以只能由 JS 按当前主题选值。
   */
  rgbLight: string;
  /** 出厂色。整个表里只该有一个 `true` —— 表里查不到 key 时退到它。 */
  def?: boolean;
};

export const READ_COLORS: ReadColor[] = [
  { key: "red", name: "红", rgb: "239 68 68", rgbLight: "220 38 38" },
  { key: "orange", name: "橙", rgb: "249 146 42", rgbLight: "194 105 10" },
  { key: "yellow", name: "黄", rgb: "250 204 21", rgbLight: "180 138 4" },
  { key: "green", name: "绿", rgb: "52 211 153", rgbLight: "5 150 105", def: true },
  { key: "teal", name: "青", rgb: "45 212 191", rgbLight: "13 148 136" },
  { key: "blue", name: "天蓝", rgb: "56 189 248", rgbLight: "3 130 200" },
  { key: "purple", name: "紫", rgb: "167 139 250", rgbLight: "124 58 237" },
];

/**
 * 把任意值收敛成色板里**确实存在**的一个条目。
 *
 * 查不到就退到出厂色：老配置里可能留着一个已经删掉的 key，
 * 或者用户手改过 config.json。**不抛错也不返回空** ——
 * 颜色是纯外观，为它崩掉设置页不值当；而返回空会让正文高亮整块消失，
 * 那看起来像"功能坏了"，比颜色不对严重得多。
 */
function pick(key: string | null | undefined): ReadColor {
  return (
    READ_COLORS.find((c) => c.key === key) ??
    READ_COLORS.find((c) => c.def) ??
    READ_COLORS[0]
  );
}

/**
 * key + 主题 → `"r g b"`，喂给 `rgb(var(--read) / .06)` 这类式子。
 *
 * 默认 `"dark"`：色板的**主用场景就是深色**（产品默认主题），
 * 而且调用方忘了传主题时，退到"更亮的那一套"只会让浅色下不好看，
 * 退到压深的则会让默认的深色场景吃亏 —— 前者是次要场景，后者是主要场景。
 */
export function readColorRgb(
  key: string | null | undefined,
  theme: "dark" | "light" = "dark",
): string {
  const c = pick(key);
  return theme === "light" ? c.rgbLight : c.rgb;
}

/**
 * key → 色板里实际生效的那个 key。
 *
 * 设置页用它决定"哪一枚色点该画成当前项" —— 若直接拿配置值比，
 * 一个失效的 key 会让**所有**色点都不亮，用户看到的是"没选中任何颜色"。
 */
export function effectiveReadColor(key: string | null | undefined): string {
  return pick(key).key;
}
