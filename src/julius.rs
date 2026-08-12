use std::error::Error;
use std::f64::consts::PI;
use std::fs;
use std::path::Path;

use crate::models::MfaUtterance;

const JULIUS_SAMPLE_RATE: u32 = 16_000;
const RESAMPLE_RADIUS: isize = 32;

pub(crate) fn build_julius_transcription(utterance: &MfaUtterance) -> String {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut pending_pause = false;

    for token in &utterance.tokens {
        let kana = if !token.reading.is_empty() {
            Some(token.reading.as_str())
        } else if is_kana(&token.surface) {
            Some(token.surface.as_str())
        } else {
            None
        };

        if let Some(kana) = kana {
            if pending_pause {
                flush_chunk(&mut current, &mut parts);
                if !parts.is_empty() && parts.last().is_none_or(|part| part != "sp") {
                    parts.push("sp".to_string());
                }
                pending_pause = false;
            }
            current.push_str(&katakana_to_hiragana(kana));
            continue;
        }

        // Julius' segmentation kit does not consume punctuation. A VPP pause
        // token inside an utterance becomes an explicit short-pause candidate.
        // Leading/trailing pauses are omitted because the kit inserts silB/silE.
        if token.pause {
            pending_pause = !current.is_empty() || !parts.is_empty();
        }
    }

    flush_chunk(&mut current, &mut parts);
    while parts.last().is_some_and(|part| part == "sp") {
        parts.pop();
    }
    parts.join(" ")
}

fn flush_chunk(current: &mut String, parts: &mut Vec<String>) {
    if !current.is_empty() {
        parts.push(std::mem::take(current));
    }
}

fn is_kana(value: &str) -> bool {
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
            // segment_julius.pl contains rules for the decomposed spelling う゛.
            'ヴ' | 'ゔ' => output.push_str("う゛"),
            '\u{30a1}'..='\u{30f6}' => {
                output.push(char::from_u32(character as u32 - 0x60).unwrap_or(character));
            }
            _ => output.push(character),
        }
    }
    output
}

pub(crate) fn write_julius_wav(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(source)?;
    let wav = parse_wav(&bytes).map_err(|error| format!("{}: {error}", source.display()))?;
    let mono = decode_mono(&wav).map_err(|error| format!("{}: {error}", source.display()))?;
    let resampled = resample_bandlimited(&mono, wav.sample_rate, JULIUS_SAMPLE_RATE);
    write_pcm16_mono(destination, &resampled, JULIUS_SAMPLE_RATE)?;
    Ok(())
}

struct ParsedWav<'a> {
    format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    block_align: u16,
    data: &'a [u8],
}

fn parse_wav(bytes: &[u8]) -> Result<ParsedWav<'_>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }

    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| "WAV chunk size overflow".to_string())?;
        if end > bytes.len() {
            return Err("truncated WAV chunk".to_string());
        }

        if id == b"fmt " {
            if size < 16 {
                return Err("WAV fmt chunk is too short".to_string());
            }
            let mut format = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
            let channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap());
            let sample_rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
            let block_align = u16::from_le_bytes(bytes[start + 12..start + 14].try_into().unwrap());
            let bits_per_sample = u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap());

            // WAVE_FORMAT_EXTENSIBLE stores the real format tag at the start of
            // SubFormat. Supporting it avoids depending on one VOICEPEAK WAV layout.
            if format == 0xfffe && size >= 40 {
                format = u16::from_le_bytes(bytes[start + 24..start + 26].try_into().unwrap());
            }
            fmt = Some((format, channels, sample_rate, bits_per_sample, block_align));
        } else if id == b"data" {
            data = Some(&bytes[start..end]);
        }

        offset = end + (size & 1);
    }

    let (format, channels, sample_rate, bits_per_sample, block_align) =
        fmt.ok_or_else(|| "WAV has no fmt chunk".to_string())?;
    let data = data.ok_or_else(|| "WAV has no data chunk".to_string())?;
    if channels == 0 || sample_rate == 0 || block_align == 0 {
        return Err("WAV has invalid channel/rate/alignment metadata".to_string());
    }

    Ok(ParsedWav {
        format,
        channels,
        sample_rate,
        bits_per_sample,
        block_align,
        data,
    })
}

