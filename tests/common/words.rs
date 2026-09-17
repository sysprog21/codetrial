//! Comparing prose word by word, for the tests that hold interviewer text to
//! what it must not repeat.
//!
//! A file of its own rather than a function in `mod.rs`, because every test
//! that declares `mod common` compiles all of it, and a helper only two of
//! them call is dead code in the rest, which `clippy -D warnings` refuses. The
//! two that need it include this file by `#[path]`.

/// The longest run of consecutive words two texts share, compared as lowercase
/// letters and digits so punctuation cannot hide a copied phrase.
pub fn shared_run(left: &str, right: &str) -> usize {
    let (left, right) = (words(left), words(right));
    let mut longest = 0;
    for start in 0..left.len() {
        for other in 0..right.len() {
            let run = left[start..]
                .iter()
                .zip(&right[other..])
                .take_while(|(a, b)| a == b)
                .count();
            longest = longest.max(run);
        }
    }
    longest
}

/// Lowercase ASCII words, the way `spelled_words` in
/// scripts/problem_bank/rules.py splits them: `minStackCreate` is min, stack,
/// create, and `LRUCache` is lru, cache.
///
/// Delegates so the tests split words the same way the report validator does.
/// Two copies of the rule is how the generator, the prompt tests and the
/// server stop agreeing about what a word is.
pub fn words(text: &str) -> Vec<String> {
    codetrial::agent::spelled_words(text)
}

/// Whether `text` names a published problem by its title: the rule
/// `names_source` in scripts/problem_bank/rules.py applies, so the generator,
/// the prompt tests and the behaviour check agree.
///
/// A title that is a single ordinary word, "Candy" or "Triangle", is exempt,
/// because a scenario uses the word. Any other title counts when a run of
/// consecutive words spells it with the spaces gone: "LRUCache", "lru cache"
/// and "3 Sum" all name their problems, and "those 3 sums" does not.
pub fn names_title(title: &str, text: &str) -> bool {
    codetrial::agent::names_published_problem(title, text)
}
