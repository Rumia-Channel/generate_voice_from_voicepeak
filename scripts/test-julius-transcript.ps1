Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "julius-transcript.ps1")

function Assert-Equal {
    param(
        [Parameter(Mandatory = $true)][string]$Expected,
        [Parameter(Mandatory = $true)][string]$Actual,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($Expected -cne $Actual) {
        throw "$Name failed.`nExpected: $Expected`nActual:   $Actual"
    }
}

Assert-Equal `
    -Name "full VPP reading" `
    -Expected "どおすんの sp このおみせ sp かんっぜんにかんこどりがないちゃってるじゃない" `
    -Actual (Convert-VppKatakanaToJuliusTranscript "ドオスンノ、コノオミセ。カンッゼンニカンコドリガナイチャッテルジャナイ。")

Assert-Equal `
    -Name "standalone geminate is not a pause" `
    -Expected "かんっぜんに" `
    -Actual (Convert-VppKatakanaToJuliusTranscript "カンッゼンニ。")

Assert-Equal `
    -Name "internal punctuation becomes one sp" `
    -Expected "えっ sp うそ" `
    -Actual (Convert-VppKatakanaToJuliusTranscript "エッ、、ウソ。")

Assert-Equal `
    -Name "long vowel survives for yomi2voca" `
    -Expected "らーめん" `
    -Actual (Convert-VppKatakanaToJuliusTranscript "ラーメン。")

Assert-Equal `
    -Name "vu uses upstream decomposed spelling" `
    -Expected (([string][char]0x3046) + ([string][char]0x309B) + "ぁ") `
    -Actual (Convert-VppKatakanaToJuliusTranscript "ヴァ。")

Write-Host "Julius transcript conversion tests passed."
