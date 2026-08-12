@echo off
setlocal EnableExtensions DisableDelayedExpansion

if "%~1"=="" (
    echo Usage: %~nx0 "path\to\voicepeak.vpp" [output-directory]
    echo.
    echo The output directory defaults to ^<VPP directory^>\^<VPP name^>_dataset.
    exit /b 64
)

if not exist "%~1" (
    echo ERROR: VPP file not found: %~1 1>&2
    exit /b 2
)

set "SCRIPT_DIR=%~dp0"
set "VPP_PATH=%~f1"
set "OUTPUT_DIR=%~dpn1_dataset"
if not "%~2"=="" set "OUTPUT_DIR=%~f2"

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

where perl.exe >nul 2>&1
if errorlevel 1 (
    where perl >nul 2>&1
    if errorlevel 1 (
        echo ERROR: Perl is required by the official Julius segmentation-kit and was not found on PATH. 1>&2
        exit /b 5
    )
)

set "ALIGNER=%SCRIPT_DIR%align_julius.ps1"
if not exist "%ALIGNER%" (
    echo ERROR: align_julius.ps1 was not found next to this BAT. 1>&2
    exit /b 6
)

set "SEGMENTER=%SCRIPT_DIR%third_party\segmentation-kit\segment_julius.pl"
if not exist "%SEGMENTER%" (
    echo ERROR: Julius segmentation-kit script was not found: %SEGMENTER% 1>&2
    exit /b 7
)

set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%POWERSHELL%" (
    echo ERROR: Windows PowerShell was not found. 1>&2
    exit /b 8
)
if "%JULIUS_ROOT%"=="" set "JULIUS_ROOT=%SCRIPT_DIR%julius"

echo [1/2] Synthesizing VOICEPEAK audio and preparing Julius inputs...
"%GENERATOR%" "%VPP_PATH%" "%OUTPUT_DIR%" --strict
if errorlevel 1 (
    echo ERROR: audio synthesis or label preparation failed. 1>&2
    exit /b 10
)

echo [2/2] Running Julius segmentation-kit forced phoneme alignment...
"%POWERSHELL%" -NoProfile -ExecutionPolicy Bypass -File "%ALIGNER%" -DatasetRoot "%OUTPUT_DIR%" -JuliusRoot "%JULIUS_ROOT%"
if errorlevel 1 (
    echo ERROR: Julius alignment failed. Check *.julius.log under the output directory. 1>&2
    exit /b 11
)

echo Completed.
echo Output: %OUTPUT_DIR%
exit /b 0
