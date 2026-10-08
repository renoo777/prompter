#!/bin/bash
# ---------------------------------------------------------------
#  Prompter 发布构建（macOS，产出 .app）
#
#  与 build.bat 的差别（别照抄 Windows 那份，三处不同）：
#    1. 产物是 .app，不是 exe
#    2. 必须打 .app（--no-bundle 在这里会让命令成功但什么都不产出）
#    3. 不写死任何本机 Node 路径
#
#  用法：
#    ./build.sh          打 .app
#    ./build.sh --dmg    额外打 dmg 安装包
# ---------------------------------------------------------------
set -euo pipefail
cd "$(dirname "$0")"

# ---- 前置检查：把"编不过"和"行为不对"两类问题分开 -------------
if ! command -v npm >/dev/null 2>&1; then
  echo "[错误] 找不到 npm。请安装 Node.js 22 或以上版本。"
  echo "       macOS 可用 nvm 或 https://nodejs.org 官方安装包。"
  exit 1
fi

if ! command -v xcode-select >/dev/null 2>&1 || ! xcode-select -p >/dev/null 2>&1; then
  echo "[错误] 缺 Xcode 命令行工具。本项目要链接 ObjC 框架，没有它编不过。"
  echo "       装法：xcode-select --install"
  exit 1
fi

if [ ! -d node_modules ]; then
  echo "正在安装前端依赖..."
  npm install --no-audit --no-fund
fi

# ---- 1. 先只做编译检查 -------------------------------------------
echo
echo "[1/3] 编译检查（cargo check）..."
( cd src-tauri && cargo check --lib )

# ---- 2. 跑单测 ---------------------------------------------------
echo
echo "[2/3] 单元测试（cargo test --lib）..."
( cd src-tauri && cargo test --lib )

# ---- 3. 打包 -----------------------------------------------------
# ★ 关键：这里**必须**直接调 CLI，不能写 `npm run tauri:build -- --bundles app`
#   —— package.json 里的 tauri:build 带了 --no-bundle，会把它静默压掉：
#      命令成功、只出裸可执行文件、没有 .app、且**不报错**。
echo
if [ "${1:-}" = "--dmg" ]; then
  echo "[3/3] 打包 .app 与 .dmg（dmg 需要 Apple 签名，未签名也能出）..."
  npx --no-install tauri build --bundles app,dmg
else
  echo "[3/3] 打包 .app..."
  npx --no-install tauri build --bundles app
fi

echo
echo "完成。产物位置："
echo "  src-tauri/target/release/bundle/macos/Prompter.app"
echo
echo "首次打开会提示「来自身份不明的开发者」——本项目用了未文档化的私有"
echo "API（macOS 15+ 已无公开 API 可阻止屏幕捕获），无法通过签名与公证。"
echo "在「系统设置 → 隐私与安全性」里点「仍要打开」，或执行："
echo "  xattr -d com.apple.quarantine \"src-tauri/target/release/bundle/macos/Prompter.app\""
echo
echo "★ 装不上的话看 docs/macOS上机测试清单.md，那里有完整排错步骤。"
