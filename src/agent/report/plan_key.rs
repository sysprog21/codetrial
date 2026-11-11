//! How a plan's weakness is matched to the improvement it names.

/// A weakness names its improvement up to case, spacing and a final period,
/// which the validator forgives and the sanitizer therefore matches on too.
pub(super) fn plan_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .trim_end()
        .to_lowercase()
}

/// Records what an improvement was replaced with, under its key. A key two
/// improvements share is ambiguous, so it holds nothing and is left for
/// validation to refuse, as the validator's own matching leaves it.
pub(super) fn note_replacement(
    replacements: &mut std::collections::HashMap<String, Option<String>>,
    text: &str,
    kept: String,
) {
    replacements
        .entry(plan_key(text))
        .and_modify(|replacement| *replacement = None)
        .or_insert(Some(kept));
}

#[cfg(test)]
#[path = "../../../tests/unit/report_plan_key.rs"]
mod tests;
