use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use vpsdk::vpp::ProjectFile;
use vpsdk::{
    EditPhoneme, EditResponsePayload, EditSyllable, EditWordEntry, Installation, ManagedProcess,
    PipeSession, PlaybackPayload, SynthesisParams,
};

const DEFAULT_VPP_PATH: &str = r"voicepeak.vpp";
const DEFAULT_OUTPUT_DIR: &str = "dataset";
const DEFAULT_VARIANTS_PER_BLOCK: usize = 15;
const SPEED_VALUES: [f64; 5] = [0.75, 0.875, 1.0, 1.125, 1.25];

#[derive(Debug, Clone)]
struct Config {
    vpp_path: PathBuf,
    output_dir: PathBuf,
    variants_per_block: usize,
    max_blocks: Option<usize>,
    strict: bool,
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct RuntimeVoice {
    name: String,
    emotions: Vec<String>,
}

#[derive(Debug, Clone)]
struct Variant {
    index: usize,
    speed: f64,
    narrator: String,
    params: SynthesisParams,
    emotions: Map<String, Value>,
    duration_scale: f64,
    intonation_scale: f64,
    intonation_offset: f64,
    intonation_contour: f64,
    is_source: bool,
}
#[derive(Debug, Clone)]
struct Sbv2Equivalent {
    normalized_text: String,
    phones: Vec<String>,
    tones: Vec<i32>,
    word2ph: Vec<usize>,
}
#[derive(Debug, Clone, Serialize)]
struct MfaTokenRecord {
    sentence_index: usize,
    token_index: usize,
    surface: String,
    reading: String,
    pause: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct MfaUtterance {
    text: String,
    lab_tokens: Vec<String>,
    tokens: Vec<MfaTokenRecord>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct FlatVppPhone {
    symbol: String,
    duration_multiplier: f64,
    n: bool,
    dt: i32,
    mora: String,
    mora_index: usize,
    accent: u32,
    intonation: f64,
    token_index: usize,
    token_surface: String,
}
#[derive(Debug, Clone)]
struct FlatRuntimePhone {
    symbol: String,
    duration_sec: f64,
    start_sec: f64,
    end_sec: f64,
    mora: String,
    mora_index: usize,
    accent: u32,
    intonation: f64,
    token_index: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_args()?;
    if config.dry_run {
        let project = ProjectFile::from_path(&config.vpp_path)?;
        print_plan(&project, &config);
        return Ok(());
    }

    generate_dataset(&config)
}

fn parse_args() -> Result<Config, Box<dyn Error>> {
    let mut positional = Vec::new();
    let mut variants_per_block = DEFAULT_VARIANTS_PER_BLOCK;
    let mut max_blocks = None;
    let mut strict = false;
    let mut dry_run = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--variants" => {
                variants_per_block = args
                    .next()
                    .ok_or("--variants requires a positive integer")?
                    .parse()?;
                if variants_per_block == 0 {
                    return Err("--variants must be greater than zero".into());
                }
            }
            "--max-blocks" => {
                let value: usize = args
                    .next()
                    .ok_or("--max-blocks requires a positive integer")?
                    .parse()?;
                if value == 0 {
                    return Err("--max-blocks must be greater than zero".into());
                }
                max_blocks = Some(value);
            }
            "--strict" => strict = true,
            "--dry-run" => dry_run = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {value}").into());
            }
            value => positional.push(PathBuf::from(value)),
        }
    }

    let vpp_path = positional
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_VPP_PATH));
    let output_dir = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_DIR));
    if !variants_per_block.is_multiple_of(SPEED_VALUES.len()) {
        return Err(format!(
            "--variants must be divisible by {} for balanced speed folders",
            SPEED_VALUES.len()
        )
        .into());
    }

    Ok(Config {
        vpp_path,
        output_dir,
        variants_per_block,
        max_blocks,
        strict,
        dry_run,
    })
}

