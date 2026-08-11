use std::error::Error;

use serde_json::{Value, json};
use vpsdk::EditResponsePayload;

use crate::models::{FlatRuntimePhone, FlatVppPhone, Sbv2Equivalent};
use crate::util::{required_bool, required_f64, required_string, required_u32};

pub(crate) fn sbv2_from_vpp(text: &str, tokens: &[Value]) -> Result<Sbv2Equivalent, String> {
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

pub(crate) fn flatten_vpp_tokens(values: &[Value]) -> Result<Vec<FlatVppPhone>, String> {
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

pub(crate) fn flatten_runtime_tokens(
    response: &EditResponsePayload,
) -> Result<Vec<FlatRuntimePhone>, String> {
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

pub(crate) fn build_phone_labels(
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

pub(crate) fn symbols_match(sbv2: &str, source: &str) -> bool {
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

pub(crate) fn validate_sbv2(output: &Sbv2Equivalent) -> Result<(), Box<dyn Error>> {
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
