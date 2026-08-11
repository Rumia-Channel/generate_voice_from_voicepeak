use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io::Write;

use serde_json::json;
use vpsdk::vpp::ProjectFile;
use vpsdk::{EditResponsePayload, PipeSession, PlaybackPayload};

use crate::config::{Config, SPEED_VALUES};
use crate::julius::{FfmpegInfo, convert_wav, prepare_output, sbv2_to_julius};
use crate::mfa::{build_mfa_utterance, write_dictionary_artifacts};
use crate::models::RuntimeVoice;
use crate::output::{SpeedWriter, speed_group_index, write_reject};
use crate::sbv2::{
    build_phone_labels, flatten_runtime_tokens, flatten_vpp_tokens, sbv2_from_vpp, validate_sbv2,
};
use crate::util::hex_sha256;
use crate::voicepeak::{
    build_variant_payload, build_variants, connect_voicepeak, values_to_edit_tokens,
};

fn select_blocks<'a>(
    project: &'a ProjectFile,
    config: &Config,
) -> Result<Vec<(usize, &'a vpsdk::vpp::Block)>, Box<dyn Error>> {
    if let Some(indices) = &config.block_indices {
        return indices
            .iter()
            .map(|&index| {
                project
                    .project
                    .blocks
                    .get(index)
                    .map(|block| (index, block))
                    .ok_or_else(|| {
                        format!(
                            "--blocks index {index} is out of range (blocks: {})",
                            project.project.blocks.len()
                        )
                        .into()
                    })
            })
            .collect();
    }

    let block_count = config
        .max_blocks
        .unwrap_or(project.project.blocks.len())
        .min(project.project.blocks.len());
    Ok(project
        .project
        .blocks
        .iter()
        .take(block_count)
        .enumerate()
        .collect())
}

