//! What the index leaves out, read from a file's text. The index says which
//! names a file writes and what each resolves to, but not the syntax around
//! them: which names are in `use` declarations, which type an `impl` block
//! is for, which modules are built only for tests, and whether a function's
//! body computes anything.

mod cfg;
mod operators;
mod tokens;
mod uses;

pub use uses::{Access, Plain};

use crate::scip::{Position, Range};
use tokens::{Kind, Token};

/// What holds a value and is peeled off a type to reach it: `Rc<RefCell<T>>`
/// holds a `T`.
const HOLDERS: [&str; 8] = [
    "Option", "Cell", "RefCell", "Rc", "Arc", "Box", "Mutex", "RwLock",
];

/// Types and what a getter of one may hand out instead: what it dereferences
/// to, or the value an atomic holds. `Vec<T>` hands out `[T]` too.
const HANDED_OUT: [(&str, &str); 15] = [
    ("String", "str"),
    ("PathBuf", "Path"),
    ("OsString", "OsStr"),
    ("CString", "CStr"),
    ("AtomicBool", "bool"),
    ("AtomicI8", "i8"),
    ("AtomicI16", "i16"),
    ("AtomicI32", "i32"),
    ("AtomicI64", "i64"),
    ("AtomicIsize", "isize"),
    ("AtomicU8", "u8"),
    ("AtomicU16", "u16"),
    ("AtomicU32", "u32"),
    ("AtomicU64", "u64"),
    ("AtomicUsize", "usize"),
];

/// A file's text, read as tokens.
pub struct Source {
    text: String,
    line_starts: Vec<usize>,
    tokens: Vec<Token>,
}

/// An `impl` block as its text writes it.
#[derive(Debug, PartialEq, Eq)]
pub struct ImplBlock {
    /// From `impl` to the closing brace.
    pub block: Range,
    /// The name of the type it is for, past `&`, `Box<..>` and the like:
    /// `Ready` in `impl Display for &Ready`. `None` for a tuple, slice or
    /// array.
    pub self_type: Option<Range>,
}

/// What a function's text says about it.
pub struct FunctionText {
    /// Its body has an arithmetic, comparison or logic operator, invokes a
    /// macro, or calls something other than one of its own parameters that
    /// does more than pass a value on.
    pub computes: bool,
    /// It takes `self` by value, so it gives or takes fields whole.
    pub whole: bool,
    /// It takes no `&mut self`, what it returns points nowhere, and its body
    /// overwrites nothing and calls no parameter: it can only return copies.
    pub returns_copies: bool,
    /// It takes `&self`, its body overwrites nothing and calls no
    /// parameter, and what it returns lends nothing to write: what it lends
    /// can only be read.
    pub lends_to_read: bool,
    /// What comes before the body, as [`normalized`] tokens.
    signature: Vec<String>,
}

impl FunctionText {
    /// Whether the signature writes one of `types`, each as
    /// [`Source::field_types`] gives it.
    pub fn writes_any(&self, types: &[Vec<String>]) -> bool {
        types.iter().any(|written| {
            !written.is_empty()
                && self
                    .signature
                    .windows(written.len())
                    .any(|window| window == written.as_slice())
        })
    }
}

impl Source {
    pub fn new(text: String) -> Source {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        let tokens = tokens::read(&text);
        Source {
            text,
            line_starts,
            tokens,
        }
    }

    /// The name of the type `written` writes, past what holds it: `Ready` for
    /// `&'a Box<crate::Ready<T>>`.
    pub fn type_name(written: &str) -> Option<String> {
        let source = Source::new(written.to_owned());
        let name = source.name_of(&source.tokens)?;
        Some(source.text(name).to_owned())
    }

    /// The range of the whole text, which the index gives the file's module.
    pub fn whole(&self) -> Range {
        self.range(0, self.text.len())
    }

