//! 云端识别引擎 —— **接口已就位，尚未接入具体服务**。
//!
//! # 为什么先把接口留出来
//!
//! 本地模型和云服务在汇报场景下各有硬伤，而且**取决于具体会议**：
//!
//! | | 本地离线 | 云端 |
//! |---|---|---|
//! | 专业术语 / 人名准确率 | 一般 | 明显更好 |
//! | 断网 / 弱网 | 不受影响 | 直接不可用 |
//! | 隐私 | 音频不出本机 | **整段汇报语音上传到第三方** |
//! | 成本 | 0 | 按量计费 |
//! | 首次启动 | 需加载 ~300MB 模型 | 无需准备 |
//!
//! 用户很可能出现「平时用本地，重要汇报前切云」这种摇摆。如果引擎层不做抽象，
//! 到时候就得改对位、改 UI、改状态机 —— 所以 trait 先立在这里，
//! 接入时只需要在这一个文件里补实现。
//!
//! # 接入需要做的事（按顺序）
//!
//! 1. 在 [`CloudConfig`] 里确认端点与密钥已经由设置页写入（字段已存在）。
//! 2. 把麦克风采集接进来：可以直接复用 `local` 模块的 cpal 采集逻辑 ——
//!    建议把它抽成独立的 `mic` 模块，两个引擎共用一份。
//! 3. 把采集到的 PCM 按服务要求编码（多数服务要 16kHz / 16bit / 单声道）
//!    后推流，接收侧的增量文本走 `sink` 推送，格式与本地引擎保持一致。
//! 4. 断线重连：网络抖动要自动重连，并把「正在重连」通过
//!    [`AsrUpdate::error`] 告知用户 —— 静默失联在会议上是很糟的体验。
//!
//! 在 2~4 完成之前，这个引擎会明确地报「未接入」而不是假装在跑。

use super::{AsrEngine, CloudConfig, UpdateSink};

pub struct CloudAsr {
    config: Option<CloudConfig>,
    running: bool,
}

impl CloudAsr {
    pub fn new(config: Option<CloudConfig>) -> Self {
        Self {
            config,
            running: false,
        }
    }
}

impl AsrEngine for CloudAsr {
    fn name(&self) -> &'static str {
        "云端识别"
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn start(&mut self, sink: UpdateSink) -> Result<(), String> {
        let msg = match &self.config {
            None => "云端识别尚未配置服务地址与密钥。".to_string(),
            Some(c) if c.endpoint.trim().is_empty() => {
                "云端识别的服务地址为空，请在设置里填写。".to_string()
            }
            Some(_) => "云端识别通道尚未实现。当前版本请使用「本地离线识别」或「手动模式」。"
                .to_string(),
        };
        sink(super::AsrUpdate::error(msg.clone()));
        Err(msg)
    }

    fn stop(&mut self) {
        self.running = false;
    }
}
