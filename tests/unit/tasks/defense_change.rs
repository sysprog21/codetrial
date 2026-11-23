//! The change each line item asks for.

use super::*;

/// What the course's `scripts/defense.py` `change_of` answers for each line.
#[test]
fn the_change_is_the_course_scripts_change() {
    let cases: [(&str, Option<&str>, &str); 16] = [
        (
            "when (a && b) { x := 1.U }",
            Some("when (a || b) { x := 1.U }"),
            "`&&` becomes `||`",
        ),
        (
            "io.valid := a || !b",
            Some("io.valid := a && !b"),
            "`||` becomes `&&`",
        ),
        (
            "val hit = tag =/= addr",
            Some("val hit = tag === addr"),
            "`=/=` becomes `===`",
        ),
        (
            "val eq = x === y",
            Some("val eq = x =/= y"),
            "`===` becomes `=/=`",
        ),
        (
            "when (count <= 3.U)",
            Some("when (count < 3.U)"),
            "`<=` becomes `<`",
        ),
        ("when (a < b)", Some("when (a <= b)"), "`<` becomes `<=`"),
        ("x := a << 2", None, "delete the line"),
        (
            "io.out := !io.in",
            Some("io.out := io.in"),
            "the `!` is removed",
        ),
        ("io.out := a != b", None, "delete the line"),
        (
            "val w = 31.U + width",
            Some("val w = 32.U + width"),
            "the constant 31.U becomes 32.U",
        ),
        ("val n = r12.U", None, "delete the line"),
        ("val s = a + b", Some("val s = a - b"), "`+` becomes `-`"),
        ("val y = Cat(a, b)", None, "delete the line"),
        (
            "state := 0.U(4.W)",
            Some("state := 1.U(4.W)"),
            "the constant 0.U becomes 1.U",
        ),
        ("when (x =< y)", None, "delete the line"),
        (
            "val x = 2.5 + y",
            Some("val x = 2.5 - y"),
            "`+` becomes `-`",
        ),
    ];
    for (line, changed, what) in cases {
        let change = change_of(line);
        assert_eq!(change.changed.as_deref(), changed, "{line}");
        assert_eq!(change.what, what, "{line}");
    }
}

/// A literal inside a word, or followed by one, is no constant to change.
#[test]
fn a_constant_stands_as_a_word_of_its_own() {
    for line in ["state := a12.U", "state := 12.Uh"] {
        let change = change_of(line);
        assert_eq!(
            (change.changed, change.what.as_str()),
            (None, "delete the line"),
            "{line}"
        );
    }
    assert_eq!(
        change_of("state := (12.U)").what,
        "the constant 12.U becomes 13.U"
    );
    // Every `<k>.U` becomes `<k+1>.U`, however large, and leading zeros go.
    for (line, changed, what) in [
        (
            "x := 18446744073709551615.U",
            "x := 18446744073709551616.U",
            "the constant 18446744073709551615.U becomes 18446744073709551616.U",
        ),
        (
            "x := 99999999999999999999999.U",
            "x := 100000000000000000000000.U",
            "the constant 99999999999999999999999.U becomes 100000000000000000000000.U",
        ),
        ("x := 0.U", "x := 1.U", "the constant 0.U becomes 1.U"),
        ("x := 007.U", "x := 8.U", "the constant 7.U becomes 8.U"),
        ("x := 000.U", "x := 1.U", "the constant 0.U becomes 1.U"),
        (
            "x := 129.U",
            "x := 130.U",
            "the constant 129.U becomes 130.U",
        ),
    ] {
        let change = change_of(line);
        assert_eq!(change.changed.as_deref(), Some(changed), "{line}");
        assert_eq!(change.what, what, "{line}");
    }
}