    /// Where each `use` declaration is: from its `use` to its `;`.
    pub fn use_declarations(&self) -> Vec<Range> {
        let tokens = &self.tokens;
        let mut declarations = Vec::new();
        for (at, token) in tokens.iter().enumerate() {
            let precise_capture = tokens.get(at + 1).is_some_and(|next| self.is(*next, "<"));
            if !self.is(*token, "use") || precise_capture {
                continue;
            }
            let end = tokens[at..]
                .iter()
                .find(|t| self.is(**t, ";"))
                .map_or(self.text.len(), |t| t.end);
            declarations.push(self.range(token.start, end));
        }
        declarations
    }

    /// Every `impl` block, in order.
    pub fn impl_blocks(&self) -> Vec<ImplBlock> {
        let tokens = &self.tokens;
        let mut blocks = Vec::new();
        for (at, token) in tokens.iter().enumerate() {
            if !self.is(*token, "impl") || !self.starts_item(at) {
                continue;
            }
            let Some((self_type, open)) = self.impl_header(at + 1) else {
                continue;
            };
            let close = self.matching(open).unwrap_or(tokens.len() - 1);
            blocks.push(ImplBlock {
                block: self.range(token.start, tokens[close].end),
                self_type: self
                    .name_of(self_type)
                    .map(|name| self.range(name.start, name.end)),
            });
        }
        blocks
    }

    /// What the text of the function written at `item` says about it, if it has
    /// a body.
    pub fn function(&self, item: Range) -> Option<FunctionText> {
        let tokens = self.tokens_in(item)?;
        let last = tokens.len().checked_sub(1)?;
        if !self.is(tokens[last], "}") {
            return None;
        }
        let mut depth = 0;
        let open = (0..=last).rev().find(|&at| {
            match tokens[at].kind {
                Kind::Close => depth += 1,
                Kind::Open => depth -= 1,
                _ => {}
            }
            depth == 0
        })?;
        let signature: Vec<String> = self
            .normalized(&tokens[..open])
            .into_iter()
            .map(|(_, text)| text.to_owned())
            .collect();
        let is_parameter = |name: &str| signature.windows(2).any(|w| w[0] == name && w[1] == ":");
        let body = &tokens[open + 1..last];
        let hands_out = self.hands_out(&tokens[..open], body, is_parameter);
        Some(FunctionText {
            computes: self.computes(body, is_parameter),
            whole: hands_out.whole,
            returns_copies: hands_out.copies,
            lends_to_read: hands_out.to_read,
            signature,
        })
    }

    /// The field written at `field`'s declared type, with `&`, lifetimes, paths
    /// and [`HOLDERS`] peeled off, then what a getter may hand out instead
    /// ([`HANDED_OUT`]), as [`normalized`] tokens:
    /// `[["Vec", "<", "u8", ">"], ["[", "u8", "]"]]` for `pub(crate) bytes:
    /// Rc<Vec<u8>>`.
    pub fn field_types(&self, field: Range) -> Vec<Vec<String>> {
        let Some(tokens) = self.tokens_in(field) else {
            return Vec::new();
        };
        let tokens = self.declared_type(tokens);
        let normalized = self.normalized(tokens);
        let peeled: Vec<String> = peel(&normalized)
            .iter()
            .map(|&(_, text)| text.to_owned())
            .collect();
        let instead = match peeled.as_slice() {
            [vec, open, inner @ .., close] if vec == "Vec" && open == "<" && close == ">" => {
                Some([&["[".to_owned()], inner, &["]".to_owned()]].concat())
            }
            [owned] => HANDED_OUT
                .iter()
                .find(|(from, _)| from == owned)
                .map(|(_, to)| vec![(*to).to_owned()]),
            _ => None,
        };
        std::iter::once(peeled).chain(instead).collect()
    }