pub(crate) fn generate_dataset(config: &Config) -> Result<(), Box<dyn Error>> {
    let project = ProjectFile::from_path(&config.vpp_path)?;
    let source_bytes = fs::read(&config.vpp_path)?;
    let source_sha256 = hex_sha256(&source_bytes);
    let ffmpeg = FfmpegInfo::detect();
    if ffmpeg.available {
        println!(
            "julius_ffmpeg=available version={}",
            ffmpeg.version.as_deref().unwrap_or("unknown")
        );
    } else {
        println!(
            "julius_ffmpeg=not_available reason={}",
            ffmpeg.error.as_deref().unwrap_or("unknown")
        );
    }
    let julius_output_root = config.output_dir.join("julius");
    prepare_output(&julius_output_root, &SPEED_VALUES)?;

    let mut speed_writers = SPEED_VALUES
        .iter()
        .copied()
        .map(|speed| SpeedWriter::new(&config.output_dir, speed))
        .collect::<Result<Vec<_>, _>>()?;

    let (mut session, mut _voicepeak) = connect_voicepeak()?;
    let runtime_voices = session
        .get_narrators()?
        .into_iter()
        .map(|voice| RuntimeVoice {
            name: voice.name,
            emotions: voice.emotions.into_iter().map(|(key, _)| key).collect(),
        })
        .collect::<Vec<_>>();

    if runtime_voices.is_empty() {
        return Err("VOICEPEAK returned no narrators".into());
    }

    let selected_blocks = select_blocks(&project, config)?;
    let block_count = selected_blocks.len();
    let mut generated = 0usize;
    let mut failed = 0usize;
    let mut julius_generated = 0usize;
    let mut julius_failed = 0usize;
    let mut julius_phones_generated = 0usize;
    let mut julius_phones_failed = 0usize;
    let mut mfa_records = Vec::new();
    let mut mfa_dictionary_words = BTreeSet::new();
    let mut mfa_pause_token_count = 0usize;
    let mut mfa_warning_count = 0usize;

    for (block_index, block) in selected_blocks {
        let mfa_utterance = build_mfa_utterance(block);
        mfa_dictionary_words.extend(mfa_utterance.dictionary_words.iter().cloned());
        let base_payload = block.to_playback_request();
        let variants = build_variants(
            block_index,
            config.variants_per_block,
            block,
            &runtime_voices,
        );

        for variant in variants {
            let speed_index = speed_group_index(variant.speed)?;
            let payload = build_variant_payload(&base_payload, &variant);
            let sbv2 = sbv2_from_vpp(&payload.text, &payload.tokens)?;
            validate_sbv2(&sbv2)?;
            let sample_id = format!("b{block_index:03}_v{:03}", variant.index);
            let audio_rel_path = format!("wav/{sample_id}.wav");
            let lab_rel_path = format!("wav/{sample_id}.lab");
            let audio_path = speed_writers[speed_index]
                .root
                .join("wav")
                .join(format!("{sample_id}.wav"));
            let speed_directory = format!("speed_{:.3}", variant.speed);
            let julius_phone_rel_path = format!("julius/{speed_directory}/phones/{sample_id}.txt");
            let julius_phone_path = julius_output_root
                .join(&speed_directory)
                .join("phones")
                .join(format!("{sample_id}.txt"));
            let mut julius_phone_error = None;
            let julius_phones = match sbv2_to_julius(&sbv2) {
                Ok(sequence) => {
                    fs::write(&julius_phone_path, format!("{}\n", sequence.line()))?;
                    julius_phones_generated += 1;
                    Some(sequence)
                }
                Err(error) => {
                    julius_phones_failed += 1;
                    julius_phone_error = Some(error.clone());
                    write_reject(
                        &mut speed_writers[speed_index].rejects,
                        &sample_id,
                        "julius_phone_conversion",
                        &error,
                    )?;
                    if config.strict {
                        return Err(error.into());
                    }
                    None
                }
            };

            let audio = match session.synthesize_payload(&payload) {
                Ok(audio) => Ok(audio),
                Err(initial_error) => match connect_voicepeak() {
                    Ok((new_session, new_voicepeak)) => {
                        session = new_session;
                        _voicepeak = new_voicepeak;
                        session.synthesize_payload(&payload).map_err(|retry_error| {
                            format!(
                                "synthesis failed before and after reconnect: initial={initial_error}; retry={retry_error}"
                            )
                        })
                    }
                    Err(reconnect_error) => Err(format!(
                        "synthesis failed and VOICEPEAK reconnect failed: initial={initial_error}; reconnect={reconnect_error}"
                    )),
                },
            };
            let audio = match audio {
                Ok(audio) => audio,
                Err(error) => {
                    if julius_phones.is_some() {
                        let _ = fs::remove_file(&julius_phone_path);
                        julius_phones_generated -= 1;
                    }
                    failed += 1;
                    speed_writers[speed_index].failed += 1;
                    write_reject(
                        &mut speed_writers[speed_index].rejects,
                        &sample_id,
                        "synthesis",
                        &error,
                    )?;
                    if config.strict {
                        return Err(error.into());
                    }
                    continue;
                }
            };
            audio.save_wav(&audio_path)?;
            fs::write(
                audio_path.with_extension("lab"),
                format!("{}\n", mfa_utterance.katakana),
            )?;
            let mut julius_record = if ffmpeg.available {
                let julius_audio_rel_path = format!("julius/{speed_directory}/wav/{sample_id}.wav");
                let julius_lab_rel_path = format!("julius/{speed_directory}/wav/{sample_id}.lab");
                let julius_audio_path = julius_output_root
                    .join(&speed_directory)
                    .join("wav")
                    .join(format!("{sample_id}.wav"));
                let julius_lab_path = julius_audio_path.with_extension("lab");
                let conversion =
                    convert_wav(&ffmpeg, &audio_path, &julius_audio_path).and_then(|_| {
                        fs::write(&julius_lab_path, format!("{}\n", mfa_utterance.katakana))
                            .map_err(|error| format!("write Julius LAB failed: {error}"))
                    });
                match conversion {
                    Ok(()) => {
                        julius_generated += 1;
                        json!({
                            "status": "generated",
                            "audio_path": julius_audio_rel_path,
                            "lab_path": julius_lab_rel_path,
                            "sample_rate": 16_000,
                            "channels": 1,
                            "sample_format": "s16le",
                        })
                    }
                    Err(error) => {
                        julius_failed += 1;
                        failed += 1;
                        speed_writers[speed_index].failed += 1;
                        write_reject(
                            &mut speed_writers[speed_index].rejects,
                            &sample_id,
                            "julius_ffmpeg",
                            &error,
                        )?;
                        if config.strict {
                            return Err(error.into());
                        }
                        json!({
                            "status": "failed",
                            "error": error,
                            "sample_rate": 16_000,
                            "channels": 1,
                            "sample_format": "s16le",
                        })
                    }
                }
            } else {
                json!({
                    "status": "not_available",
                    "error": ffmpeg.error.clone(),
                    "sample_rate": 16_000,
                    "channels": 1,
                    "sample_format": "s16le",
                })
            };
            let julius_object = julius_record
                .as_object_mut()
                .ok_or("Julius metadata must be a JSON object")?;
            if let Some(sequence) = julius_phones.as_ref() {
                julius_object.insert("phone_status".to_string(), json!("generated"));
                julius_object.insert("phones_path".to_string(), json!(julius_phone_rel_path));
                julius_object.insert("phones".to_string(), json!(&sequence.phones));
                julius_object.insert("phone_line".to_string(), json!(sequence.line()));
                julius_object.insert(
                    "lexical_phone_line".to_string(),
                    json!(sequence.lexical_line()),
                );
            } else {
                julius_object.insert("phone_status".to_string(), json!("failed"));
                julius_object.insert("phone_error".to_string(), json!(julius_phone_error));
            }

            let request_value = payload.to_payload();

            let edit_tokens = match values_to_edit_tokens(&payload.tokens) {
                Ok(tokens) => tokens,
                Err(error) => {
                    failed += 1;
                    speed_writers[speed_index].failed += 1;
                    write_reject(
                        &mut speed_writers[speed_index].rejects,
                        &sample_id,
                        "edit_request",
                        &error,
                    )?;
                    if config.strict {
                        return Err(error.into());
                    }
                    Vec::new()
                }
            };

            let runtime_response = edit_response_with_reconnect(
                &mut session,
                &mut _voicepeak,
                EditResponseContext {
                    payload: &payload,
                    edit_tokens: &edit_tokens,
                    config,
                    writer: &mut speed_writers[speed_index],
                    sample_id: &sample_id,
                    failed: &mut failed,
                },
            )?;

            let flat_vpp = flatten_vpp_tokens(&payload.tokens)?;
            let flat_runtime = runtime_response
                .as_ref()
                .map(flatten_runtime_tokens)
                .transpose()?;
            let phone_labels =
                build_phone_labels(&sbv2, &flat_vpp, flat_runtime.as_deref().unwrap_or(&[]));

            let label = json!({
                "schema_version": "vpp-sbv2-compatible-2",
                "id": sample_id,
                "source": {
                    "vpp_path": config.vpp_path,
                    "block_index": block_index,
                    "variant_index": variant.index,
                    "speed": variant.speed,
                    "source_variant": variant.is_source,
                },
                "audio": {
                    "path": audio_rel_path,
                    "sample_rate": audio.sample_rate,
                    "channels": audio.channels,
                    "duration_sec": audio.duration_secs(),
                    "vpp_export_sample_rate": project.project.export.sample_rate,
                },
                "julius": julius_record,
                "synthesis": {
                    "text": payload.text,
                    "narrator": payload.narrator,
                    "params": payload.params,
                    "emotions": payload.emotions,
                    "start_time": payload.start_time,
                    "variation": {
                        "duration_scale": variant.duration_scale,
                        "intonation_scale": variant.intonation_scale,
                        "intonation_offset": variant.intonation_offset,
                        "intonation_contour": variant.intonation_contour,
                    },
                    "vpp_tokens": payload.tokens,
                },
                "sbv2": {
                    "normalized_text": sbv2.normalized_text,
                    "phones": sbv2.phones,
                    "tones": sbv2.tones,
                    "word2ph": sbv2.word2ph,
                },
                "mfa": {
                    "lab_path": lab_rel_path,
                    "katakana": mfa_utterance.katakana,
                    "pause_token_count": mfa_utterance
                        .tokens
                        .iter()
                        .filter(|token| token.pause)
                        .count(),
                    "warnings": mfa_utterance.warnings.clone(),
                },
                "phone_labels": phone_labels,
                "runtime": {
                    "edit_response": runtime_response.is_some(),
                    "status": if runtime_response.is_some() { "ok" } else { "unavailable" },
                },
            });
            writeln!(
                &mut speed_writers[speed_index].labels,
                "{}",
                serde_json::to_string(&label)?
            )?;

            let sbv2_record = json!({
                "id": sample_id,
                "audio": audio_rel_path,
                "normalized_text": sbv2.normalized_text,
                "phones": sbv2.phones,
                "tones": sbv2.tones,
                "word2ph": sbv2.word2ph,
            });
            writeln!(
                &mut speed_writers[speed_index].sbv2_labels,
                "{}",
                serde_json::to_string(&sbv2_record)?
            )?;

            let request_record = json!({
                "id": sample_id,
                "payload": request_value,
            });
            writeln!(
                &mut speed_writers[speed_index].requests,
                "{}",
                serde_json::to_string(&request_record)?
            )?;

            let mfa_pause_count = mfa_utterance
                .tokens
                .iter()
                .filter(|token| token.pause)
                .count();

            mfa_records.push(json!({
                "utterance_id": sample_id,
                "audio": format!("{speed_directory}/{audio_rel_path}"),
                "lab": format!("{speed_directory}/{lab_rel_path}"),
                "speed": variant.speed,
                "text": mfa_utterance.text,
                "katakana": mfa_utterance.katakana,
                "tokens": mfa_utterance.tokens.clone(),
                "warnings": mfa_utterance.warnings.clone(),
            }));
            mfa_pause_token_count += mfa_pause_count;
            mfa_warning_count += mfa_utterance.warnings.len();

            generated += 1;
            speed_writers[speed_index].generated += 1;
            if generated.is_multiple_of(10) {
                println!("generated={generated} failed={failed} latest={sample_id}");
            }
        }
    }

    for writer in &mut speed_writers {
        writer.labels.flush()?;
        writer.sbv2_labels.flush()?;
        writer.requests.flush()?;
        writer.rejects.flush()?;
    }

    let dictionary = write_dictionary_artifacts(&config.output_dir, &mfa_dictionary_words)?;
    let speed_groups = speed_writers
        .iter()
        .map(|writer| {
            json!({
                "speed": writer.speed,
                "directory": format!("speed_{:.3}", writer.speed),
                "generated": writer.generated,
                "failed": writer.failed,
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": "vpp-sbv2-compatible-2",
        "source": {
            "vpp_path": config.vpp_path,
            "vpp_sha256": source_sha256,
            "vpp_version": project.version,
        },
            "blocks_selected": config.block_indices,
        "counts": {
            "blocks_total": project.project.blocks.len(),
            "blocks_processed": block_count,
            "variants_per_block": config.variants_per_block,
            "variants_per_speed": config.variants_per_block / SPEED_VALUES.len(),
            "generated": generated,
            "failed": failed,
            "julius_generated": julius_generated,
            "julius_failed": julius_failed,
            "mfa_pause_tokens": mfa_pause_token_count,
            "mfa_warnings": mfa_warning_count,
            "mfa_dictionary_words": mfa_dictionary_words.len(),
            "julius_phones_generated": julius_phones_generated,
            "julius_phones_failed": julius_phones_failed,
        },
        "speed_groups": speed_groups,
        "julius": {
            "status": if !ffmpeg.available && julius_phones_failed == 0 {
                "not_available"
            } else if julius_failed > 0 || julius_phones_failed > 0 {
                "failed"
            } else {
                "generated"
            },
            "ffmpeg": {
                "command": ffmpeg.command,
                "status": ffmpeg.status(),
                "version": ffmpeg.version,
                "error": ffmpeg.error,
            },
            "audio_pattern": "julius/speed_*/wav/{utterance_id}.wav",
            "lab_pattern": "julius/speed_*/wav/{utterance_id}.lab",
            "phones_pattern": "julius/speed_*/phones/{utterance_id}.txt",
            "phones_format": "one space-separated Julius phone line including silB and silE",
            "sample_rate": 16_000,
            "channels": 1,
            "sample_format": "s16le",
            "generated": julius_generated,
            "failed": julius_failed,
            "phones_generated": julius_phones_generated,
            "phones_failed": julius_phones_failed,
        },
        "mfa": {
            "metadata_path": "metadata.json",
            "lab_pattern": "speed_*/wav/{utterance_id}.lab",
            "lab_content": "one concatenated VPP-derived Katakana string per WAV; no artificial token spaces",
            "tokenization": "MFA Japanese tokenizer",
            "g2p_model": "japanese_katakana_mfa",
            "acoustic_model": "japanese_mfa",
            "dictionary": dictionary,
        },
        "variation_ranges": {
            "duration_multiplier": [0.5, 2.0],
            "ui_intonation_reference": [-3.0, 3.0],
            "speed": SPEED_VALUES,
            "pitch": [-1.0, 1.0],
            "pause": [0.8, 1.2],
            "emotion_weight": [0.0, 1.0],
        },
        "notes": [
            "VPP i is preserved as a raw synthesis value and is not clamped to the UI range.",
            "VPP phoneme d is a synthesis control value; runtime duration comes from EditResponse.",
            "Each speed group contains the same variation slots at an explicit speed value.",
            "SBV2-compatible phones, tones, and word2ph are converted directly from VPP tokens.",
            "The complete playback payload is stored in each speed directory's requests.jsonl for reproducibility.",
            "MFA lab text preserves sentence punctuation and Katakana geminate markers from VPP.",
            "SBV2 guards and pause punctuation are converted to Julius silB, silE, and sp in julius/speed_*/phones/*.txt.",
        ],
    });
    fs::write(
        config.output_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let mfa_metadata = json!({
        "schema_version": "vpp-mfa-katakana-2",
        "source": {
            "vpp_path": config.vpp_path,
            "vpp_sha256": source_sha256,
            "vpp_version": project.version,
        },
        "policy": {
            "reading_source": "VPP sentence-list token syl[].s",
            "lab": "one concatenated Katakana string per WAV, with original punctuation and no inserted spaces",
            "tokenization": "delegated to MFA Japanese tokenizer",
            "g2p_model": "japanese_katakana_mfa",
            "acoustic_model": "japanese_mfa",
            "empty_reading": "Katakana and punctuation surfaces are preserved; other missing readings are warned and omitted",
        },
        "counts": {
            "utterances": mfa_records.len(),
            "pause_tokens": mfa_pause_token_count,
            "warnings": mfa_warning_count,
            "dictionary_words": mfa_dictionary_words.len(),
        },
        "dictionary": dictionary,
        "utterances": mfa_records,
    });
    fs::write(
        config.output_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&mfa_metadata)?,
    )?;

    println!("generated={generated} failed={failed}");
    Ok(())
}

struct EditResponseContext<'a> {
    payload: &'a PlaybackPayload,
    edit_tokens: &'a [vpsdk::EditWordEntry],
    config: &'a Config,
    writer: &'a mut SpeedWriter,
    sample_id: &'a str,
    failed: &'a mut usize,
}

fn edit_response_with_reconnect(
    session: &mut PipeSession,
    voicepeak: &mut Option<vpsdk::ManagedProcess>,
    context: EditResponseContext<'_>,
) -> Result<Option<EditResponsePayload>, Box<dyn Error>> {
    let EditResponseContext {
        payload,
        edit_tokens,
        config,
        writer,
        sample_id,
        failed,
    } = context;
    if edit_tokens.is_empty() {
        return Ok(None);
    }
    match session.edit_response(
        &payload.text,
        &payload.narrator,
        &payload.params,
        &payload.emotions,
        edit_tokens,
    ) {
        Ok(response) => Ok(Some(response)),
        Err(initial_error) => match connect_voicepeak() {
            Ok((new_session, new_voicepeak)) => {
                *session = new_session;
                *voicepeak = new_voicepeak;
                match session.edit_response(
                    &payload.text,
                    &payload.narrator,
                    &payload.params,
                    &payload.emotions,
                    edit_tokens,
                ) {
                    Ok(response) => Ok(Some(response)),
                    Err(retry_error) => {
                        *failed += 1;
                        writer.failed += 1;
                        write_reject(
                            &mut writer.rejects,
                            sample_id,
                            "edit_response",
                            &format!(
                                "edit response failed before and after reconnect: initial={initial_error}; retry={retry_error}"
                            ),
                        )?;
                        if config.strict {
                            Err(retry_error.into())
                        } else {
                            Ok(None)
                        }
                    }
                }
            }
            Err(reconnect_error) => {
                *failed += 1;
                writer.failed += 1;
                write_reject(
                    &mut writer.rejects,
                    sample_id,
                    "edit_response",
                    &format!(
                        "edit response failed and VOICEPEAK reconnect failed: initial={initial_error}; reconnect={reconnect_error}"
                    ),
                )?;
                if config.strict {
                    Err(initial_error.into())
                } else {
                    Ok(None)
                }
            }
        },
    }
}

pub(crate) fn print_plan(project: &ProjectFile, config: &Config) {
    let block_count = config.block_indices.as_ref().map_or_else(
        || {
            config
                .max_blocks
                .unwrap_or(project.project.blocks.len())
                .min(project.project.blocks.len())
        },
        Vec::len,
    );
    println!("VPP: {}", config.vpp_path.display());
    println!("version: {}", project.version);
    println!("blocks: {block_count}/{}", project.project.blocks.len());
    if let Some(indices) = &config.block_indices {
        println!("selected blocks: {indices:?}");
    }
    println!("variants per block: {}", config.variants_per_block);
    println!("speed groups: {:?}", SPEED_VALUES);
    println!(
        "planned samples: {}",
        block_count * config.variants_per_block
    );
    println!("output: {}", config.output_dir.display());
    println!("variation: narrator, emotion, speed, pitch, pause, duration, intonation");
}
