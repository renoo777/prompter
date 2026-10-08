#!/bin/bash
# ---------------------------------------------------------------
#  Prompter 开发模式启动（改前端代码即时热更新，macOS 版）
#
#  与 run-dev.bat 的差别：只去掉 Windows 特有的那两行
#  （写死的本机 Node 路径、NODE_OPTIONS 垫片）——那两个是 WorkBuddy
#  环境的问题，在你自己的 Mac 上不适用。
# ---------------------------------------------------------------
set -euo pipefail
cd "$(dirname "$0")"

# WorkBuddy 会向 Node 注入 NODE_OPTIONS（指向一个 safe-delete 垫片），
# 它会让 npm CLI 直接崩溃。在 WorkBuddy 里跑时必须清掉。
unset NODE_OPTIONS || true

if ! command -v npm >/dev/null 2>&1; then
  echo "[错误] 找不到 npm。请安装 Node.js 22 或以上版本。"
  echo "       macOS 可用 nvm 或 https://nodejs.org 官方安装包。"
  exit 1
fi

if [ ! -d node_modules ]; then
  echo "首次运行，正在安装前端依赖..."
  npm install --no-audit --no-fund
fi

echo
echo "启动 Prompter（首次编译 Rust 依赖需要几分钟）..."
echo "★ 开发模式连的是 localhost:5273，所以**不能**双击产物 .app 来跑 ——"
echo "  要跑打包产物请用 ./build.sh。"
echo
npm run tauri dev
