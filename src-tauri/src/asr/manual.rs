//! 手动模式：不使用语音识别，完全靠快捷键/滚轮推进。
//!
//! # 这个模式存在的三个理由
//!
//! 1. **兜底**：麦克风坏了、环境嘈杂、不想让软件听自己说话时，产品仍然可用。
//!    这也是「隐身」失效时的第一道退路 —— 核心价值（投屏看不见）还在。
//! 2. **联调替身**：开发对位算法时用它替代真实识别，可以在没有麦克风的环境里
//!    复现问题，也让自动化测试不必依赖音频输入。
//! 3. **对齐基线**：手动模式下光标完全由人控制，没有任何自动推进 ——
//!    这是对照「自动跟随到底准不准」的基准线。
//!
//! `start` / `stop` 是空实现，因为这里不需要采集任何东西。

use super::{AsrEngine, UpdateSink};

pub struct ManualAsr {
    running: bool,
}

impl ManualAsr {
    pub fn new() -> Self {
        Self { running: false }
    }
}

impl Default for ManualAsr {
    fn default() -> Self {
        Self::new()
    }
}

impl AsrEngine for ManualAsr {
    fn name(&self) -> &'static str {
        "手动模式"
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn start(&mut self, _sink: UpdateSink) -> Result<(), String> {
        self.running = true;
        log::info!("[asr] 手动模式已启用（不采集音频）");
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }
}
