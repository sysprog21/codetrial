//! What code does with a field it names, read from the tokens around the
//! name, and whether a field's declared type is plain data.

use super::Source;
use super::tokens::{Kind, Token};
use crate::scip::Range;

/// What code does with a field it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Takes a copy of its value, or gives or takes the value whole:
    /// `queue.head + 1`, `queue.name.clone()`, `Queue { head, .. }`.
    Copy,
    /// Borrows it to read: `&queue.items`, `queue.name.as_deref()`.
    Borrow,
    /// Calls a method on it: `queue.items.len()`.
    Call,
    /// Writes it, borrows it to write, or indexes it: `queue.head = 0`,
    /// `&mut queue.items`, `queue.items[0]`.
    Reach,
}

/// What a field's declared type holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plain {
    /// A number, `bool` or `char`: nothing can be done to it but read or
    /// overwrite it.
    Scalar,
    /// Numbers, text and bytes, in a `Vec`, `Option`, `Box`, array, slice or
    /// tuple: nothing is inside but its value.
    Data,
    /// A marker, `PhantomData` or `PhantomPinned`: nothing at all.
    Nothing,
    /// Anything else.
    No,
}

/// Methods that hand out a copy of the value they are called on. `get` does
/// when it is given nothing, as a `Cell`'s is.
const COPIES_OUT: [&str; 4] = ["clone", "get", "load", "to_owned"];

/// Methods that borrow the value they are called on to read.
const BORROWS: [&str; 8] = [
    "as_bytes", "as_deref", "as_path", "as_ref", "as_slice", "as_str", "borrow", "deref",
];

/// Calls that overwrite what they are called on through a shared reference.
const OVERWRITES: [&str; 5] = ["replace", "set", "store", "swap", "take"];

const SCALARS: [&str; 16] = [
    "bool", "char", "f32", "f64", "i128", "i16", "i32", "i64", "i8", "isize", "u128", "u16", "u32",
    "u64", "u8", "usize",
];

const TEXT: [&str; 8] = [
    "CStr", "CString", "OsStr", "OsString", "Path", "PathBuf", "String", "str",
];

/// What holds plain data and is plain data with it.
const HOLDS_DATA: [&str; 3] = ["Box", "Option", "Vec"];

/// Types that hold nothing and only tell the compiler something.
const MARKERS: [&str; 2] = ["PhantomData", "PhantomPinned"];

/// What a function's signature and body say about what it hands out.
pub(super) struct HandsOut {
    /// It takes `self` by value: it gives or takes fields whole, as a
    /// struct literal or a pattern does.
    pub whole: bool,
    /// It takes no `&mut self`, what it returns points nowhere, and its body
    /// overwrites nothing and calls no parameter: it can only return copies.
    pub copies: bool,
    /// It takes `&self`, its body overwrites nothing and calls no
    /// parameter, and what it returns lends nothing to write: what it lends
    /// can only be read.
    pub to_read: bool,
}

impl Source {
    /// What the code that names a field at `name` does with it, or with a
    /// part of it. A field named in a struct literal or a pattern is given or
    /// taken whole.
    pub fn access(&self, name: Range) -> Access {
        let tokens = &self.tokens;
        let Some(offset) = self.offset(name.start) else {
            return Access::Reach;
        };
        let at = tokens.partition_point(|token| token.start < offset);
        if at == 0 || at >= tokens.len() || !self.is(tokens[at - 1], ".") {
            return Access::Copy;
        }
        self.reached(at, at + 1).unwrap_or(Access::Copy)
    }

    /// What the code that calls a method at `name` does with what the call
    /// hands out, read as [`access`](Self::access) reads a field, for a
    /// method that lends a field: `*queue.head()` is `queue.head`. A borrow
    /// handed on as it is, `let items = queue.items();`, is borrowed; one
    /// called other than as `value.name(..)` reaches.
    pub fn call_access(&self, name: Range) -> Access {
        let tokens = &self.tokens;
        let Some(offset) = self.offset(name.start) else {
            return Access::Reach;
        };
        let at = tokens.partition_point(|token| token.start < offset);
        let called = tokens.get(at + 1).is_some_and(|token| self.is(*token, "("));
        if at == 0 || !called || !self.is(tokens[at - 1], ".") {
            return Access::Reach;
        }
        let Some(close) = self.matching(at + 1) else {
            return Access::Reach;
        };
        let goes_on = tokens
            .get(close + 1)
            .is_some_and(|token| self.is(*token, "."));
        match self.reached(at, close + 1) {
            Some(access) => access,
            None if goes_on || self.dereferenced(at) => Access::Copy,
            None => Access::Borrow,
        }
    }

