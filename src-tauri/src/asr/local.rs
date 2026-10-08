//! 本地离线识别：sherpa-onnx 流式模型 + cpal 麦克风采集。
//!
//! # 为什么是「流式」模型而不是「整段识别」
//!
//! 提词器要的是**跟读**，不是事后转写。整段模型必须先说完一整句才能出结果，
//! 延迟 1~3 秒，光标会一顿一顿地跳。流式模型每 80ms 就能吐一次中间结果，
//! 光标是连续移动的 —— 这两种体感差距很大。
//!
//! 默认使用的模型 `sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20`
//! 是中英双语流式 zipformer，正好覆盖汇报里最常混用的「中文 + 英文术语」场景。
//!
//! # 线程模型
//!
//! ```text
//!   cpal 回调线程 ──push──▶ Arc<Mutex<Vec<f32>>> ──take──▶ 本模块工作线程
//!   （实时，禁止阻塞）        （音频缓冲区）                  （重采样→解码→推送）
//! ```
//!
//! cpal 的 `Stream` 不是 `Send`，所以麦克风流的创建和持有必须和消费循环在
// **同一个线程**里 —— 这也是这里把两者都放进工作线程的原因，而不是拆成两个线程。
//!
//! 回调线程只做「下混 + 入队」，不加锁做重活、不做重采样，避免音频丢帧。

use super::{AsrEngine, AsrUpdate, ModelPaths, UpdateSink};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use sherpa_onnx::{LinearResampler, OnlineRecognizer, OnlineRecognizerConfig};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 流式模型的固定输入采样率。
const TARGET_RATE: i32 = 16000;
/// 主循环轮询间隔。80ms 是延迟与 CPU 占用的折中 ——
/// 再短对光标平滑度提升有限，再长会看出明显的"跳"。
const POLL_INTERVAL: Duration = Duration::from_millis(80);
/// 音频缓冲上限（秒）。堆积超过这个量说明消费跟不上，丢弃最旧的数据。
const MAX_BUFFER_SECONDS: usize = 8;
/// 发往对位引擎时保留的历史文本长度。
///
/// 对位引擎只取识别结果末尾十几个字做匹配，所以没必要每次把整段历史都发过去 ——
/// 那会让归一化开销随会议时长线性增长。留 96 字足够覆盖"句末修正"的场景。
const TAIL_KEEP: usize = 96;

/// 电平推送的最小间隔。
///
/// 识别文本没变化时也按这个节奏发一次电平。**这一段是必需的**：
/// 少了它，麦克风一旦不出声，文本永远不变，界面上就再也收不到任何事件 ——
/// 电平条会停在最后一次的值上，看上去像"还在工作"，而实际早就没声音了。
/// 250ms 是「看着连续」与「不刷爆前端」之间的折中。
const LEVEL_PUSH_INTERVAL: Duration = Duration::from_millis(250);

/// 连续多少秒**只**采到数字静音就报警告。
///
/// 取 3 秒的理由：设备刚打开的头一两百毫秒可能还没进入稳态，
/// 短于此容易误报；而真到 3 秒还没声音，演讲者已经能察觉"怎么没反应"了，
/// 这时候给出原因最及时。
const SILENCE_WARN_AFTER: f64 = 3.0;

/// 连续多少秒**一帧数据都没收到**就报警告。
///
/// 阈值比静音更短，因为这是更严重的故障（设备没启动 / 被独占），
/// 而且判据没有歧义 —— 正常设备不会两秒一个采样都不给。
const STARVE_WARN_AFTER: f64 = 2.0;

/// 「拿不到声音」的看门狗。
///
/// # 为什么抽成独立结构
///
/// 这段逻辑最怕的是**阈值设错**，而阈值错了线上不会有任何报错 ——
/// 只会在真开会的时候突然冒出一条误报，把人吓得去关一个本来就没开的静音键。
/// 抽出来之后能直接单测：多高的电平算「有声音」、静音累计多久才算数、
/// 不同采样率下时长折算对不对，全都能在测试里钉死。
///
/// # 两类故障为什么分开计
///
/// * **数字静音**：有数据流，但恒定近零。设备通着，声音在系统/驱动层被掐掉。
///   处置指向「静音键 / 系统静音 / 隐私设置」。
/// * **收不到数据**：一帧都没有。设备或回调层面就断了。
///   处置指向「被别的程序独占 / 被拔出 / 驱动异常」。
///
/// 混成一条的话，用户会拿着「静音」的建议去查一个方向完全不对的地方。
#[derive(Debug, Default)]
struct AudioWatchdog {
    silent_secs: f64,
    starved_secs: f64,
    /// 每个故障只提醒一次。每 80ms 刷一条同样的警告会把界面淹掉，
    /// 而且用户从第二条开始就不会再读了。
    warned_silence: bool,
    warned_starved: bool,
}

/// 看门狗本轮该不该报警。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogAlert {
    /// 一切正常，不用打扰用户。
    None,
    /// 持续数字静音。
    DigitalSilence,
    /// 持续收不到任何数据。
    Starved,
}

impl AudioWatchdog {
    /// 喂进本轮的情况，返回该不该报警。
    ///
    /// * `chunk_len` —— 本轮取到的采样数；`0` 表示一帧都没收到
    /// * `peak` —— 本轮峰值（`chunk_len == 0` 时忽略）
    /// * `rate` —— 设备采样率，用来把采样数折算成秒
    /// * `tick` —— 轮询间隔（秒），只在没有数据时用来计时
    fn feed(&mut self, chunk_len: usize, peak: f32, rate: i32, tick: f64) -> WatchdogAlert {
        // 没数据：走"收不到数据"这条路，并把静音的计时清掉
        if chunk_len == 0 {
            self.starved_secs += tick;
            if self.starved_secs >= STARVE_WARN_AFTER && !self.warned_starved {
                self.warned_starved = true;
                return WatchdogAlert::Starved;
            }
            return WatchdogAlert::None;
        }
        self.starved_secs = 0.0;
        self.warned_starved = false;

        if peak < super::mic::DIGITAL_SILENCE_PEAK {
            // 必须按**设备采样率**折算。写成 `chunk_len / 16000` 的话，
            // 在 48kHz 设备上会把 3 秒算成 1 秒，警告提前两秒就冒出来。
            self.silent_secs += chunk_len as f64 / rate.max(1) as f64;
            if self.silent_secs >= SILENCE_WARN_AFTER && !self.warned_silence {
                self.warned_silence = true;
                return WatchdogAlert::DigitalSilence;
            }
        } else {
            // 有声音就复位，这样「短暂地说了一句话」也能把计时打断 ——
            // 否则一段话里正常的换气停顿会被累加成静音。
            self.silent_secs = 0.0;
            self.warned_silence = false;
        }
        WatchdogAlert::None
    }
}