    /// The tokens of the type a field's definition declares: past its
    /// attributes, visibility and name.
    fn declared_type<'t>(&self, mut tokens: &'t [Token]) -> &'t [Token] {
        while tokens.first().is_some_and(|t| self.is(*t, "#")) {
            let open = tokens.iter().position(|t| self.is(*t, "["));
            let close = open.and_then(|open| self.matching_in(tokens, open));
            tokens = close.map_or(&[], |close| &tokens[close + 1..]);
        }
        if tokens.first().is_some_and(|t| self.is(*t, "pub")) {
            tokens = &tokens[1..];
            if tokens.first().is_some_and(|t| self.is(*t, "(")) {
                let close = self.matching_in(tokens, 0).unwrap_or(tokens.len() - 1);
                tokens = &tokens[close + 1..];
            }
        }
        if let [name, colon, ..] = tokens
            && name.kind == Kind::Ident
            && self.is(*colon, ":")
        {
            tokens = &tokens[2..];
        }
        tokens
    }

    fn text(&self, token: Token) -> &str {
        &self.text[token.start..token.end]
    }

    fn is(&self, token: Token, text: &str) -> bool {
        self.text(token) == text
    }

    fn position(&self, offset: usize) -> Position {
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(offset - self.line_starts[line]).unwrap_or(u32::MAX),
        }
    }

    fn offset(&self, position: Position) -> Option<usize> {
        let start = *self.line_starts.get(position.line as usize)?;
        Some(start + position.column as usize).filter(|&at| at <= self.text.len())
    }

    fn range(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.position(start),
            end: self.position(end),
        }
    }

    fn tokens_in(&self, range: Range) -> Option<&[Token]> {
        let (start, end) = (self.offset(range.start)?, self.offset(range.end)?);
        let first = self.tokens.partition_point(|t| t.start < start);
        let last = self.tokens.partition_point(|t| t.end <= end);
        self.tokens.get(first..last.max(first))
    }

    /// Whether an item can start at token `at`: after a brace, a `;`, an
    /// attribute, or nothing. This tells an `impl` block from `impl Trait`.
    fn starts_item(&self, at: usize) -> bool {
        let before = self.tokens[..at]
            .iter()
            .rev()
            .find(|t| !self.is(**t, "unsafe") && !self.is(**t, "default"));
        before.is_none_or(|t| ["{", "}", ";", "]"].contains(&self.text(*t)))
    }

    /// The tokens of an `impl` header's self type, and where its block opens.
    fn impl_header(&self, mut at: usize) -> Option<(&[Token], usize)> {
        let tokens = &self.tokens;
        if tokens.get(at).is_some_and(|t| self.is(*t, "<")) {
            at = self.past_angles(at)?;
        }
        let start = at;
        let mut self_type = start;
        let mut end = None;
        let mut depth = 0i32;
        for (index, token) in tokens.iter().enumerate().skip(start) {
            match self.text(*token) {
                "<" | "(" | "[" => depth += 1,
                ">" | ")" | "]" => depth -= 1,
                ";" => return None,
                "{" if depth == 0 => {
                    let type_end = end.unwrap_or(index);
                    return Some((&tokens[self_type..type_end], index));
                }
                "for"
                    if depth == 0
                        && end.is_none()
                        && !tokens.get(index + 1).is_some_and(|t| self.is(*t, "<")) =>
                {
                    self_type = index + 1;
                }
                "where" if depth == 0 => end = Some(index),
                _ => {}
            }
        }
        None
    }

    /// The token naming the type `tokens` write, past what [`peel`] takes off.
    fn name_of(&self, tokens: &[Token]) -> Option<Token> {
        let start = tokens
            .iter()
            .position(|t| !self.is(*t, "const") && !self.is(*t, "!"))?;
        let normalized = self.normalized(&tokens[start..]);
        let &(index, _) = peel(&normalized).first()?;
        let name = tokens[start + index];
        (name.kind == Kind::Ident).then_some(name)
    }

    /// `tokens`' texts with lifetimes and paths left out, each with its index:
    /// `&'a crate::kinds::Ready<'a, T>` is `& Ready < T >`.
    fn normalized<'s>(&'s self, tokens: &[Token]) -> Vec<(usize, &'s str)> {
        let mut out: Vec<(usize, &str)> = Vec::new();
        let mut at = 0;
        while at < tokens.len() {
            let token = tokens[at];
            let next = tokens.get(at + 1);
            if token.kind == Kind::Lifetime {
                at += if next.is_some_and(|t| self.is(*t, ",")) {
                    2
                } else {
                    1
                };
                continue;
            }
            if self.is(token, "::")
                || (token.kind == Kind::Ident && next.is_some_and(|t| self.is(*t, "::")))
            {
                at += 1;
                continue;
            }
            let text = self.text(token);
            match (out.last().map(|&(_, last)| last), text) {
                (Some("<"), ">") => {
                    out.pop();
                }
                (Some(","), ">") => {
                    out.pop();
                    out.push((at, text));
                }
                _ => out.push((at, text)),
            }
            at += 1;
        }
        out
    }

    /// The index past the `>` that closes the `<` at `at`.
    fn past_angles(&self, at: usize) -> Option<usize> {
        let end = self.past_angles_in(&self.tokens, at);
        (end <= self.tokens.len()).then_some(end)
    }

    fn past_angles_in(&self, tokens: &[Token], at: usize) -> usize {
        let mut depth = 0;
        for (index, token) in tokens.iter().enumerate().skip(at) {
            match self.text(*token) {
                "<" => depth += 1,
                ">" => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return index + 1;
            }
        }
        tokens.len()
    }

    /// The bracket that closes the one at `open`.
    fn matching(&self, open: usize) -> Option<usize> {
        self.matching_in(&self.tokens, open)
    }

    fn matching_in(&self, tokens: &[Token], open: usize) -> Option<usize> {
        let mut depth = 0;
        for (index, token) in tokens.iter().enumerate().skip(open) {
            match token.kind {
                Kind::Open => depth += 1,
                Kind::Close => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return Some(index);
            }
        }
        None
    }
}

