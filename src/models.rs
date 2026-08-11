use serde::Serialize;
use serde_json::{Map, Value};
use vpsdk::SynthesisParams;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeVoice {
    pub(crate) name: String,
    pub(crate) emotions: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Variant {
    pub(crate) index: usize,
    pub(crate) speed: f64,
    pub(crate) narrator: String,
    pub(crate) params: SynthesisParams,
    pub(crate) emotions: Map<String, Value>,
    pub(crate) duration_scale: f64,
    pub(crate) intonation_scale: f64,
    pub(crate) intonation_offset: f64,
    pub(crate) intonation_contour: f64,
    pub(crate) is_source: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Sbv2Equivalent {
    pub(crate) normalized_text: String,
    pub(crate) phones: Vec<String>,
    pub(crate) tones: Vec<i32>,
    pub(crate) word2ph: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MfaTokenRecord {
    pub(crate) sentence_index: usize,
    pub(crate) token_index: usize,
    pub(crate) surface: String,
    pub(crate) reading: String,
    pub(crate) pause: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MfaUtterance {
    pub(crate) text: String,
    pub(crate) katakana: String,
    pub(crate) tokens: Vec<MfaTokenRecord>,
    pub(crate) warnings: Vec<String>,
    pub(crate) dictionary_words: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FlatVppPhone {
    pub(crate) symbol: String,
    pub(crate) duration_multiplier: f64,
    pub(crate) n: bool,
    pub(crate) dt: i32,
    pub(crate) mora: String,
    pub(crate) mora_index: usize,
    pub(crate) accent: u32,
    pub(crate) intonation: f64,
    pub(crate) token_index: usize,
    pub(crate) token_surface: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FlatRuntimePhone {
    pub(crate) symbol: String,
    pub(crate) duration_sec: f64,
    pub(crate) start_sec: f64,
    pub(crate) end_sec: f64,
    pub(crate) mora: String,
    pub(crate) mora_index: usize,
    pub(crate) accent: u32,
    pub(crate) intonation: f64,
    pub(crate) token_index: usize,
}
