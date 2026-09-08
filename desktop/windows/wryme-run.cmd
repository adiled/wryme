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

rem --- Silent auto-update (Windows) --------------------------------------
rem Fire-and-forget PowerShell that checks GitHub once per 24h and replaces wme.exe
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
      "$ErrorActionPreference='SilentlyContinue';" ^
      "$cur=(cmd /c '\"%WME%\" --version' 2>$null | Select-String -Pattern '[0-9]+\.[0-9]+\.[0-9]+' | %% { $_.Matches[0].Value } | Select-Object -First 1); if(-not $cur){$cur='0.0.0'};" ^
      "$json=(Invoke-RestMethod -Uri 'https://api.github.com/repos/adiled/wryme/releases/latest' -TimeoutSec 8 -Headers @{'Accept'='application/vnd.github+json'} -ErrorAction SilentlyContinue);" ^
      "$tag=$json.tag_name.TrimStart('v'); if(-not $tag){exit};" ^
      "function parse($v){ try{ return [int]$v.Split('.')[0],[int]$v.Split('.')[1],[int]$v.Split('.')[2]}catch{return 0,0,0} };" ^
      "$need=(parse $tag) -join '.' -gt (parse $cur) -join '.'; $c=parse $cur; $t=parse $tag; $need=($t[0]-gt$c[0]) -or ($t[0]-eq$c[0]-and $t[1]-gt$c[1]) -or ($t[0]-eq$c[0]-and $t[1]-eq$c[1]-and $t[2]-gt$c[2]); if(-not $need){ (Get-Date).ToString() | Out-File '%STAMP%' -Force; exit };" ^
      "$asset='wryme-windows-x86_64.zip'; $url=($json.assets | Where-Object { $_.name -eq $asset } | Select-Object -First 1).browser_download_url; if(-not $url){exit};" ^
      "$tmp=Join-Path $env:TEMP ('wryme-upd-'+[guid]::NewGuid()); New-Item -ItemType Directory -Path $tmp -Force | Out-Null;" ^
      "$zip=Join-Path $tmp 'bundle.zip'; try{ Invoke-WebRequest -Uri $url -OutFile $zip -TimeoutSec 90 -UseBasicParsing }catch{ (Get-Date).ToString() | Out-File '%STAMP%' -Force; Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue; exit };" ^
      "try{ Expand-Archive -Path $zip -DestinationPath $tmp -Force }catch{ (Get-Date).ToString() | Out-File '%STAMP%' -Force; Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue; exit };" ^
      "$new=Get-ChildItem -Path $tmp -Filter 'wme.exe' -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1; if($new){ Copy-Item $new.FullName '%WME%.new' -Force; Move-Item '%WME%.new' '%WME%' -Force };" ^
      "(Get-Date).ToString() | Out-File '%STAMP%' -Force; Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue" >nul 2>&1
)

"%WEZTERM%" --config-file "%CFG%" start --class wryme -- "%WME%"
exit /b %errorlevel%