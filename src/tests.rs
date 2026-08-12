use super::julius::{FfmpegInfo, convert_wav, sbv2_to_julius};
use super::mfa::build_mfa_utterance_from_sentences;
use super::models::{Sbv2Equivalent, Variant};
use super::sbv2::{sbv2_from_vpp, symbols_match};
use super::util::hex_sha256;
use super::voicepeak::mutate_token_values;
use serde_json::{Map, json};
use std::fs;
use std::path::{Path, PathBuf};
use vpsdk::SynthesisParams;

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
                "p": [{"s": "e", "d": 1.0}, {"s": "cl", "d": 1.0}]
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
fn converts_sbv2_phones_to_julius_format() {
    let sbv2 = Sbv2Equivalent {
        normalized_text: "えっ、そう。".to_string(),
        phones: vec![
            "_".to_string(),
            "e".to_string(),
            "q".to_string(),
            ",".to_string(),
            "s".to_string(),
            "o".to_string(),
            ".".to_string(),
            "_".to_string(),
        ],
        tones: vec![0; 8],
        word2ph: vec![1; 8],
    };

    let output = sbv2_to_julius(&sbv2).expect("Julius conversion");
    assert_eq!(
        output.phones,
        vec![
            "silB".to_string(),
            "e".to_string(),
            "q".to_string(),
            "sp".to_string(),
            "s".to_string(),
            "o".to_string(),
            "sp".to_string(),
            "silE".to_string(),
        ]
    );
    assert_eq!(output.line(), "silB e q sp s o sp silE");
    assert_eq!(output.lexical_line(), "e q sp s o sp");
}

#[test]
fn converts_jp_g2p_special_phone_aliases_for_julius() {
    let sbv2 = Sbv2Equivalent {
        normalized_text: "ラー".to_string(),
        phones: vec![
            "_".to_string(),
            "r".to_string(),
            "a".to_string(),
            "ー".to_string(),
            "_".to_string(),
        ],
        tones: vec![0; 5],
        word2ph: vec![1; 5],
    };

    let output = sbv2_to_julius(&sbv2).expect("Julius conversion");
    assert_eq!(
        output.phones,
        vec![
            "silB".to_string(),
            "r".to_string(),
            "a".to_string(),
            "a:".to_string(),
            "silE".to_string(),
        ]
    );
    assert_eq!(output.line(), "silB r a a: silE");
}

#[test]
fn converts_small_vowel_and_long_vowel_for_julius() {
    let sbv2 = Sbv2Equivalent {
        normalized_text: "ひぇーん".to_string(),
        phones: vec![
            "_".to_string(),
            "h".to_string(),
            "i".to_string(),
            "ぇ".to_string(),
            "ー".to_string(),
            "N".to_string(),
            "_".to_string(),
        ],
        tones: vec![0; 7],
        word2ph: vec![1; 7],
    };

    let output = sbv2_to_julius(&sbv2).expect("Julius conversion");
    assert_eq!(output.line(), "silB h i e e: N silE");
}

#[test]
fn converts_jp_g2p_ty_for_julius_model() {
    let sbv2 = Sbv2Equivalent {
        normalized_text: "テョ".to_string(),
        phones: vec![
            "_".to_string(),
            "ty".to_string(),
            "o".to_string(),
            "_".to_string(),
        ],
        tones: vec![0; 4],
        word2ph: vec![1; 4],
    };

    let output = sbv2_to_julius(&sbv2).expect("Julius conversion");
    assert_eq!(output.line(), "silB ch o silE");
}

#[test]
fn rejects_sbv2_without_julius_boundary_guards() {
    let sbv2 = Sbv2Equivalent {
        normalized_text: "え".to_string(),
        phones: vec!["e".to_string(), "_".to_string()],
        tones: vec![0; 2],
        word2ph: vec![1; 2],
    };

    let error = sbv2_to_julius(&sbv2).expect_err("missing leading guard");
    assert!(error.contains("start with '_'"));
}

#[test]
fn reconstructs_concatenated_katakana_lab_without_inserted_spaces() {
    let sentence: vpsdk::vpp::Sentence = serde_json::from_value(json!({
        "text": "完ッ全に。",
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
                "s": "に",
                "pos": 4097,
                "lang": 0,
                "pe": false,
                "r8": [9, 12],
                "r32": [3, 4],
                "syl": [{
                    "s": "ニ",
                    "ig": true,
                    "a": 8192,
                    "i": 0.0,
                    "u": false,
                    "p": [{"s": "n", "d": 1.0, "n": false, "dt": 0}]
                }]
            },
            {
                "s": "。",
                "pos": 4106,
                "lang": 0,
                "pe": false,
                "r8": [12, 15],
                "r32": [4, 5],
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
    let utterance = build_mfa_utterance_from_sentences(&[sentence], "完ッ全に。".to_string());

    assert_eq!(utterance.katakana, "カンッゼンニ。");
    assert!(!utterance.katakana.contains(' '));
    assert_eq!(utterance.tokens[1].surface, "ッ");
    assert!(utterance.tokens[1].pause);
    assert!(utterance.warnings.is_empty());
    assert_eq!(utterance.dictionary_words, ["カン", "ゼン", "ッ", "ニ"]);
}

#[test]
fn computes_sha256_for_manifest() {
    assert_eq!(
        hex_sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn converts_wav_to_julius_format_when_ffmpeg_is_available() {
    let ffmpeg = FfmpegInfo::detect();
    if !ffmpeg.available {
        return;
    }

    let root = std::env::temp_dir().join(format!(
        "generate_voice_from_voicepeak-julius-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temporary test directory");
    let source = root.join("source.wav");
    let destination = root.join("julius.wav");
    write_test_wav(&source);

    convert_wav(&ffmpeg, &source, &destination).expect("ffmpeg conversion");
    let bytes = fs::read(&destination).expect("read converted WAV");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1);
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        16_000
    );
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16);

    fs::remove_dir_all(root).expect("remove temporary test directory");
}

#[test]
fn accepts_repeated_vpp_options_with_shared_output() {
    let config = super::config::parse_args_from(vec![
        "--vpp".to_string(),
        "first.vpp".to_string(),
        "--vpp".to_string(),
        "second.vpp".to_string(),
        "dataset".to_string(),
        "--variants".to_string(),
        "10".to_string(),
    ])
    .expect("parse repeated VPP options");

    assert_eq!(config.vpp_paths.len(), 2);
    assert_eq!(config.vpp_paths[0], Path::new("first.vpp"));
    assert_eq!(config.vpp_paths[1], Path::new("second.vpp"));
    assert_eq!(config.output_dir, Path::new("dataset"));
    assert_eq!(config.variants_per_block, 10);
}

#[test]
fn preserves_single_vpp_positional_syntax() {
    let config =
        super::config::parse_args_from(vec!["voicepeak.vpp".to_string(), "dataset".to_string()])
            .expect("parse legacy positional syntax");

    assert_eq!(config.vpp_paths, vec![PathBuf::from("voicepeak.vpp")]);
    assert_eq!(config.output_dir, Path::new("dataset"));
}

fn write_test_wav(path: &Path) {
    let samples = [0i16, 1000, -1000, 500, -500, 250, -250, 0];
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }

    let data_len = data.len() as u32;
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&8_000u32.to_le_bytes());
    wav.extend_from_slice(&(8_000u32 * 2 * 2).to_le_bytes());
    wav.extend_from_slice(&(2u16 * 2).to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&data);
    fs::write(path, wav).expect("write test WAV");
}
