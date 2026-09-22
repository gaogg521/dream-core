@echo off
REM ============================================================
REM  ComfyUI 启动脚本（Windows）
REM  使用前修改：
REM    1) COMFYUI_DIR 改成你的 ComfyUI 目录（路径含空格必须加引号）
REM    2) VRAM_MODE 按显存选一个：LOW / MED / NORMAL / HIGH
REM ============================================================

set "COMFYUI_DIR=D:\ComfyUI"
set "VRAM_MODE=MED"
set "PORT=8188"

cd /d "%COMFYUI_DIR%" || (echo [ERROR] 目录不存在: %COMFYUI_DIR% & pause & exit /b 1)

REM 激活虚拟环境（整合包请改用 python_embeded\python.exe）
call "%COMFYUI_DIR%\venv\Scripts\activate.bat" || (echo [ERROR] venv 未找到 & pause & exit /b 1)

REM ---- 显存档位 ----
if /i "%VRAM_MODE%"=="LOW"    set "VRAM_FLAG=--lowvram"
if /i "%VRAM_MODE%"=="MED"    set "VRAM_FLAG=--medvram"
if /i "%VRAM_MODE%"=="NORMAL" set "VRAM_FLAG=--normalvram"
if /i "%VRAM_MODE%"=="HIGH"   set "VRAM_FLAG=--highvram"

echo [INFO] ComfyUI 目录 : %COMFYUI_DIR%
echo [INFO] 显存模式     : %VRAM_MODE%
echo [INFO] 端口         : %PORT%
echo.

python main.py --port %PORT% %VRAM_FLAG% --bf16-vae

echo.
echo [INFO] ComfyUI 已退出。浏览器地址: http://127.0.0.1:%PORT%
pause
