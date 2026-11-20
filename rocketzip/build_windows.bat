@echo off
rem ======================================================================
rem  RocketZip (Rust) - builds RocketZip.exe, rocketzip-cli.exe and the
rem  installer RocketZip-Setup-1.0.0.exe
rem
rem  Needs on this PC (one-time):
rem    * Rust            https://rustup.rs  (accept its offer to install the
rem                      Visual Studio C++ Build Tools - they include CMake)
rem    * Inno Setup 6.3+ https://jrsoftware.org/isdl.php
rem                      or:  winget install JRSoftware.InnoSetup
rem
rem  Usage: double-click, or:  build_windows.bat [--skip-tests]
rem ======================================================================
setlocal EnableExtensions
cd /d "%~dp0"

echo.
echo === [1/5] Checking tools
where cargo >nul 2>nul || (echo ERROR: Rust not found - install it from https://rustup.rs & goto :fail)
where cmake >nul 2>nul || call :find_cmake
where cmake >nul 2>nul || (
  echo ERROR: CMake not found. Install the "C++ CMake tools for Windows" component of the
  echo        Visual Studio Build Tools, or run:  winget install Kitware.CMake
  goto :fail
)

echo.
echo === [2/5] Building (the first build downloads libraries and takes a few minutes)
cargo build --release || goto :fail

echo.
echo === [3/5] Running tests
if /i "%~1"=="--skip-tests" (
  echo skipped
) else (
  cargo test --release
  if errorlevel 1 (
    echo WARNING: some tests failed - check the output above before shipping.
    choice /m "Continue anyway"
    if errorlevel 2 goto :fail
  )
)

echo.
echo === [4/5] Smoke test
target\release\rocketzip-cli.exe formats || goto :fail
target\release\rocketzip-cli.exe a "%TEMP%\zf_smoke.zfx" README.md || goto :fail
target\release\rocketzip-cli.exe t "%TEMP%\zf_smoke.zfx" || goto :fail
del "%TEMP%\zf_smoke.zfx" >nul 2>nul

echo.
echo === [5/5] Building the installer with Inno Setup
set "ISCC="
for %%P in ("%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" "%ProgramFiles%\Inno Setup 6\ISCC.exe" "%LocalAppData%\Programs\Inno Setup 6\ISCC.exe") do (
  if exist "%%~P" set "ISCC=%%~P"
)
if not defined ISCC (
  where iscc >nul 2>nul && set "ISCC=iscc"
)
if not defined ISCC (
  echo Inno Setup 6 not found - skipping the installer.
  echo The programs are ready in target\release\ ^(RocketZip.exe, rocketzip-cli.exe^).
  echo Install Inno Setup ^(winget install JRSoftware.InnoSetup^) and run this again for the installer.
  exit /b 0
)
python tools\gen_notices.py 2>nul || echo    (Python not found - using the included THIRD-PARTY-NOTICES.txt)
"%ISCC%" installer\rocketzip.iss || goto :fail

echo.
echo ======================================================================
echo  DONE
echo    Programs:   target\release\RocketZip.exe, target\release\rocketzip-cli.exe
echo    Installer:  installer\Output\RocketZip-Setup-1.0.0.exe
echo ======================================================================
exit /b 0

:find_cmake
rem CMake ships with Visual Studio but isn't on PATH outside its developer prompt.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" exit /b 0
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -find Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe`) do (
  set "PATH=%PATH%;%%~dpi"
)
exit /b 0

:fail
echo.
echo BUILD FAILED - see the messages above.
exit /b 1