/// `type` past what holds a value: `&`, `mut`, `dyn`, and [`HOLDERS`] with one
/// argument. `& mut Rc < RefCell < T > >` is `T`.
fn peel<'n, 's>(mut written: &'n [(usize, &'s str)]) -> &'n [(usize, &'s str)] {
    loop {
        match written {
            [(_, "&" | "&&" | "*" | "mut" | "const" | "dyn"), rest @ ..] => written = rest,
            [(_, holder), (_, "<"), inner @ .., (_, ">")]
                if HOLDERS.contains(holder) && is_one_argument(inner) =>
            {
                written = inner
            }
            _ => return written,
        }
    }
}

/// Whether `inner` is one generic argument whose own brackets balance.
fn is_one_argument(inner: &[(usize, &str)]) -> bool {
    let mut depth = 0i32;
    for &(_, text) in inner {
        match text {
            "<" | "(" | "[" => depth += 1,
            ">" | ")" | "]" => depth -= 1,
            "," if depth == 0 => return false,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32, column: u32) -> Position {
        Position { line, column }
    }

    #[test]
    fn the_whole_text_ends_where_rust_analyzer_ends_a_files_module() {
        // What rust-analyzer 1.99.0 writes for these files: it reads `\r\n` as
        // `\n`, and counts a byte order mark's 3 bytes.
        for (text, end) in [
            ("", at(0, 0)),
            ("pub fn d() {}", at(0, 13)),
            ("pub fn a() {}\r\npub fn b() {}\r\n", at(2, 0)),
            ("pub fn e() {}\r\npub fn ff() {}", at(1, 14)),
            ("\u{feff}pub fn c() {}", at(0, 16)),
        ] {
            let whole = Source::new(text.into()).whole();
            assert_eq!((whole.start, whole.end), (at(0, 0), end), "{text:?}");
        }
    }

    #[test]
    fn use_declarations_but_not_precise_captures() {
        let source = Source::new(
            "pub use a::{b,\n c};\nfn f() -> impl Sized + use<> { use d::e; }\n".into(),
        );
        assert_eq!(
            source.use_declarations(),
            [
                Range {
                    start: at(0, 4),
                    end: at(1, 4)
                },
                Range {
                    start: at(2, 31),
                    end: at(2, 40)
                },
            ]
        );
    }

    #[test]
    fn an_impl_blocks_self_type_is_past_references_and_holders() {
        let text = "fn f() -> impl Display { 1 }\n\
                    #[cfg(x)]\n\
                    impl<'a, T: Fn() -> u8> fmt::Display for &'a Box<crate::Ready<T>> where T: for<'b> X<'b> {\n}\n\
                    unsafe impl Send for (u8, u8) {}\n";
        let blocks = Source::new(text.into()).impl_blocks();
        assert_eq!(
            blocks,
            [
                ImplBlock {
                    block: Range {
                        start: at(2, 0),
                        end: at(3, 1)
                    },
                    self_type: Some(Range {
                        start: at(2, 56),
                        end: at(2, 61)
                    }),
                },
                ImplBlock {
                    block: Range {
                        start: at(4, 7),
                        end: at(4, 32)
                    },
                    self_type: None,
                },
            ]
        );
    }

    #[test]
    fn a_getter_computes_nothing_and_writes_its_fields_type() {
        let getter = Source::new(
            "pub fn with<R>(&self, f: impl FnOnce(&Credentials) -> R) -> R {\n    f(&self.credentials.borrow())\n}".into(),
        );
        let text = getter.function(getter.whole()).unwrap();
        assert!(!text.computes);
        let field =
            Source::new("#[allow(x)]\npub(crate) credentials: RefCell<crate::Credentials>".into());
        let types = field.field_types(field.whole());
        assert_eq!(types, [["Credentials"]]);
        assert!(text.writes_any(&types));
    }

    #[test]
    fn operators_macros_and_calls_that_work_compute_but_passing_a_value_on_does_not() {
        let computes = |body: &str| {
            let source = Source::new(format!("fn f(&self, set: u64, each: F) {{ {body} }}"));
            source.function(source.whole()).unwrap().computes
        };
        for quiet in [
            "&mut *self.head",
            "self.x as u64",
            "self.set = set;",
            "&self.items[..]",
            "let items = &self.items;\n items",
            "Some(self.inner.lock().unwrap().clone())",
            "self.held.load(Ordering::Relaxed)",
            "each(&self.items)",
            "if (self.a, self.b).0 { return (self.c); }",
        ] {
            assert!(!computes(quiet), "{quiet}");
        }
        for busy in [
            "self.end - self.start",
            "self.a && self.b",
            "!self.ready",
            "self.x < self.y",
            "self.mask & 1",
            "self.x as u64 * 2",
            "-self.x",
            "format!(\"{}\", self.x)",
            "self.limit.saturating_sub(self.held.load(Relaxed))",
            "self.a.max(self.b)",
            "self.items.iter().map(|x| x.id).collect::<Vec<_>>()",
            "self.items.iter().map(|x| x + 1)",
            "self.parse::<u64>()",
            "(self.hook)()",
        ] {
            assert!(computes(busy), "{busy}");
        }
    }

    #[test]
    fn a_vec_field_may_be_handed_out_as_a_slice_and_an_atomic_as_its_value() {
        let field = Source::new("items: Option<Vec<u32>>".into());
        assert_eq!(
            field.field_types(field.whole()),
            [vec!["Vec", "<", "u32", ">"], vec!["[", "u32", "]"]]
        );
        let field = Source::new("doomed: core::sync::atomic::AtomicBool".into());
        assert_eq!(field.field_types(field.whole()), [["AtomicBool"], ["bool"]]);
    }
}
