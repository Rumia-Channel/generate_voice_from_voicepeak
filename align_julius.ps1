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

function Resolve-Perl {
    $command = Get-Command perl.exe -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        $command = Get-Command perl -ErrorAction SilentlyContinue
    }
    if ($null -eq $command) {
        throw "Perl is required by Julius segmentation-kit but was not found on PATH."
    }
    return $command.Source
}

function Test-IsPausePunctuation {
    param([char]$Character)
    return "。、，,！？!?・：:；;「」『』（）()….".Contains([string]$Character)
}

function Convert-KatakanaCharToHiragana {
    param([char]$Character)

    $code = [int][char]$Character
    if ($code -eq 0x30F4) {
        # The upstream yomi2voca() uses the historical decomposed spelling う゛.
        return ([string][char]0x3046) + ([string][char]0x309B)
    }
    if ($code -ge 0x30A1 -and $code -le 0x30F6) {
        return [string][char]($code - 0x60)
    }
    return [string]$Character
}

function Convert-VppKatakanaToJuliusTranscript {
    param([Parameter(Mandatory = $true)][string]$Katakana)

    $parts = New-Object System.Collections.Generic.List[string]
    $current = New-Object System.Text.StringBuilder
    $pendingPause = $false

    foreach ($character in $Katakana.ToCharArray()) {
        if (Test-IsPausePunctuation -Character $character) {
            if ($current.Length -gt 0) {
                $parts.Add($current.ToString())
                [void]$current.Clear()
            }
            if ($parts.Count -gt 0) {
                $pendingPause = $true
            }
            continue
        }

        if ([char]::IsWhiteSpace($character)) {
            continue
        }

        if ($pendingPause) {
            if ($parts.Count -gt 0 -and $parts[$parts.Count - 1] -ne "sp") {
                $parts.Add("sp")
            }
            $pendingPause = $false
        }
        [void]$current.Append((Convert-KatakanaCharToHiragana -Character $character))
    }

    if ($current.Length -gt 0) {
        $parts.Add($current.ToString())
    }
    while ($parts.Count -gt 0 -and $parts[$parts.Count - 1] -eq "sp") {
        $parts.RemoveAt($parts.Count - 1)
    }

    return ($parts -join " ")
}

$resolvedDatasetRoot = (Resolve-Path -LiteralPath $DatasetRoot).Path
$resolvedJuliusRoot = (Resolve-Path -LiteralPath $JuliusRoot).Path
$perl = Resolve-Perl
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
$segmentationScript = Resolve-ExistingFile `
    -Candidates @(
        (Join-Path $PSScriptRoot "third_party\segmentation-kit\segment_julius.pl"),
        (Join-Path $resolvedJuliusRoot "segmentation-kit\segment_julius.pl")
    ) `
    -Description "Julius segmentation-kit script"

$juliusDatasetRoot = Join-Path $resolvedDatasetRoot "julius"
if (-not (Test-Path -LiteralPath $juliusDatasetRoot -PathType Container)) {
    throw "Julius dataset directory was not found: $juliusDatasetRoot"
}

$wavDirectories = @(Get-ChildItem -LiteralPath $juliusDatasetRoot -Directory -Recurse | Where-Object {
    $_.Name -eq "wav"
} | Sort-Object FullName)
if ($wavDirectories.Count -eq 0) {
    throw "No Julius WAV directories were found under $juliusDatasetRoot"
}

# Upstream segment_julius.pl uses fixed relative paths and a shell pipeline.
# Run it against a simple staging tree so spaces in the user's dataset path do
# not leak into that legacy command line.
$kitRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("voicepeak-julius-segmentation-" + [guid]::NewGuid().ToString("N"))
$kitBin = Join-Path $kitRoot "bin"
$kitModels = Join-Path $kitRoot "models"
$kitWav = Join-Path $kitRoot "wav"
New-Item -ItemType Directory -Force -Path $kitBin, $kitModels, $kitWav | Out-Null
Copy-Item -LiteralPath $juliusExecutable -Destination (Join-Path $kitBin "julius-4.3.1.exe") -Force
Get-ChildItem -LiteralPath ([System.IO.Path]::GetDirectoryName($juliusExecutable)) -File -Filter "*.dll" | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $kitBin $_.Name) -Force
}
Copy-Item -LiteralPath $hmmdefs -Destination (Join-Path $kitModels "hmmdefs_monof_mix16_gid.binhmm") -Force
Copy-Item -LiteralPath $segmentationScript -Destination (Join-Path $kitRoot "segment_julius.pl") -Force

