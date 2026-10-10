@echo off
rem
rem wryme desktop runner - Windows
rem
rem Opens wryme in a clean, app-like WezTerm window. No terminal chrome.
rem
rem The wryme binary is NEVER bundled. It always comes from cargo: this
rem launcher resolves the cargo-installed wme.exe, installs it from
rem crates.io if missing, and keeps it current. No fallbacks.
rem
rem This script does the actual work; wryme-launcher.vbs invokes it hidden
rem so double-clicking "wryme" feels like opening an app, not a console.

setlocal enableextensions
set "HERE=%~dp0"
set "CFG=%HERE%wezterm.lua"

rem --- Cargo bin dir (CARGO_HOME wins, else ~/.cargo) ----------------------
if defined CARGO_HOME (
    set "CARGO_BIN=%CARGO_HOME%\bin"
) else (
    set "CARGO_BIN=%USERPROFILE%\.cargo\bin"
)
set "WME=%CARGO_BIN%\wme.exe"

rem --- Locate wezterm -------------------------------------------------------
set "WEZTERM="
for %%X in (wezterm.exe) do set "WEZTERM=%%~$PATH:X"
if defined WEZTERM goto :found

if exist "%LOCALAPPDATA%\Programs\wezterm\wezterm.exe" (
    set "WEZTERM=%LOCALAPPDATA%\Programs\wezterm\wezterm.exe"
    goto :found
)
if exist "%ProgramFiles%\WezTerm\wezterm.exe" (
    set "WEZTERM=%ProgramFiles%\WezTerm\wezterm.exe"
    goto :found
)

rem --- WezTerm not found: try to install it ---------------------------------
powershell -NoProfile -Command ^
  "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('wryme needs WezTerm (a small native terminal) to open as its window. Install it now?', 'wryme', 'YesNo', 'Information')" >nul 2>&1
if errorlevel 1 exit /b 0

where winget >nul 2>&1
if errorlevel 1 goto :no_winget
winget install --id wezterm.wezterm --exact --silent --accept-package-agreements --accept-source-agreements >nul 2>&1
if errorlevel 1 (
    powershell -NoProfile -Command ^
      "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('WezTerm install failed. Install it from https://wezterm.org/download.html then open wryme again.', 'wryme', 'OK', 'Error')" >nul 2>&1
    exit /b 1
)
set "WEZTERM=%LOCALAPPDATA%\Programs\wezterm\wezterm.exe"
if not exist "%WEZTERM%" set "WEZTERM=%ProgramFiles%\WezTerm\wezterm.exe"
goto :found

:no_winget
powershell -NoProfile -Command ^
  "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('WezTerm is needed. Install it from https://wezterm.org/download.html then open wryme again.', 'wryme', 'OK', 'Error')" >nul 2>&1
exit /b 1

:found

rem --- Cargo-first wryme: install from crates.io if missing -----------------
if exist "%WME%" goto :wme_ok

if not exist "%CARGO_BIN%" mkdir "%CARGO_BIN%" >nul 2>&1
where cargo >nul 2>&1
if errorlevel 1 (
    powershell -NoProfile -Command ^
      "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('wryme needs the cargo tools to install itself. Install Rust from https://rustup.rs then open wryme again.', 'wryme', 'OK', 'Error')" >nul 2>&1
    exit /b 1
)

powershell -NoProfile -Command ^
  "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('Installing wryme from crates.io. This can take a minute…', 'wryme', 'OK', 'Information')" >nul 2>&1
cargo install wryme --locked
if errorlevel 1 (
    powershell -NoProfile -Command ^
      "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('Installing wryme from cargo failed. Run `cargo install wryme` manually, then open wryme again.', 'wryme', 'OK', 'Error')" >nul 2>&1
    exit /b 1
)
if not exist "%WME%" goto :wme_missing
:wme_ok

rem --- Silent auto-update (Windows) --------------------------------------
rem Keeps the cargo-installed wme current: plain `cargo install` upgrades
rem when a newer version exists and is a no-op when already current.
set "CACHE_DIR=%LOCALAPPDATA%\wryme"
set "STAMP=%CACHE_DIR%\last_update_check"
if not exist "%CACHE_DIR%" mkdir "%CACHE_DIR%" >nul 2>&1
set "DO_UPDATE=1"
if exist "%STAMP%" (
    for /f %%T in ('powershell -NoProfile -Command "(Get-Date) - (Get-Item ''%STAMP%'' -ErrorAction SilentlyContinue).LastWriteTime | %% { $_.TotalSeconds }" 2^>nul') do (
        if %%T LSS 86400 set "DO_UPDATE=0"
    )
)
if "%DO_UPDATE%"=="1" (
    start /b powershell -NoProfile -WindowStyle Hidden -Command ^
      "Start-Process cargo -ArgumentList 'install','wryme','--locked' -WindowStyle Hidden -Wait; (Get-Date).ToString() | Out-File '%STAMP%' -Force" >nul 2>&1
)

"%WEZTERM%" --config-file "%CFG%" start --class wryme -- "%WME%"
exit /b %errorlevel%

:wme_missing
powershell -NoProfile -Command ^
  "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('wme not found after install at %WME%.', 'wryme', 'OK', 'Error')" >nul 2>&1
exit /b 1
