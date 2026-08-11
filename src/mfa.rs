use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::json;
use vpsdk::vpp::{Block, Sentence};

use crate::models::{MfaTokenRecord, MfaUtterance};

pub(crate) fn build_mfa_utterance(block: &Block) -> MfaUtterance {
    build_mfa_utterance_from_sentences(&block.sentence_list, block.joined_text())
}

pub(crate) fn build_mfa_utterance_from_sentences(
    sentences: &[Sentence],
    text: String,
) -> MfaUtterance {
    let mut katakana = String::new();
    let mut tokens = Vec::new();
    let mut warnings = Vec::new();
    let mut dictionary_words = BTreeSet::new();

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
            let surface_is_kana = is_kana_surface(&token.s);
            let surface_is_punctuation = is_mfa_punctuation(&token.s);
            let warning = if reading.is_empty()
                && !token.s.is_empty()
                && !surface_is_kana
                && !surface_is_punctuation
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

            // Keep the VPP sentence structure.  In particular, a standalone `ッ`
            // can be represented by a pause-like VPP token but is still required
            // by Japanese G2P as an orthographic geminate marker.
            let katakana_fragment = if !reading.is_empty() {
                Some(reading.clone())
            } else if surface_is_kana {
                Some(normalize_katakana(&token.s))
            } else if surface_is_punctuation {
                Some(token.s.clone())
            } else {
                None
            };
            if let Some(fragment) = katakana_fragment {
                katakana.push_str(&fragment);
                if !is_mfa_punctuation(&fragment) {
                    dictionary_words.insert(fragment);
                }
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
        katakana,
        tokens,
        warnings,
        dictionary_words: dictionary_words.into_iter().collect(),
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

fn normalize_katakana(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if ('\u{3041}'..='\u{3096}').contains(&character) {
                char::from_u32(character as u32 + 0x60).unwrap_or(character)
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn is_mfa_punctuation(surface: &str) -> bool {
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

pub(crate) fn write_dictionary_artifacts(
    output_dir: &Path,
    words: &BTreeSet<String>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let mfa_dir = output_dir.join("mfa");
    fs::create_dir_all(&mfa_dir)?;
    let words_path = mfa_dir.join("custom_words.txt");
    let dict_path = mfa_dir.join("custom.dict");
    let words_content = if words.is_empty() {
        String::new()
    } else {
        format!("{}\n", words.iter().cloned().collect::<Vec<_>>().join("\n"))
    };
    fs::write(&words_path, words_content)?;
    let _ = fs::remove_file(&dict_path);

    let command = format!(
        "mfa g2p \"{}\" japanese_katakana_mfa \"{}\" --sorted",
        words_path.display(),
        dict_path.display()
    );
    let g2p_status = match Command::new("mfa")
        .args([
            "g2p",
            words_path.to_string_lossy().as_ref(),
            "japanese_katakana_mfa",
            dict_path.to_string_lossy().as_ref(),
            "--sorted",
        ])
        .status()
    {
        Ok(status) if status.success() && dict_path.exists() => {
            json!({
                "status": "generated",
                "words_path": words_path.strip_prefix(output_dir)?.to_string_lossy(),
                "dict_path": dict_path.strip_prefix(output_dir)?.to_string_lossy(),
                "g2p_model": "japanese_katakana_mfa",
                "command": command,
            })
        }
        Ok(status) => json!({
            "status": "failed",
            "exit_code": status.code(),
            "words_path": words_path.strip_prefix(output_dir)?.to_string_lossy(),
            "g2p_model": "japanese_katakana_mfa",
            "command": command,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({
            "status": "not_available",
            "words_path": words_path.strip_prefix(output_dir)?.to_string_lossy(),
            "g2p_model": "japanese_katakana_mfa",
            "command": command,
        }),
        Err(error) => return Err(error.into()),
    };
    Ok(g2p_status)
}
