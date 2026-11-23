//! The change each line item asks for: the course script's change table.

use super::*;

/// What the course's `scripts/defense.py` `change_of` answers for each line.
#[test]
fn the_change_is_the_course_scripts_change() {
    let cases: [(&str, Option<&str>, &str); 15] = [
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
    ];
    for (line, changed, what) in cases {
        let change = change_of(line);
        assert_eq!(change.changed.as_deref(), changed, "{line}");
        assert_eq!(change.what, what, "{line}");
    }
}
