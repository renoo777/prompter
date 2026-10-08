//! 语音识别层：把麦克风里的声音变成文本，喂给对位引擎。
//!
//! # 架构：引擎可插拔
//!
//! 上层（对位引擎、UI）只认 [`AsrEngine`] 这个 trait 和 [`AsrUpdate`] 这个事件结构，
//! 不关心文本是从本地模型出来的还是从云服务回来的。这样：
//!
//! * **本地优先**（默认）：`local` 模块，sherpa-onnx 流式模型，离线、免费、隐私安全。
//! * **云端可选**：`cloud` 模块，接口已就位，填入服务地址与密钥即可切换。
//!   汇报场景对准确率敏感，将来想换就换，不需要改动其它任何一层。
//! * **无语音**：`manual` 模块，纯手动/固定语速滚动，同时充当算法联调时的测试替身。
//!
//! 换引擎的唯一影响面是配置里改一个枚举值。

pub mod auto_scroll;
pub mod cloud;
pub mod manual;

#[cfg(feature = "local-asr")]
pub mod local;

/// 麦克风探测。依赖 cpal，所以跟本地识别绑在同一个特性上。
#[cfg(feature = "local-asr")]
pub mod mic;

use serde::{Deserialize, Serialize};

/// **谁决定文稿往下走**。
///
/// ★ 这个枚举的名字还叫 "engine"，但它列的其实是四个并列的答案：
///   我的声音（Local / Cloud）、一只时钟（Auto）、我的手（Manual）。
///   设置页按这个语义把它呈现成一行三选一（「推进方式」）——
///   把 Local / Cloud 合成「语音跟随」，而不是让用户去选"引擎"这种黑话。
///
///   为什么不拆成"一个语音引擎 + 一个自动滚动开关"：那会给出
///   「自动滚动＝开，引擎＝本地离线」这种自相矛盾的组合，
///   而屏幕上没有任何东西能告诉用户该听谁的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// 本地离线语音跟随（sherpa-onnx 流式模型）。
    Local,
    /// 云端语音跟随。
    Cloud,
    /// 不使用语音，纯手动推进。
    Manual,
    /// 不使用语音，按固定语速自动推进。
    Auto,
}

impl Default for EngineKind {
    fn default() -> Self {
        EngineKind::Manual
    }
}

impl EngineKind {
    pub fn label(self) -> &'static str {
        match self {
            EngineKind::Local => "本地离线识别",
            EngineKind::Cloud => "云端识别",
            EngineKind::Manual => "纯手动",
            EngineKind::Auto => "匀速滚动",
        }
    }

    /// 这个方式要不要占用麦克风。
    ///
    /// 界面靠它决定要不要提示"得给麦克风权限"，以及为什么
    /// 「匀速滚动 / 纯手动」不用做麦克风自检。
    pub fn uses_mic(self) -> bool {
        matches!(self, EngineKind::Local | EngineKind::Cloud)
    }
}

/// 一次识别结果的推送。
///
/// `text` 是**累积假设**而非增量片段 —— 流式识别会不断修正前面的内容，
/// 下游按增量拼接会拼出重复或错乱的字。这里由引擎侧负责把「已确定部分 +
/// 当前未确定部分」拼好再发出来，下游直接整体替换即可。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AsrUpdate {
    /// 当前累积文本（可直接整体替换上一次的结果）。
    pub text: String,
    /// 引擎**直接指定**的绝对位置（字符索引），`Some` 时上层跳过文本匹配。
    ///
    /// ★ 两条来源的处境正好相反，所以走两条路：
    ///   * 语音识别 —— "我知道听到了什么，但不知道这对应稿子哪里" ⇒ 文本匹配（`text`）；
    ///   * 匀速滚动 —— "位置是我自己按时间算出来的，确定无疑" ⇒ 直接落（`seek`）。
    ///
    ///   让一个已知的答案去走匹配，除了多一次猜错的机会没有任何好处
    ///   （稿子里重复的词很多，匹配本来就有跳错的可能）。
    pub seek: Option<usize>,
    /// 这个 utterance 是否已经结束（端点检测触发）。
    pub is_final: bool,
    /// 当前音量电平 0..1，用于 UI 上的电平指示 —— 用户靠它判断麦克风有没有在工作。
    pub level: f32,
    /// 出错时带上原因，UI 直接展示。`None` 表示正常。
    ///
    /// 语义是**致命的**：引擎已停止工作，UI 会把状态改成「已停止」。
    pub error: Option<String>,
    /// 非致命警告：识别仍在跑，但结果大概不对，用户需要处理一下。
    ///
    /// 与 `error` 分开是必须的 —— 典型的例子是「麦克风在送数字静音」：
    /// 流是通的、解码在跑、程序没有任何一步失败，但屏幕上一个字也等不来。
    /// 这种静默失败如果只用 `error`，UI 就得在「没错」和「已停止」之间二选一，
    /// 而两者都不对：它既没停，也确实有问题。
    pub warn: Option<String>,
}