    /// What the code does with the place whose name is at `at` and that goes
    /// on to `end`, or with a part of it; `None` when it only takes its value.
    fn reached(&self, at: usize, end: usize) -> Option<Access> {
        let tokens = &self.tokens;
        // Past the parts of the place the code goes on to name.
        let mut end = end;
        while let [dot, part, ..] = &tokens[end..]
            && self.is(*dot, ".")
            && matches!(part.kind, Kind::Ident | Kind::Literal)
        {
            let after = tokens.get(end + 2).map(|token| self.text(*token));
            if matches!(after, Some("(" | "::")) {
                let given_nothing = tokens.get(end + 3).is_some_and(|t| self.is(*t, ")"));
                return Some(match self.text(*part) {
                    "get" if !given_nothing => Access::Call,
                    method if COPIES_OUT.contains(&method) => Access::Copy,
                    method if BORROWS.contains(&method) => Access::Borrow,
                    _ => Access::Call,
                });
            }
            end += 2;
        }
        let next = |ahead: usize| tokens.get(end + ahead).map(|token| self.text(*token));
        let shifted = matches!(
            (next(0), next(1), next(2)),
            (Some("<"), Some("<"), Some("=")) | (Some(">"), Some(">"), Some("="))
        );
        let written = matches!(
            next(0),
            Some("=" | "+=" | "-=" | "*=" | "/=" | "%=" | "^=" | "&=" | "|=" | "[")
        );
        match self.borrowed(at) {
            _ if written || shifted => Some(Access::Reach),
            Some(true) => Some(Access::Reach),
            Some(false) => Some(Access::Borrow),
            None => None,
        }
    }

    /// Whether the place that ends with the name at `at` is borrowed, and if
    /// so, whether to write: `&self.queue.items`, `&mut queue(now).items`.
    fn borrowed(&self, at: usize) -> Option<bool> {
        let tokens = &self.tokens;
        let (start, _) = self.place(at);
        let mut sign = start.checked_sub(1)?;
        let to_write = self.is(tokens[sign], "mut");
        if to_write {
            sign = sign.checked_sub(1)?;
        }
        let borrows =
            ["&", "&&"].contains(&self.text(tokens[sign])) && !self.ends_operand_before(sign);
        borrows.then_some(to_write)
    }

    /// Whether the place that ends with the name at `at` is dereferenced:
    /// `*self.queue.head()`.
    fn dereferenced(&self, at: usize) -> bool {
        let (start, place) = self.place(at);
        start < place
    }

    /// Where the place that ends with the name at `at` starts, with the `*`s
    /// that dereference it and without: `*self.queue.items` and
    /// `self.queue.items`.
    fn place(&self, at: usize) -> (usize, usize) {
        let tokens = &self.tokens;
        let mut start = at;
        while start >= 2 && [".", "::"].contains(&self.text(tokens[start - 1])) {
            let mut before = start - 2;
            while before > 0 && self.is(tokens[before], "?") {
                before -= 1;
            }
            if tokens[before].kind == Kind::Close {
                let Some(open) = self.opening(before) else {
                    break;
                };
                // `name(..)` and `name[..]` go on from their name.
                let named = open > 0 && tokens[open - 1].kind == Kind::Ident;
                before = if named { open - 1 } else { open };
            } else if !matches!(tokens[before].kind, Kind::Ident | Kind::Literal) {
                break;
            }
            start = before;
        }
        let place = start;
        while start > 0 && self.is(tokens[start - 1], "*") && !self.ends_operand_before(start - 1) {
            start -= 1;
        }
        (start, place)
    }

