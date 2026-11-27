//! Rust's tokens, as far as this tool reads them: identifiers, lifetimes,
//! literals, punctuation and brackets, with comments left out. `<` and `>`
//! are always tokens of their own, so `Vec<Vec<u8>>` closes twice.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An identifier or keyword, raw ones included.
    Ident,
    Lifetime,
    /// A number, string, or character.
    Literal,
    Punct,
    Open,
    Close,
}

/// A token, by its byte offsets in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
}

/// Punctuation of more than one character, longest first.
const PUNCTUATION: [&str; 18] = [
    "...", "..=", "..", "::", "->", "=>", "==", "!=", "&&", "||", "+=", "-=", "*=", "/=", "%=",
    "^=", "&=", "|=",
];

pub fn read(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &text[at..];
        let after_dot = tokens
            .last()
            .is_some_and(|token| &text[token.start..token.end] == ".");
        let (kind, length) = match bytes[at] {
            b if b.is_ascii_whitespace() => (None, 1),
            _ if rest.starts_with("//") => (None, rest.find('\n').unwrap_or(rest.len())),
            _ if rest.starts_with("/*") => (None, block_comment(rest)),
            b'"' => (Some(Kind::Literal), quoted(rest, 0, b'"')),
            b'\'' => character_or_lifetime(rest),
            b'0'..=b'9' => (Some(Kind::Literal), number(rest, after_dot)),
            b'(' | b'[' | b'{' => (Some(Kind::Open), 1),
            b')' | b']' | b'}' => (Some(Kind::Close), 1),
            b if b == b'_' || b.is_ascii_alphabetic() || !b.is_ascii() => word(rest),
            _ => {
                let length = PUNCTUATION
                    .iter()
                    .find(|punctuation| rest.starts_with(*punctuation))
                    .map_or_else(
                        || rest.chars().next().map_or(1, char::len_utf8),
                        |p| p.len(),
                    );
                (Some(Kind::Punct), length)
            }
        };
        let mut end = (at + length.max(1)).min(bytes.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        if let Some(kind) = kind {
            tokens.push(Token {
                kind,
                start: at,
                end,
            });
        }
        at = end;
    }
    tokens
}

