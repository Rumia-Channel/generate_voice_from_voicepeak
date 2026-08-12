[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DatasetRoot,

    [Parameter(Mandatory = $true)]
    [string]$JuliusRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-ExistingFile {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Candidates,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    foreach ($candidate in $Candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    throw "$Description was not found. Checked: $($Candidates -join ', ')"
}

function Write-SingleWordGrammar {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DfaPath,
        [Parameter(Mandatory = $true)]
        [string]$DictionaryPath,
        [Parameter(Mandatory = $true)]
        [string]$PhoneLine
    )

    # This is the one-word linear DFA used by segmentation-kit. The dictionary
    # entry contains the complete known Julius phone sequence.
    @(
        "0 0 1 0 1"
        "1 -1 -1 1 0"
    ) | Set-Content -LiteralPath $DfaPath -Encoding ascii
    "0 [w_0] $PhoneLine" | Set-Content -LiteralPath $DictionaryPath -Encoding ascii
}

function Start-JuliusProcess {
    param(
        [string]$Executable,
        [string]$Hmmdefs,
        [string]$DfaPath,
        [string]$DictionaryPath,
        [string]$WavePath,
        [string]$LogPath
    )

    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $Executable
    $startInfo.WorkingDirectory = [System.IO.Path]::GetDirectoryName($Executable)
    $startInfo.Arguments = '-h "' + $Hmmdefs + '" -dfa "' + $DfaPath + '" -v "' + $DictionaryPath + '" -b 0 -palign -input file'
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    try {
        [void]$process.Start()
        $process.StandardInput.WriteLine($WavePath)
        $process.StandardInput.Close()
        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $process.WaitForExit()

        $log = $stdout
        if (-not [string]::IsNullOrWhiteSpace($stderr)) {
            $log += "`r`n--- stderr ---`r`n$stderr"
        }
        [System.IO.File]::WriteAllText($LogPath, $log, (New-Object System.Text.UTF8Encoding($false)))

        if ($process.ExitCode -ne 0) {
            throw "Julius exited with status $($process.ExitCode). See $LogPath"
        }
        return $stdout
    }
    finally {
        $process.Dispose()
    }
}

function Convert-ToAlignmentLab {
    param(
        [Parameter(Mandatory = $true)]
        [string]$JuliusOutput,
        [Parameter(Mandatory = $true)]
        [string]$LabPath
    )

    $insideAlignment = $false
    $labLines = New-Object System.Collections.Generic.List[string]
    foreach ($line in ($JuliusOutput -split "`r?`n")) {
        if ($line -match 'begin forced alignment') {
            $insideAlignment = $true
            continue
        }
        if ($line -match 'end forced alignment') {
            $insideAlignment = $false
            break
        }
        if (-not $insideAlignment) {
            continue
        }

        if ($line -match '^\[\s*(\d+)\s+(\d+)\]\s*[-+0-9.]+\s*(.+?)\s*$') {
            $beginFrame = [int]$Matches[1]
            $endFrame = [int]$Matches[2]
            $unit = $Matches[3].Trim()
            if ([string]::IsNullOrWhiteSpace($unit)) {
                continue
            }

            $beginTime = $beginFrame * 0.01
            if ($beginFrame -ne 0) {
                $beginTime += 0.0125
            }
            $endTime = ($endFrame + 1) * 0.01 + 0.0125
            $labLines.Add([string]::Format(
                [System.Globalization.CultureInfo]::InvariantCulture,
                "{0:F7} {1:F7} {2}",
                $beginTime,
                $endTime,
                $unit
            ))
        }
    }

    if ($labLines.Count -eq 0) {
        throw "Julius output did not contain a forced phoneme alignment. See $([System.IO.Path]::ChangeExtension($LabPath, '.julius.log'))"
    }

    [System.IO.File]::WriteAllLines(
        $LabPath,
        [string[]]$labLines,
        (New-Object System.Text.UTF8Encoding($false))
    )
}

$resolvedDatasetRoot = (Resolve-Path -LiteralPath $DatasetRoot).Path
$resolvedJuliusRoot = (Resolve-Path -LiteralPath $JuliusRoot).Path
$juliusExecutable = Resolve-ExistingFile `
    -Candidates @(
        (Join-Path $resolvedJuliusRoot "bin\julius.exe"),
        (Join-Path $resolvedJuliusRoot "julius.exe")
    ) `
    -Description "Julius executable"
$hmmdefs = Resolve-ExistingFile `
    -Candidates @(
        (Join-Path $resolvedJuliusRoot "grammar-kit\model\phone_m\hmmdefs_monof_mix16_gid.binhmm"),
        (Join-Path $resolvedJuliusRoot "model\phone_m\hmmdefs_monof_mix16_gid.binhmm")
    ) `
    -Description "Julius monophone acoustic model"

$juliusDatasetRoot = Join-Path $resolvedDatasetRoot "julius"
if (-not (Test-Path -LiteralPath $juliusDatasetRoot -PathType Container)) {
    throw "Julius dataset directory was not found: $juliusDatasetRoot"
}

$wavFiles = @(Get-ChildItem -LiteralPath $juliusDatasetRoot -Recurse -File -Filter "*.wav" | Sort-Object FullName)
if ($wavFiles.Count -eq 0) {
    throw "No Julius WAV files were found under $juliusDatasetRoot"
}

$failed = 0
$aligned = 0
foreach ($wavFile in $wavFiles) {
    $speedDirectory = $wavFile.Directory.Parent.FullName
    $phonesPath = Join-Path (Join-Path $speedDirectory "phones") ($wavFile.BaseName + ".txt")
    $dfaPath = Join-Path $wavFile.Directory ($wavFile.BaseName + ".julius.dfa")
    $dictionaryPath = Join-Path $wavFile.Directory ($wavFile.BaseName + ".julius.dict")
    $logPath = Join-Path $wavFile.Directory ($wavFile.BaseName + ".julius.log")
    $labPath = [System.IO.Path]::ChangeExtension($wavFile.FullName, ".lab")

    try {
        if (-not (Test-Path -LiteralPath $phonesPath -PathType Leaf)) {
            throw "Julius phone transcription was not found: $phonesPath"
        }
        $phoneLine = ([System.IO.File]::ReadAllText($phonesPath)).Trim()
        if ([string]::IsNullOrWhiteSpace($phoneLine)) {
            throw "Julius phone transcription is empty: $phonesPath"
        }

        Write-SingleWordGrammar -DfaPath $dfaPath -DictionaryPath $dictionaryPath -PhoneLine $phoneLine
        $juliusOutput = Start-JuliusProcess `
            -Executable $juliusExecutable `
            -Hmmdefs $hmmdefs `
            -DfaPath $dfaPath `
            -DictionaryPath $dictionaryPath `
            -WavePath $wavFile.FullName `
            -LogPath $logPath
        Convert-ToAlignmentLab -JuliusOutput $juliusOutput -LabPath $labPath
        $aligned++
        Write-Host ("aligned={0} lab={1}" -f $aligned, $labPath)
    }
    catch {
        $failed++
        $detail = ($_ | Out-String).Trim()
        Write-Error ("Julius alignment failed for {0}: {1}`n{2}" -f $wavFile.FullName, $_.Exception.Message, $detail)
    }
    finally {
        Remove-Item -LiteralPath $dfaPath, $dictionaryPath -Force -ErrorAction SilentlyContinue
    }
}

if ($failed -gt 0) {
    throw "Julius alignment completed with $failed failure(s); $aligned file(s) aligned successfully"
}
Write-Host ("Julius alignment completed: {0} file(s)" -f $aligned)