fn print_help() {
    println!(
        "Usage: generate_voice_from_voicepeak [VPP_PATH] [OUTPUT_DIR] [OPTIONS]\n\n\
         Generates SBV2-compatible data converted directly from VPP plus VPP-conditioned VOICEPEAK audio.\n\n\
         Options:\n\
           --variants N       Total variants per block; evenly split by speed (default: {DEFAULT_VARIANTS_PER_BLOCK})\n\
           --max-blocks N      Process only the first N blocks\n\
           --strict            Stop at the first synthesis or alignment error\n\
           --dry-run           Print the generation plan without launching VOICEPEAK\n\
           -h, --help          Show this help\n\n\
         Defaults:\n\
           VPP_PATH    {DEFAULT_VPP_PATH}\n\
           OUTPUT_DIR  {DEFAULT_OUTPUT_DIR}"
    );
}

struct SpeedWriter {
    speed: f64,
    root: PathBuf,
    labels: BufWriter<File>,
    sbv2_labels: BufWriter<File>,
    requests: BufWriter<File>,
    rejects: BufWriter<File>,
    generated: usize,
    failed: usize,
}

impl SpeedWriter {
    fn new(output_dir: &Path, speed: f64) -> Result<Self, Box<dyn Error>> {
        let root = output_dir.join(format!("speed_{speed:.3}"));
        let wav_dir = root.join("wav");
        fs::create_dir_all(&wav_dir)?;
        for entry in fs::read_dir(&wav_dir)? {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("wav" | "lab")
            ) {
                fs::remove_file(path)?;
            }
        }
        Ok(Self {
            speed,
            labels: BufWriter::new(File::create(root.join("labels.jsonl"))?),
            sbv2_labels: BufWriter::new(File::create(root.join("metadata.sbv2.jsonl"))?),
            requests: BufWriter::new(File::create(root.join("requests.jsonl"))?),
            rejects: BufWriter::new(File::create(root.join("rejects.jsonl"))?),
            root,
            generated: 0,
            failed: 0,
        })
    }
}

fn speed_group_index(speed: f64) -> Result<usize, Box<dyn Error>> {
    SPEED_VALUES
        .iter()
        .position(|candidate| (*candidate - speed).abs() < f64::EPSILON)
        .ok_or_else(|| format!("unsupported speed group: {speed}").into())
}

