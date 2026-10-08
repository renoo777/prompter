//! 麦克风探测：主动回答「这个设备到底有没有在送声音」。
//!
//! # 为什么需要这一层
//!
//! 起因是一次真实故障：一台笔记本的麦克风阵列在系统里显示 `ACTIVE`、
//! 未静音、音量 82.5%，但驱动送出来的全是恒定近零值（-113 dBFS）。
//! 现象是「点了开始识别，光标一动不动」——
//! 而代码里**每一步都成功**：流打开了、回调按额定速率送数据、解码器正常输出（空串）。
//!
//! 这是最糟的一类失败：**静默失败**。用户手里没有任何线索，
//! 只会得出「这软件是坏的」。
//!
//! 所以这里把这件事做成能被问出来的能力：
//!
//! * 实时链路上有看门狗（见 `local::run_worker`），持续数字静音就报警告；
//! * 设置页有「麦克风自检」按钮，开会前 5 秒点一下就能确认链路是活的。
//!
//! # 判据为什么可信
//!
//! 真实麦克风即便在深夜安静的房间里，本底噪声也在 -60 ~ -70 dBFS 量级。
//! 而故障设备输出 -113 dBFS。两者差着七个数量级，中间留的余量足够大，
//! 「房间安静」和「拿不到声音」不会混。判据本身就是本模块的
//! `DIGITAL_SILENCE_PEAK` 与 `classify_mic_peak`。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::local::build_input_stream;
use super::{MicProbe, MicVerdict};

/// 默认探测时长。够覆盖「说一句话」，又不至于让用户等。
pub const DEFAULT_PROBE_SECONDS: f64 = 2.5;

// ---------------------------------------------------------------------------
// 电平判定
// ---------------------------------------------------------------------------
//
// 判据集中在这里，是因为**探测命令和实时看门狗必须用同一套标准**。
// 一旦两处各写一份阈值，就会出现最坑的一种故障：
// 「自检说正常，实际不跟随」—— 用户拿着"正常"的结论到处找原因。
// 端到端测试 `watchdog_sees_dead_microphone` 专门断言这两条链路结论一致。

/// 低于这个峰值就认定是**数字静音**（-80 dBFS）。
///
/// # 这个数是怎么定出来的
///
/// 判据的本质是「绝对电平」，所以只能靠区分两种物理来源来定：
///
/// * **死设备**输出的不是精确的 0，而是围绕某个常数的**量化噪声** ——
///   幅度被量化位数卡在若干 LSB 以内（16bit 时 1 LSB ≈ -90 dBFS，
///   几个 LSB 就到 -83 dBFS）。本机故障设备实测跨多次测量在
///   **-138 ~ -83 dBFS** 之间浮动，量级差 55dB。这个巨大的浮动本身
///   就是「不是声学信号」的证据：真实声音在同一个房间里不会这么跳。
/// * **真实麦克风**即便在深夜的安静房间里，声学底噪也在
///   **-60 ~ -70 dBFS** 量级。
///
/// 阈值取 -80 dBFS：往上给真实声学底噪留 10dB 余量，
/// 往下覆盖到量化噪声的上沿。
///
/// ⚠️ 这个值**不要再往上调**。调到 -70 dBFS 附近就开始吃掉真实房间的
/// 声学底噪 —— 表现是「每次开会刚打开程序就弹一条警告，让人去关一个
/// 本来就没开的静音键」。边界由
/// `threshold_sits_between_quantization_floor_and_acoustic_floor` 钉住。
pub const DIGITAL_SILENCE_PEAK: f32 = 1e-4;

/// 峰值低于这个值认为「有信号但偏弱」（-40 dBFS）。
///
/// 正常对着麦克风说话的峰值一般在 -20 ~ -6 dBFS。落在 -80 ~ -40 dBFS
/// 之间通常是「离麦克风太远」「系统增益被压低」「降噪把声音压掉了」。
pub const WEAK_PEAK: f32 = 0.01;

/// 把 0..1 的幅度换算成 dBFS（满量程 1.0 记 0 dB）。
///
/// 只用于展示 —— 数字比「0.000034」直观得多，用户看到 -89dBFS
/// 就知道这不正常，看到 -18dBFS 就知道正常。
pub fn to_dbfs(amplitude: f32) -> f32 {
    20.0 * amplitude.max(1e-12).log10()
}

/// 按峰值判定档位。
pub fn classify_mic_peak(peak: f32) -> MicVerdict {
    if peak < DIGITAL_SILENCE_PEAK {
        MicVerdict::Silent
    } else if peak < WEAK_PEAK {
        MicVerdict::Weak
    } else {
        MicVerdict::Ok
    }
}

