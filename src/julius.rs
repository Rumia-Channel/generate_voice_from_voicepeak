use crate::models::{MfaUtterance, Sbv2Equivalent};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JuliusPhoneSequence {
    pub(crate) phones: Vec<String>,
}

impl JuliusPhoneSequence {
    pub(crate) fn line(&self) -> String {
        self.phones.join(" ")
    }
    pub(crate) fn lexical_line(&self) -> String {
        self.phones[1..self.phones.len() - 1].join(" ")
    }
}

/// Build the transcription format consumed by Julius' official segmentation-kit.
///
/// The source of truth is the pronunciation already fixed in the VPP, not a
/// second Japanese G2P pass.  Katakana is converted to Hiragana because
/// segment_julius.pl's yomi2voca() accepts Hiragana.  Real internal VPP pause
/// tokens become `sp`; leading/trailing pauses are omitted because
/// segmentation-kit inserts silB/silE itself.
pub(crate) fn build_julius_transcription(utterance: &MfaUtterance) -> String {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut pending_pause = false;

    for token in &utterance.tokens {
        let kana = if !token.reading.is_empty() {
            Some(token.reading.as_str())
        } else if is_kana_surface(&token.surface) {
            // Important: VOICEPEAK can store a standalone small ッ with an
            // empty reading and a pau-like phone.  It is still an orthographic
            // geminate marker, not a pause, so preserve the surface first.
            Some(token.surface.as_str())
        } else {
            None
        };

        if let Some(kana) = kana {
            if pending_pause {
                flush_transcription_chunk(&mut current, &mut parts);
                if !parts.is_empty() && parts.last().is_none_or(|part| part != "sp") {
                    parts.push("sp".to_string());
                }
                pending_pause = false;
            }
            current.push_str(&katakana_to_hiragana(kana));
            continue;
        }

        if token.pause {
            // Delay emission until another voiced/kana chunk is observed, so
            // final punctuation does not create a redundant trailing `sp`.
            pending_pause = !current.is_empty() || !parts.is_empty();
        }
    }

    flush_transcription_chunk(&mut current, &mut parts);
    while parts.last().is_some_and(|part| part == "sp") {
        parts.pop();
    }
    parts.join(" ")
}

fn flush_transcription_chunk(current: &mut String, parts: &mut Vec<String>) {
    if !current.is_empty() {
        parts.push(std::mem::take(current));
    }
}

fn is_kana_surface(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            matches!(
                character,
                '\u{3040}'..='\u{309f}'
                    | '\u{30a0}'..='\u{30ff}'
                    | '\u{31f0}'..='\u{31ff}'
            )
        })
}

fn katakana_to_hiragana(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            // segment_julius.pl contains its ヴ-family rules in the historical
            // decomposed form `う゛`.
            'ヴ' | 'ゔ' => output.push_str("う゛"),
            '\u{30a1}'..='\u{30f6}' => {
                output.push(char::from_u32(character as u32 - 0x60).unwrap_or(character));
            }
            _ => output.push(character),
        }
    }
    output
}

/// Legacy/self-contained path used by align_julius.ps1.
///
/// This remains available so release archives do not acquire a Perl runtime
/// dependency.  The canonical segmentation-kit-compatible transcription is
/// generated separately by build_julius_transcription().
pub(crate) fn sbv2_to_julius(input: &Sbv2Equivalent) -> Result<JuliusPhoneSequence, String> {
    if input.phones.len() < 2 {
        return Err("SBV2 phone sequence must contain boundary guards".to_string());
    }

    let last_index = input.phones.len() - 1;
    let mut phones = Vec::with_capacity(input.phones.len());
    for (index, phone) in input.phones.iter().enumerate() {
        if index == 0 {
            if phone != "_" {
                return Err(format!(
                    "SBV2 phone sequence must start with '_', found {phone:?}"
                ));
            }
            phones.push("silB".to_string());
            continue;
        }
        if index == last_index {
            if phone != "_" {
                return Err(format!(
                    "SBV2 phone sequence must end with '_', found {phone:?}"
                ));
            }
            phones.push("silE".to_string());
            continue;
        }
        let previous = phones.last().map(String::as_str);
        phones.push(julius_phone(phone, previous)?);
    }

    Ok(JuliusPhoneSequence { phones })
}