fn decode_mono(wav: &ParsedWav<'_>) -> Result<Vec<f64>, String> {
    let bytes_per_sample = usize::from(wav.bits_per_sample.div_ceil(8));
    if bytes_per_sample == 0
        || usize::from(wav.block_align) < bytes_per_sample * usize::from(wav.channels)
    {
        return Err("invalid WAV sample/block alignment".to_string());
    }

    let frame_size = usize::from(wav.block_align);
    let frame_count = wav.data.len() / frame_size;
    let mut output = Vec::with_capacity(frame_count);
    for frame_index in 0..frame_count {
        let frame = &wav.data[frame_index * frame_size..(frame_index + 1) * frame_size];
        let mut sum = 0.0;
        for channel in 0..usize::from(wav.channels) {
            let start = channel * bytes_per_sample;
            let sample = &frame[start..start + bytes_per_sample];
            sum += decode_sample(sample, wav.format, wav.bits_per_sample)?;
        }
        output.push(sum / f64::from(wav.channels));
    }
    Ok(output)
}

fn decode_sample(sample: &[u8], format: u16, bits: u16) -> Result<f64, String> {
    match (format, bits) {
        (1, 8) => Ok((f64::from(sample[0]) - 128.0) / 128.0),
        (1, 16) => Ok(f64::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32768.0),
        (1, 24) => {
            let raw = i32::from(sample[0]) | (i32::from(sample[1]) << 8) | (i32::from(sample[2]) << 16);
            let signed = if raw & 0x0080_0000 != 0 {
                raw | !0x00ff_ffff
            } else {
                raw
            };
            Ok(signed as f64 / 8_388_608.0)
        }
        (1, 32) => Ok(i32::from_le_bytes(sample.try_into().unwrap()) as f64 / 2_147_483_648.0),
        (3, 32) => Ok(f32::from_le_bytes(sample.try_into().unwrap()) as f64),
        (3, 64) => Ok(f64::from_le_bytes(sample.try_into().unwrap())),
        _ => Err(format!("unsupported WAV format tag={format}, bits={bits}")),
    }
}

fn write_pcm16_mono(path: &Path, samples: &[f64], sample_rate: u32) -> Result<(), Box<dyn Error>> {
    let data_size = samples
        .len()
        .checked_mul(2)
        .ok_or("WAV output size overflow")?;
    let data_size_u32 = u32::try_from(data_size).map_err(|_| "WAV output exceeds RIFF size limit")?;
    let riff_size = 36u32
        .checked_add(data_size_u32)
        .ok_or("WAV RIFF size overflow")?;

    let mut output = Vec::with_capacity(44 + data_size);
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&riff_size.to_le_bytes());
    output.extend_from_slice(b"WAVEfmt ");
    output.extend_from_slice(&16u32.to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&sample_rate.to_le_bytes());
    output.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    output.extend_from_slice(&2u16.to_le_bytes());
    output.extend_from_slice(&16u16.to_le_bytes());
    output.extend_from_slice(b"data");
    output.extend_from_slice(&data_size_u32.to_le_bytes());

    for &sample in samples {
        let value = if sample <= -1.0 {
            i16::MIN
        } else {
            (sample.min(1.0) * f64::from(i16::MAX)).round() as i16
        };
        output.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(path, output)?;
    Ok(())
}

fn resample_bandlimited(input: &[f64], input_rate: u32, output_rate: u32) -> Vec<f64> {
    if input.is_empty() || input_rate == 0 || output_rate == 0 {
        return Vec::new();
    }
    if input_rate == output_rate {
        return input.to_vec();
    }

    let output_len = ((input.len() as u64 * output_rate as u64 + input_rate as u64 / 2)
        / input_rate as u64) as usize;
    let cutoff = (output_rate as f64 / input_rate as f64).min(1.0) * 0.95;
    let radius = RESAMPLE_RADIUS as f64;
    let mut output = Vec::with_capacity(output_len);

    for output_index in 0..output_len {
        let position = output_index as f64 * input_rate as f64 / output_rate as f64;
        let center = position.floor() as isize;
        let mut sum = 0.0;
        let mut weight_sum = 0.0;

        for sample_index in (center - RESAMPLE_RADIUS + 1)..=(center + RESAMPLE_RADIUS) {
            if sample_index < 0 || sample_index >= input.len() as isize {
                continue;
            }
            let distance = position - sample_index as f64;
            if distance.abs() >= radius {
                continue;
            }
            let x = PI * distance * cutoff;
            let sinc = if x.abs() < 1.0e-12 { 1.0 } else { x.sin() / x };
            let window = 0.5 + 0.5 * (PI * distance / radius).cos();
            let weight = cutoff * sinc * window;
            sum += input[sample_index as usize] * weight;
            weight_sum += weight;
        }

        output.push(if weight_sum.abs() > 1.0e-12 {
            sum / weight_sum
        } else {
            0.0
        });
    }
    output
}

#[cfg(test)]
pub(crate) fn resample_for_test(input: &[f64], input_rate: u32, output_rate: u32) -> Vec<f64> {
    resample_bandlimited(input, input_rate, output_rate)
}
