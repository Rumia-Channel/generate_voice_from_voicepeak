use std::error::Error;
use std::f64::consts::PI;
use std::fs;
use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

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

        // Julius' segmentation kit does not consume punctuation.  A VPP pause
        // token inside an utterance becomes an explicit optional short pause.
        // Leading/trailing pauses are intentionally omitted because the kit
        // inserts silB/silE itself.
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
            // segment_julius.pl uses the decomposed historical spelling for vu.
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
    let mut reader = WavReader::open(source)?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err(format!("invalid WAV format: {}", source.display()).into());
    }

    if spec.channels == 1
        && spec.sample_rate == JULIUS_SAMPLE_RATE
        && spec.bits_per_sample == 16
        && spec.sample_format == SampleFormat::Int
    {
        fs::copy(source, destination)?;
        return Ok(());
    }

    let mono = match spec.sample_format {
        SampleFormat::Int => {
            let shift = spec.bits_per_sample.saturating_sub(1) as u32;
            let scale = ((1_i64 << shift.min(31)) - 1).max(1) as f64;
            let samples = reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f64 / scale))
                .collect::<Result<Vec<_>, _>>()?;
            downmix(&samples, spec.channels as usize)
        }
        SampleFormat::Float => {
            let samples = reader
                .samples::<f32>()
                .map(|sample| sample.map(|value| value as f64))
                .collect::<Result<Vec<_>, _>>()?;
            downmix(&samples, spec.channels as usize)
        }
    };

    let resampled = resample_bandlimited(&mono, spec.sample_rate, JULIUS_SAMPLE_RATE);
    let output_spec = WavSpec {
        channels: 1,
        sample_rate: JULIUS_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(destination, output_spec)?;
    for sample in resampled {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f64).round() as i16;
        writer.write_sample(value)?;
    }
    writer.finalize()?;
    Ok(())
}

fn downmix(samples: &[f64], channels: usize) -> Vec<f64> {
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f64>() / channels as f64)
        .collect()
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
