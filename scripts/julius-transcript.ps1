Set-StrictMode -Version Latest

function Test-IsJuliusPausePunctuation {
    param([char]$Character)
    return "。、，,！？!?・：:；;「」『』（）()….".Contains([string]$Character)
}

function Convert-KatakanaCharToJuliusHiragana {
    param([char]$Character)

    $code = [int][char]$Character
    if ($code -eq 0x30F4) {
        # Upstream segment_julius.pl yomi2voca() uses う゛ for the ヴ series.
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
        if (Test-IsJuliusPausePunctuation -Character $character) {
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
        [void]$current.Append((Convert-KatakanaCharToJuliusHiragana -Character $character))
    }

    if ($current.Length -gt 0) {
        $parts.Add($current.ToString())
    }
    while ($parts.Count -gt 0 -and $parts[$parts.Count - 1] -eq "sp") {
        $parts.RemoveAt($parts.Count - 1)
    }

    return ($parts -join " ")
}
