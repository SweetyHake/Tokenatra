@echo off
setlocal
rem РЎР±РѕСЂРєР° Windows-СѓСЃС‚Р°РЅРѕРІС‰РёРєР°: cargo build + Inno Setup (installer.iss)
rem Inno Setup: https://jrsoftware.org/isdl.php (ISCC.exe РІ PATH РёР»Рё Program Files)

set ROOT=%~dp0

echo [1/3] cargo build --release --bins
pushd "%ROOT%rust"
cargo build --release --bins
if errorlevel 1 (
    echo [ERROR] cargo build failed
    popd & exit /b 1
)
popd

set ISCC="%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not exist %ISCC% set ISCC="%ProgramFiles%\Inno Setup 6\ISCC.exe"
if not exist %ISCC% (
    echo [ERROR] Inno Setup 6 not found - install from https://jrsoftware.org/isdl.php
    exit /b 1
)

echo [2/3] Р’РµСЂСЃРёСЏ РёР· rust\src\consts.rs
for /f "tokens=2 delims==" %%v in ('findstr /C:"pub const VERSION" "%ROOT%rust\src\consts.rs"') do set APPVER=%%~v
set APPVER=%APPVER:"=%
set APPVER=%APPVER:;=%
set APPVER=%APPVER: =%
echo   VERSION=%APPVER%

echo [3/3] ISCC installer.iss
%ISCC% /DMyAppVersion=%APPVER% "%ROOT%installer.iss"
if errorlevel 1 (
    echo [ERROR] ISCC failed
    exit /b 1
)
echo.
echo OK: dist\installer\Tokenatra_v%APPVER%_windows_x86_64_setup.exe