impl AsrUpdate {
    /// 构造一条**只带位置**的推送（匀速滚动用，见 `seek` 字段的说明）。
    pub fn seek_to(pos: usize) -> Self {
        Self {
            seek: Some(pos),
            ..Default::default()
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            error: Some(msg.into()),
            ..Default::default()
        }
    }

    /// 构造一条**只带电平**的推送。
    ///
    /// 需要它是因为：识别文本没变化时也要让 UI 知道「我还在收音」。
    /// 否则麦克风一旦不出声，文本永远不变，UI 就再也收不到任何更新 ——
    /// 连「麦克风死了」这件事本身都显示不出来。
    ///
    /// 目前只有本地引擎在用（云端引擎还没接），所以裁掉语音模块时它会
    /// 变成未引用。这类"暂时只有一条路径在用"的方法刻意保留 ——
    /// 它是引擎接口的一部分，不是残留。
    #[cfg_attr(not(feature = "local-asr"), allow(dead_code))]
    pub fn level_only(level: f32) -> Self {
        Self {
            level,
            ..Default::default()
        }
    }

    /// 构造一条非致命警告。同样只有本地引擎在用，理由见 `level_only`。
    #[cfg_attr(not(feature = "local-asr"), allow(dead_code))]
    pub fn warn(msg: impl Into<String>) -> Self {
        Self {
            warn: Some(msg.into()),
            ..Default::default()
        }
    }
}

/// 识别结果接收端。
pub type UpdateSink = Box<dyn Fn(AsrUpdate) + Send + 'static>;

/// 识别引擎统一接口。
pub trait AsrEngine: Send {
    /// 界面上显示的名字。
    fn name(&self) -> &'static str;

    /// 启动识别。应立即返回，真正的采集与解码在工作线程里跑 ——
    /// 模型加载可能耗时数秒，不能阻塞 UI 线程。
    fn start(&mut self, sink: UpdateSink) -> Result<(), String>;

    /// 停止识别并释放麦克风。
    fn stop(&mut self);

    fn is_running(&self) -> bool;

    /// 运行中调速（字/分）。
    ///
    /// 只有匀速滚动用得到 —— 语音引擎没有"速度"这个旋钮。
    /// 默认实现是空操作，好让调用方不必先判断"当前是哪种推进方式"：
    /// 那种判断散在几处，迟早会有一处忘了跟着改。
    fn set_rate(&mut self, _wpm: f32) {}
}

/// 引擎启动需要的**稿件上下文**。
///
/// 语音引擎用不到（它们自己从麦克风拿文本），但匀速滚动**必须知道**
/// 稿子多长、从哪个字开始、多快 —— 它的位置就是按这三样算出来的。
///
/// ★ 做成参数而不是"让引擎自己去全局状态里查"：引擎拿到的是一个不可变快照，
///   于是它跑在自己的线程里时不需加锁，也不会与"用户中途换了稿子"这类事纠缠。
///   稿子变了，下一次 `start_asr` 会带着新的 seed 重建引擎。
#[derive(Debug, Clone, Default)]
pub struct EngineSeed {
    /// 稿件总字数。推到这里就停。
    pub total: usize,
    /// 起点（字符索引），通常是当前光标 —— 「暂停一下、回头接着念」要的就是它。
    pub start: usize,
    /// 匀速滚动的速度（字/分）。其它推进方式忽略它。
    pub wpm: f32,
}

/// 根据配置构造引擎。
pub fn create(
    kind: EngineKind,
    cfg: &AsrConfig,
    seed: &EngineSeed,
) -> Result<Box<dyn AsrEngine>, String> {
    match kind {
        EngineKind::Local => {
            #[cfg(feature = "local-asr")]
            {
                let paths = cfg
                    .local_model
                    .clone()
                    .ok_or_else(|| "尚未配置本地模型路径".to_string())?;
                Ok(Box::new(local::LocalAsr::new(paths, cfg.num_threads)))
            }
            #[cfg(not(feature = "local-asr"))]
            {
                Err("当前构建未包含本地识别能力（编译时未开启 local-asr 特性）".to_string())
            }
        }
        EngineKind::Cloud => Ok(Box::new(cloud::CloudAsr::new(cfg.cloud.clone()))),
        EngineKind::Manual => Ok(Box::new(manual::ManualAsr::new())),
        EngineKind::Auto => Ok(Box::new(auto_scroll::AutoScrollAsr::new(
            seed.total,
            seed.start,
            seed.wpm,
        ))),
    }
}

