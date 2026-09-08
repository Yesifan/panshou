use serde_json::Value;

pub(crate) fn contains_any(text: &str, words: &[&str]) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|word| text.contains(&word.to_lowercase()))
}
pub(crate) fn string<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("")
}
pub(crate) fn number(v: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|k| v.get(k).and_then(Value::as_i64))
        .unwrap_or_default()
}
pub(crate) fn json(body: &[u8]) -> Result<Value, super::CheckError> {
    serde_json::from_slice(body).map_err(|e| super::CheckError::Parse(e.to_string()))
}
