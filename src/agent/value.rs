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
mod tests {
    use super::{python_repr, python_string, python_truthy};
    use serde_json::json;

    /// These render the values a candidate's Python assertion failed on, so an
    /// empty list has to read as `[]` and false, not as `None` or true. Kept
    /// here rather than in `tests/`, which would mean making three internal
    /// helpers public to reach them.
    #[test]
    fn python_coercion_matches_the_interpreter() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!python_truthy(&falsy), "{falsy} should be falsy");
        }
        for truthy in [
            json!(true),
            json!(42.5),
            json!("hello"),
            json!([1]),
            json!({"k": 1}),
        ] {
            assert!(python_truthy(&truthy), "{truthy} should be truthy");
        }

        assert_eq!(python_string(&json!(null)), "None");
        assert_eq!(python_string(&json!(true)), "True");
        assert_eq!(python_string(&json!(false)), "False");
        assert_eq!(python_string(&json!(42.5)), "42.5");
        assert_eq!(python_string(&json!("hello")), "hello");
        assert_eq!(
            python_string(&json!([1, "a", null, true])),
            "[1, 'a', None, True]"
        );

        // The difference that matters: `repr` quotes a string, `str` does not.
        assert_eq!(python_repr(&json!("hello")), "'hello'");
        assert_eq!(python_repr(&json!(42.5)), "42.5");
    }
}