/// 本地模型文件路径集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPaths {
    pub encoder: String,
    pub decoder: String,
    pub joiner: String,
    pub tokens: String,
    /// 模型展示名，仅用于 UI。
    #[serde(default)]
    pub label: String,
}

impl ModelPaths {
    /// 检查四个文件是否都在。返回缺失的文件列表。
    ///
    /// 在**启动识别之前**就查完 —— 缺文件时 sherpa 的报错信息非常笼统，
    /// 提前查能给用户一句「缺哪个文件」，省掉一轮排查。
    pub fn validate(&self) -> Vec<String> {
        let mut missing = Vec::new();
        for (name, p) in [
            ("encoder", &self.encoder),
            ("decoder", &self.decoder),
            ("joiner", &self.joiner),
            ("tokens", &self.tokens),
        ] {
            if p.trim().is_empty() || !std::path::Path::new(p).is_file() {
                missing.push(format!("{name}: {p}"));
            }
        }
        missing
    }
}

/// 云端引擎配置（预留）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudConfig {
    /// 兼容 OpenAI 协议的实时识别端点，例如 `wss://api.example.com/v1/realtime`。
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

/// 识别层总配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AsrConfig {
    #[serde(default)]
    pub engine: EngineKind,
    #[serde(default)]
    pub local_model: Option<ModelPaths>,
    #[serde(default)]
    pub cloud: Option<CloudConfig>,
    /// 推理线程数。`0` 表示按 CPU 核数自动取。
    #[serde(default)]
    pub num_threads: i32,
}

// ---------------------------------------------------------------------------
// 麦克风探测的结果类型
// ---------------------------------------------------------------------------
//
// 这里只放**载荷的形状**，「怎么判定电平」在 `mic` 模块里。
//
// 这样切分是被 `--no-default-features`（裁掉语音模块）这条构建路径逼出来的：
// 命令 `mic_check` 的返回类型必须始终存在，否则那个构建会直接编不过；
// 而判定函数只有带语音模块时才有调用方，留在本模块就会变成一堆
// dead-code 警告。分开之后两种构建都干净。

/// 麦克风拾音判定档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicVerdict {
    /// 拾音正常。
    Ok,
    /// 有信号但很弱。
    Weak,
    /// 数字静音：驱动层送来的是恒定近零值，不是「房间安静」。
    Silent,
}

/// 一次麦克风探测的结果。UI 直接展示，用户也可以直接复制出来求助。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicProbe {
    /// 被探测的设备名。
    pub device: String,
    pub sample_rate: i32,
    pub channels: usize,
    /// 该设备的默认采样格式（`F32` / `I16` / `U16`）。
    pub format: String,
    /// 实际录制时长（秒）。
    pub seconds: f64,
    /// 采到的帧数。理论上应约等于 `seconds * sample_rate`。
    pub frames: usize,
    /// 峰值（0..1）。
    pub peak: f32,
    /// 均方根电平（0..1）。
    pub rms: f32,
    /// 判定档位。
    pub verdict: MicVerdict,
    /// 一句话结论 + 处置建议。
    pub detail: String,
    /// 系统里所有输入设备的清单（排查「换一个麦克风试试」时用）。
    pub devices: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 警告和错误必须是两条通道 —— 这是修掉「麦克风无声却一声不吭」的核心。
    ///
    /// 混成一条的话，UI 只有两个选择：当成没错（症状照旧，静默失败），
    /// 或者当成"功能已停"（用户会去重启识别，而问题不在那儿）。
    #[test]
    fn warn_and_error_are_distinct_channels() {
        let w = AsrUpdate::warn("测试");
        assert!(w.warn.is_some());
        assert!(w.error.is_none(), "警告不能被当成致命错误");

        let e = AsrUpdate::error("测试");
        assert!(e.error.is_some());
        assert!(e.warn.is_none(), "致命错误不该同时是警告");

        let h = AsrUpdate::level_only(0.5);
        assert_eq!(h.level, 0.5);
        assert!(h.text.is_empty());
        assert!(h.warn.is_none() && h.error.is_none());
    }
}
