param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [Parameter(Mandatory = $true)]
    [string]$JuliusBuildRoot,

    [Parameter(Mandatory = $true)]
    [string]$GrammarKitRoot,

    [Parameter(Mandatory = $true)]
    [string]$JuliusRef,

    [Parameter(Mandatory = $true)]
    [string]$GrammarKitRef,

    [string]$OutputDir = "dist",

    [string]$PackageRoot = "generate_voice_from_voicepeak",

    [string]$Version
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-RequiredFile {
    param([string]$Path, [string]$Description)

    if (-not (Test-Path -Path $Path -PathType Leaf)) {
        throw "$Description not found: $Path"
    }

    return (Resolve-Path -Path $Path).Path
}

function Resolve-RequiredDirectory {
    param([string]$Path, [string]$Description)

    if (-not (Test-Path -Path $Path -PathType Container)) {
        throw "$Description not found: $Path"
    }

    return (Resolve-Path -Path $Path).Path
}

$resolvedBinary = Resolve-RequiredFile -Path $BinaryPath -Description "Release binary"
$resolvedJuliusRoot = Resolve-RequiredDirectory -Path $JuliusBuildRoot -Description "Julius build directory"
$resolvedGrammarKitRoot = Resolve-RequiredDirectory -Path $GrammarKitRoot -Description "Grammar-kit directory"
$resolvedOutputDir = (New-Item -ItemType Directory -Force -Path $OutputDir).FullName
$repoRoot = (Resolve-Path -Path (Join-Path $PSScriptRoot "..")).Path

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = $env:GITHUB_REF_NAME
}
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = "local"
}

$archiveName = "generate_voice_from_voicepeak_windows_x64_{0}.zip" -f $Version
$archivePath = Join-Path $resolvedOutputDir $archiveName
$stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("generate_voice_from_voicepeak-release-" + [guid]::NewGuid().ToString("N"))
$packageDir = Join-Path $stagingRoot $PackageRoot

try {
    New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
    $juliusPackageDir = Join-Path $packageDir "julius"
    $juliusBinDir = Join-Path $juliusPackageDir "bin"
    $grammarKitPackageDir = Join-Path $juliusPackageDir "grammar-kit"
    New-Item -ItemType Directory -Force -Path $juliusBinDir, $grammarKitPackageDir | Out-Null

    Copy-Item -Path $resolvedBinary -Destination (Join-Path $packageDir "generate_voice_from_voicepeak.exe")

    $packageFiles = @(
        "README.md"
        "generate_from_vpp.bat"
        "align_julius.ps1"
    )
    foreach ($packageFile in $packageFiles) {
        $sourcePath = Resolve-RequiredFile `
            -Path (Join-Path $repoRoot $packageFile) `
            -Description "Package file $packageFile"
        Copy-Item -Path $sourcePath -Destination (Join-Path $packageDir $packageFile)
    }

    $scriptsPackageDir = Join-Path $packageDir "scripts"
    New-Item -ItemType Directory -Force -Path $scriptsPackageDir | Out-Null
    Copy-Item `
        -Path (Resolve-RequiredFile -Path (Join-Path $repoRoot "scripts\julius-transcript.ps1") -Description "Julius transcript helper") `
        -Destination (Join-Path $scriptsPackageDir "julius-transcript.ps1")

    $segmentationKitSource = Resolve-RequiredDirectory `
        -Path (Join-Path $repoRoot "third_party\segmentation-kit") `
        -Description "Julius segmentation-kit helper"
    $segmentationKitDestination = Join-Path $packageDir "third_party\segmentation-kit"
    New-Item -ItemType Directory -Force -Path $segmentationKitDestination | Out-Null
    Copy-Item -Path (Join-Path $segmentationKitSource "*") -Destination $segmentationKitDestination -Recurse -Force

    $juliusRuntimeFiles = @(Get-ChildItem -Path $resolvedJuliusRoot -Recurse -File | Where-Object {
        $_.Extension.ToLowerInvariant() -in @(".exe", ".dll")
    })
    if ($juliusRuntimeFiles.Count -eq 0) {
        throw "No Julius executable or DLL was produced under $resolvedJuliusRoot"
    }

    $juliusNames = @{}
    foreach ($file in $juliusRuntimeFiles) {
        $name = $file.Name.ToLowerInvariant()
        if ($juliusNames.ContainsKey($name)) {
            throw "Duplicate Julius runtime filename: $($file.Name)"
        }
        $juliusNames[$name] = $true
        Copy-Item -Path $file.FullName -Destination (Join-Path $juliusBinDir $file.Name)
    }

    $juliusExe = Join-Path $juliusBinDir "julius.exe"
    if (-not (Test-Path -Path $juliusExe -PathType Leaf)) {
        throw "The Julius decoder executable was not produced: $juliusExe"
    }
    Copy-Item `
        -Path (Resolve-RequiredFile -Path (Join-Path $resolvedJuliusRoot "LICENSE") -Description "Julius license") `
        -Destination (Join-Path $juliusPackageDir "JULIUS-LICENSE")

    foreach ($item in Get-ChildItem -Path $resolvedGrammarKitRoot -Force) {
        if ($item.Name -eq ".git" -or $item.Name -eq "bin") {
            continue
        }
        Copy-Item -Path $item.FullName -Destination (Join-Path $grammarKitPackageDir $item.Name) -Recurse -Force
    }

    $grammarKitScripts = Join-Path $resolvedGrammarKitRoot "bin\win32"
    if (Test-Path -Path $grammarKitScripts -PathType Container) {
        Get-ChildItem -Path $grammarKitScripts -File | Where-Object {
            $_.Extension.ToLowerInvariant() -in @(".pl", ".bat")
        } | ForEach-Object {
            Copy-Item -Path $_.FullName -Destination (Join-Path $juliusBinDir $_.Name) -Force
        }
    }

    $modelPath = Join-Path $grammarKitPackageDir "model\phone_m\hmmdefs_ptm_gid.binhmm"
    $monophoneModelPath = Join-Path $grammarKitPackageDir "model\phone_m\hmmdefs_monof_mix16_gid.binhmm"
    $hmmListPath = Join-Path $grammarKitPackageDir "model\phone_m\logicalTri"
    if (-not (Test-Path -Path $modelPath -PathType Leaf)) {
        throw "Japanese Julius acoustic model not found: $modelPath"
    }
    if (-not (Test-Path -Path $monophoneModelPath -PathType Leaf)) {
        throw "Japanese Julius monophone acoustic model not found: $monophoneModelPath"
    }
    if (-not (Test-Path -Path $hmmListPath -PathType Leaf)) {
        throw "Japanese Julius HMM list not found: $hmmListPath"
    }

    $buildInfo = @(
        "generate_voice_from_voicepeak release: $Version"
        "Julius repository: https://github.com/Rumia-Channel/julius.git"
        "Julius commit: $JuliusRef"
        "Grammar-kit repository: https://github.com/julius-speech/grammar-kit.git"
        "Grammar-kit commit: $GrammarKitRef"
        "Segmentation-kit: vendored under third_party/segmentation-kit (MIT)"
    )
    $buildInfo | Set-Content -Path (Join-Path $juliusPackageDir "BUILD-INFO.txt") -Encoding utf8

    if (Test-Path -Path $archivePath -PathType Leaf) {
        Remove-Item -Path $archivePath -Force
    }
    Compress-Archive -Path $packageDir -DestinationPath $archivePath -CompressionLevel Optimal
}
finally {
    if (Test-Path -Path $stagingRoot) {
        Remove-Item -Path $stagingRoot -Recurse -Force
    }
}

Write-Output $archivePath