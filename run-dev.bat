@echo off
REM ---------------------------------------------------------------
REM  Prompter 开发模式启动（改前端代码即时热更新）
REM ---------------------------------------------------------------
setlocal
cd /d "%~dp0"

REM WorkBuddy 管理的 Node 不在系统 PATH 里，这里补上。
REM 如果你自己全局装过 Node 22+，可以删掉下面两行。
set "NODE_HOME=C:\Users\26611\.workbuddy\binaries\node\versions\22.22.2-3"
if exist "%NODE_HOME%\npm.cmd" set "PATH=%NODE_HOME%;%PATH%"

REM WorkBuddy 会向 Node 注入 NODE_OPTIONS（指向一个 safe-delete 垫片），
REM 它会让 npm CLI 直接崩溃。必须清掉。
set "NODE_OPTIONS="

where npm >nul 2>nul
if errorlevel 1 (
  echo [错误] 找不到 npm。请安装 Node.js 22 或以上版本。
  pause
  exit /b 1
)

if not exist "node_modules" (
  echo 首次运行，正在安装前端依赖...
  call npm install --no-audit --no-fund || goto :fail
)

echo.
echo 启动 Prompter（首次编译 Rust 依赖需要几分钟）...
echo.
call npm run tauri dev
goto :eof

:fail
echo.
echo [错误] 依赖安装失败。
pause
exit /b 1