fn generate_dataset(config: &Config) -> Result<(), Box<dyn Error>> {
    let project = ProjectFile::from_path(&config.vpp_path)?;
    let source_bytes = fs::read(&config.vpp_path)?;
    let source_sha256 = hex_sha256(&source_bytes);

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

    let block_count = config
        .max_blocks
        .unwrap_or(project.project.blocks.len())
        .min(project.project.blocks.len());
    let mut generated = 0usize;
    let mut failed = 0usize;
    let mut mfa_records = Vec::new();
    let mut mfa_lab_token_count = 0usize;
    let mut mfa_pause_token_count = 0usize;
    let mut mfa_warning_count = 0usize;

    for (block_index, block) in project.project.blocks.iter().take(block_count).enumerate() {
        let mfa_utterance = build_mfa_utterance(block);
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
            let lab_path = audio_path.with_extension("lab");
            let lab_content = if mfa_utterance.lab_tokens.is_empty() {
                String::new()
            } else {
                format!("{}\n", mfa_utterance.lab_tokens.join(" "))
            };
            fs::write(&lab_path, lab_content)?;

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

            let runtime_response = if edit_tokens.is_empty() {
                None
            } else {
                match session.edit_response(
                    &payload.text,
                    &payload.narrator,
                    &payload.params,
                    &payload.emotions,
                    &edit_tokens,
                ) {
                    Ok(response) => Some(response),
                    Err(initial_error) => match connect_voicepeak() {
                        Ok((new_session, new_voicepeak)) => {
                            session = new_session;
                            _voicepeak = new_voicepeak;
                            match session.edit_response(
                                &payload.text,
                                &payload.narrator,
                                &payload.params,
                                &payload.emotions,
                                &edit_tokens,
                            ) {
                                Ok(response) => Some(response),
                                Err(retry_error) => {
                                    failed += 1;
                                    speed_writers[speed_index].failed += 1;
                                    write_reject(
                                        &mut speed_writers[speed_index].rejects,
                                        &sample_id,
                                        "edit_response",
                                        &format!(
                                            "edit response failed before and after reconnect: initial={initial_error}; retry={retry_error}"
                                        ),
                                    )?;
                                    if config.strict {
                                        return Err(retry_error.into());
                                    }
                                    None
                                }
                            }
                        }
                        Err(reconnect_error) => {
                            failed += 1;
                            speed_writers[speed_index].failed += 1;
                            write_reject(
                                &mut speed_writers[speed_index].rejects,
                                &sample_id,
                                "edit_response",
                                &format!(
                                    "edit response failed and VOICEPEAK reconnect failed: initial={initial_error}; reconnect={reconnect_error}"
                                ),
                            )?;
                            if config.strict {
                                return Err(initial_error.into());
                            }
                            None
                        }
                    },
                }
            };

            let flat_vpp = flatten_vpp_tokens(&payload.tokens)?;
            let flat_runtime = runtime_response
                .as_ref()
                .map(flatten_runtime_tokens)
                .transpose()?;
            let phone_labels =
                build_phone_labels(&sbv2, &flat_vpp, flat_runtime.as_deref().unwrap_or(&[]));

            let label = json!({
                "schema_version": "vpp-sbv2-compatible-1",
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
                    "lab_tokens": mfa_utterance.lab_tokens.clone(),
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
            let speed_directory = format!("speed_{:.3}", variant.speed);
            mfa_records.push(json!({
                "utterance_id": sample_id,
                "audio": format!("{speed_directory}/{audio_rel_path}"),
                "lab": format!("{speed_directory}/{lab_rel_path}"),
                "speed": variant.speed,
                "text": mfa_utterance.text,
                "lab_tokens": mfa_utterance.lab_tokens.clone(),
                "tokens": mfa_utterance.tokens.clone(),
                "warnings": mfa_utterance.warnings.clone(),
            }));
            mfa_lab_token_count += mfa_utterance.lab_tokens.len();
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
        "schema_version": "vpp-sbv2-compatible-1",
        "source": {
            "vpp_path": config.vpp_path,
            "vpp_sha256": source_sha256,
            "vpp_version": project.version,
        },
        "counts": {
            "blocks_total": project.project.blocks.len(),
            "blocks_processed": block_count,
            "variants_per_block": config.variants_per_block,
            "variants_per_speed": config.variants_per_block / SPEED_VALUES.len(),
            "generated": generated,
            "failed": failed,
            "mfa_lab_tokens": mfa_lab_token_count,
            "mfa_pause_tokens": mfa_pause_token_count,
            "mfa_warnings": mfa_warning_count,
        },
        "speed_groups": speed_groups,
        "mfa_lab": {
            "metadata_path": "metadata.json",
            "lab_pattern": "speed_*/wav/{utterance_id}.lab",
            "tokenization": "space-separated confirmed VPP Katakana readings",
            "punctuation": "omitted from lab and retained in metadata when represented as pause",
            "pause": "omitted from lab and retained as pause=true in metadata",
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
            "SBV2-compatible phones, tones, and word2ph are converted directly from VPP tokens; jp_g2p is not used.",
            "The complete playback payload is stored in each speed directory's requests.jsonl for reproducibility.",
            "MFA lab files contain only confirmed VPP Katakana readings; token surface, reading, pause, and warnings are in metadata.json.",
        ],
    });
    fs::write(
        config.output_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let mfa_metadata = json!({
        "schema_version": "vpp-mfa-lab-1",
        "source": {
            "vpp_path": config.vpp_path,
            "vpp_sha256": source_sha256,
            "vpp_version": project.version,
        },
        "policy": {
            "reading_source": "VPP sentence-list token syl[].s",
            "lab_tokens": "space-separated confirmed Katakana readings",
            "punctuation": "pause and punctuation tokens are omitted from .lab",
            "empty_reading": "non-punctuation tokens are omitted from .lab and listed as warnings",
            "phoneme_source": "VPP p[] is used only to identify pause tokens",
        },
        "counts": {
            "utterances": mfa_records.len(),
            "lab_tokens": mfa_lab_token_count,
            "pause_tokens": mfa_pause_token_count,
            "warnings": mfa_warning_count,
        },
        "utterances": mfa_records,
    });
    fs::write(
        config.output_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&mfa_metadata)?,
    )?;

    println!("generated={generated} failed={failed}");
    Ok(())
}

fn connect_voicepeak() -> Result<(PipeSession, Option<ManagedProcess>), Box<dyn Error>> {
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

fn print_plan(project: &ProjectFile, config: &Config) {
    let block_count = config
        .max_blocks
        .unwrap_or(project.project.blocks.len())
        .min(project.project.blocks.len());
    println!("VPP: {}", config.vpp_path.display());
    println!("version: {}", project.version);
    println!("blocks: {block_count}/{}", project.project.blocks.len());
    println!("variants per block: {}", config.variants_per_block);
    println!("speed groups: {:?}", SPEED_VALUES);
    println!(
        "planned samples: {}",
        block_count * config.variants_per_block
    );
    println!("output: {}", config.output_dir.display());
    println!("variation: narrator, emotion, speed, pitch, pause, duration, intonation");
}

fn build_mfa_utterance(block: &vpsdk::vpp::Block) -> MfaUtterance {
    build_mfa_utterance_from_sentences(&block.sentence_list, block.joined_text())
}

fn build_mfa_utterance_from_sentences(
    sentences: &[vpsdk::vpp::Sentence],
    text: String,
) -> MfaUtterance {
    let mut lab_tokens = Vec::new();
    let mut tokens = Vec::new();
    let mut warnings = Vec::new();

    for (sentence_index, sentence) in sentences.iter().enumerate() {
        for (token_index, token) in sentence.tokens.iter().enumerate() {
            let reading = token
                .syl
                .iter()
                .filter(|syllable| !syllable.s.is_empty())
                .map(|syllable| syllable.s.as_str())
                .collect::<String>();
            let phones = token
                .syl
                .iter()
                .flat_map(|syllable| syllable.p.iter())
                .map(|phoneme| phoneme.s.as_str())
                .collect::<Vec<_>>();
            let pause = !phones.is_empty() && phones.iter().all(|phone| *phone == "pau");
            let warning = if reading.is_empty()
                && !token.s.is_empty()
                && !is_mfa_punctuation(&token.s)
            {
                Some(format!(
                    "sentence={sentence_index} token={token_index} surface={:?}: empty VPP reading omitted from lab",
                    token.s
                ))
            } else {
                None
            };
            if let Some(warning) = &warning {
                warnings.push(warning.clone());
            }
            if !pause && !reading.is_empty() {
                lab_tokens.push(reading.clone());
            }
            tokens.push(MfaTokenRecord {
                sentence_index,
                token_index,
                surface: token.s.clone(),
                reading,
                pause,
                warning,
            });
        }
    }

    MfaUtterance {
        text,
        lab_tokens,
        tokens,
        warnings,
    }
}

fn is_mfa_punctuation(surface: &str) -> bool {
    !surface.is_empty()
        && surface.chars().all(|character| {
            matches!(
                character,
                '。' | '、'
                    | '，'
                    | ','
                    | '！'
                    | '!'
                    | '？'
                    | '?'
                    | '・'
                    | '：'
                    | ':'
                    | '；'
                    | ';'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '（'
                    | '）'
                    | '('
                    | ')'
                    | '…'
                    | '.'
            )
        })
}

fn build_variants(
    block_index: usize,
    count: usize,
    block: &vpsdk::vpp::Block,
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

fn build_variant_payload(base: &PlaybackPayload, variant: &Variant) -> PlaybackPayload {
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

fn mutate_token_values(tokens: &mut [Value], variant: &Variant) {
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

fn values_to_edit_tokens(values: &[Value]) -> Result<Vec<EditWordEntry>, String> {
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

fn sbv2_from_vpp(text: &str, tokens: &[Value]) -> Result<Sbv2Equivalent, String> {
    let mut normalized_text = String::new();
    let mut inner_word2ph = Vec::new();
    let mut pending_phones = 0usize;

    for token in tokens {
        let object = token
            .as_object()
            .ok_or_else(|| "token is not an object".to_string())?;
        let surface = required_string(object, "s")?;
        let normalized_surface = normalize_sbv2_surface(&surface);
        let phone_count = object
            .get("syl")
            .and_then(Value::as_array)
            .ok_or_else(|| "token is missing syl[]".to_string())?
            .iter()
            .map(|syllable| {
                syllable
                    .get("p")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            })
            .sum::<usize>();
        normalized_text.push_str(&normalized_surface);
        let char_count = normalized_surface.chars().count();
        if char_count == 0 {
            pending_phones += phone_count;
        } else {
            inner_word2ph.extend(distribute_phone_counts(phone_count, char_count));
        }
    }

    if normalized_text.is_empty() {
        normalized_text = normalize_sbv2_surface(text);
        inner_word2ph = distribute_phone_counts(
            flatten_vpp_tokens(tokens)?.len(),
            normalized_text.chars().count(),
        );
    } else if pending_phones != 0 {
        if let Some(first) = inner_word2ph.first_mut() {
            *first += pending_phones;
        } else {
            inner_word2ph.push(pending_phones);
        }
    }

    let flat_vpp = flatten_vpp_tokens(tokens)?;
    let mut phones = Vec::with_capacity(flat_vpp.len() + 2);
    let mut tones = Vec::with_capacity(flat_vpp.len() + 2);
    phones.push("_".to_string());
    tones.push(0);
    for source in &flat_vpp {
        phones.push(vpp_phone_to_sbv2(source));
        tones.push(if source.accent == 8193 { 1 } else { 0 });
    }
    phones.push("_".to_string());
    tones.push(0);

    if inner_word2ph.iter().sum::<usize>() != flat_vpp.len() {
        return Err(format!(
            "VPP-to-SBV2 word2ph mismatch: chars={}, phones={}",
            inner_word2ph.len(),
            flat_vpp.len()
        ));
    }
    let mut word2ph = Vec::with_capacity(inner_word2ph.len() + 2);
    word2ph.push(1);
    word2ph.extend(inner_word2ph);
    word2ph.push(1);

    Ok(Sbv2Equivalent {
        normalized_text,
        phones,
        tones,
        word2ph,
    })
}

fn distribute_phone_counts(total: usize, slots: usize) -> Vec<usize> {
    if slots == 0 {
        return Vec::new();
    }
    let mut counts = vec![0; slots];
    for index in 0..total {
        counts[index % slots] += 1;
    }
    counts
}

fn normalize_sbv2_surface(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '。' => '.',
            '、' => ',',
            '！' => '!',
            '？' => '?',
            '：' => ':',
            '；' => ';',
            '（' => '(',
            '）' => ')',
            '「' | '」' => '"',
            '　' => ' ',
            '〜' | '～' => '~',
            other => other,
        })
        .collect()
}

fn vpp_phone_to_sbv2(source: &FlatVppPhone) -> String {
    if source.symbol == "cl" {
        return "q".to_string();
    }
    if source.symbol == "pau" {
        return normalize_sbv2_surface(&source.token_surface)
            .chars()
            .next()
            .map_or_else(|| "pau".to_string(), |character| character.to_string());
    }
    source.symbol.clone()
}

fn flatten_vpp_tokens(values: &[Value]) -> Result<Vec<FlatVppPhone>, String> {
    let mut phones = Vec::new();
    for (token_index, value) in values.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| "token is not an object".to_string())?;
        let token_surface = required_string(object, "s")?;
        let syllables = object
            .get("syl")
            .and_then(Value::as_array)
            .ok_or_else(|| "token is missing syl[]".to_string())?;
        for (mora_index, syllable) in syllables.iter().enumerate() {
            let syllable_object = syllable
                .as_object()
                .ok_or_else(|| "syllable is not an object".to_string())?;
            let mora = required_string(syllable_object, "s")?;
            let accent = required_u32(syllable_object, "a")?;
            let intonation = required_f64(syllable_object, "i")?;
            let phonemes = syllable_object
                .get("p")
                .and_then(Value::as_array)
                .ok_or_else(|| "syllable is missing p[]".to_string())?;
            for phoneme in phonemes {
                let phoneme_object = phoneme
                    .as_object()
                    .ok_or_else(|| "phoneme is not an object".to_string())?;
                phones.push(FlatVppPhone {
                    symbol: required_string(phoneme_object, "s")?,
                    duration_multiplier: required_f64(phoneme_object, "d")?,
                    n: required_bool(phoneme_object, "n").unwrap_or(false),
                    dt: phoneme_object
                        .get("dt")
                        .and_then(Value::as_i64)
                        .unwrap_or(0) as i32,
                    mora: mora.clone(),
                    mora_index,
                    accent,
                    intonation,
                    token_index,
                    token_surface: token_surface.clone(),
                });
            }
        }
    }
    Ok(phones)
}

fn flatten_runtime_tokens(response: &EditResponsePayload) -> Result<Vec<FlatRuntimePhone>, String> {
    let mut phones = Vec::new();
    for (token_index, token) in response.tokens.iter().enumerate() {
        for (mora_index, syllable) in token.syl.iter().enumerate() {
            for phoneme in &syllable.p {
                phones.push(FlatRuntimePhone {
                    symbol: phoneme.s.clone(),
                    duration_sec: phoneme.d,
                    start_sec: phoneme.t,
                    end_sec: phoneme.t + phoneme.d,
                    mora: syllable.s.clone(),
                    mora_index,
                    accent: syllable.a,
                    intonation: syllable.i,
                    token_index,
                });
            }
        }
    }
    Ok(phones)
}

fn build_phone_labels(
    sbv2: &Sbv2Equivalent,
    vpp: &[FlatVppPhone],
    runtime: &[FlatRuntimePhone],
) -> Vec<Value> {
    let mut vpp_index = 0usize;
    let mut runtime_index = 0usize;
    let mut labels = Vec::with_capacity(sbv2.phones.len());

    for (phone_index, phone) in sbv2.phones.iter().enumerate() {
        let is_guard = phone == "_" && (phone_index == 0 || phone_index + 1 == sbv2.phones.len());
        let mut label = json!({
            "phone_index": phone_index,
            "phone": phone,
            "tone": sbv2.tones[phone_index],
            "loss_mask": if is_guard { 0 } else { 1 },
            "alignment_status": if is_guard { "guard" } else { "unmatched" },
        });
        if is_guard {
            labels.push(label);
            continue;
        }

        if let Some(index) = find_matching_vpp(phone, vpp, vpp_index) {
            let source = &vpp[index];
            label["vpp"] = json!({
                "symbol": source.symbol,
                "duration_multiplier": source.duration_multiplier,
                "n": source.n,
                "dt": source.dt,
                "mora": source.mora,
                "mora_index": source.mora_index,
                "accent": source.accent,
                "intonation_raw": source.intonation,
                "token_surface": source.token_surface,
                "token_index": source.token_index,
            });
            label["alignment_status"] = Value::from(if source.symbol == "cl" && phone == "q" {
                "cl_to_q"
            } else if source.symbol == "pau" {
                "pause_mapping"
            } else {
                "exact"
            });
            vpp_index = index + 1;
        }

        if let Some(index) = find_matching_runtime(phone, runtime, runtime_index) {
            let source = &runtime[index];
            label["runtime"] = json!({
                "symbol": source.symbol,
                "duration_sec": source.duration_sec,
                "start_sec": source.start_sec,
                "end_sec": source.end_sec,
                "mora": source.mora,
                "mora_index": source.mora_index,
                "accent": source.accent,
                "intonation_absolute": source.intonation,
                "token_index": source.token_index,
            });
            runtime_index = index + 1;
        }

        labels.push(label);
    }
    labels
}

fn find_matching_vpp(phone: &str, phones: &[FlatVppPhone], start: usize) -> Option<usize> {
    phones
        .iter()
        .enumerate()
        .skip(start)
        .take(8)
        .find(|(_, candidate)| symbols_match(phone, &vpp_phone_to_sbv2(candidate)))
        .map(|(index, _)| index)
}

fn find_matching_runtime(phone: &str, phones: &[FlatRuntimePhone], start: usize) -> Option<usize> {
    phones
        .iter()
        .enumerate()
        .skip(start)
        .take(8)
        .find(|(_, candidate)| symbols_match(phone, &candidate.symbol))
        .map(|(index, _)| index)
}

fn symbols_match(sbv2: &str, source: &str) -> bool {
    if source == "cl" {
        return sbv2 == "q";
    }
    if source == "pau" {
        return matches!(
            sbv2,
            "," | "." | "、" | "。" | "!" | "?" | "！" | "？" | "pau"
        );
    }
    sbv2 == source
}

fn validate_sbv2(output: &Sbv2Equivalent) -> Result<(), Box<dyn Error>> {
    if output.phones.len() != output.tones.len() {
        return Err("SBV2 phones/tones length mismatch".into());
    }
    if output.word2ph.iter().sum::<usize>() != output.phones.len() {
        return Err("SBV2 word2ph sum mismatch".into());
    }
    if output.word2ph.len() != output.normalized_text.chars().count() + 2 {
        return Err("SBV2 word2ph/text length mismatch".into());
    }
    Ok(())
}

fn write_reject(
    writer: &mut BufWriter<File>,
    id: &str,
    stage: &str,
    error: &str,
) -> Result<(), Box<dyn Error>> {
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&json!({
            "id": id,
            "stage": stage,
            "error": error,
        }))?
    )?;
    eprintln!("skip {id}: {stage}: {error}");
    Ok(())
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing string field {key}"))
}

