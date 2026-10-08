@echo off
REM ---------------------------------------------------------------
REM  Prompter 发布构建（产出单个 exe，不打安装包）
REM
REM  为什么用 --no-bundle：
REM    打 NSIS 安装包需要额外下载一套工具链，在国内网络下经常卡住，
REM    而多数人只是想要一个能双击运行的 exe。需要安装包时再执行
REM    npm run tauri build（不带 --no-bundle）即可。
REM ---------------------------------------------------------------
setlocal
cd /d "%~dp0"

set "NODE_HOME=C:\Users\26611\.workbuddy\binaries\node\versions\22.22.2-3"
if exist "%NODE_HOME%\npm.cmd" set "PATH=%NODE_HOME%;%PATH%"

set "NODE_OPTIONS="

where npm >nul 2>nul
if errorlevel 1 (
  echo [错误] 找不到 npm。请安装 Node.js 22 或以上版本。
  pause
  exit /b 1
)

if not exist "node_modules" (
  echo 正在安装前端依赖...
  call npm install --no-audit --no-fund || goto :fail
)

echo.
echo 构建中（release 编译开了 LTO，会比较慢，请耐心等）...
echo.
call npm run tauri:build || goto :fail

echo.
echo 完成。产物位置：
echo   src-tauri\target\release\prompter.exe
echo.
echo 直接双击即可运行。分发时把这个 exe 拷给别人就行。
pause
goto :eof

:fail
echo.
echo [错误] 构建失败，请查看上面的输出。
pause
exit /b 1
