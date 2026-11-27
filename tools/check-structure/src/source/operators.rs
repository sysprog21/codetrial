//! Whether code computes, read from tokens, which the index does not show: it
//! has an arithmetic, comparison or logic operator, invokes a macro, or calls
//! something that does more than pass a value on.

use super::Source;
use super::tokens::{Kind, Token};

/// Keywords after which an operator stands before its operand: `return -x`.
const NOT_OPERANDS: [&str; 30] = [
    "as", "async", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "static", "struct", "trait", "unsafe", "while",
];

/// What passes a value on without computing one: copies it, reaches through
/// what holds it ([`super::HOLDERS`] and atomics), or hands it out as what it
/// dereferences to.
const PASSES: [&str; 32] = [
    "Ok",
    "Some",
    "as_deref",
    "as_deref_mut",
    "as_mut",
    "as_mut_slice",
    "as_path",
    "as_ref",
    "as_slice",
    "as_str",
    "borrow",
    "borrow_mut",
    "clone",
    "cloned",
    "copied",
    "deref",
    "deref_mut",
    "expect",
    "get",
    "get_mut",
    "into_inner",
    "load",
    "lock",
    "read",
    "replace",
    "set",
    "store",
    "swap",
    "take",
    "to_owned",
    "unwrap",
    "write",
];

impl Source {
    /// Whether `body` computes; `passes` says whether a call of a name only
    /// passes a value on, beyond [`PASSES`].
    pub(super) fn computes(&self, body: &[Token], passes: impl Fn(&str) -> bool) -> bool {
        self.has_operator(body) || self.calls(body, |name| !PASSES.contains(&name) && !passes(name))
    }

    /// Whether `body` calls a name that `works` or a value that is no name.
    /// `if (a, b)` and `return (a)` call nothing.
    fn calls(&self, body: &[Token], works: impl Fn(&str) -> bool) -> bool {
        body.windows(2).any(|pair| {
            let (callee, open) = (pair[0], pair[1]);
            self.is(open, "(")
                && match callee.kind {
                    Kind::Ident => {
                        let name = self.text(callee);
                        !NOT_OPERANDS.contains(&name) && works(name)
                    }
                    Kind::Close => true,
                    Kind::Punct => self.is(callee, ">"),
                    Kind::Lifetime | Kind::Literal | Kind::Open => false,
                }
        })
    }

    /// Whether `body` has an arithmetic, comparison or logic operator, or
    /// invokes a macro. `&` and `*` borrow and dereference before an operand,
    /// `|` opens a closure, and `<` after `::` or `as` opens generic arguments.
    fn has_operator(&self, body: &[Token]) -> bool {
        let mut previous: Option<Token> = None;
        let mut at = 0;
        while at < body.len() {
            let token = body[at];
            let binary = previous.is_some_and(|p| self.ends_operand(p));
            match self.text(token) {
                "+" | "-" | "/" | "%" | "^" | "==" | "!=" | "+=" | "-=" | "*=" | "/=" | "%="
                | "^=" | "&=" | "|=" => return true,
                "*" | "&" | "&&" | "<" | ">" | "|" | "||" if binary => return true,
                "!" => return true,
                "|" => {
                    at += 2 + body[at + 1..]
                        .iter()
                        .position(|t| self.is(*t, "|"))
                        .unwrap_or(body.len());
                    previous = None;
                    continue;
                }
                "::" if body.get(at + 1).is_some_and(|t| self.is(*t, "<")) => {
                    at = self.past_angles_in(body, at + 1);
                    previous = None;
                    continue;
                }
                "as" => {
                    at = self.past_cast_type(body, at + 1);
                    previous = body.get(at - 1).copied();
                    continue;
                }
                _ => {}
            }
            previous = Some(token);
            at += 1;
        }
        false
    }

    /// The index past the type a cast at `at` names: `*const u8`, `Box<dyn
    /// Error>`.
    fn past_cast_type(&self, tokens: &[Token], mut at: usize) -> usize {
        let is = |at: usize, texts: &[&str]| {
            tokens
                .get(at)
                .is_some_and(|t| texts.contains(&self.text(*t)))
        };
        while is(at, &["&", "*", "mut", "const", "dyn"])
            || tokens.get(at).is_some_and(|t| t.kind == Kind::Lifetime)
        {
            at += 1;
        }
        while tokens.get(at).is_some_and(|t| t.kind == Kind::Ident) {
            at += 1;
            if is(at, &["<"]) {
                at = self.past_angles_in(tokens, at);
            }
            if !is(at, &["::"]) {
                break;
            }
            at += 1;
        }
        at
    }

    /// Whether an operator after `token` stands between two operands.
    pub(super) fn ends_operand(&self, token: Token) -> bool {
        match token.kind {
            Kind::Literal | Kind::Close => true,
            Kind::Ident => !NOT_OPERANDS.contains(&self.text(token)),
            Kind::Punct => self.is(token, "?"),
            Kind::Lifetime | Kind::Open => false,
        }
    }
}
