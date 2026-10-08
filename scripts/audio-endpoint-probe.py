"""枚举 Windows 音频采集端点，读出「静音 / 音量」——判定麦克风到底是不是被静音了。

用 pycaw（Core Audio 的 COM 封装）。写成独立文件而不是 `python -c`，
是因为要输出的文本里有反斜杠与引号，走一行命令容易被 shell 吃掉。
"""

import sys

from pycaw.utils import AudioUtilities


STATE_NAME = {
    1: "ACTIVE",
    2: "DISABLED",
    4: "NOT_PRESENT",
    8: "UNPLUGGED",
    0x10000000: "FLAG_0x10000000",
}


def describe(dev, indent="  "):
    raw_state = getattr(dev, "state", None)
    # pycaw 返回的是 AudioDeviceState 枚举，不能直接 int()，得先取 .value
    if hasattr(raw_state, "value"):
        state = int(raw_state.value)
    elif raw_state is not None:
        state = int(raw_state)
    else:
        state = -1

    lines = []
    lines.append(f"{indent}{dev.FriendlyName}")
    lines.append(f"{indent}  state    = {state} ({STATE_NAME.get(state, '?')})")
    lines.append(f"{indent}  id       = {dev.id}")
    try:
        vol = dev.EndpointVolume
        muted = bool(vol.GetMute())
        scalar = vol.GetMasterVolumeLevelScalar()
        db = vol.GetMasterVolumeLevel()
        chans = vol.GetChannelCount()
        lines.append(f"{indent}  muted    = {muted}   <-- 关键")
        lines.append(f"{indent}  volume   = {scalar * 100:.1f}%   ({db:.2f} dB)")
        lines.append(f"{indent}  channels = {chans}")
    except Exception as exc:  # noqa: BLE001
        lines.append(f"{indent}  (读端点音量失败：{type(exc).__name__}: {exc})")
    return lines


def main():
    out = []
    out.append("=== 全部采集端点（data_flow = eCapture）===")
    try:
        captures = AudioUtilities.GetAllDevices(data_flow=1, device_state=15)
    except Exception as exc:  # noqa: BLE001
        out.append(f"  枚举失败：{type(exc).__name__}: {exc}")
        captures = []

    if not captures:
        out.append("  (没有枚举到任何采集端点)")
    for dev in captures:
        out.extend(describe(dev))

    out.append("")
    out.append("=== 所有角色（Console / Multimedia / Communications）的默认采集设备 ===")
    for label, getter in (
        ("GetMicrophone (Communications)", getattr(AudioUtilities, "GetMicrophone", None)),
    ):
        if getter is None:
            out.append(f"  {label}: 不可用")
            continue
        try:
            dev = getter()
            out.append(f"  {label}:")
            out.extend(describe(dev, indent="    "))
        except Exception as exc:  # noqa: BLE001
            out.append(f"  {label}: 失败 {type(exc).__name__}: {exc}")

    text = "\n".join(out)
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
