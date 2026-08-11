use std::error::Error;
use std::process::Command;
use std::time::Duration;

use serde_json::{Map, Value};
use vpsdk::vpp::Block;
use vpsdk::{
    EditPhoneme, EditSyllable, EditWordEntry, Installation, ManagedProcess, PipeSession,
    PlaybackPayload, SynthesisParams,
};

use crate::config::SPEED_VALUES;
use crate::models::{RuntimeVoice, Variant};
use crate::util::{required_bool, required_f64, required_range, required_string, required_u32};

pub(crate) fn connect_voicepeak() -> Result<(PipeSession, Option<ManagedProcess>), Box<dyn Error>> {
    if let Ok(session) = PipeSession::connect(None, Duration::from_secs(3)) {
        return Ok((session, None));
    }

    let installation = Installation::locate()?;
    let managed = installation
        .launch_managed(Vec::<String>::new(), Duration::from_secs(20))
        .ok()
        .map(|process| process.with_close_on_drop(true));

    if let Some(process) = managed
        && let Ok(session) = connect_with_retries(10, Duration::from_secs(2))
    {
        return Ok((session, Some(process)));
    }

    let _launcher = Command::new("cmd.exe")
        .current_dir(installation.install_dir())
        .args(["/C", "start", ""])
        .arg(installation.exe_path())
        .spawn()?;
    let session = connect_with_retries(15, Duration::from_secs(2)).map_err(|error| {
        format!("VOICEPEAK named-pipe connection failed after managed and shell launch: {error}")
    })?;
    Ok((session, None))
}