impl MicVerdict {
    /// 给用户看的一句话结论。**必须带上「怎么办」** ——
    /// 只说「静音」而不说去关哪个开关，等于没帮上忙。
    pub fn detail(self) -> &'static str {
        match self {
            MicVerdict::Ok => "拾音正常，识别应该能跟上。",
            MicVerdict::Weak => {
                "有信号但很弱。如果刚才没说话，这是正常的；\
                 如果说了话还是这个电平，检查麦克风距离、系统输入增益，\
                 以及是否开了过强的降噪。"
            }
            MicVerdict::Silent => {
                "采到的是**数字静音** —— 驱动送来的就是恒定的近零值，\
                 这不是「房间安静」（真实麦克风总有本底噪声，会高 40dB 以上）。\n\
                 常见原因，按可能性排序：\n\
                 ① 笔记本键盘上的麦克风静音键被按下（这类静音在系统里查不到，\
                    端点仍显示 ACTIVE、静音状态为否）；\n\
                 ② 声音设置 → 输入 → 该设备被静音或音量被拉到最低；\n\
                 ③ 隐私设置 → 麦克风 → 允许桌面应用访问，被关掉了。"
            }
        }
    }
}

/// 探测时长下限 / 上限，防止调用方传入离谱的值把界面卡住。
const MIN_SECONDS: f64 = 0.5;
const MAX_SECONDS: f64 = 15.0;

/// 列出系统里所有输入设备（名字 + 默认格式）。
///
/// 单独列出来是因为排查时最常见的一句话是「你是不是还有别的麦克风」——
/// 有清单就能当场回答。
fn list_input_devices(host: &cpal::Host) -> Vec<String> {
    let Ok(devices) = host.input_devices() else {
        return vec!["（枚举输入设备失败）".to_string()];
    };

    let mut out = Vec::new();
    for d in devices {
        let name = d.name().unwrap_or_else(|_| "（读不到名字）".to_string());
        let cfg = match d.default_input_config() {
            Ok(c) => format!(
                "{}Hz / {}声道 / {:?}",
                c.sample_rate().0,
                c.channels(),
                c.sample_format()
            ),
            Err(e) => format!("配置读不到：{e}"),
        };
        out.push(format!("{name}   [{cfg}]"));
    }
    if out.is_empty() {
        out.push("（系统里没有任何输入设备）".to_string());
    }
    out
}

