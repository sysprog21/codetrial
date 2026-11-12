use super::*;

const LIMIT: usize = 200;

/// A remote answer is cut to a length a log line can carry and cannot end the
/// line it is quoted in.
#[test]
fn a_remote_body_is_bounded_and_stays_on_one_line() {
    let body = format!("first\nsecond\r\n{}", "x".repeat(1000));
    let quoted = quoted_body(&body, LIMIT);
    assert!(!quoted.contains('\n') && !quoted.contains('\r'), "{quoted}");
    assert!(quoted.starts_with("first\\nsecond\\r\\n"), "{quoted}");
    // 200 characters of the body are kept, 14 of them the text before the x's.
    assert_eq!(quoted.matches('x').count(), LIMIT - 14);
    assert!(quoted.ends_with("..."), "a cut body says so");
    assert_eq!(quoted_body("short", LIMIT), "short");
    // Characters, not bytes: a cut cannot split one.
    let wide = "é".repeat(LIMIT + 5);
    assert_eq!(
        quoted_body(&wide, LIMIT)
            .chars()
            .filter(|c| *c == 'é')
            .count(),
        LIMIT
    );
    // The cut is announced only when something was cut.
    let exact = "y".repeat(LIMIT);
    assert_eq!(quoted_body(&exact, LIMIT), exact);
    assert_eq!(
        quoted_body(&format!("{exact}y"), LIMIT),
        format!("{exact}...")
    );
}

/// The body is read by a person, so a quote in it is left readable.
#[test]
fn quotes_in_a_body_are_left_as_written() {
    let body = r#"{"error":"it's \"bad\""}"#;
    assert_eq!(quoted_body(body, LIMIT), body);
}

/// A line separator or a bidirectional control is not a control character to
/// Rust's `is_control`, but a viewer that honours it splits or reorders the log
/// line the body is quoted into, so it is spelled out too.
#[test]
fn separators_and_bidirectional_controls_are_spelled_out() {
    let body = "a\u{2028}b\u{2029}c\u{202e}d\u{2066}e\u{200f}f\u{feff}g\u{61c}h";
    let quoted = quoted_body(body, LIMIT);
    assert_eq!(
        quoted,
        "a\\u{2028}b\\u{2029}c\\u{202e}d\\u{2066}e\\u{200f}f\\u{feff}g\\u{61c}h"
    );
    // Ordinary text outside those ranges, in any script, is left as written.
    assert_eq!(
        quoted_body("Grüße, κόσμε, Ωmega", LIMIT),
        "Grüße, κόσμε, Ωmega"
    );
}