    /// Whether the token before the one at `at` ends an operand, so that an
    /// operator at `at` stands between two.
    fn ends_operand_before(&self, at: usize) -> bool {
        at > 0 && self.ends_operand(self.tokens[at - 1])
    }

    /// The bracket that the one at `close` closes.
    fn opening(&self, close: usize) -> Option<usize> {
        let mut depth = 0;
        for (index, token) in self.tokens[..=close].iter().enumerate().rev() {
            match token.kind {
                Kind::Close => depth += 1,
                Kind::Open => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return Some(index);
            }
        }
        None
    }

    /// What the field written at `field` holds, by its declared type.
    pub fn plain(&self, field: Range) -> Plain {
        let Some(tokens) = self.tokens_in(field) else {
            return Plain::No;
        };
        let written: Vec<&str> = self
            .normalized(self.declared_type(tokens))
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        plain(&written)
    }

    /// What a method with this signature and body, which computes nothing,
    /// hands out.
    pub(super) fn hands_out(
        &self,
        signature: &[Token],
        body: &[Token],
        is_parameter: impl Fn(&str) -> bool,
    ) -> HandsOut {
        let nothing = HandsOut {
            whole: false,
            copies: false,
            to_read: false,
        };
        let text = |token: &Token| self.text(*token);
        let Some(name) = signature.iter().position(|token| text(token) == "fn") else {
            return nothing;
        };
        let mut open = name + 2;
        if signature.get(open).is_some_and(|token| text(token) == "<") {
            open = self.past_angles_in(signature, open);
        }
        let Some(close) = self.matching_in(signature, open) else {
            return nothing;
        };
        let receiver: Vec<&str> = signature[open + 1..close]
            .iter()
            .map(text)
            .take_while(|&part| part != ",")
            .collect();
        let by_reference = receiver.contains(&"&");
        let whole = receiver.contains(&"self") && !by_reference;
        let to_write = by_reference && receiver.contains(&"mut");
        let returned = match signature.get(close + 1) {
            Some(arrow) if text(arrow) == "->" => {
                let rest = signature[close + 2..].iter();
                Some(rest.take_while(|token| text(token) != "where"))
            }
            _ => None,
        };
        let points = returned.clone().is_none_or(|mut returned| {
            returned.any(|token| {
                token.kind == Kind::Lifetime
                    || ["&", "&&", "*", "impl", "Ref", "RefMut"].contains(&text(token))
                    || text(token).ends_with("Guard")
            })
        });

        // What it returns lets its caller write: `&mut`, a raw pointer,
        // `RefMut`, a lock's guard but a read guard, or a trait's value.
        let lends_to_write = returned.is_some_and(|mut returned| {
            returned.any(|token| {
                let name = text(token);
                ["mut", "*", "impl"].contains(&name)
                    || name.ends_with("Mut")
                    || (name.ends_with("Guard") && !name.ends_with("ReadGuard"))
            })
        });

        // A `let` binding's `=` names a value and writes nothing, until a block
        // opens: `if let P = e { self.x = 1 }` still writes.
        let mut binding = false;
        let overwrites = body.windows(2).any(|pair| {
            let called = text(&pair[1]) == "(";
            called && (OVERWRITES.contains(&text(&pair[0])) || is_parameter(text(&pair[0])))
        }) || body.iter().any(|token| {
            let name = text(token);
            match name {
                "let" => binding = true,
                ";" | "{" | "}" => binding = false,
                _ => {}
            }
            (name == "=" && !binding)
                || matches!(name, "+=" | "-=" | "*=" | "/=" | "%=" | "^=" | "&=" | "|=")
        });
        HandsOut {
            whole,
            copies: !to_write && !points && !overwrites,
            to_read: by_reference && !to_write && !overwrites && !lends_to_write,
        }
    }
}

