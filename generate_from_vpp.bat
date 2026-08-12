@echo off
setlocal EnableExtensions DisableDelayedExpansion

if "%~1"=="" goto usage

set "SCRIPT_DIR=%~dp0"
set "VPP_ANCHOR=%~f1"
set "OUTPUT_DIR="
set "VPP_ARGS="
set /a VPP_COUNT=0

rem Keep the original "one VPP, one output directory" syntax working.
if "%~3"=="" if not "%~2"=="" if /i not "%~x2"==".vpp" (
    if not exist "%~f1" (
        echo ERROR: VPP file not found: %~1 1>&2
        exit /b 2
    )
    set "VPP_ARGS=--vpp ^"%~f1^""
    set "VPP_COUNT=1"
    set "OUTPUT_DIR=%~f2"
    goto parsed
)

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--output" goto parse_output
if /i "%~1"=="--output-dir" goto parse_output
if not exist "%~f1" (
    echo ERROR: VPP file not found: %~1 1>&2
    exit /b 2
)
set "VPP_ARGS=%VPP_ARGS% --vpp ^"%~f1^""
set /a VPP_COUNT+=1
shift
goto parse

:parse_output
if "%~2"=="" (
    echo ERROR: %~1 requires an output directory. 1>&2
    exit /b 64
)
set "OUTPUT_DIR=%~f2"
shift
shift
goto parse

:parsed
if "%VPP_COUNT%"=="0" (
    echo ERROR: at least one VPP file is required. 1>&2
    exit /b 64
)
if not defined OUTPUT_DIR for %%I in ("%VPP_ANCHOR%") do set "OUTPUT_DIR=%%~dpnI_dataset"

set "GENERATOR=%SCRIPT_DIR%generate_voice_from_voicepeak.exe"
if not exist "%GENERATOR%" if exist "%SCRIPT_DIR%target\release\generate_voice_from_voicepeak.exe" set "GENERATOR=%SCRIPT_DIR%target\release\generate_voice_from_voicepeak.exe"
if not exist "%GENERATOR%" (
    echo ERROR: generate_voice_from_voicepeak.exe was not found next to this BAT or under target\release. 1>&2
    exit /b 3
)

where ffmpeg.exe >nul 2>&1
if errorlevel 1 (
    echo ERROR: ffmpeg.exe is required for Julius 16 kHz WAV conversion and was not found on PATH. 1>&2
    exit /b 4
)

set "ALIGNER=%SCRIPT_DIR%align_julius.ps1"
if not exist "%ALIGNER%" (
    echo ERROR: align_julius.ps1 was not found next to this BAT. 1>&2
    exit /b 5
)

set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%POWERSHELL%" (
    echo ERROR: Windows PowerShell was not found. 1>&2
    exit /b 6
)
if "%JULIUS_ROOT%"=="" set "JULIUS_ROOT=%SCRIPT_DIR%julius"

echo [1/2] Synthesizing VOICEPEAK audio and preparing Julius inputs...
"%GENERATOR%" %VPP_ARGS% "%OUTPUT_DIR%" --strict
if errorlevel 1 (
    echo ERROR: audio synthesis or label preparation failed. 1>&2
    exit /b 10
)

echo [2/2] Running Julius forced phoneme alignment...
"%POWERSHELL%" -NoProfile -ExecutionPolicy Bypass -File "%ALIGNER%" -DatasetRoot "%OUTPUT_DIR%" -JuliusRoot "%JULIUS_ROOT%"
if errorlevel 1 (
    echo ERROR: Julius alignment failed. Check *.julius.log under the output directory. 1>&2
    exit /b 11
)

echo Completed.
echo Output: %OUTPUT_DIR%
exit /b 0

:usage
echo Usage: %~nx0 "first.vpp" ["second.vpp" ...] [--output "directory"]
echo.
echo Legacy syntax: %~nx0 "voicepeak.vpp" ["output-directory"]
exit /b 64