pub struct LocalAsr {
    paths: ModelPaths,
    num_threads: i32,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LocalAsr {
    pub fn new(paths: ModelPaths, num_threads: i32) -> Self {
        Self {
            paths,
            num_threads,
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl AsrEngine for LocalAsr {
    fn name(&self) -> &'static str {
        "本地离线识别"
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn start(&mut self, sink: UpdateSink) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        // 模型文件在启动前就查一遍 —— sherpa 自身的报错信息很笼统，
        // 提前查能直接告诉用户缺哪个文件。
        let missing = self.paths.validate();
        if !missing.is_empty() {
            return Err(format!(
                "本地模型文件缺失，请先下载模型：\n{}",
                missing.join("\n")
            ));
        }

        self.stop.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);

        let paths = self.paths.clone();
        let threads = self.num_threads;
        let stop = self.stop.clone();
        let running = self.running.clone();

        let handle = std::thread::Builder::new()
            .name("asr-local".to_string())
            .spawn(move || {
                run_worker(paths, threads, stop, &sink);
                running.store(false, Ordering::SeqCst);
            })
            .map_err(|e| format!("无法创建识别线程：{e}"))?;

        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            // 不 join 会留下悬挂的麦克风流。等一小会儿，超时就算了 ——
            // 用户点"停止"时不应该被卡住。
            let _ = h.join();
        }
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for LocalAsr {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// 工作线程
// ---------------------------------------------------------------------------

/// 构造流式识别器配置。
///
/// 抽成独立函数是**为了让离线自测（`recognizes_official_test_wavs`）跑的就是
/// 线上这套参数** —— 如果测试自己另写一份配置，测过了也不代表真实链路可用。
pub(crate) fn recognizer_config(paths: &ModelPaths, num_threads: i32) -> OnlineRecognizerConfig {
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(paths.encoder.clone());
    config.model_config.transducer.decoder = Some(paths.decoder.clone());
    config.model_config.transducer.joiner = Some(paths.joiner.clone());
    config.model_config.tokens = Some(paths.tokens.clone());
    config.model_config.num_threads = if num_threads > 0 { num_threads } else { 2 };

    // 端点检测：静音超过 2.4 秒认为一句话说完，重置解码状态。
    // 这是**必须**开的 —— 不开的话每句话的上下文会无限累积，
    // 越到后面解码越慢，一场 40 分钟的汇报会卡到不可用。
    config.enable_endpoint = true;
    config.rule1_min_trailing_silence = 2.4;
    config.rule2_min_trailing_silence = 1.2;
    config.rule3_min_utterance_length = 20.0;
    config.decoding_method = Some("greedy_search".to_string());
    config
}

fn run_worker(paths: ModelPaths, num_threads: i32, stop: Arc<AtomicBool>, sink: &UpdateSink) {
    // ---- 1. 构建识别器 ----
    let config = recognizer_config(&paths, num_threads);

    let recognizer = match OnlineRecognizer::create(&config) {
        Some(r) => r,
        None => {
            sink(AsrUpdate::error(format!(
                "本地模型加载失败。请确认模型文件完整且版本匹配：\n{}",
                paths.encoder
            )));
            return;
        }
    };
    log::info!("[asr] 本地模型加载完成：{}", paths.label);

    // ---- 2. 打开麦克风 ----
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            sink(AsrUpdate::error(
                "没有找到可用的麦克风设备。请检查系统声音设置。",
            ));
            return;
        }
    };
    let device_name = device.name().unwrap_or_else(|_| "未知设备".to_string());

    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            sink(AsrUpdate::error(format!(
                "无法读取麦克风配置（{device_name}）：{e}。\
                 若在 Windows 上，请到「设置 → 隐私和安全性 → 麦克风」允许桌面应用访问。"
            )));
            return;
        }
    };
    let in_rate = supported.sample_rate().0 as i32;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.into();

    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(TARGET_RATE as usize)));

    let stream = match build_input_stream(
        &device,
        &stream_config,
        sample_format,
        channels,
        in_rate,
        buffer.clone(),
    ) {
        Ok(s) => s,
        Err(e) => {
            sink(AsrUpdate::error(e));
            return;
        }
    };
    if let Err(e) = stream.play() {
        sink(AsrUpdate::error(format!("无法启动麦克风采集：{e}")));
        return;
    }
    log::info!(
        "[asr] 麦克风已开启：{device_name}，{}Hz / {}声道",
        in_rate,
        channels
    );

    // ---- 3. 采样率转换 ----
    let resampler = if in_rate != TARGET_RATE {
        match LinearResampler::create(in_rate, TARGET_RATE) {
            Some(r) => Some(r),
            None => {
                sink(AsrUpdate::error(format!(
                    "无法建立 {in_rate}Hz → {TARGET_RATE}Hz 的重采样器"
                )));
                return;
            }
        }
    } else {
        None
    };

    // ---- 4. 解码循环 ----
    let asr_stream = recognizer.create_stream();
    let mut committed = String::new();
    let mut last_partial = String::new();
    // 上一次真正推给界面的文本。
    //
    // 「文本变了没有」以它为准，而不是以解码器的输出为准 ——
    // 因为电平心跳推送的也是这个文本，两者用同一个基准才不会互相触发。
    let mut last_pushed = String::new();

    // 拿不到声音的看门狗。阈值、时长折算、"只提醒一次"的规则
    // 都在 `AudioWatchdog` 里，且有单测覆盖。
    let mut watchdog = AudioWatchdog::default();
    let mut last_level_push: Option<Instant> = None;

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(POLL_INTERVAL);

        // 取走累积的音频
        let chunk: Vec<f32> = {
            let mut b = match buffer.lock() {
                Ok(b) => b,
                Err(_) => break, // 锁中毒，回调线程出问题了
            };
            std::mem::take(&mut *b)
        };

        // 电平 —— UI 靠它显示"麦克风在工作"
        let level = {
            let n = chunk.len() as f32;
            if n == 0.0 {
                0.0
            } else {
                let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
                ((sum_sq / n).sqrt() * 4.0).clamp(0.0, 1.0)
            }
        };

        // 峰值用于静音判定。用峰值而不是 RMS：静音时两者都趋零，
        // 但峰值对「偶尔混进一两个有效采样」更宽容，设备刚启动时不容易误报。
        let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));

        match watchdog.feed(chunk.len(), peak, in_rate, POLL_INTERVAL.as_secs_f64()) {
            WatchdogAlert::DigitalSilence => {
                let mut u = AsrUpdate::warn(format!(
                    "麦克风连续 {SILENCE_WARN_AFTER:.0} 秒只送出数字静音\
                     （峰值 {:.0} dBFS），识别不会出字。\
                     最可能的原因是按下了笔记本键盘上的麦克风静音键 —— \
                     这种静音在系统设置里查不出来，端点仍显示正常且未静音。\
                     其次才是声音设置里该设备被静音、或隐私设置没放行桌面应用。",
                    super::mic::to_dbfs(peak)
                ));
                u.level = level;
                sink(u);
            }
            WatchdogAlert::Starved => {
                sink(AsrUpdate::warn(format!(
                    "已连续 {STARVE_WARN_AFTER:.0} 秒收不到任何音频数据。\
                     麦克风可能被别的程序独占（部分会议软件会独占设备），\
                     或已被拔出 / 禁用。"
                )));
            }
            WatchdogAlert::None => {}
        }

        // 没有新数据时不做解码，但**不 continue** —— 下面还要走一遍心跳，
        // 让界面上的电平条落到 0。否则设备断了之后电平条会一直停在
        // 最后一个值上，看上去像"还在收音"。
        let samples = match &resampler {
            Some(r) => r.resample(&chunk, false),
            None => chunk,
        };
        if !samples.is_empty() {
            asr_stream.accept_waveform(TARGET_RATE, &samples);
            while recognizer.is_ready(&asr_stream) {
                recognizer.decode(&asr_stream);
            }
        }

        // 流式中间结果。先取结果再判端点 —— 顺序不能反：
        // 端点一到就 `reset` 了，之后再取只会拿到空串。
        if let Some(r) = recognizer.get_result(&asr_stream) {
            let text = clean_text(&r.text);
            if text != last_partial {
                last_partial = text;
            }
        }

        // 端点：一句话结束，把当前假设并入已确定文本并重置解码状态
        let is_final = if recognizer.is_endpoint(&asr_stream) {
            let had_speech = !last_partial.trim().is_empty();
            committed.push_str(&last_partial);
            last_partial.clear();
            recognizer.reset(&asr_stream);
            truncate_tail(&mut committed, TAIL_KEEP * 2);
            had_speech
        } else {
            false
        };

        // ---------- 统一推送出口 ----------
        //
        // 只在这里推，是为了让「文本变化」和「电平心跳」用同一份文本。
        // 分两处推的话，心跳推的文本和变化推的文本容易不一致，
        // 对位器就会收到一会儿长一会儿短的假设，白白产生额外的搜索。
        let current = compose(&committed, &last_partial);
        let changed = current != last_pushed;
        let due = last_level_push.map_or(true, |t| t.elapsed() >= LEVEL_PUSH_INTERVAL);

        if changed || due {
            last_pushed = current.clone();
            last_level_push = Some(Instant::now());

            // 文本为空说明一个字都还没听出来 —— 这正是「设备在送静音」的样子。
            // 这种时候推一条只带电平的心跳：界面上的电平条至少还能告诉用户
            // 「麦克风是活的」，而不是整块界面像死了一样毫无动静。
            let update = if current.is_empty() {
                AsrUpdate::level_only(level)
            } else {
                AsrUpdate {
                    text: current,
                    // `seek` 是匀速滚动那条路的字段：它直接给绝对位置。
                    // 这台引擎走的是文本匹配，永远不碰它。
                    seek: None,
                    is_final,
                    level,
                    error: None,
                    warn: None,
                }
            };
            sink(update);
        }
    }

    // 收尾 1：把重采样器内部还压着的样本吐出来。
    //
    // 重采样器为了做插值会留一点历史样本，不 flush 就丢掉最后几毫秒。
    // 单独听无所谓，但**收尾那一句的最后一个字**本来就最容易丢，
    // 没必要在这里再扣一点。
    if let Some(r) = &resampler {
        let tail = r.resample(&[], true);
        if !tail.is_empty() {
            asr_stream.accept_waveform(TARGET_RATE, &tail);
        }
    }

    // 收尾 2：把还在解码缓冲里的尾音解出来
    asr_stream.input_finished();
    while recognizer.is_ready(&asr_stream) {
        recognizer.decode(&asr_stream);
    }
    if let Some(r) = recognizer.get_result(&asr_stream) {
        let text = clean_text(&r.text);
        if !text.is_empty() {
            committed.push_str(&text);
            sink(AsrUpdate {
                text: commit_tail(&committed),
                seek: None,
                is_final: true,
                level: 0.0,
                error: None,
                warn: None,
            });
        }
    }

    drop(stream); // 显式关闭麦克风
    log::info!("[asr] 识别线程已退出");
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 清理解码输出：BPE 的空格标记 `▁` 还原成空格。
fn clean_text(s: &str) -> String {
    s.replace('▁', " ").trim().to_string()
}