/// 录一小段并判定拾音是否正常。
///
/// 走的是**和实时识别完全相同的采集链路**（同一个 `build_input_stream`），
/// 所以这里说正常，实时链路就没理由不正常。
///
/// 注意：这个函数会**阻塞** `seconds` 秒。调用方要么放在命令里由前端
/// 异步等待（本项目的做法），要么放进后台线程。
pub fn probe_microphone(seconds: f64) -> Result<MicProbe, String> {
    let seconds = seconds.clamp(MIN_SECONDS, MAX_SECONDS);

    let host = cpal::default_host();
    let devices = list_input_devices(&host);

    let device = host
        .default_input_device()
        .ok_or_else(|| "系统里没有可用的麦克风设备。".to_string())?;
    let device_name = device.name().unwrap_or_else(|_| "未知设备".to_string());

    let supported = device.default_input_config().map_err(|e| {
        format!(
            "无法读取麦克风（{device_name}）的默认配置：{e}\n\
             若在 Windows 上，请到「设置 → 隐私和安全性 → 麦克风」\
             允许桌面应用访问。"
        )
    })?;

    let in_rate = supported.sample_rate().0 as i32;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.into();

    // 缓冲上限按「探测时长 + 一点余量」给，避免长时间探测时被裁剪。
    let buffer: Arc<Mutex<Vec<f32>>> =
        Arc::new(Mutex::new(Vec::with_capacity((in_rate as f64 * seconds) as usize + in_rate as usize)));

    let stream = build_input_stream(
        &device,
        &stream_config,
        sample_format,
        channels,
        in_rate,
        buffer.clone(),
    )?;
    stream
        .play()
        .map_err(|e| format!("无法启动麦克风采集：{e}"))?;

    let started = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(seconds));
    let elapsed = started.elapsed().as_secs_f64();

    let samples = {
        let mut b = buffer
            .lock()
            .map_err(|_| "音频缓冲锁中毒（回调线程出问题了）".to_string())?;
        std::mem::take(&mut *b)
    };
    drop(stream); // 显式关麦，别一直占着

    let frames = samples.len();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = if frames == 0 {
        0.0
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / frames as f32).sqrt()
    };

    let verdict = classify_mic_peak(peak);

    // 一帧都没来，是比「静音」更前面的故障：设备没启动 / 回调没被调用。
    // 单独说清楚，否则用户会按照「静音」的建议去关静音键，白忙一场。
    let detail = if frames == 0 {
        "一帧音频都没采到 —— 这不是静音，是设备根本没启动。\n\
         检查：① 设备是否被别的程序独占（某些会议软件会独占麦克风）；\
         ② 设备是否刚被拔出/禁用；③ 驱动是否异常。"
            .to_string()
    } else {
        verdict.detail().to_string()
    };

    log::info!(
        "[mic] 探测 {}：{} 帧，峰值 {:.6}（{:.1} dBFS），RMS {:.6}，判定 {:?}",
        device_name,
        frames,
        peak,
        to_dbfs(peak),
        rms,
        verdict
    );

    Ok(MicProbe {
        device: device_name,
        sample_rate: in_rate,
        channels,
        format: format!("{sample_format:?}"),
        seconds: elapsed,
        frames,
        peak,
        rms,
        verdict,
        detail,
        devices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 静音阈值必须能分开「真静音」和「深夜的安静房间」。
    ///
    /// 存在的意义是防止有人觉得阈值太保守而把它调大 ——
    /// 一旦调到 -70dBFS 附近，安静房间里的正常麦克风就会被判成静音，
    /// 用户会跑去关一个本来就没开的静音键，白折腾一圈。
    #[test]
    fn separates_digital_silence_from_quiet_room() {
        // 本机故障设备的实测值：跨多次测量在 -138 ~ -83 dBFS 之间浮动。
        // 这些值必须**全部**判成静音 —— 阈值定在 -100dBFS 时，
        // 同一个设备在两次测量里一次判静音、一次判"有信号"。
        assert_eq!(classify_mic_peak(0.0), MicVerdict::Silent);
        assert_eq!(classify_mic_peak(1.26e-7), MicVerdict::Silent); // -138 dBFS
        assert_eq!(classify_mic_peak(6.1e-5), MicVerdict::Silent); // -84 dBFS
        assert_eq!(classify_mic_peak(7.49e-5), MicVerdict::Silent); // -83 dBFS
        assert_eq!(classify_mic_peak(2.9e-5), MicVerdict::Silent); //  -91 dBFS
        // 安静房间的本底噪声约 -60 dBFS，必须判成「有信号」
        assert_ne!(classify_mic_peak(1e-3), MicVerdict::Silent);
        // 正常说话
        assert_eq!(classify_mic_peak(0.1), MicVerdict::Ok);
    }

    /// 把阈值本身钉在「量化底噪」和「真实声学底噪」之间。
    ///
    /// 这条不测行为，测的是**这个数本身合不合理** ——
    /// 它是唯一能拦住"顺手把阈值调大一点"的地方。
    #[test]
    fn threshold_sits_between_quantization_floor_and_acoustic_floor() {
        let db = to_dbfs(DIGITAL_SILENCE_PEAK);
        assert!(
            db > -90.0,
            "阈值 {db:.0} dBFS 低于 16bit 的量化底噪（-90dBFS），死设备会漏判"
        );
        assert!(
            db < -70.0,
            "阈值 {db:.0} dBFS 高过真实房间的声学底噪（-70dBFS），会误报"
        );
    }

    #[test]
    fn weak_band_sits_between_silence_and_normal() {
        // 判据是严格小于，所以刚好等于阈值时不算静音
        assert_eq!(classify_mic_peak(DIGITAL_SILENCE_PEAK), MicVerdict::Weak);
        assert_eq!(classify_mic_peak(1e-3), MicVerdict::Weak);
        assert_eq!(classify_mic_peak(9e-3), MicVerdict::Weak);
        assert_eq!(classify_mic_peak(WEAK_PEAK), MicVerdict::Ok);
    }

    /// dBFS 换算要合行业习惯：满量程 = 0dB，每减半约 -6dB。
    #[test]
    fn dbfs_matches_industry_convention() {
        assert!((to_dbfs(1.0) - 0.0).abs() < 0.01);
        assert!((to_dbfs(0.5) + 6.02).abs() < 0.05);
        assert!((to_dbfs(1e-3) + 60.0).abs() < 0.05);
        // 零不能算出 -inf，否则界面上会出现「inf dBFS」这种东西
        assert!(to_dbfs(0.0).is_finite());
    }

    /// 每个档位都必须说清「怎么办」。只说结论不给出路，等于没帮上忙。
    #[test]
    fn every_verdict_explains_what_to_do() {
        for v in [MicVerdict::Ok, MicVerdict::Weak, MicVerdict::Silent] {
            assert!(!v.detail().trim().is_empty());
        }
        assert!(
            MicVerdict::Silent.detail().contains("静音键"),
            "静音档必须点出最常见的原因，否则用户会先去查错方向"
        );
    }
}