/// `/* .. */`, nested ones included.
fn block_comment(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// A literal that opens at `text[open]` with `quote` and ends at the next
/// unescaped one: `"a\"b"`, `'\n'`.
fn quoted(text: &str, open: usize, quote: u8) -> usize {
    let bytes = text.as_bytes();
    let mut at = open + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b if b == quote => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// `r"..."`, `r#"..."#`: `hashes` is how many `#` come before the quote at
/// `open`.
fn raw(text: &str, open: usize, hashes: usize) -> usize {
    let closing = format!("\"{}", "#".repeat(hashes));
    text[open + 1..]
        .find(&closing)
        .map_or(text.len(), |at| open + 1 + at + closing.len())
}

/// `'a'` and `'\''` are characters, `'a` and `'static` lifetimes.
fn character_or_lifetime(text: &str) -> (Option<Kind>, usize) {
    let mut chars = text[1..].chars();
    match (chars.next(), chars.next()) {
        (Some('\\'), _) => (Some(Kind::Literal), quoted(text, 0, b'\'')),
        (Some(c), Some('\'')) => (Some(Kind::Literal), 1 + c.len_utf8() + 1),
        _ => (Some(Kind::Lifetime), 1 + identifier(&text[1..])),
    }
}

/// An identifier, or a literal with a prefix: `b"..."`, `br#"..."#`, `c"..."`,
/// `b'x'`.
fn word(text: &str) -> (Option<Kind>, usize) {
    let length = identifier(text);
    if length == 0 {
        return (
            Some(Kind::Punct),
            text.chars().next().map_or(1, char::len_utf8),
        );
    }
    let bytes = text.as_bytes();
    let next = bytes.get(length).copied();
    match (&text[..length], next) {
        ("b" | "c" | "r" | "br" | "cr", Some(b'"')) if text.as_bytes()[length - 1] == b'r' => {
            (Some(Kind::Literal), raw(text, length, 0))
        }
        ("b" | "c", Some(b'"')) => (Some(Kind::Literal), quoted(text, length, b'"')),
        ("b", Some(b'\'')) => (Some(Kind::Literal), quoted(text, length, b'\'')),
        ("r" | "br" | "cr", Some(b'#')) => {
            let hashes = bytes[length..].iter().take_while(|&&b| b == b'#').count();
            match bytes.get(length + hashes) {
                Some(b'"') => (Some(Kind::Literal), raw(text, length + hashes, hashes)),
                _ if &text[..length] == "r" => (
                    Some(Kind::Ident),
                    length + 1 + identifier(&text[length + 1..]),
                ),
                _ => (Some(Kind::Ident), length),
            }
        }
        _ => (Some(Kind::Ident), length),
    }
}

fn identifier(text: &str) -> usize {
    text.find(|c: char| !(c == '_' || c.is_alphanumeric()))
        .unwrap_or(text.len())
}

/// `12`, `0x1f_u8`, `1.5e3`; after a `.`, only digits: `pair.0.1` is two
/// fields.
fn number(text: &str, after_dot: bool) -> usize {
    let bytes = text.as_bytes();
    let digits = |from: usize, only_digits: bool| {
        from + bytes[from..]
            .iter()
            .take_while(|b| {
                b.is_ascii_digit() || (!only_digits && (b.is_ascii_alphanumeric() || **b == b'_'))
            })
            .count()
    };
    let end = digits(0, after_dot);
    let fraction = !after_dot
        && bytes.get(end) == Some(&b'.')
        && bytes.get(end + 1).is_some_and(u8::is_ascii_digit);
    let end = if fraction {
        digits(end + 1, false)
    } else {
        end
    };

    // A signed exponent: `1e+3` is one number, while `0xe+3` is a sum.
    let hex = text.starts_with("0x") || text.starts_with("0X");
    let exponent = !after_dot
        && !hex
        && matches!(bytes[end - 1], b'e' | b'E')
        && matches!(bytes.get(end), Some(b'+' | b'-'))
        && bytes.get(end + 1).is_some_and(u8::is_ascii_digit);
    if exponent {
        digits(end + 1, false)
    } else {
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<&str> {
        read(text).iter().map(|t| &text[t.start..t.end]).collect()
    }

    #[test]
    fn a_signed_exponent_is_one_number_and_a_hex_sum_is_not() {
        assert_eq!(texts("1e+3 1.5e-3"), ["1e+3", "1.5e-3"]);
        assert_eq!(texts("0xe+3"), ["0xe", "+", "3"]);
    }

    #[test]
    fn comments_go_and_angle_brackets_stay_apart() {
        assert_eq!(
            texts("a /* b /* c */ */ // d\n Vec<Vec<u8>> x>=y"),
            [
                "a", "Vec", "<", "Vec", "<", "u8", ">", ">", "x", ">", "=", "y"
            ]
        );
    }

    #[test]
    fn characters_lifetimes_strings_and_numbers() {
        assert_eq!(
            texts(r##"'a' 'b '\'' "x\"y" b'z' r#"q"# r#use self.0.1 1.5 1..2"##),
            [
                "'a'",
                "'b",
                r"'\''",
                r#""x\"y""#,
                "b'z'",
                "r#\"q\"#",
                "r#use",
                "self",
                ".",
                "0",
                ".",
                "1",
                "1.5",
                "1",
                "..",
                "2"
            ]
        );
    }

    #[test]
    fn longest_punctuation_first() {
        assert_eq!(
            texts("a..=b -> c => d && e |= f ::g"),
            [
                "a", "..=", "b", "->", "c", "=>", "d", "&&", "e", "|=", "f", "::", "g"
            ]
        );
    }
}
