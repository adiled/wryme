@echo off
rem
rem wryme desktop runner - Windows
rem
rem Opens wryme in a clean, app-like WezTerm window. No terminal chrome.
rem
rem This script does the actual work; wryme-launcher.vbs invokes it hidden
rem so double-clicking "wryme" feels like opening an app, not a console.

setlocal enableextensions
set "HERE=%~dp0"
set "WME=%HERE%wme.exe"
set "CFG=%HERE%wezterm.lua"

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
if not exist "%WME%" (
    powershell -NoProfile -Command ^
      "Add-Type -AssemblyName PresentationFramework; [System.Windows.Forms.MessageBox]::Show('Could not find wme.exe next to this launcher.', 'wryme', 'OK', 'Error')" >nul 2>&1
    exit /b 1
)

"%WEZTERM%" --config-file "%CFG%" start --always-new-process --class wryme -- "%WME%"
exit /b %errorlevel%