fn required_f64(object: &Map<String, Value>, key: &str) -> Result<f64, String> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("missing finite number field {key}"))
}

fn required_u32(object: &Map<String, Value>, key: &str) -> Result<u32, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("missing u32 field {key}"))
}

fn required_bool(object: &Map<String, Value>, key: &str) -> Result<bool, String> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing bool field {key}"))
}

fn required_range(object: &Map<String, Value>, key: &str) -> Result<[u32; 2], String> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing range field {key}"))?;
    if values.len() != 2 {
        return Err(format!("range field {key} must contain two values"));
    }
    let start = values[0]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("invalid range start in {key}"))?;
    let end = values[1]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("invalid range end in {key}"))?;
    Ok([start, end])
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_voicepeak_phone_symbols_to_sbv2() {
        assert!(symbols_match("q", "cl"));
        assert!(symbols_match(".", "pau"));
        assert!(symbols_match("sh", "sh"));
        assert!(!symbols_match("q", "k"));
    }

    #[test]
    fn applies_duration_and_intonation_variation_without_leaving_duration_range() {
        let mut tokens = vec![json!({
            "syl": [{
                "s": "カ",
                "a": 8193,
                "i": 0.5,
                "p": [{"s": "k", "d": 1.0}, {"s": "a", "d": 1.0}]
            }]
        })];
        let variant = Variant {
            index: 1,
            speed: 1.0,
            narrator: "宮舞モカ".to_string(),
            params: SynthesisParams::default(),
            emotions: Map::new(),
            duration_scale: 2.0,
            intonation_scale: 1.25,
            intonation_offset: 1.0,
            intonation_contour: 0.75,
            is_source: false,
        };

        mutate_token_values(&mut tokens, &variant);

        assert_eq!(tokens[0]["syl"][0]["p"][0]["d"], json!(2.0));
        assert_eq!(tokens[0]["syl"][0]["p"][1]["d"], json!(2.0));
        assert_eq!(tokens[0]["syl"][0]["i"], json!(1.625));
    }

    #[test]
    fn converts_vpp_tokens_to_sbv2_compatible_data() {
        let tokens = vec![
            json!({
                "s": "えっ",
                "syl": [{
                    "s": "エ",
                    "a": 8192,
                    "i": 0.0,
                    "p": [
                        {"s": "e", "d": 1.0},
                        {"s": "cl", "d": 1.0}
                    ]
                }]
            }),
            json!({
                "s": "。",
                "syl": [{
                    "s": "",
                    "a": 4096,
                    "i": 0.0,
                    "p": [{"s": "pau", "d": 1.0}]
                }]
            }),
        ];
        let output = sbv2_from_vpp("えっ。", &tokens).expect("VPP conversion");
        assert_eq!(output.normalized_text, "えっ.");
        assert_eq!(output.phones, vec!["_", "e", "q", ".", "_"]);
        assert_eq!(output.tones, vec![0, 0, 0, 0, 0]);
        assert_eq!(output.word2ph.iter().sum::<usize>(), output.phones.len());
        assert_eq!(
            output.word2ph.len(),
            output.normalized_text.chars().count() + 2
        );
    }

    #[test]
    fn builds_mfa_lab_from_vpp_readings_and_warns_on_nonpunctuation_pause() {
        let sentence: vpsdk::vpp::Sentence = serde_json::from_value(json!({
            "text": "完ッ全。",
            "has-eos": true,
            "tokens": [
                {
                    "s": "完",
                    "pos": 4097,
                    "lang": 0,
                    "pe": false,
                    "r8": [0, 3],
                    "r32": [0, 1],
                    "syl": [{
                        "s": "カン",
                        "ig": true,
                        "a": 8192,
                        "i": 0.0,
                        "u": false,
                        "p": [{"s": "k", "d": 1.0, "n": false, "dt": 0}]
                    }]
                },
                {
                    "s": "ッ",
                    "pos": 4097,
                    "lang": 0,
                    "pe": false,
                    "r8": [3, 6],
                    "r32": [1, 2],
                    "syl": [{
                        "s": "",
                        "ig": false,
                        "a": 4096,
                        "i": 0.0,
                        "u": false,
                        "p": [{"s": "pau", "d": 1.0, "n": false, "dt": 0}]
                    }]
                },
                {
                    "s": "全",
                    "pos": 4097,
                    "lang": 0,
                    "pe": false,
                    "r8": [6, 9],
                    "r32": [2, 3],
                    "syl": [{
                        "s": "ゼン",
                        "ig": true,
                        "a": 8193,
                        "i": 0.0,
                        "u": false,
                        "p": [{"s": "z", "d": 1.0, "n": false, "dt": 0}]
                    }]
                },
                {
                    "s": "。",
                    "pos": 4106,
                    "lang": 0,
                    "pe": false,
                    "r8": [9, 12],
                    "r32": [3, 4],
                    "syl": [{
                        "s": "",
                        "ig": false,
                        "a": 4096,
                        "i": 0.0,
                        "u": false,
                        "p": [{"s": "pau", "d": 1.0, "n": false, "dt": 0}]
                    }]
                }
            ]
        }))
        .expect("VPP sentence");
        let utterance = build_mfa_utterance_from_sentences(&[sentence], "完ッ全。".to_string());

        assert_eq!(utterance.lab_tokens, ["カン", "ゼン"]);
        assert_eq!(utterance.tokens[1].surface, "ッ");
        assert!(utterance.tokens[1].pause);
        assert_eq!(utterance.warnings.len(), 1);
        assert!(utterance.warnings[0].contains("surface=\"ッ\""));
    }
    #[test]
    fn computes_sha256_for_manifest() {
        assert_eq!(
            hex_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