$failed = 0
$aligned = 0
try {
    foreach ($wavDirectory in $wavDirectories) {
        Get-ChildItem -LiteralPath $kitWav -Force | Remove-Item -Recurse -Force
        $wavFiles = @(Get-ChildItem -LiteralPath $wavDirectory.FullName -File -Filter "*.wav" | Sort-Object Name)
        if ($wavFiles.Count -eq 0) {
            continue
        }

        foreach ($wavFile in $wavFiles) {
            $sourceLabPath = [System.IO.Path]::ChangeExtension($wavFile.FullName, ".lab")
            if (-not (Test-Path -LiteralPath $sourceLabPath -PathType Leaf)) {
                throw "Pre-alignment VPP Katakana transcription was not found: $sourceLabPath"
            }
            $katakana = ([System.IO.File]::ReadAllText($sourceLabPath)).Trim()
            if ([string]::IsNullOrWhiteSpace($katakana)) {
                throw "Pre-alignment VPP Katakana transcription is empty: $sourceLabPath"
            }
            $transcript = Convert-VppKatakanaToJuliusTranscript -Katakana $katakana
            if ([string]::IsNullOrWhiteSpace($transcript)) {
                throw "Julius Hiragana transcription is empty: $sourceLabPath"
            }

            $stagedWav = Join-Path $kitWav ($wavFile.BaseName + ".wav")
            $stagedTxt = Join-Path $kitWav ($wavFile.BaseName + ".txt")
            Copy-Item -LiteralPath $wavFile.FullName -Destination $stagedWav -Force
            [System.IO.File]::WriteAllText(
                $stagedTxt,
                $transcript + "`n",
                (New-Object System.Text.UTF8Encoding($false))
            )

            # Keep the exact segmentation-kit input beside the source audio for
            # reproducibility and manual inspection.
            $datasetTxt = [System.IO.Path]::ChangeExtension($wavFile.FullName, ".txt")
            [System.IO.File]::WriteAllText(
                $datasetTxt,
                $transcript + "`n",
                (New-Object System.Text.UTF8Encoding($false))
            )
        }

        Push-Location $kitRoot
        try {
            & $perl ".\segment_julius.pl" ".\wav"
            if ($LASTEXITCODE -ne 0) {
                throw "segment_julius.pl exited with status $LASTEXITCODE for $($wavDirectory.FullName)"
            }
        }
        finally {
            Pop-Location
        }

        foreach ($wavFile in $wavFiles) {
            $stagedLab = Join-Path $kitWav ($wavFile.BaseName + ".lab")
            $stagedLog = Join-Path $kitWav ($wavFile.BaseName + ".log")
            $destinationLab = [System.IO.Path]::ChangeExtension($wavFile.FullName, ".lab")
            $destinationLog = [System.IO.Path]::ChangeExtension($wavFile.FullName, ".julius.log")

            if (-not (Test-Path -LiteralPath $stagedLab -PathType Leaf)) {
                $failed++
                Write-Error "Julius did not generate an alignment LAB: $stagedLab"
                continue
            }
            $lab = ([System.IO.File]::ReadAllText($stagedLab)).Trim()
            if ([string]::IsNullOrWhiteSpace($lab) -or $lab -notmatch '^\d+\.\d+\s+\d+\.\d+\s+') {
                $failed++
                Write-Error "Julius generated an invalid or empty alignment LAB: $stagedLab"
                continue
            }

            Copy-Item -LiteralPath $stagedLab -Destination $destinationLab -Force
            if (Test-Path -LiteralPath $stagedLog -PathType Leaf) {
                Copy-Item -LiteralPath $stagedLog -Destination $destinationLog -Force
            }
            $aligned++
            Write-Host ("aligned={0} lab={1}" -f $aligned, $destinationLab)
        }
    }
}
finally {
    Remove-Item -LiteralPath $kitRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failed -gt 0) {
    throw "Julius alignment completed with $failed failure(s); $aligned file(s) aligned successfully"
}
Write-Host ("Julius alignment completed: {0} file(s)" -f $aligned)
