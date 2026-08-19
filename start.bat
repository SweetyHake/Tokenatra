@echo off
chcp 65001 > nul
cd /d "%~dp0"
title Tokenatra - Starting...

rem Rebuild the packaged executable on every launch so it always contains
rem the current source files.
set "PACKAGED_APP=%~dp0dist\Tokenatra\Tokenatra.exe"
taskkill /F /IM Tokenatra.exe >nul 2>&1

where python >nul 2>&1
if errorlevel 1 (
    echo ERROR: Python was not found.
    echo Install Python from python.org
    goto :error
)

echo [1/5] Stopping the previous instance...
taskkill /F /IM pythonw.exe /FI "WINDOWTITLE eq app.py" >nul 2>&1
powershell -Command "Get-Process pythonw,python -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -match 'app\\.py' } | Stop-Process -Force" >nul 2>&1

echo [2/5] Freeing port 7878...
for /f "tokens=5" %%a in ('netstat -ano ^| find ":7878" ^| find "LISTENING"') do taskkill /F /PID %%a >nul 2>&1

echo [3/5] Checking dependencies...
python -c "import onnxruntime, imageio_ffmpeg" >nul 2>&1
if errorlevel 1 (
    echo   Installing dependencies...
    python -m pip install onnxruntime-directml numpy Pillow flask pywebview psutil imageio-ffmpeg
    if errorlevel 1 (
        echo   ERROR: Could not install dependencies.
        echo   Try this manually in the command prompt:
        echo   pip install onnxruntime-directml numpy Pillow flask pywebview psutil imageio-ffmpeg
        goto :error
    )
)

echo [4/5] Checking PyInstaller...
python -m PyInstaller --version >nul 2>&1
if errorlevel 1 (
    echo   Installing PyInstaller...
    python -m pip install pyinstaller
    if errorlevel 1 (
        echo   ERROR: Could not install PyInstaller.
        goto :error
    )
)

echo [5/5] Building and starting...
python -m PyInstaller build.spec --noconfirm
if errorlevel 1 (
    echo ERROR: Could not build the application.
    goto :error
)
if not exist "%PACKAGED_APP%" (
    echo ERROR: Built Tokenatra.exe was not found.
    goto :error
)

set "TOKENMAKER_DIR=%~dp0"
start "" "%PACKAGED_APP%"
echo Tokenatra started.
timeout /t 2 >nul
exit /b 0

:error
echo.
echo =============================================
echo  AN ERROR OCCURRED - this window will remain open.
echo  Copy the text above and send it to the developer.
echo =============================================
pause
exit /b 1
