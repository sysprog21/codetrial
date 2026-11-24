//! The change part b of a line item asks for: the first rule of the course's
//! `scripts/defense.py` that applies to the line, or deleting it.

/// What the student is asked to do to a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The line after the change, or `None` when the change deletes it.
    pub changed: Option<String>,
    /// The change in words, as the page and Jim state it.
    pub what: String,
}

/// The change of the first rule that applies to `line`, in the order of the
/// course's table: `&&`, `||`, `=/=`, `===`, `<=`, a lone `<`, a `!` not
/// before `=`, a decimal `<k>.U`, `+`; a line none of them fits is deleted.
pub fn change_of(line: &str) -> Change {
    let chars: Vec<char> = line.chars().collect();
    let swaps: [(&str, &str); 5] = [
        ("&&", "||"),
        ("||", "&&"),
        ("=/=", "==="),
        ("===", "=/="),
        ("<=", "<"),
    ];
    for (from, to) in swaps {
        if let Some(at) = line.find(from) {
            return replaced(line, at, from.len(), to, format!("`{from}` becomes `{to}`"));
        }
    }
    if let Some(at) = position(&chars, lone_less) {
        return replaced(
            line,
            byte_at(line, at),
            1,
            "<=",
            "`<` becomes `<=`".to_owned(),
        );
    }
    if let Some(at) = position(&chars, bang_not_equal) {
        return replaced(
            line,
            byte_at(line, at),
            1,
            "",
            "the `!` is removed".to_owned(),
        );
    }
    if let Some((start, end, k)) = unsigned_literal(&chars) {
        let next = successor(&k);
        return replaced(
            line,
            byte_at(line, start),
            byte_at(line, end) - byte_at(line, start),
            &format!("{next}.U"),
            format!("the constant {k}.U becomes {next}.U"),
        );
    }
    if let Some(at) = line.find('+') {
        return replaced(line, at, 1, "-", "`+` becomes `-`".to_owned());
    }
    Change {
        changed: None,
        what: "delete the line".to_owned(),
    }
}

fn replaced(line: &str, at: usize, len: usize, with: &str, what: String) -> Change {
    let mut changed = String::with_capacity(line.len() + with.len());
    changed.push_str(&line[..at]);
    changed.push_str(with);
    changed.push_str(&line[at + len..]);
    Change {
        changed: Some(changed),
        what,
    }
}

/// The byte offset of the `index`-th character.
fn byte_at(line: &str, index: usize) -> usize {
    line.char_indices()
        .nth(index)
        .map_or(line.len(), |(offset, _)| offset)
}

fn position(chars: &[char], at: fn(&[char], usize) -> bool) -> Option<usize> {
    (0..chars.len()).find(|&index| at(chars, index))
}

/// A `<` with no `<`, `=` or `!` before it and no `<` or `=` after it.
fn lone_less(chars: &[char], index: usize) -> bool {
    chars[index] == '<'
        && !index
            .checked_sub(1)
            .is_some_and(|before| matches!(chars[before], '<' | '=' | '!'))
        && !chars
            .get(index + 1)
            .is_some_and(|after| matches!(after, '<' | '='))
}

/// A `!` that is not the start of `!=`.
fn bang_not_equal(chars: &[char], index: usize) -> bool {
    chars[index] == '!' && chars.get(index + 1) != Some(&'=')
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `digits` plus one, in decimal digits, of any length.
fn successor(digits: &str) -> String {
    let mut out = digits.as_bytes().to_vec();
    // From the last digit, nines become zeros until one digit takes the one.
    let carried = out.iter_mut().rev().all(|digit| {
        let nine = *digit == b'9';
        *digit = if nine { b'0' } else { *digit + 1 };
        nine
    });
    if carried {
        out.insert(0, b'1');
    }
    String::from_utf8(out).unwrap_or_default()
}

/// The first `<digits>.U` standing as a word of its own: (start, end, its
/// value in decimal digits without leading zeros).
fn unsigned_literal(chars: &[char]) -> Option<(usize, usize, String)> {
    // A run of digits starts a word where the character before it is no word
    // character; the digits after its first are inside that run.
    (0..chars.len()).find_map(|index| {
        let starts_word = index == 0 || !word(chars[index - 1]);
        if !(chars[index].is_ascii_digit() && starts_word) {
            return None;
        }
        let digits_end = (index..chars.len())
            .find(|&at| !chars[at].is_ascii_digit())
            .unwrap_or(chars.len());
        let end = digits_end + 2;
        let literal = chars.get(digits_end) == Some(&'.')
            && chars.get(digits_end + 1) == Some(&'U')
            && !chars.get(end).copied().is_some_and(word);
        let digits: String = chars[index..digits_end].iter().collect();
        let value = match digits.trim_start_matches('0') {
            "" => "0".to_owned(),
            value => value.to_owned(),
        };
        literal.then_some((index, end, value))
    })
}