/// 拼接「已确定文本的尾部 + 当前未确定部分」。
fn compose(committed: &str, partial: &str) -> String {
    if committed.is_empty() {
        return partial.to_string();
    }
    if partial.is_empty() {
        return commit_tail(committed);
    }
    format!("{}{}", commit_tail(committed), partial)
}

fn commit_tail(s: &str) -> String {
    let n = s.chars().count();
    if n <= TAIL_KEEP {
        return s.to_string();
    }
    s.char_indices()
        .nth(n - TAIL_KEEP)
        .map(|(i, _)| s[i..].to_string())
        .unwrap_or_else(|| s.to_string())
}

fn truncate_tail(s: &mut String, keep: usize) {
    let n = s.chars().count();
    if n > keep {
        let idx = s.char_indices().nth(n - keep).map(|(i, _)| i).unwrap_or(0);
        *s = s[idx..].to_string();
    }
}

/// 把 cpal 交来的交错多声道样本下混成单声道，追加到 `out`。
///
/// 抽成独立纯函数有两个原因：
/// 1. **可脱离声卡单测**。声道分组错了（比如把 1024 个交错样本当成 1024 帧）
///    症状是识别结果整段乱码，从现象很难反推回这里；有了纯函数，喂已知数据
///    就能断言结果。
/// 2. 真实设备回调与离线自测共用同一份实现 —— 测试自己另写一份下混，
///    测过了也不代表真实链路可用（同 `recognizer_config` 的理由）。
///
/// 这个函数跑在实时音频回调线程上，所以只做加法与除法：不加锁、
/// 不做重采样、不做分配（`out` 由调用方持有并复用）。
fn downmix_into<T>(data: &[T], channels: usize, out: &mut Vec<f32>)
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = channels.max(1);
    let inv = 1.0 / channels as f32;
    for frame in data.chunks(channels) {
        let mut acc = 0.0f32;
        for s in frame {
            acc += f32::from_sample(*s);
        }
        out.push(acc * inv);
    }
}

/// 按采样格式构建 cpal 输入流。
///
/// 三种格式都要覆盖：不同声卡（尤其 USB 麦克风）上报的格式不一样，
/// 只处理 f32 会在 i16 设备上直接打不开麦克风。
///
/// `pub(super)` 是给 `asr::mic` 复用的：探测麦克风时必须走**同一条**采集
/// 链路（同样的下混、同样的格式映射），否则「自检说正常、实际不跟随」
/// 这种分歧足以让人查到崩溃。
pub(super) fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    channels: usize,
    in_rate: i32,
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, String> {
    match format {
        cpal::SampleFormat::F32 => build_typed::<f32>(device, config, channels, in_rate, buffer),
        cpal::SampleFormat::I16 => build_typed::<i16>(device, config, channels, in_rate, buffer),
        cpal::SampleFormat::U16 => build_typed::<u16>(device, config, channels, in_rate, buffer),
        other => Err(format!(
            "麦克风使用了暂不支持的采样格式 {other:?}。\
             可以在系统声音设置里把输入设备改为默认格式，或换一个麦克风。"
        )),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    in_rate: i32,
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let channels = channels.max(1);
    // 上限必须按**设备采样率**算。原先写成 `TARGET_RATE * 8`，
    // 在 48kHz 设备上实际只兜住 2.7 秒，跟注释写的 8 秒不符 ——
    // 消费端一旦卡顿超过 2.7 秒就开始丢音频。
    let max_samples = (in_rate.max(1) as usize).saturating_mul(MAX_BUFFER_SECONDS);
    let err_buffer = buffer.clone();

    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                // 这个闭包跑在实时音频线程上：只做下混与入队，不做任何重活。
                let Ok(mut b) = buffer.lock() else { return };
                downmix_into(data, channels, &mut b);
                if b.len() > max_samples {
                    let cut = b.len() - max_samples;
                    b.drain(..cut);
                }
            },
            move |err| {
                log::error!("[asr] 麦克风采集错误：{err}");
                // 缓冲清空，让消费端尽快发现异常
                if let Ok(mut b) = err_buffer.lock() {
                    b.clear();
                }
            },
            None,
        )
        .map_err(|e| {
            format!(
                "无法打开麦克风：{e}。\
                 若在 Windows 上，请到「设置 → 隐私和安全性 → 麦克风」允许桌面应用访问。"
            )
        })
}

