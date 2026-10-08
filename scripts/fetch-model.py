#!/usr/bin/env python3
"""下载 sherpa-onnx 中文流式识别模型。

用法：
    python scripts/fetch-model.py                  # 用 hf-mirror 镜像（国内推荐）
    python scripts/fetch-model.py --mirror hf      # 用 huggingface 官方
    python scripts/fetch-model.py --dest D:\\models # 自定义目录

为什么用 Python 脚本而不是让程序自己下载：
    模型约 300MB。放在安装包里会让安装包膨胀十倍，而且多数用户一辈子
    只用一次下载。做成一个独立脚本，装完程序想用语音时再跑一次就行；
    跑不动脚本的人，程序本身用「手动模式」也是完整可用的。

为什么默认走 hf-mirror：
    huggingface.co 在国内经常连不上或速度只有几十 KB/s，
    而这个模型是分片文件下载，中断一次就得重来。hf-mirror 是完整镜像，
    文件名与校验值一致。
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import time
import urllib.error
import urllib.request

# 模型仓库与文件清单。
# 优先下载 int8 量化版本：体积约为 float32 的三分之一，推理更快，
# 在"跟读定位"这个任务上精度损失可以忽略（我们只需要识别对，不需要标点完美）。
REPO = "csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20"
MODEL_DIR_NAME = "sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20"

FILES = [
    "encoder-epoch-99-avg-1.int8.onnx",
    "decoder-epoch-99-avg-1.onnx",
    "joiner-epoch-99-avg-1.int8.onnx",
    "tokens.txt",
]

MIRRORS = {
    "hf-mirror": "https://hf-mirror.com",
    "hf": "https://huggingface.co",
}

# 应用标识，需与 tauri.conf.json 里的 identifier 一致
APP_ID = "cn.bobby.prompter"


def default_dest() -> str:
    """Tauri 的 app_data_dir() 在 Windows/macOS 上是应用支持目录下的应用 ID。"""
    if sys.platform == "win32":
        base = os.environ.get("APPDATA") or os.path.expanduser("~")
        return os.path.join(base, APP_ID, "models")
    if sys.platform == "darwin":
        return os.path.expanduser(f"~/Library/Application Support/{APP_ID}/models")
    base = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    return os.path.join(base, APP_ID, "models")


def human(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.1f}{unit}" if unit != "B" else f"{int(n)}B"
        n /= 1024
    return f"{n:.1f}GB"


def download(url: str, dest: str, label: str) -> None:
    """带断点续传的单文件下载。

    断点续传不是锦上添花：这个模型是几百 MB 的分片文件，国内连接中途被 reset
    很常见。没有续传的话，用户会反复卡在同一个位置。
    """
    tmp = dest + ".part"
    existing = os.path.getsize(tmp) if os.path.exists(tmp) else 0

    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    if existing:
        req.add_header("Range", f"bytes={existing}-")

    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            total = resp.headers.get("Content-Length")
            total = int(total) + existing if total else 0
            mode = "ab" if existing else "wb"
            done = existing
            start = time.time()
            last = 0.0

            with open(tmp, mode) as f:
                while True:
                    chunk = resp.read(256 * 1024)
                    if not chunk:
                        break
                    f.write(chunk)
                    done += len(chunk)

                    now = time.time()
                    if now - last > 0.25:
                        last = now
                        speed = (done - existing) / max(now - start, 0.001)
                        pct = f"{done / total * 100:5.1f}%" if total else "  ..."
                        bar = ""
                        if total:
                            filled = int(done / total * 24)
                            bar = "[" + "#" * filled + "-" * (24 - filled) + "]"
                        sys.stdout.write(
                            f"\r  {label:<40} {bar} {pct} {human(done)} "
                            f"@ {human(speed)}/s   "
                        )
                        sys.stdout.flush()
    except urllib.error.HTTPError as e:
        if e.code == 416:  # 已经下完了
            pass
        else:
            raise
    except Exception:
        raise

    if os.path.exists(tmp):
        os.replace(tmp, dest)
    sys.stdout.write("\n")
    sys.stdout.flush()


def main() -> int:
    ap = argparse.ArgumentParser(description="下载中文流式识别模型")
    ap.add_argument("--mirror", choices=list(MIRRORS), default="hf-mirror",
                    help="下载源（默认 hf-mirror，国内推荐）")
    ap.add_argument("--dest", default=None, help="模型根目录")
    ap.add_argument("--force", action="store_true", help="已存在也重新下载")
    args = ap.parse_args()

    dest_root = args.dest or default_dest()
    model_dir = os.path.join(dest_root, MODEL_DIR_NAME)
    os.makedirs(model_dir, exist_ok=True)

    base = MIRRORS[args.mirror]
    print(f"下载源   : {base}")
    print(f"目标目录 : {model_dir}")
    print(f"文件数   : {len(FILES)}")
    print()

    failed = []
    for name in FILES:
        dest = os.path.join(model_dir, name)
        if os.path.exists(dest) and not args.force:
            print(f"  已存在，跳过：{name}（{human(os.path.getsize(dest))}）")
            continue

        url = f"{base}/{REPO}/resolve/main/{name}"
        try:
            download(url, dest, name)
        except Exception as e:
            failed.append((name, str(e)))
            print(f"  失败：{name} — {e}")

    print()
    if failed:
        print("以下文件未能下载完成：")
        for n, e in failed:
            print(f"  - {n}: {e}")
        print("\n可以直接重跑本脚本，已下载的部分会续传。")
        print("若一直失败，试试换源：python scripts/fetch-model.py --mirror hf")
        return 1

    total = sum(os.path.getsize(os.path.join(model_dir, f)) for f in FILES)
    print(f"完成，共 {human(total)}。")
    print()
    print("下一步：打开 Prompter → 设置 → 语音识别 → 模型选这个目录，" )
    print("        然后点「开始识别」。首次加载模型需要几秒。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