fn julius_phone(phone: &str, previous: Option<&str>) -> Result<String, String> {
    if phone.is_empty() || phone.chars().any(char::is_whitespace) {
        return Err(format!("invalid Julius phone token: {phone:?}"));
    }
    if phone == "_" {
        return Err("SBV2 '_' is only valid as a boundary guard".to_string());
    }

    let mapped = match phone {
        "cl" | "ッ" => "q".to_string(),
        "ぁ" => "a".to_string(),
        "ぃ" => "i".to_string(),
        "ぅ" => "u".to_string(),
        "ぇ" => "e".to_string(),
        "ぉ" => "o".to_string(),
        "ー" => {
            let vowel = previous
                .filter(|value| matches!(*value, "a" | "i" | "u" | "e" | "o"))
                .ok_or_else(|| "Julius long-vowel phone requires a preceding vowel".to_string())?;
            format!("{vowel}:")
        }
        "pau" | "sp" => "sp".to_string(),
        "、" | "。" | "，" | "," | "！" | "!" | "？" | "?" | "・" | "；" | ";" | "…" | "'"
        | "-" | "." => "sp".to_string(),
        _ => phone.to_string(),
    };
    Ok(mapped)
}

#[derive(Clone, Debug)]
pub(crate) struct FfmpegInfo {
    pub(crate) available: bool,
    pub(crate) command: &'static str,
    pub(crate) version: Option<String>,
    pub(crate) error: Option<String>,
}

impl FfmpegInfo {
    pub(crate) fn detect() -> Self {
        match Command::new("ffmpeg").arg("-version").output() {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned);
                Self {
                    available: true,
                    command: "ffmpeg",
                    version,
                    error: None,
                }
            }
            Ok(output) => Self {
                available: false,
                command: "ffmpeg",
                version: None,
                error: Some(first_error_line(
                    &output.stderr,
                    "ffmpeg returned a non-zero status",
                )),
            },
            Err(error) => Self {
                available: false,
                command: "ffmpeg",
                version: None,
                error: Some(format!("could not execute ffmpeg: {error}")),
            },
        }
    }

    pub(crate) fn status(&self) -> &'static str {
        if self.available {
            "available"
        } else {
            "not_available"
        }
    }
}

pub(crate) fn prepare_output(root: &Path, speeds: &[f64]) -> Result<(), Box<dyn Error>> {
    for speed in speeds {
        let speed_dir = root.join(format!("speed_{speed:.3}"));
        let wav_dir = speed_dir.join("wav");
        let phones_dir = speed_dir.join("phones");
        fs::create_dir_all(&wav_dir)?;
        fs::create_dir_all(&phones_dir)?;
        for entry in fs::read_dir(&wav_dir)? {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("wav" | "lab" | "txt" | "log" | "dfa" | "dict")
            ) {
                fs::remove_file(path)?;
            }
        }
        for entry in fs::read_dir(&phones_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) == Some("txt") {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn convert_wav(
    ffmpeg: &FfmpegInfo,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    if !ffmpeg.available {
        return Err(ffmpeg
            .error
            .clone()
            .unwrap_or_else(|| "ffmpeg is not available".to_string()));
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create Julius WAV directory failed: {error}"))?;
    }

    let output = Command::new(ffmpeg.command)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(source)
        .args([
            "-vn",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
        ])
        .arg(destination)
        .output()
        .map_err(|error| format!("could not execute ffmpeg: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let _ = fs::remove_file(destination);
        Err(first_error_line(
            &output.stderr,
            &format!("ffmpeg exited with status {}", output.status),
        ))
    }
}

fn first_error_line(bytes: &[u8], fallback: &str) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback.to_string())
}
