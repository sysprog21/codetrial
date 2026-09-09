//! Coercing JSON the way the browser and Python actually behave.
//!
//! The browser has historically sent the same field as a number, a numeric
//! string, and a boolean, and a candidate's failed assertion has to be rendered
//! the way their interpreter would print it. Neither rule is about interviews,
//! which is why they are here rather than in the runtime beside them.

/// Python-compatible coercion of a JSON scalar to a number. The browser has
/// historically sent durations and scores as numbers, numeric strings, and
/// booleans, so both the token endpoint and the agent decode them this way.
pub fn json_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        serde_json::Value::Bool(value) => Some(f64::from(*value)),
        _ => None,
    }
}

pub(crate) fn json_int(value: &serde_json::Value) -> Option<i64> {
    json_number(value).map(|number| number as i64)
}

pub(crate) fn truthy_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .filter(|value| python_truthy(value))
        .map(python_string)
}

pub(crate) fn value_string(value: Option<&serde_json::Value>) -> Option<String> {
    value.map(python_string)
}

pub(crate) fn python_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(number) => number.as_f64() != Some(0.0),
        serde_json::Value::String(text) => !text.is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(fields) => !fields.is_empty(),
    }
}

fn python_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(true) => "True".to_string(),
        serde_json::Value::Bool(false) => "False".to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        serde_json::Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!("'{key}': {}", python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_repr(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("'{text}'"),
        value => python_string(value),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/agent/value.rs"]
mod tests;

/// Model or candidate text cut to a length and stripped of what could forge a
/// line break in a prompt.
///
/// The control filter alone is not the bound. U+2028 and U+2029 are not control
/// characters and `str::lines` does not split on them, so text carrying one
/// survives every line-oriented check this tree makes and still reaches the
/// model as two lines: the thing being defended is what the model sees, not
/// what `str::lines` splits on. That reasoning was written once, for a test
/// run's failure text, and then re-derived three more times without it.
pub(crate) fn bounded_model_text(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|character| {
            !character.is_control() && !matches!(character, '\u{2028}' | '\u{2029}')
        })
        .take(max_chars)
        .collect()
}