fn connect_with_retries(attempts: usize, timeout: Duration) -> Result<PipeSession, String> {
    let mut last_error = String::from("no connection attempt was made");
    for _ in 0..attempts {
        match PipeSession::connect(None, timeout) {
            Ok(session) => return Ok(session),
            Err(error) => {
                last_error = error.to_string();
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    Err(last_error)
}

pub(crate) fn build_variants(
    block_index: usize,
    count: usize,
    block: &Block,
    voices: &[RuntimeVoice],
) -> Vec<Variant> {
    const PITCHES: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];
    const PAUSES: [f64; 3] = [0.8, 1.0, 1.2];
    const DURATIONS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
    const INTONATION_SCALES: [f64; 5] = [0.75, 0.9, 1.0, 1.1, 1.25];
    const INTONATION_OFFSETS: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];
    const CONTOURS: [f64; 3] = [-0.75, 0.0, 0.75];

    let per_speed = count / SPEED_VALUES.len();
    let mut variants = Vec::with_capacity(count);
    let base_pitch = block.params.pitch.unwrap_or(0.0);
    let base_pause = block.params.pause.unwrap_or(1.0);
    let base_speed = block.params.speed.unwrap_or(1.0);
    let volume = block.params.volume.unwrap_or(1.0);

    for (speed_index, speed) in SPEED_VALUES.iter().copied().enumerate() {
        for local_index in 0..per_speed {
            let index = speed_index * per_speed + local_index;
            let is_baseline = local_index == 0;
            let (narrator, emotions, pitch, pause, duration_scale, intonation_scale) =
                if is_baseline {
                    (
                        block.narrator.key.clone(),
                        block.emotions.clone(),
                        base_pitch,
                        base_pause,
                        1.0,
                        1.0,
                    )
                } else {
                    let voice_index = (block_index + local_index) % voices.len();
                    let voice = &voices[voice_index];
                    let emotion_index =
                        (block_index * 13 + local_index * 7) % voice.emotions.len().max(1);
                    let mut emotions = Map::new();
                    if !voice.emotions.is_empty() && local_index % 5 != 0 {
                        let weight = [0.25_f64, 0.5, 0.75, 1.0][(block_index + local_index) % 4];
                        emotions.insert(voice.emotions[emotion_index].clone(), Value::from(weight));
                        if local_index % 3 == 0 && voice.emotions.len() > 1 {
                            let second = (emotion_index + 1) % voice.emotions.len();
                            emotions.insert(
                                voice.emotions[second].clone(),
                                Value::from((weight * 0.5).min(1.0)),
                            );
                        }
                    }
                    (
                        voice.name.clone(),
                        emotions,
                        base_pitch + PITCHES[(block_index * 3 + local_index) % PITCHES.len()],
                        (base_pause * PAUSES[(block_index + local_index * 2) % PAUSES.len()])
                            .clamp(0.5, 2.0),
                        DURATIONS[(block_index + local_index) % DURATIONS.len()],
                        INTONATION_SCALES
                            [(block_index + local_index * 2) % INTONATION_SCALES.len()],
                    )
                };
            let intonation_offset = if is_baseline {
                0.0
            } else {
                INTONATION_OFFSETS[(block_index * 2 + local_index) % INTONATION_OFFSETS.len()]
            };
            let intonation_contour = if is_baseline {
                0.0
            } else {
                CONTOURS[(block_index + local_index) % CONTOURS.len()]
            };

            variants.push(Variant {
                index,
                speed,
                narrator,
                params: SynthesisParams {
                    speed: Some(speed),
                    pitch: Some(pitch),
                    pause: Some(pause),
                    volume: Some(volume),
                },
                emotions,
                duration_scale,
                intonation_scale,
                intonation_offset,
                intonation_contour,
                is_source: is_baseline && (speed - base_speed).abs() < f64::EPSILON,
            });
        }
    }
    variants
}

pub(crate) fn build_variant_payload(base: &PlaybackPayload, variant: &Variant) -> PlaybackPayload {
    let mut payload = base.clone();
    payload.narrator = variant.narrator.clone();
    payload.params = variant.params.clone();
    payload.emotions = variant.emotions.clone();

    if variant.is_source {
        return payload;
    }

    mutate_token_values(&mut payload.edit, variant);
    mutate_token_values(&mut payload.tokens, variant);
    payload
}

pub(crate) fn mutate_token_values(tokens: &mut [Value], variant: &Variant) {
    for token in tokens {
        let Some(syllables) = token.get_mut("syl").and_then(Value::as_array_mut) else {
            continue;
        };
        let total = syllables.len();
        for (mora_index, syllable) in syllables.iter_mut().enumerate() {
            let is_pause = syllable.get("a").and_then(Value::as_u64) == Some(4096)
                || syllable
                    .get("s")
                    .and_then(Value::as_str)
                    .map(|value| value.is_empty())
                    .unwrap_or(false);
            if !is_pause {
                let centered = if total <= 1 {
                    0.0
                } else {
                    let position = mora_index as f64 / (total - 1) as f64;
                    position * 2.0 - 1.0
                };
                let base_i = syllable.get("i").and_then(Value::as_f64).unwrap_or(0.0);
                let intonation = base_i * variant.intonation_scale
                    + variant.intonation_offset
                    + centered * variant.intonation_contour;
                syllable["i"] = Value::from(intonation);
            }

            let Some(phonemes) = syllable.get_mut("p").and_then(Value::as_array_mut) else {
                continue;
            };
            for phoneme in phonemes {
                if let Some(duration) = phoneme.get("d").and_then(Value::as_f64) {
                    phoneme["d"] = Value::from((duration * variant.duration_scale).clamp(0.5, 2.0));
                }
            }
        }
    }
}

pub(crate) fn values_to_edit_tokens(values: &[Value]) -> Result<Vec<EditWordEntry>, String> {
    values.iter().map(value_to_edit_token).collect()
}

fn value_to_edit_token(value: &Value) -> Result<EditWordEntry, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "edit token is not an object".to_string())?;
    let syl_values = object
        .get("syl")
        .and_then(Value::as_array)
        .ok_or_else(|| "edit token is missing syl[]".to_string())?;

    let syl = syl_values
        .iter()
        .map(value_to_edit_syllable)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(EditWordEntry {
        lang: required_u32(object, "lang")?,
        pe: required_bool(object, "pe")?,
        pos: required_u32(object, "pos")?,
        r32: required_range(object, "r32")?,
        r8: required_range(object, "r8")?,
        s: required_string(object, "s")?,
        syl,
    })
}

fn value_to_edit_syllable(value: &Value) -> Result<EditSyllable, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "edit syllable is not an object".to_string())?;
    let phonemes = object
        .get("p")
        .and_then(Value::as_array)
        .ok_or_else(|| "edit syllable is missing p[]".to_string())?
        .iter()
        .map(value_to_edit_phoneme)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(EditSyllable {
        a: required_u32(object, "a")?,
        i: required_f64(object, "i")?,
        ig: required_bool(object, "ig")?,
        p: phonemes,
        s: required_string(object, "s")?,
        u: required_bool(object, "u")?,
    })
}

fn value_to_edit_phoneme(value: &Value) -> Result<EditPhoneme, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "edit phoneme is not an object".to_string())?;
    Ok(EditPhoneme {
        d: required_f64(object, "d")?,
        dt: object
            .get("dt")
            .and_then(Value::as_i64)
            .map(|value| value as i32),
        s: required_string(object, "s")?,
        t: object.get("t").and_then(Value::as_f64).unwrap_or(0.0),
    })
}
