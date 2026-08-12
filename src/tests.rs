use serde_json::{Map, Value, json};
use vpsdk::SynthesisParams;

use super::julius::{build_julius_transcription, resample_for_test};
use super::mfa::build_mfa_utterance_from_sentences;
use super::models::{MfaTokenRecord, MfaUtterance, Variant};
use super::sbv2::{sbv2_from_vpp, symbols_match};
use super::util::hex_sha256;
use super::voicepeak::mutate_token_values;

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
fn builds_julius_hiragana_with_only_real_internal_pauses_as_sp() {
    let token = |surface: &str, reading: &str, pause: bool| MfaTokenRecord {
        sentence_index: 0,
        token_index: 0,
        surface: surface.to_string(),
        reading: reading.to_string(),
        pause,
        warning: None,
    };
    let utterance = MfaUtterance {
        text: "どうすんの、このお店。完ッ全に閑古鳥が鳴いちゃってるじゃない。".to_string(),
        katakana: "ドオスンノ、コノオミセ。カンッゼンニカンコドリガナイチャッテルジャナイ。".to_string(),
        tokens: vec![
            token("どうすんの", "ドオスンノ", false),
            token("、", "", true),
            token("このお店", "コノオミセ", false),
            token("。", "", true),
            token("完", "カン", false),
            token("ッ", "", true),
            token("全に閑古鳥が鳴いちゃってるじゃない", "ゼンニカンコドリガナイチャッテルジャナイ", false),
            token("。", "", true),
        ],
        warnings: Vec::new(),
        dictionary_words: Vec::new(),
    };

    assert_eq!(
        build_julius_transcription(&utterance),
        "どおすんの sp このおみせ sp かんっぜんにかんこどりがないちゃってるじゃない"
    );
}

#[test]
fn julius_resampler_produces_the_expected_16khz_length() {
    let input = vec![0.0; 48_000];
    let output = resample_for_test(&input, 48_000, 16_000);
    assert_eq!(output.len(), 16_000);
}

#[test]
fn computes_sha256_for_manifest() {
    assert_eq!(
        hex_sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
