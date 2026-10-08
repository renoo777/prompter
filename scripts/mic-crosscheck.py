# -*- coding: utf-8 -*-
"""
独立交叉验证：用 PortAudio（sounddevice）录同一支麦克风。

目的：判断「采到 -113dBFS 有效静音」是我们的 cpal 链路问题，
还是系统/硬件层面的问题。

如果独立实现也拿到静音 -> 系统层问题（硬件静音键 / 驱动 / 阵列被关）。
如果独立实现拿到正常底噪 -> 我们的采集链路有问题，回去查代码。
"""

import sys

import numpy as np
import sounddevice as sd

SECONDS = 3.0

# 按「同一支麦克风、不同后端」各测一次：
#   9  = 麦克风阵列 / Windows WASAPI   （和我们 cpal 用的是同一个后端）
#   5  = 麦克风阵列 / Windows DirectSound
#   1  = 麦克风阵列 / MME
TARGETS = [
    (9, "麦克风阵列 / WASAPI  ← 与我们的 cpal 同后端"),
    (5, "麦克风阵列 / DirectSound"),
    (1, "麦克风阵列 / MME"),
]


def measure(dev_index: int, label: str) -> None:
    info = sd.query_devices(dev_index)
    rate = int(info["default_samplerate"])
    chans = int(info["max_input_channels"])

    print(f"\n{'=' * 64}")
    print(f"设备 #{dev_index}  {info['name']}")
    print(f"  {label}")
    print(f"  默认采样率 = {rate}Hz   可用输入声道 = {chans}")
    print(f"  录制 {SECONDS:.0f} 秒，请正常说话（说「测试一二三」）…")

    try:
        rec = sd.rec(
            int(SECONDS * rate),
            samplerate=rate,
            channels=min(chans, 4),
            dtype="float32",
            device=dev_index,
            blocking=True,
        )
    except Exception as e:  # noqa: BLE001
        print(f"  [失败] 打不开设备：{type(e).__name__}: {e}")
        return

    peak = float(np.max(np.abs(rec))) if rec.size else 0.0
    rms_all = float(np.sqrt(np.mean(rec ** 2))) if rec.size else 0.0

    print(f"  frames = {rec.shape[0]}  channels = {rec.shape[1]}")
    print(f"  峰值   = {peak:.6f}   ({20 * np.log10(max(peak, 1e-12)):.1f} dBFS)")
    print(f"  总 RMS = {rms_all:.6f}   ({20 * np.log10(max(rms_all, 1e-12)):.1f} dBFS)")

    for c in range(rec.shape[1]):
        ch = rec[:, c]
        cpeak = float(np.max(np.abs(ch))) if ch.size else 0.0
        crms = float(np.sqrt(np.mean(ch ** 2))) if ch.size else 0.0
        print(f"    ch{c}: peak = {cpeak:.6f}   RMS = {crms:.6f}")

    if peak < 1e-4:
        verdict = "有效静音 —— 与我们的 cpal 结果一致"
    elif peak < 1e-2:
        verdict = "有信号但电平很低（可能只是在轻声/远离麦克风）"
    else:
        verdict = "有正常信号 —— 说明设备本身有声音，问题在我们的链路"
    print(f"  [判定] {verdict}")


def main() -> int:
    print("PortAudio 版本:", sd.get_portaudio_version()[1])
    print("默认输入设备:", sd.default.device[0])

    for idx, label in TARGETS:
        measure(idx, label)

    print(f"\n{'=' * 64}")
    print("说明：三个后端里只要有一个拿到正常电平，就说明问题可按后端规避；")
    print("      三个都是静音，则结论是「系统层拿不到声音」，与我们代码无关。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