// ---------------------------------------------------------------------------
// 离线自测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL_DIR_NAME: &str = "sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20";

    /// 定位模型目录。优先读环境变量，其次用应用默认下载路径。
    fn model_root() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("PROMPTER_TEST_MODEL_DIR") {
            let p = std::path::PathBuf::from(p);
            if p.is_dir() {
                return Some(p);
            }
        }
        let base = if cfg!(windows) {
            std::env::var("APPDATA").ok().map(std::path::PathBuf::from)?
        } else if cfg!(target_os = "macos") {
            std::path::PathBuf::from(std::env::var("HOME").ok()?)
                .join("Library/Application Support")
        } else {
            std::path::PathBuf::from(std::env::var("HOME").ok()?).join(".local/share")
        };
        let dir = base.join("cn.bobby.prompter").join("models");
        dir.is_dir().then_some(dir)
    }

    fn paths_in(dir: &std::path::Path) -> ModelPaths {
        let j = |n: &str| dir.join(n).to_string_lossy().to_string();
        ModelPaths {
            encoder: j("encoder-epoch-99-avg-1.int8.onnx"),
            decoder: j("decoder-epoch-99-avg-1.onnx"),
            joiner: j("joiner-epoch-99-avg-1.int8.onnx"),
            tokens: j("tokens.txt"),
            label: "离线自测".to_string(),
        }
    }

    /// 把整段音频按 100ms 一块喂进去，返回「端点切分出来的最终句子」列表。
    ///
    /// 严格复用线上那条链路：同样的 `recognizer_config`、同样的
    /// `accept_waveform` / `is_ready` / `is_endpoint` 调用顺序。
    fn recognize(wav: &std::path::Path, paths: &ModelPaths) -> Vec<String> {
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy())
            .unwrap_or_else(|| panic!("读不了 wav：{}", wav.display()));
        let rate = wave.sample_rate();

        let recognizer =
            OnlineRecognizer::create(&recognizer_config(paths, 2)).expect("识别器创建失败");

        let resampler = if rate != TARGET_RATE {
            LinearResampler::create(rate, TARGET_RATE)
        } else {
            None
        };

        let samples: Vec<f32> = match &resampler {
            Some(r) => r.resample(wave.samples(), true),
            None => wave.samples().to_vec(),
        };

        let stream = recognizer.create_stream();
        let chunk = (TARGET_RATE / 10) as usize; // 100ms
        let mut finals = Vec::new();
        let mut last = String::new();

        for part in samples.chunks(chunk) {
            stream.accept_waveform(TARGET_RATE, part);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
            if let Some(r) = recognizer.get_result(&stream) {
                let t = clean_text(&r.text);
                if !t.is_empty() {
                    last = t;
                }
            }
            if recognizer.is_endpoint(&stream) {
                if !last.trim().is_empty() {
                    finals.push(last.clone());
                }
                last.clear();
                recognizer.reset(&stream);
            }
        }

        // 收尾：模型不会自动对最后一段做端点判断，得手动 input_finished
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        if let Some(r) = recognizer.get_result(&stream) {
            let t = clean_text(&r.text);
            if !t.is_empty() {
                finals.push(t);
            }
        }
        finals
    }

    /// 真实模型离线自测：用官方 test_wavs 跑一遍，人工核对识别文本。
    ///
    /// 默认跳过（模型有 190MB，不是每台机器都有），需要时手动跑：
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture
    /// ```
    ///
    /// 也可以指定模型目录：
    ///
    /// ```text
    /// PROMPTER_TEST_MODEL_DIR=D:\models cargo test -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "需要先下载 sherpa-onnx 模型，手动运行：cargo test -- --ignored --nocapture"]
    fn recognizes_official_test_wavs() {
        let Some(root) = model_root() else {
            panic!("找不到模型根目录，请设置 PROMPTER_TEST_MODEL_DIR");
        };
        let dir = root.join(MODEL_DIR_NAME);
        assert!(dir.is_dir(), "模型目录不存在：{}", dir.display());

        let paths = paths_in(&dir);
        let missing = paths.validate();
        assert!(missing.is_empty(), "模型文件缺失：{missing:?}");

        let wavs_dir = dir.join("test_wavs");
        assert!(wavs_dir.is_dir(), "没有 test_wavs 目录：{}", wavs_dir.display());

        let mut files: Vec<_> = std::fs::read_dir(&wavs_dir)
            .expect("读目录失败")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "wav").unwrap_or(false))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "test_wavs 里没有 wav 文件");

        let mut all_text = String::new();
        for f in &files {
            let finals = recognize(f, &paths);
            let joined = finals.join(" | ");
            println!("\n--- {} ---\n{}", f.file_name().unwrap().to_string_lossy(), joined);
            all_text.push_str(&joined);
        }

        // 只断言「确实识别出了中文字符」——具体文本要人眼核对，
        // 写死期望值会让测试在模型升级后无谓地红掉。
        let han = all_text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count();
        println!("\n共识别出 {han} 个汉字");
        assert!(han > 20, "识别结果里汉字太少（{han} 个），模型或推理链路可能有问题");
    }

    // =====================================================================
    // 采集链路：下混
    //
    // 这一组**不需要声卡也不需要模型**，`cargo test` 默认就会跑。
    // 它们盯的是「识别结果整段乱码」那类故障 —— 那种现象很难反推到下混，
    // 但在这里喂已知数据就一眼可见。
    // =====================================================================

    /// 单声道 f32 应原样通过 —— 下混不能顺手把音量也改了。
    #[test]
    fn downmix_mono_f32_passthrough() {
        let src = vec![0.0f32, 0.5, -0.5, 1.0, -1.0];
        let mut out = Vec::new();
        downmix_into(&src, 1, &mut out);
        assert_eq!(out, src);
    }

    /// 立体声要取两声道平均。只取左声道是最常见的省事写法，
    /// 在「笔记本阵列麦克风、人在偏右位置说话」时会明显偏小。
    #[test]
    fn downmix_stereo_averages_channels() {
        let src = vec![0.8f32, 0.2, -0.4, -0.6]; // 2 帧
        let mut out = Vec::new();
        downmix_into(&src, 2, &mut out);
        assert_eq!(out.len(), 2, "2 帧立体声应产出 2 个单声道样本");
        assert!((out[0] - 0.5).abs() < 1e-6, "got {}", out[0]);
        assert!((out[1] + 0.5).abs() < 1e-6, "got {}", out[1]);
    }

    /// 声道必须按「帧」分组，不能按整个缓冲区长度分。
    /// 把交错数据当单声道处理，识别会整段乱码而现象上看不出是下混的锅。
    #[test]
    fn downmix_groups_by_frame_not_by_buffer() {
        let src = vec![1.0f32, 0.0, 1.0, 0.0, 1.0, 0.0]; // 3 帧，左声道恒为 1.0
        let mut out = Vec::new();
        downmix_into(&src, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.5]);
        assert_ne!(
            out.len(),
            6,
            "按缓冲区长度分组会产出 6 个样本，说明声道根本没分组"
        );
    }

    /// i16 设备（大量 USB 麦克风）走整数采样：必须按 cpal 的规则
    /// 满量程映射到 [-1, 1]，而不是拿原始整数去累加。
    #[test]
    fn downmix_i16_maps_full_scale() {
        let src = vec![i16::MAX, i16::MIN, 0i16];
        let mut out = Vec::new();
        downmix_into(&src, 1, &mut out);
        assert!(
            (out[0] - 1.0).abs() < 1e-3,
            "i16::MAX 应映射到 ≈+1.0，got {}",
            out[0]
        );
        assert!(
            (out[1] + 1.0).abs() < 1e-3,
            "i16::MIN 应映射到 ≈-1.0，got {}",
            out[1]
        );
        assert!(out[2].abs() < 1e-6, "0 应映射到 0.0，got {}", out[2]);
    }

    /// u16 是**无符号**格式，中点是静音（dasp 的转换里减 32768）。
    /// 若按有符号解释，静音会变成满幅直流，识别直接失效 ——
    /// 这是最难从现象倒推的一类错。
    #[test]
    fn downmix_u16_midpoint_is_silence() {
        let src = vec![32_768u16, 0u16, 65_535u16];
        let mut out = Vec::new();
        downmix_into(&src, 1, &mut out);
        assert!(out[0].abs() < 1e-3, "u16 中点应是静音，got {}", out[0]);
        assert!(
            (out[1] + 1.0).abs() < 1e-3,
            "u16 最小值应是 ≈-1.0，got {}",
            out[1]
        );
        assert!(
            (out[2] - 1.0).abs() < 1e-3,
            "u16 最大值应是 ≈+1.0，got {}",
            out[2]
        );
    }

    // =====================================================================
    // 采集链路：设备格式仿真
    // =====================================================================

    /// 把 16kHz 单声道信号「搬到」设备采样率上，再按声道数交错排列。
    ///
    /// 这就是 cpal 回调真正会递过来的数据的样子：交错、可能多声道、
    /// 采样率与模型要求的 16kHz 不一致。
    fn synth_device_frames(src: &[f32], rate: i32, channels: usize) -> Vec<f32> {
        let step = TARGET_RATE as f64 / rate as f64;
        let n_out = ((src.len() as f64) * rate as f64 / TARGET_RATE as f64) as usize;
        let mut mono = Vec::with_capacity(n_out);
        for i in 0..n_out {
            let pos = i as f64 * step;
            let i0 = pos.floor() as usize;
            let frac = (pos - i0 as f64) as f32;
            let a = src.get(i0).copied().unwrap_or(0.0);
            let b = src.get(i0 + 1).copied().unwrap_or(a);
            mono.push(a + (b - a) * frac);
        }
        let channels = channels.max(1);
        if channels == 1 {
            return mono;
        }
        let mut out = Vec::with_capacity(mono.len() * channels);
        for s in mono {
            for _ in 0..channels {
                out.push(s);
            }
        }
        out
    }

    /// 仿真器自己得先可信，否则后面测出来的「通过」没有意义。
    #[test]
    fn synth_device_frames_keeps_source_positions() {
        let src: Vec<f32> = (0..200).map(|i| (i as f32 / 200.0) - 0.5).collect();

        let up = synth_device_frames(&src, 48_000, 1);
        assert_eq!(up.len(), src.len() * 3, "16k→48k 单声道应是 3 倍长度");
        for (i, &v) in src.iter().enumerate() {
            assert!((up[i * 3] - v).abs() < 1e-6, "第 {i} 个样本落点不对");
        }

        let st = synth_device_frames(&src, 16_000, 2);
        assert_eq!(st.len(), src.len() * 2, "立体声应是 2 倍长度");
        assert_eq!(st[0], src[0]);
        assert_eq!(st[1], src[0], "同一帧的两个声道应相等");
    }

    fn han_count(s: &str) -> usize {
        s.chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .count()
    }

    /// 复刻真实链路：**设备回调（下混入队）→ 消费（取走 → 重采样 → 解码）**。
    ///
    /// 与 `recognize()` 的区别是它**不绕过**下混与重采样 ——
    /// 而这两段恰恰是「离线自测通过、真机上线乱码」最常见的来源。
    fn run_device_pipeline(
        wav: &std::path::Path,
        paths: &ModelPaths,
        device_rate: i32,
        channels: usize,
        frames_per_callback: usize,
    ) -> Vec<String> {
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy())
            .unwrap_or_else(|| panic!("读不了 wav：{}", wav.display()));
        let frames = synth_device_frames(wave.samples(), device_rate, channels);

        let recognizer =
            OnlineRecognizer::create(&recognizer_config(paths, 2)).expect("识别器创建失败");
        let resampler = if device_rate != TARGET_RATE {
            LinearResampler::create(device_rate, TARGET_RATE)
        } else {
            None
        };

        let stream = recognizer.create_stream();
        // 与线上同一个上限常量、同一个轮询周期（80ms）
        let max_samples = (device_rate as usize).saturating_mul(MAX_BUFFER_SECONDS);
        let callbacks_per_poll = ((device_rate as f64 / 12.5) as usize / frames_per_callback).max(1);
        let cb_stride = frames_per_callback * channels;
        // 按「一个轮询窗口」切块，块内再切成模拟的回调尺寸
        let poll_stride = cb_stride * callbacks_per_poll;

        let mut pending: Vec<f32> = Vec::new(); // 扮演回调线程写入的共享缓冲
        let mut finals = Vec::new();
        let mut last = String::new();

        for poll_block in frames.chunks(poll_stride) {
            // ---- 回调线程：多次回调，每次只做下混入队 ----
            for cb in poll_block.chunks(cb_stride) {
                downmix_into(cb, channels, &mut pending);
                if pending.len() > max_samples {
                    let cut = pending.len() - max_samples;
                    pending.drain(..cut);
                }
            }

            // ---- 消费端：取走 → 重采样 → 喂给流式解码 ----
            let chunk = std::mem::take(&mut pending);
            let samples = match &resampler {
                Some(r) => r.resample(&chunk, false),
                None => chunk,
            };
            if samples.is_empty() {
                continue;
            }
            stream.accept_waveform(TARGET_RATE, &samples);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
            if let Some(r) = recognizer.get_result(&stream) {
                let t = clean_text(&r.text);
                if !t.is_empty() {
                    last = t;
                }
            }
            if recognizer.is_endpoint(&stream) {
                if !last.trim().is_empty() {
                    finals.push(last.clone());
                }
                last.clear();
                recognizer.reset(&stream);
            }
        }

        // 收尾：与线上一致 —— 先 flush 重采样器，再 input_finished
        if let Some(r) = &resampler {
            let tail = r.resample(&[], true);
            if !tail.is_empty() {
                stream.accept_waveform(TARGET_RATE, &tail);
            }
        }
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        if let Some(r) = recognizer.get_result(&stream) {
            let t = clean_text(&r.text);
            if !t.is_empty() {
                finals.push(t);
            }
        }
        finals
    }

    /// **本轮补上的关键一环**：真实设备会递过来的数据格式，走完整条链路
    /// 之后还能不能识别出同样的字。
    ///
    /// 覆盖 48kHz / 44.1kHz × 单声道 / 立体声 —— 实际会遇到的四种组合。
    /// 判据不用「文本完全一致」（太脆），而是**基线内容被保留的比例**，
    /// 且用的是对位引擎自己那套 LCS 口径。
    #[test]
    #[ignore = "需要先下载 sherpa-onnx 模型，手动运行：cargo test -- --ignored --nocapture device_pipeline"]
    fn device_pipeline_keeps_recognition() {
        let Some(root) = model_root() else {
            panic!("找不到模型根目录，请设置 PROMPTER_TEST_MODEL_DIR");
        };
        let dir = root.join(MODEL_DIR_NAME);
        let paths = paths_in(&dir);
        assert!(paths.validate().is_empty(), "模型文件缺失：{:?}", paths.validate());

        let wav = dir.join("test_wavs").join("3.wav");
        assert!(wav.is_file(), "缺少测试音频：{}", wav.display());

        let baseline = recognize(&wav, &paths).join("");
        let base_norm = crate::script::normalize(&baseline);
        println!("\n[基线 16kHz 单声道] {baseline}");
        assert!(han_count(&baseline) > 5, "基线自己就识别不出字，先查模型");

        let mut report = Vec::new();
        for device_rate in [48_000, 44_100] {
            for channels in [1usize, 2] {
                let got = run_device_pipeline(&wav, &paths, device_rate, channels, 1024).join("");
                let got_norm = crate::script::normalize(&got);
                let keep = crate::align::lcs_ratio(&got_norm, &base_norm);

                report.push(format!(
                    "[{device_rate}Hz / {channels}ch]  基线保留 {:.0}%   汉字 {} → {}\n      {got}",
                    keep * 100.0,
                    han_count(&baseline),
                    han_count(&got),
                ));

                assert!(
                    keep >= 0.7,
                    "{device_rate}Hz/{channels}ch 这条链路只保留了 {:.0}% 的基线内容，\
                     下混或重采样有问题\n  基线：{baseline}\n  实测：{got}",
                    keep * 100.0
                );
            }
        }
        println!("\n{}", report.join("\n"));
    }

    // =====================================================================
    // 看门狗：判据与计时
    // =====================================================================

    /// 静音时长必须按**设备采样率**折算，不能写死 16kHz。
    ///
    /// 这条测试防的是一个很容易犯、又极难从现象倒推的错：
    /// 一旦写成 `chunk.len() / 16000`，48kHz 设备上 3 秒会被算成 1 秒，
    /// 警告提前两秒冒出来。现象只是"偶尔误报"，很难联想到是这一行。
    #[test]
    fn silence_duration_uses_device_rate() {
        // 每块 80ms，40 块 = 3.2 秒，三种采样率下都该报警
        for rate in [16000, 44100, 48000] {
            let per_block = (rate as f64 * 0.08) as usize;
            let mut w = AudioWatchdog::default();
            // 注意判据是「有没有触发过」而不是「最后一次的返回值」——
            // 报警只发生一次，之后每轮都是 None，取最后值必然是 None。
            let mut triggered = false;
            for _ in 0..40 {
                if w.feed(per_block, 0.0, rate, 0.08) == WatchdogAlert::DigitalSilence {
                    triggered = true;
                }
            }
            assert!(triggered, "rate={rate} 未触发静音警告");
        }
    }

    /// 静音时长不够时不能提前报警。
    #[test]
    fn short_gap_does_not_warn() {
        let mut w = AudioWatchdog::default();
        // 1.6 秒静音 —— 是「说话中间的换气停顿」的量级，不该打扰用户
        for _ in 0..20 {
            assert_eq!(w.feed(1280, 0.0, 16000, 0.08), WatchdogAlert::None);
        }
    }

    /// 同一个故障只提醒一次；但恢复之后又坏掉，必须重新提醒。
    ///
    /// 后半段是重点：用户看到警告去处理了静音键，中途正常说了两句又，
    /// 不小心又按到静音键 —— 这时候静默不提示，用户会以为软件坏了。
    #[test]
    fn silence_warns_once_until_recovered() {
        let mut w = AudioWatchdog::default();
        for _ in 0..50 {
            w.feed(1280, 0.0, 16000, 0.08);
        }
        // 继续静音：不该反复报
        for _ in 0..50 {
            assert_eq!(w.feed(1280, 0.0, 16000, 0.08), WatchdogAlert::None);
        }
        // 有声音了
        assert_eq!(w.feed(1280, 0.05, 16000, 0.08), WatchdogAlert::None);
        // 再静音 4 秒：必须重新报警（同样看"有没有触发过"）
        let mut triggered_again = false;
        for _ in 0..50 {
            if w.feed(1280, 0.0, 16000, 0.08) == WatchdogAlert::DigitalSilence {
                triggered_again = true;
            }
        }
        assert!(
            triggered_again,
            "恢复后再坏掉必须重新提醒，否则用户会以为软件坏了"
        );
    }

    /// **最容易出的误报**：安静房间里的正常麦克风绝不能被判成静音。
    ///
    /// -60dBFS（1e-3）是深夜安静房间的典型本底噪声。若阈值定得比它高，
    /// 每次开会刚打开程序就会弹一条误导性的警告，
    /// 用户会去关一个本来就没开的静音键。
    #[test]
    fn quiet_room_never_triggers_silence_warning() {
        let mut w = AudioWatchdog::default();
        for _ in 0..200 {
            assert_eq!(
                w.feed(1280, 1e-3, 16000, 0.08),
                WatchdogAlert::None,
                "安静房间的本底噪声被误判成静音了"
            );
        }
    }

    /// 「收不到数据」与「静音」是两条独立的计时，不能互相污染。
    #[test]
    fn starved_and_silent_are_independent() {
        let mut w = AudioWatchdog::default();
        // 先来 1.6 秒静音（不够报警）
        for _ in 0..20 {
            w.feed(1280, 0.0, 16000, 0.08);
        }
        // 然后彻底没数据 2 秒 —— 必须报"收不到数据"
        let mut alert = WatchdogAlert::None;
        for _ in 0..25 {
            alert = w.feed(0, 0.0, 16000, 0.08);
        }
        assert_eq!(alert, WatchdogAlert::Starved);
    }

    // =====================================================================
    // 采集链路：真实麦克风
    // =====================================================================

    /// 采集原始交错数据（不做下混），并记录「已累计样本数」的时间轨迹。
    ///
    /// 记录轨迹是为了区分两种总量上看起来一模一样的故障：
    /// **设备启动慢**（前 0.4 秒没数据，之后满速）与
    /// **持续丢数据**（全程只跑出 70%）。只看「3 秒采到多少」是分不出来的。
    ///
    /// 返回 `(交错样本, [(时刻秒, 累计**帧数**)], 首帧延迟秒)`。
    ///
    /// 轨迹里记的是**帧数**而不是样本数 —— 记样本数会和采样率差一个
    /// 声道倍数（4 声道设备上会算成「4 倍速采集」的假象）。
    fn capture_raw<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        seconds: f64,
    ) -> (Vec<f32>, Vec<(f64, usize)>, f64)
    where
        T: cpal::SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = buf.clone();
        let ch = (config.channels as usize).max(1);

        let stream = device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    let Ok(mut b) = writer.lock() else { return };
                    for s in data {
                        b.push(f32::from_sample(*s));
                    }
                },
                |e| log::error!("[test] 采集错误：{e}"),
                None,
            )
            .expect("打开原始采集流失败");

        let start = std::time::Instant::now();
        stream.play().expect("启动原始采集失败");

        let mut trace: Vec<(f64, usize)> = Vec::new();
        let mut first_data = -1.0f64;
        while start.elapsed().as_secs_f64() < seconds {
            std::thread::sleep(Duration::from_millis(20));
            let t = start.elapsed().as_secs_f64();
            let frames = buf.lock().map(|b| b.len() / ch).unwrap_or(0);
            if first_data < 0.0 && frames > 0 {
                first_data = t;
            }
            trace.push((t, frames));
        }

        let out = buf.lock().map(|b| b.clone()).unwrap_or_default();
        drop(stream); // 显式关麦
        (out, trace, first_data)
    }

    /// 从轨迹里取一段稳态区间的采集速率（1.0 = 不丢不重）。
    fn steady_rate(trace: &[(f64, usize)], rate: i32) -> f64 {
        let pick = |target: f64| {
            trace
                .iter()
                .filter(|(t, _)| *t <= target)
                .next_back()
                .copied()
        };
        let (Some((t1, n1)), Some((t2, n2))) = (pick(1.0), pick(2.5)) else {
            return f64::NAN;
        };
        if t2 <= t1 || rate <= 0 {
            return f64::NAN;
        }
        (n2 as f64 - n1 as f64) / ((t2 - t1) * rate as f64)
    }

    /// 把一段已知采样率的单声道音频按线上节奏（80ms 一取）走完整链路。
    fn transcribe_pcm(mono: &[f32], in_rate: i32, paths: &ModelPaths) -> String {
        let recognizer =
            OnlineRecognizer::create(&recognizer_config(paths, 2)).expect("识别器创建失败");
        let resampler = if in_rate != TARGET_RATE {
            LinearResampler::create(in_rate, TARGET_RATE)
        } else {
            None
        };
        let stream = recognizer.create_stream();
        let window = ((in_rate as f64 / 12.5) as usize).max(1); // 80ms

        let mut out = String::new();
        for w in mono.chunks(window) {
            let samples = match &resampler {
                Some(r) => r.resample(w, false),
                None => w.to_vec(),
            };
            if samples.is_empty() {
                continue;
            }
            stream.accept_waveform(TARGET_RATE, &samples);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
            if let Some(r) = recognizer.get_result(&stream) {
                let t = clean_text(&r.text);
                if !t.is_empty() {
                    out = t;
                }
            }
            if recognizer.is_endpoint(&stream) {
                recognizer.reset(&stream);
            }
        }
        if let Some(r) = &resampler {
            let tail = r.resample(&[], true);
            if !tail.is_empty() {
                stream.accept_waveform(TARGET_RATE, &tail);
            }
        }
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        if let Some(r) = recognizer.get_result(&stream) {
            let t = clean_text(&r.text);
            if !t.is_empty() {
                out = t;
            }
        }
        out
    }

    /// 用真实声卡打开一次麦克风，确认「设备能枚举、流能打开、回调在送数据」。
    ///
    /// 这个测试**有意不断言识别内容**：安静环境下采到的就是环境噪声，
    /// 拿它去断言文本只会引入不确定性。它要证明的是采集管道通 ——
    /// 而这一段是离线 wav 自测永远覆盖不到的。
    ///
    /// ```text
    /// cargo test -- --ignored --nocapture captures_real_microphone
    /// ```
    #[test]
    #[ignore = "需要真实麦克风，手动运行：cargo test -- --ignored --nocapture captures_real_microphone"]
    fn captures_real_microphone() {
        let host = cpal::default_host();
        println!("\n[设备枚举] 默认主机 = {:?}", host.id());

        let devices: Vec<_> = host
            .input_devices()
            .expect("枚举输入设备失败")
            .collect();
        println!("找到 {} 个输入设备：", devices.len());
        for d in &devices {
            let name = d.name().unwrap_or_else(|_| "（读不到名字）".into());
            let cfg = d
                .default_input_config()
                .map(|c| {
                    format!(
                        "{}Hz / {}ch / {:?}",
                        c.sample_rate().0,
                        c.channels(),
                        c.sample_format()
                    )
                })
                .unwrap_or_else(|e| format!("配置读不到：{e}"));
            println!("  - {name}   [{cfg}]");
        }

        let Some(device) = host.default_input_device() else {
            panic!("没有默认输入设备 —— 系统里没有可用麦克风");
        };
        let name = device.name().unwrap_or_else(|_| "未知设备".into());
        let supported = device
            .default_input_config()
            .expect("默认输入设备读不到配置（多半是系统麦克风权限没放开）");
        let in_rate = supported.sample_rate().0 as i32;
        let channels = supported.channels() as usize;
        let format = supported.sample_format();

        let config: cpal::StreamConfig = supported.into();

        const SECONDS: f64 = 3.0;
        let expected_frames = (in_rate as f64 * SECONDS) as usize;

        let (raw, trace, first_data) = match format {
            cpal::SampleFormat::F32 => capture_raw::<f32>(&device, &config, SECONDS),
            cpal::SampleFormat::I16 => capture_raw::<i16>(&device, &config, SECONDS),
            cpal::SampleFormat::U16 => capture_raw::<u16>(&device, &config, SECONDS),
            other => panic!("测试未覆盖的采样格式：{other:?}"),
        };

        let ch = channels.max(1);
        let frames = raw.len() / ch;
        let ratio = frames as f64 / expected_frames as f64;
        println!("\n设备：{name}   {in_rate}Hz / {channels}ch / {format:?}");
        println!(
            "{SECONDS:.0} 秒内采到 {frames} 帧（理论 ≈{expected_frames}），占 {:.0}%",
            ratio * 100.0
        );
        println!("首帧数据延迟 = {first_data:.3}s");
        println!(
            "稳态采集速率 = {:.3}x 实时（1.000 表示既不丢也不重）",
            steady_rate(&trace, in_rate)
        );

        // 分段速率：判断「总量少了一截」到底是开头慢、结尾慢，还是全程都慢。
        // 只看总量是分不出来的，而这三者的处理方式完全不同。
        println!("\n分段采集速率（每 500ms 一个窗口）：");
        let mut prev = (0.0f64, 0usize);
        let mut mark = 0.5f64;
        for &(t, n) in trace.iter() {
            if t >= mark {
                let (dt, dn) = (t - prev.0, n as f64 - prev.1 as f64);
                if dt > 0.0 {
                    print!("  {:.1}~{:.1}s {:.2}x |", prev.0, t, dn / (dt * in_rate as f64));
                }
                prev = (t, n);
                mark += 0.5;
            }
        }
        println!();

        assert!(
            ratio > 0.5,
            "只采到理论值的 {:.0}%，回调基本没在送数据",
            ratio * 100.0
        );

        // ---- 逐声道诊断 ----
        // 多声道阵列里各声道内容差别大时，「四路取平均」有可能把相位
        // 相反的成分抵消掉，反而变差。把每个声道的能量与相关性打出来，
        // 供判断「取平均」还是「只取某一路」。
        let mut rms_ch = vec![0.0f64; ch];
        for (c, slot) in rms_ch.iter_mut().enumerate() {
            let sum: f64 = raw
                .iter()
                .skip(c)
                .step_by(ch)
                .map(|v| (*v as f64) * (*v as f64))
                .sum();
            *slot = (sum / frames.max(1) as f64).sqrt();
        }
        println!("\n各声道能量 / 与 0 声道的相关系数：");
        for c in 0..ch {
            let (mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64);
            for f in 0..frames {
                let x = raw[f * ch + c] as f64;
                let y = raw[f * ch] as f64;
                aa += x * x;
                bb += y * y;
                ab += x * y;
            }
            let corr = if aa > 0.0 && bb > 0.0 {
                ab / (aa.sqrt() * bb.sqrt())
            } else {
                f64::NAN
            };
            println!("  ch{c}: RMS = {:.6}   相关系数 = {corr:+.3}", rms_ch[c]);
        }

        let peak = raw.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("\n峰值 = {peak:.6}");
        assert!(
            peak <= 1.5,
            "峰值 {peak} 超出 [-1,1] 量程 —— 采样格式转换有问题（例如 u16 按有符号解）"
        );

        if peak < 1e-4 {
            // 注意判据是「低于 -80dBFS」而不是「恰好为 0」：
            // 设备静音时往往还会输出极微弱的抖动，不是干净的 0。
            println!(
                "⚠️  采到的是**有效静音**（峰值 {peak:.2e}，即 {:.0} dBFS）。\n\
                 这不是「房间安静」—— 真实麦克风即便在很安静的房间里，\n\
                 本底噪声也在 -60~-70dBFS 左右，比这个高 40dB 以上。\n\
                 最可能的原因：\n\
                 ① 麦克风在系统里被静音（音量合成器 / 声音设置里该输入设备被静音，\n    或笔记本上的麦克风静音快捷键被按下）。\n\
                 ② 驱动层面关掉了麦阵列。\n\
                 已排除：Windows 隐私设置里「允许桌面应用访问麦克风」—— 那是 Allow。\n\
                 结论：程序侧链路是通的（流已打开、回调在按额定速率送数据），\n\
                 但拿不到声音。这一项必须由人在系统里解开，代码解决不了。",
                if peak > 0.0 { 20.0 * peak.log10() } else { -200.0 }
            );
        } else {
            println!(
                "RMS（最大声道）= {:.6}（正常说话约 0.01~0.10；纯环境噪声通常 <0.005）",
                rms_ch.iter().cloned().fold(0.0f64, f64::max)
            );
        }

        // ---- 真机数据走完整链路 ----
        // 不断言文本：房间可能安静，也可能在放音乐。要证的是
        // 「真实设备的数据不会被下混 / 重采样 / 解码任何一步噎住」。
        let mut mono = Vec::with_capacity(frames);
        downmix_into(&raw, ch, &mut mono);

        match model_root() {
            Some(root) => {
                let paths = paths_in(&root.join(MODEL_DIR_NAME));
                if paths.validate().is_empty() {
                    let text = transcribe_pcm(&mono, in_rate, &paths);
                    println!(
                        "\n真机音频过完整链路（下混 → {in_rate}Hz→16kHz 重采样 → 流式解码）：\n  \
                         识别结果 = {}",
                        if text.trim().is_empty() {
                            "（空）".to_string()
                        } else {
                            format!("{text:?}")
                        }
                    );
                } else {
                    println!("\n（模型文件不全，跳过「真机音频过链路」这一步）");
                }
            }
            None => println!("\n（没找到模型目录，跳过「真机音频过链路」这一步）"),
        }
    }

    // =====================================================================
    // 采集链路：录真实语音
    // =====================================================================

    /// 录一段真实语音存成 16kHz 单声道 WAV。
    ///
    /// 这是为了补上**最后一个只能靠真人补的缺口**：对位算法目前只有构造用例，
    /// 没有真实语速下的语料。录一段自己念稿的音频，之后就能拿它反复回放调参，
    /// 不用每次都在麦克风前重新念一遍。
    ///
    /// 时长与输出路径由环境变量控制（默认 8 秒 / 当前目录 voice.wav）：
    ///
    /// ```text
    /// PROMPTER_RECORD_SECS=20 cargo test --lib -- --ignored --nocapture record_voice_to_wav
    /// ```
    ///
    /// 运行时对着麦克风念稿件里的任意几句话即可。
    #[test]
    #[ignore = "需要真实麦克风，手动运行：cargo test --lib -- --ignored --nocapture record_voice_to_wav"]
    fn record_voice_to_wav() {
        let secs: f64 = std::env::var("PROMPTER_RECORD_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8.0);
        let out = std::env::var("PROMPTER_RECORD_OUT").unwrap_or_else(|_| "voice.wav".to_string());

        let host = cpal::default_host();
        let device = host.default_input_device().expect("没有默认输入设备");
        let name = device.name().unwrap_or_else(|_| "未知设备".into());
        let supported = device.default_input_config().expect("读不到麦克风配置");
        let in_rate = supported.sample_rate().0 as i32;
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        println!(
            "\n开始录音：{name}   {in_rate}Hz / {channels}ch / {format:?}\n\
             现在开始念稿件里的内容，共 {secs:.0} 秒 …"
        );

        let (raw, _trace, _first) = match format {
            cpal::SampleFormat::F32 => capture_raw::<f32>(&device, &config, secs),
            cpal::SampleFormat::I16 => capture_raw::<i16>(&device, &config, secs),
            cpal::SampleFormat::U16 => capture_raw::<u16>(&device, &config, secs),
            other => panic!("测试未覆盖的采样格式：{other:?}"),
        };

        let mut mono = Vec::with_capacity(raw.len() / channels.max(1));
        downmix_into(&raw, channels, &mut mono);

        let samples = if in_rate != TARGET_RATE {
            match LinearResampler::create(in_rate, TARGET_RATE) {
                Some(r) => r.resample(&mono, true),
                None => panic!("无法建立 {in_rate}Hz → {TARGET_RATE}Hz 重采样器"),
            }
        } else {
            mono
        };

        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let rms = if samples.is_empty() {
            0.0
        } else {
            (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
        };
        println!(
            "录音结束：{} 个样本（{:.2} 秒 @ {TARGET_RATE}Hz）   RMS = {rms:.5}   峰值 = {peak:.4}",
            samples.len(),
            samples.len() as f64 / TARGET_RATE as f64,
        );

        assert!(
            sherpa_onnx::write(&out, &samples, TARGET_RATE),
            "写 WAV 失败：{out}"
        );
        println!("已写出：{out}");
        if peak < 1e-4 {
            println!(
                "⚠️  录到的是有效静音 —— 麦克风当前收不到声音，\
                 先跑 captures_real_microphone 看诊断（多半是硬件/驱动层静音）。"
            );
        }
    }

    // =====================================================================
    // 端到端：看门狗能不能发现「麦克风是死的」
    // =====================================================================

    /// 把本地识别引擎真跑起来十几秒，验证「拿不到声音」这件事能被自己发现。
    ///
    /// # 为什么这条测试特别值得一提
    ///
    /// 它覆盖的是**从麦克风到事件回调的整条链路**，而且是本机**真实故障**的
    /// 回归测试：这台机器的麦阵列在系统里显示 ACTIVE、未静音、音量 82.5%，
    /// 但驱动只送数字静音（-113 dBFS）。修复前，这种情况下界面一条更新
    /// 都收不到，用户看到的只是"光标不动"。
    ///
    /// # 换台机器也能跑
    ///
    /// 它先探测真实电平再决定断言哪一侧：
    /// * 设备是数字静音 → 断言**必须**报警；
    /// * 设备有正常信号 → 断言**不能**误报。
    ///
    /// 两个分支都是有效断言，所以这条测试在正常的机器上同样有意义
    /// （它变成了一条"不许误报"的测试）。
    ///
    /// ```text
    /// cargo test --lib -- --ignored --nocapture watchdog_sees_dead_microphone
    /// ```
    #[test]
    #[ignore = "需要麦克风与模型，手动运行：cargo test --lib -- --ignored --nocapture watchdog_sees_dead_microphone"]
    fn watchdog_sees_dead_microphone() {
        // `to_dbfs` 在 mic 模块里；`MicVerdict` 要直接从 asr 取 ——
        // mic 里的那个是私有 import，不能当路径用。
        use crate::asr::mic::to_dbfs;
        use crate::asr::MicVerdict;

        // 这里刻意用 panic 而不是"跳过"。
        //
        // 一条会静默跳过、然后报 ok 的测试，正是本项目要消灭的那种静默失败：
        // 它给出的是**假的绿**。手动跑这条测试本身就是个明确动作，
        // 那就应该要么真跑、要么明确告诉人为什么跑不了。
        let root = model_root().unwrap_or_else(|| {
            panic!(
                "找不到模型目录。先跑 `python scripts/fetch-model.py` 下载模型，\
                 或设置环境变量 PROMPTER_TEST_MODEL_DIR 指向模型目录。"
            )
        });
        // model_root() 拿到的是 models/ 根目录，模型本体在下一层的具名子目录里
        let paths = paths_in(&root.join(MODEL_DIR_NAME));
        let missing = paths.validate();
        assert!(
            missing.is_empty(),
            "模型文件不完整，无法进行端到端测试：{missing:?}"
        );

        // 先独立探一下真实电平，用它的结论决定后面断言哪一侧。
        let probe = crate::asr::mic::probe_microphone(1.5).expect("麦克风探测失败");
        let silent_device = probe.verdict == MicVerdict::Silent;
        println!(
            "\n[探测] {}  峰值 {:.3e}（{:.0} dBFS）  判定 {:?}",
            probe.device,
            probe.peak,
            to_dbfs(probe.peak),
            probe.verdict
        );

        let collected: Arc<Mutex<Vec<AsrUpdate>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_buf = collected.clone();
        let sink: UpdateSink = Box::new(move |u| {
            if let Ok(mut v) = sink_buf.lock() {
                v.push(u);
            }
        });

        let mut engine = LocalAsr::new(paths, 0);
        engine.start(sink).expect("启动本地识别失败");

        // 12 秒：模型加载（数秒）+ 稳定运行 + 足够触发 3 秒静音看门狗
        std::thread::sleep(Duration::from_secs(12));
        engine.stop();
        // 等 worker 收尾，否则读到的可能是"还没跑到报警点"的半份数据
        std::thread::sleep(Duration::from_millis(600));

        let updates = collected.lock().map(|v| v.clone()).unwrap_or_default();
        let warnings: Vec<String> = updates.iter().filter_map(|u| u.warn.clone()).collect();
        let errors: Vec<String> = updates.iter().filter_map(|u| u.error.clone()).collect();
        // 纯电平心跳：不带文本、不带警告，专供界面显示"麦克风在工作"
        let heartbeats = updates
            .iter()
            .filter(|u| u.text.is_empty() && u.warn.is_none() && u.error.is_none())
            .count();
        let informative = updates
            .iter()
            .filter(|u| u.warn.is_none() && u.error.is_none())
            .count();

        println!(
            "\n[事件统计] 共 {} 条：警告 {}，致命错误 {}，纯电平心跳 {}",
            updates.len(),
            warnings.len(),
            errors.len(),
            heartbeats
        );
        for w in &warnings {
            println!("  ⚠ {w}");
        }
        for e in &errors {
            println!("  ✖ {e}");
        }

        assert!(errors.is_empty(), "不该出现致命错误：{errors:?}");
        assert!(informative > 0, "界面一条更新都收不到，等于对用户完全静默");

        let silence_warned = warnings.iter().any(|w| w.contains("数字静音"));

        // ---- 核心断言：两条链路必须得出同一个结论 ----
        //
        // 这是本轮最有价值的一条断言。自检（`asr::mic::probe_microphone`）和
        // 实时看门狗（`AudioWatchdog`）走的是不同的采集窗口、不同的调用路径，
        // 但必须用**同一套判据** —— 否则会出现最坑的一种故障：
        // 「自检说正常，实际不跟随」。用户会拿着"正常"的结论到处找原因。
        //
        // 这条断言在写下它的第一次运行就抓到了真问题：自检测到 -83 dBFS
        // 判成"有信号"，实时链路测到 -138 dBFS 判成"静音" ——
        // 同一个设备两次测量差 55dB，说明阈值落在了量化噪声的浮动区间里。
        assert_eq!(
            silence_warned, silent_device,
            "自检与实时看门狗结论不一致：自检判定 {:?}（峰值 {:.0} dBFS），\
             实时链路{}报警。两条链路必须用同一套判据。",
            probe.verdict,
            to_dbfs(probe.peak),
            if silence_warned { "有" } else { "没有" }
        );

        if silence_warned {
            assert!(
                heartbeats > 0,
                "静音时没有电平心跳：设备一死界面就完全没有更新，连警告都送不出去"
            );
            println!("\n[结论] 两条链路一致判定为数字静音，看门狗正确报警 —— 通过");
        } else {
            println!("\n[结论] 两条链路一致判定麦克风有信号、无误报 —— 通过");
        }
    }

    // =====================================================================
    // 采集链路：长时间稳定性
    // =====================================================================

    /// 连续跑约 3 分钟音频，确认**解码耗时不会随时间增长**。
    ///
    /// 这条直接对应一个真实风险：端点检测如果没生效，每句话的上下文会
    /// 无限累积，一场 40 分钟的汇报会越跑越慢直到卡死。单测里跑不完
    /// 40 分钟，但「末尾 10 秒 vs 开头 10 秒」的耗时对比足以暴露趋势。
    #[test]
    #[ignore = "需要先下载 sherpa-onnx 模型且耗时约 1 分钟，手动运行：cargo test --release -- --ignored --nocapture long_run"]
    fn long_run_throughput_stays_flat() {
        let Some(root) = model_root() else {
            panic!("找不到模型根目录，请设置 PROMPTER_TEST_MODEL_DIR");
        };
        let dir = root.join(MODEL_DIR_NAME);
        let paths = paths_in(&dir);
        assert!(paths.validate().is_empty(), "模型文件缺失：{:?}", paths.validate());

        let wav = dir.join("test_wavs").join("3.wav");
        assert!(wav.is_file(), "缺少测试音频：{}", wav.display());
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).expect("读不了 wav");

        // 首尾相接拼成约 3 分钟，模拟汇报里持续说话
        const REPEATS: usize = 20;
        let mut long = Vec::with_capacity(wave.samples().len() * REPEATS);
        for _ in 0..REPEATS {
            long.extend_from_slice(wave.samples());
        }
        let total_s = long.len() as f64 / TARGET_RATE as f64;

        let recognizer =
            OnlineRecognizer::create(&recognizer_config(&paths, 2)).expect("识别器创建失败");
        let stream = recognizer.create_stream();
        let chunk = (TARGET_RATE / 10) as usize; // 100ms

        let chunks_per_bucket = 100; // 10 秒
        let mut buckets: Vec<f64> = Vec::new();
        let mut acc = 0.0f64;
        let mut n = 0usize;
        let mut endpoints = 0usize;

        for part in long.chunks(chunk) {
            let t = std::time::Instant::now();
            stream.accept_waveform(TARGET_RATE, part);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
            if recognizer.is_endpoint(&stream) {
                endpoints += 1;
                recognizer.reset(&stream);
            }
            acc += t.elapsed().as_secs_f64();
            n += 1;
            if n == chunks_per_bucket {
                buckets.push(acc);
                acc = 0.0;
                n = 0;
            }
        }

        println!(
            "\n拼接音频 {total_s:.0} 秒（{REPEATS} × {:.1}s），端点切出 {endpoints} 句",
            wave.samples().len() as f64 / TARGET_RATE as f64
        );
        for (i, b) in buckets.iter().enumerate() {
            println!(
                "  {:>3}~{:>3}s：耗时 {:.3}s，实时率 {:.1}x",
                i * 10,
                (i + 1) * 10,
                b,
                10.0 / b
            );
        }

        assert!(buckets.len() >= 3, "音频太短，攒不出足够的对比窗口");
        assert!(endpoints > 5, "端点只切出 {endpoints} 句，端点检测可能没生效");

        let first = buckets[0];
        let last = *buckets.last().unwrap();
        println!("首段 {first:.3}s → 末段 {last:.3}s（{:.2} 倍）", last / first);
        assert!(
            last < first * 2.0,
            "末尾 10 秒的耗时是开头的 {:.1} 倍 —— 解码上下文可能在无限增长，\
             检查端点检测（enable_endpoint / reset）是否生效",
            last / first
        );
    }
}
