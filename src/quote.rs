//! Quoting what a remote service answered into a message that reaches a log
//! or a form.

/// At most `limit` characters of `text`, with a control character spelled out
/// and the cut marked.
///
/// The body is the other side's to write. A newline in it would end the log
/// line it is quoted in and start one nobody wrote, a wrong host answers with a
/// whole HTML page, and a form shows one line. Quotes are left as they are,
/// since the body is read by a person.
pub(crate) fn quoted_body(text: &str, limit: usize) -> String {
    let mut quoted = String::new();
    for character in text.chars().take(limit) {
        if character.is_control() || splits_or_reorders(character) {
            quoted.extend(character.escape_default());
        } else {
            quoted.push(character);
        }
    }
    if text.chars().count() > limit {
        quoted.push_str("...");
    }
    quoted
}

/// A character a viewer may honour as a line break or a change of reading
/// order, which `is_control` does not count: the line and paragraph separators,
/// the Arabic letter mark, the zero width and mark characters, the
/// bidirectional embeddings, overrides and isolates, and the byte order mark.
fn splits_or_reorders(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

#[cfg(test)]
#[path = "../tests/unit/quote.rs"]
mod tests;
