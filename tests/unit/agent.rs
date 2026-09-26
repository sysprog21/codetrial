//! The `tests` module of `src/agent.rs`, which declares this file by path.
//! Everything here reaches into `src/agent.rs` through `super`, so private
//! items are in scope.

use super::{
    MIN_WRITTEN_CHARS, RuntimeState, changed_characters, code_written, edited_within,
    uncommented_chars,
};

/// What was typed, the half of `changed_characters` the starter check reads.
fn count(template: &str, code: &str) -> Option<usize> {
    let template = template.chars().collect::<Vec<_>>();
    let code = code.chars().collect::<Vec<_>>();
    changed_characters(&template, &code).map(|(added, _)| added)
}

/// Deletions are counted from the same table, in order, and never cancel what
/// was typed: the tested-snapshot check reads both halves.
#[test]
fn changed_characters_counts_what_was_deleted_too() {
    let changed = |before: &str, code: &str| {
        let before = before.chars().collect::<Vec<_>>();
        let code = code.chars().collect::<Vec<_>>();
        changed_characters(&before, &code)
    };
    assert_eq!(changed("abc", "abc"), Some((0, 0)));
    assert_eq!(changed("abc", ""), Some((0, 3)));
    assert_eq!(changed("abcde", "aXe"), Some((1, 3)));
    assert_eq!(changed("if(x)return;y", "y"), Some((0, 12)));
}

/// Exact counts, not only which side of the threshold they fall on: a table
/// that stopped counting shared characters, or a trim that dropped a differing
/// prefix, still agrees with the threshold on most whole starters.
#[test]
fn changed_characters_counts_what_was_typed_in_order() {
    assert_eq!(count("return0;", "return0;"), Some(0), "nothing typed");
    assert_eq!(
        count("", "abc"),
        Some(3),
        "an empty starter credits every character"
    );
    assert_eq!(
        count("abc", ""),
        Some(0),
        "deleting everything types nothing"
    );

    // A differing first or last character is not part of the shared prefix or
    // suffix the table skips.
    assert_eq!(count("abc", "xbc"), Some(1));
    assert_eq!(count("abc", "abx"), Some(1));
    // After the trim, what is left still shares characters with the starter.
    assert_eq!(count("p1q2r", "p1Xq2Yr"), Some(2));
    assert_eq!(count("abcde", "aXbYcZdWe"), Some(4));
    assert_eq!(count("a1b2c3", "a9b8c7"), Some(3));
    // Deleting a comment is not typing, and does not cancel what was typed.
    assert_eq!(
        count("f(){//Thinkoutloud!return0;}", "f(){returnsqrt(x);}"),
        Some(7)
    );
}

/// Past the table's bound there is no count at all, and no quadratic work: a
/// length difference would read a same-length rewrite as nothing typed.
#[test]
fn changed_characters_stops_counting_past_its_table() {
    assert_eq!(count(&"a".repeat(3000), &"b".repeat(2000)), None);
    assert_eq!(count(&"a".repeat(1000), &"b".repeat(5000)), None);
    // At the bound the count is still exact.
    assert_eq!(count(&"a".repeat(1000), &"b".repeat(4000)), Some(4000));
}

/// The written-code threshold on both sides: four added characters are a
/// keystroke or two, five are the shortest answers the bank's starters take.
#[test]
fn code_written_starts_at_five_added_characters() {
    assert_eq!(MIN_WRITTEN_CHARS, 5);
    let written = |template: &str, code: &str| {
        code_written(&RuntimeState {
            code: code.to_string(),
            code_templates: [("python".to_string(), template.to_string())].into(),
            ..RuntimeState::default()
        })
    };
    assert!(!written("return 0", "return 0"));
    assert!(
        !written("return 0", "return x+yz0"),
        "four characters added in order"
    );
    assert!(
        written("return 0", "return xy+yz0"),
        "five characters added in order"
    );

    // Shorter than its starter, so the length proves nothing and the table
    // decides: the comment went and a real answer came.
    assert!(written("pass # think", "return a*b"));
}

/// A tested snapshot stays current up to four characters either way and no
/// further, and a change in one direction is not excused by staying small in
/// the other. Exact counts on each side of the bound, since the bound is the
/// whole rule.
#[test]
fn edited_within_holds_the_threshold_both_ways() {
    assert_eq!(MIN_WRITTEN_CHARS, 5);
    let before = "abcdefghij";
    assert!(edited_within("python", before, before));
    assert!(
        edited_within("python", before, "abcdefghijKLMN"),
        "four added"
    );
    assert!(
        !edited_within("python", before, "abcdefghijKLMNO"),
        "five added"
    );
    assert!(edited_within("python", before, "abcdef"), "four removed");
    assert!(!edited_within("python", before, "abcde"), "five removed");
    assert!(
        edited_within("python", before, "abcdefWXYZ"),
        "four replaced"
    );
    assert!(
        !edited_within("python", before, "aWbcde"),
        "one added, five removed"
    );
}

/// Comments are dropped by the tab's own syntax, and a comment marker inside a
/// string literal is code. Exact output, so a scanner that dropped too much
/// fails as loudly as one that dropped too little.
#[test]
fn uncommented_chars_drops_comments_and_keeps_literals() {
    let kept = |language: &str, code: &str| {
        uncommented_chars(language, code)
            .into_iter()
            .collect::<String>()
    };
    assert_eq!(kept("python", "x = 1  # O(n) time\ny = 2"), "x=1y=2");
    assert_eq!(kept("python", "s = '#not' # yes"), "s='#not'");
    assert_eq!(kept("python", "s = \"a\\\"#b\" # c"), "s=\"a\\\"#b\"");
    assert_eq!(
        kept("python", "x = 1 // 2"),
        "x=1//2",
        "floor division is code"
    );
    assert_eq!(
        kept("python", "s = \"\"\"a \" b # c\"\"\" # d"),
        "s=\"\"\"a\"b#c\"\"\"",
        "a lone quote does not end a docstring"
    );
    assert_eq!(kept("python", "s = '''it's # x''' # y"), "s='''it's#x'''");
    assert_eq!(
        kept("python", "s = \"\" # c"),
        "s=\"\"",
        "an empty string is not tripled"
    );
    assert_eq!(
        kept("javascript", "s = \"\"\" // x"),
        "s=\"\"\"//x",
        "only Python triples quotes"
    );
    assert_eq!(kept("javascript", "a(); // done\nb();"), "a();b();");
    assert_eq!(
        kept("javascript", "x = a / b;"),
        "x=a/b;",
        "division is code"
    );
    assert_eq!(
        kept("c", "/* a * b / c */ d"),
        "d",
        "a star or slash alone does not close"
    );
    assert_eq!(kept("cpp", "a /* x */ + b /* y"), "a+b");
    assert_eq!(kept("java", "u = \"http://x\"; // y"), "u=\"http://x\";");
    assert_eq!(
        kept("c", "int x = 3 # 1;"),
        "intx=3#1;",
        "a hash is not a C comment"
    );
    assert_eq!(
        kept("javascript", "s = 'open // still string"),
        "s='open//stillstring"
    );
}
