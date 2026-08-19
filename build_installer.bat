@echo off
cd /d "%~dp0"

echo ========================================
echo   Tokenatra - build installer
echo ========================================
echo   Step 1: Build .exe with PyInstaller
echo ========================================
echo.

where python >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Python not found
    pause
    exit /b 1
)

pip install pyinstaller -q >nul 2>&1

echo Building .exe...
python -m PyInstaller build.spec --noconfirm
if errorlevel 1 (
    echo [ERROR] PyInstaller build failed
    pause
    exit /b 1
)

echo Copying extra folders...
xcopy /E /I /Y "token_rings" "dist\Tokenatra\token_rings" > nul
xcopy /E /I /Y "presets" "dist\Tokenatra\presets" > nul

echo Copying icon and images next to exe...
xcopy /Y "icon.ico" "dist\Tokenatra\" > nul
xcopy /Y "mask.png" "dist\Tokenatra\" > nul
xcopy /Y "logo.png" "dist\Tokenatra\" > nul
xcopy /Y "example.png" "dist\Tokenatra\" > nul

echo.
echo ========================================
echo   Step 2: Build installer with Inno Setup
echo ========================================
echo.

if not exist "dist\Tokenatra\" (
    echo [ERROR] dist\Tokenatra\ not found.
    pause
    exit /b 1
)

where iscc >nul 2>&1
if not errorlevel 1 goto run_iscc

if exist "%ProgramFiles(x86)%\Inno Setup 6\iscc.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\iscc.exe" & goto run_iscc
if exist "%ProgramFiles%\Inno Setup 6\iscc.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\iscc.exe" & goto run_iscc
if exist "%ProgramFiles(x86)%\Inno Setup 5\iscc.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 5\iscc.exe" & goto run_iscc
if exist "%ProgramFiles%\Inno Setup 5\iscc.exe" set "ISCC=%ProgramFiles%\Inno Setup 5\iscc.exe" & goto run_iscc

echo.
echo   [INFO] Inno Setup not found.
echo   Install from: https://jrsoftware.org/isdl.php
echo   Then run: iscc installer.iss
echo.
echo   Portable build ready at: dist\Tokenatra\
pause
exit /b 0

:run_iscc
if not defined ISCC set ISCC=iscc
echo Running Inno Setup...
"%ISCC%" installer.iss
if errorlevel 1 (
    echo [ERROR] Inno Setup build failed
    pause
    exit /b 1
)

echo.
echo ========================================
echo   Done! Installer in dist\installer\
echo ========================================
echo.
pause
