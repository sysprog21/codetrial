//! The `tests` module of `src/agent/value.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/agent/value.rs` through `super`, so it is
//! a unit
//! test and not an integration test: private items are in scope.

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
