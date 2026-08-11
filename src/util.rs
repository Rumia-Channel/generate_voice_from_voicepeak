use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub(crate) fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing string field {key}"))
}

pub(crate) fn required_f64(object: &Map<String, Value>, key: &str) -> Result<f64, String> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("missing finite number field {key}"))
}

pub(crate) fn required_u32(object: &Map<String, Value>, key: &str) -> Result<u32, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("missing u32 field {key}"))
}

pub(crate) fn required_bool(object: &Map<String, Value>, key: &str) -> Result<bool, String> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing bool field {key}"))
}

pub(crate) fn required_range(object: &Map<String, Value>, key: &str) -> Result<[u32; 2], String> {
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

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