/// What a type written as these [`normalized`](Source::normalized) tokens
/// holds.
fn plain(written: &[&str]) -> Plain {
    let all = |parts: &[&str]| {
        let mut depth = 0i32;
        let mut plain = true;
        for part in parts.split(|&text| {
            match text {
                "<" | "(" | "[" => depth += 1,
                ">" | ")" | "]" => depth -= 1,
                _ => {}
            }
            text == "," && depth == 0
        }) {
            plain &= part.is_empty() || self::plain(part) != Plain::No;
        }
        plain
    };
    let data = |is: bool| if is { Plain::Data } else { Plain::No };
    match written {
        [marker, ..] if MARKERS.contains(marker) => Plain::Nothing,
        [scalar] if SCALARS.contains(scalar) => Plain::Scalar,
        [text] if TEXT.contains(text) => Plain::Data,
        ["&" | "&&", "mut", rest @ ..] | ["&" | "&&", rest @ ..] => data(plain(rest) != Plain::No),
        [holder, "<", inner @ .., ">"] if HOLDS_DATA.contains(holder) => data(all(inner)),
        ["(", inner @ .., ")"] => data(all(inner)),
        ["[", inner @ .., "]"] => {
            let element = inner.split(|&text| text == ";").next().unwrap_or(inner);
            data(plain(element) != Plain::No)
        }
        _ => Plain::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip::Position;

    fn range(code: &str, name: &str, from: usize) -> Range {
        let column = (code.find(name).expect("the code has the name") + from) as u32;
        let start = Position { line: 0, column };
        let end = Position {
            line: 0,
            column: column + 5,
        };
        Range { start, end }
    }

    /// What `code` does at its first `.field`.
    fn access(code: &str) -> Access {
        Source::new(code.to_owned()).access(range(code, ".field", 1))
    }

    #[test]
    fn reading_the_value_or_a_copy_of_it_takes_a_copy() {
        for code in [
            "let head = queue.field;",
            "if queue.field == other.head && ready {}",
            "let next = (queue.field + 1) % size;",
            "send(now, queue.field, 3)",
            "let line = thread.field.at.line;",
            "let name = queue.field.clone();",
            "let hits = stats.field.load(Ordering::Relaxed);",
            "let seen = flags.field.get();",
            "let wide = queue.field as u64;",
            "mask & queue.field",
            "ready && queue.field",
            "match queue.field { Kind::A => 1, _ => 2 }",
        ] {
            assert_eq!(access(code), Access::Copy, "{code}");
        }
        let taken = "let Queue { field, .. } = queue;";
        let source = Source::new(taken.to_owned());
        assert_eq!(source.access(range(taken, "field", 0)), Access::Copy);
    }

    #[test]
    fn borrowing_it_to_read_or_calling_a_method_on_it_is_told_apart() {
        for code in [
            "let items = &queue.field;",
            "for task in &queues[0].field {}",
            "let part = &*queue.field;",
            "let line = &thread().field.at;",
            "check(ready, &queue.field)",
            "let name = queue.field.as_deref();",
        ] {
            assert_eq!(access(code), Access::Borrow, "{code}");
        }
        for code in [
            "let count = queue.field.len();",
            "queue.field.push(task);",
            "let found = queue.field.get(&key);",
            "let more = queue.field.at.checked_add(1);",
        ] {
            assert_eq!(access(code), Access::Call, "{code}");
        }
    }

    #[test]
    fn writing_it_borrowing_it_to_write_or_indexing_it_reaches_into_it() {
        for code in [
            "queue.field = 0;",
            "queue.field += 1;",
            "queue.field <<= 1;",
            "thread.field.at.line = 7;",
            "sort(&mut self.queue.field);",
            "let first = queue.field[0];",
            "let rest = &queue.field[1..];",
        ] {
            assert_eq!(access(code), Access::Reach, "{code}");
        }
    }

    #[test]
    fn a_call_that_lends_a_field_is_read_as_the_field() {
        let access = |code: &str| {
            let source = Source::new(code.to_owned());
            source.call_access(range(code, "field", 0))
        };
        for code in [
            "let head = *queue.field();",
            "if *self.tree.field() != prefix {}",
            "let name = queue.field().clone();",
            "let line = thread.field().at.line;",
        ] {
            assert_eq!(access(code), Access::Copy, "{code}");
        }
        for code in [
            "let items = queue.field();",
            "check(ready, queue.field())",
            "let bytes = tree.field().as_bytes();",
            "let part = &*queue.field();",
        ] {
            assert_eq!(access(code), Access::Borrow, "{code}");
        }
        for code in [
            "let found = tree.field().place_outside(&search);",
            "let count = queue.field(now).len();",
        ] {
            assert_eq!(access(code), Access::Call, "{code}");
        }
        for code in [
            "*queue.field() = 0;",
            "let first = queue.field()[0];",
            "sort(&mut *queue.field());",
            "let head = Queue::field(&queue);",
            "let heads = queues.map(Queue::field);",
        ] {
            assert_eq!(access(code), Access::Reach, "{code}");
        }
    }

    #[test]
    fn numbers_text_and_bytes_are_plain_data_and_what_has_an_inside_is_not() {
        let of = |written: &str| {
            let source = Source::new(written.to_owned());
            let end = Position {
                line: 0,
                column: written.len() as u32,
            };
            let start = Position { line: 0, column: 0 };
            source.plain(Range { start, end })
        };
        assert_eq!(of("pub(crate) head: usize"), Plain::Scalar);
        assert_eq!(of("_config: PhantomData<fn() -> C>"), Plain::Nothing);
        assert_eq!(of("_pin: std::marker::PhantomPinned"), Plain::Nothing);
        assert_eq!(of("#[allow(dead_code)] ready: bool"), Plain::Scalar);
        for data in [
            "name: String",
            "id: &'static str",
            "root: std::path::PathBuf",
            "bytes: Vec<u8>",
            "vendor: [u8; 12]",
            "arguments: Vec<Vec<u8>>",
            "digest: Option<Box<str>>",
            "at: (u32, u32)",
        ] {
            assert_eq!(of(data), Plain::Data, "{data}");
        }
        for other in [
            "items: Vec<Task>",
            "waiting: VecDeque<usize>",
            "hits: AtomicU64",
            "count: Cell<u32>",
            "state: Mutex<u64>",
            "by_name: HashMap<String, u32>",
            "thread: &'r Thread",
        ] {
            assert_eq!(of(other), Plain::No, "{other}");
        }
    }

    #[test]
    fn a_getter_says_whether_it_can_only_return_copies() {
        let hands_out = |code: &str| {
            let source = Source::new(code.to_owned());
            let end = Position {
                line: 0,
                column: code.len() as u32,
            };
            let start = Position { line: 0, column: 0 };
            let text = source.function(Range { start, end }).unwrap();
            (text.whole, text.returns_copies, text.lends_to_read)
        };
        for copies in [
            "fn head(&self) -> usize { *self.head.lock() }",
            "fn name(&self) -> String { self.name.borrow().clone() }",
            "fn first<'a>(&'a self) -> Option<u32> { self.items.get(0).copied() }",
        ] {
            assert_eq!(hands_out(copies), (false, true, true), "{copies}");
        }
        for to_read in [
            "fn items(&self) -> &[u32] { &self.items }",
            "fn items(&self) -> &[u32] { let items = &self.items; items }",
            "fn items(&self) -> Ref<'_, Vec<u32>> { self.items.borrow() }",
            "fn items(&self) -> RwLockReadGuard<Vec<u32>> { self.items.read() }",
        ] {
            assert_eq!(hands_out(to_read), (false, false, true), "{to_read}");
        }
        for lends in [
            "fn items(&self) -> RefMut<'_, Vec<u32>> { self.items.borrow_mut() }",
            "fn locked(&self) -> MutexGuard<Vec<u32>> { self.items.lock() }",
            "fn items(&self) -> *mut u32 { self.items.as_ptr() }",
            "fn take(&self) -> Option<u32> { self.head.take() }",
            "fn set(&self, head: usize) { self.head.set(head) }",
            "fn bump(&mut self) -> usize { self.head }",
            "fn items(&mut self) -> &mut Vec<u32> { &mut self.items }",
            "fn with<R>(&self, each: impl Fn(&u32) -> R) -> R { each(&self.head) }",
        ] {
            assert_eq!(hands_out(lends), (false, false, false), "{lends}");
        }
        let builder = "fn with_head(mut self, head: usize) -> Self { self.head = head; self }";
        assert_eq!(hands_out(builder), (true, false, false));
    }
}
