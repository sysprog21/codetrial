//! SCIP's symbol strings: `<scheme> <manager> <package> <version>
//! <descriptors>`,
//! for example `rust-analyzer cargo my-crate 0.1.0 kinds/epoll/Ready#`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, PartialEq)]
pub struct Symbol<'a> {
    package: Cow<'a, str>,
    version: Cow<'a, str>,
    text: &'a str,
    descriptors: Vec<Descriptor<'a>>,
}

#[derive(Debug, PartialEq)]
struct Descriptor<'a> {
    name: Cow<'a, str>,
    kind: Kind,
    /// Where it ends in the symbol's text.
    end: usize,
}

/// Where a symbol's item is written, as its descriptors say.
#[derive(Debug, PartialEq)]
pub enum Place<'a> {
    /// Directly in its module: `serve().`, `Ready#`, a module.
    Module,
    /// Inside the item with this symbol: `kinds/Ready#` for
    /// `kinds/Ready#field.`.
    In(&'a str),
    /// In an `impl` block, or the block itself: `kinds/impl#[Ready]new().`.
    Impl(Impl<'a>),
}

#[derive(Debug, PartialEq)]
pub struct Impl<'a> {
    /// The block's symbol, which starts every item written in it:
    /// `kinds/impl#[Ready][Display]`.
    pub block: &'a str,
    /// The type it is for, as the index writes it: `Ready`, `&'a Tuple`,
    /// `crate::Ready`.
    pub self_type: Cow<'a, str>,
    pub of_trait: bool,
    /// The symbol is the block itself, which code names as `Self`.
    pub is_block: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Namespace,
    Type,
    Term,
    Method,
    TypeParameter,
    Parameter,
    Meta,
    Macro,
}

#[derive(Debug)]
pub struct BadSymbol(pub String);

impl fmt::Display for BadSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a SCIP symbol this tool cannot read: {:?}", self.0)
    }
}

/// rust-analyzer's name for a crate's root module.
const ROOT_MODULE: &str = "crate";

impl<'a> Symbol<'a> {
    /// `None` for a local symbol: one that only its own document can name.
    pub fn parse(text: &'a str) -> Result<Option<Symbol<'a>>, BadSymbol> {
        if text.starts_with("local ") {
            return Ok(None);
        }
        let bad = || BadSymbol(text.to_owned());
        let (_scheme, rest) = space_separated(text).ok_or_else(bad)?;
        let (_manager, rest) = space_separated(rest).ok_or_else(bad)?;
        let (package, rest) = space_separated(rest).ok_or_else(bad)?;
        let (version, rest) = space_separated(rest).ok_or_else(bad)?;
        let descriptors = descriptors(text, rest).ok_or_else(bad)?;
        Ok(Some(Symbol {
            package,
            version,
            text,
            descriptors,
        }))
    }

    /// What `packages`, keyed by name and version, holds for the package the
    /// symbol is in.
    pub fn package_in<T: Copy>(&self, packages: &HashMap<(&str, &str), T>) -> Option<T> {
        packages
            .get(&(self.package.as_ref(), self.version.as_ref()))
            .copied()
    }

    /// The module this symbol names, as the path below its crate's root, if it
    /// names one.
    pub fn module(&self) -> Option<Vec<&str>> {
        let is_module = self.descriptors.iter().all(|d| d.kind == Kind::Namespace);
        is_module.then(|| self.namespaces())
    }

    /// The module the symbol is in, or is: its leading namespaces.
    pub fn namespaces(&self) -> Vec<&str> {
        let path: Vec<&str> = self
            .descriptors
            .iter()
            .take_while(|d| d.kind == Kind::Namespace)
            .map(|d| d.name.as_ref())
            .collect();
        if path == [ROOT_MODULE] {
            Vec::new()
        } else {
            path
        }
    }

    /// What its last descriptor says the symbol is; a module's is a namespace.
    pub fn kind(&self) -> Kind {
        self.descriptors.last().map_or(Kind::Namespace, |d| d.kind)
    }

    /// A function or method: `serve().`, `impl#[Ready]new().`.
    pub fn is_function(&self) -> bool {
        self.kind() == Kind::Method
    }

    /// Where the symbol's item is written.
    pub fn place(&self) -> Place<'a> {
        let rest = self
            .descriptors
            .iter()
            .position(|d| d.kind != Kind::Namespace)
            .map_or(&[][..], |first| &self.descriptors[first..]);
        match rest {
            [block, ty, more @ ..] if block.kind == Kind::Type && block.name == "impl" => {
                let traits = more
                    .iter()
                    .take_while(|d| d.kind == Kind::TypeParameter)
                    .count();
                let end = more.get(traits.wrapping_sub(1)).unwrap_or(ty).end;
                Place::Impl(Impl {
                    block: &self.text[..end],
                    self_type: ty.name.clone(),
                    of_trait: traits > 0,
                    is_block: traits == more.len(),
                })
            }
            [.., container, _] => Place::In(&self.text[..container.end]),
            _ => Place::Module,
        }
    }

    /// How a reader would write the symbol inside its module: `Ready`,
    /// `Ready::new`, `<Ready as Display>::fmt`.
    pub fn item_name(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut rest = self
            .descriptors
            .iter()
            .skip_while(|d| d.kind == Kind::Namespace)
            .peekable();
        while let Some(descriptor) = rest.next() {
            if descriptor.kind == Kind::Type && descriptor.name == "impl" {
                let mut types = Vec::new();
                while let Some(parameter) = rest.next_if(|d| d.kind == Kind::TypeParameter) {
                    types.push(parameter.name.as_ref());
                }
                parts.push(match types.as_slice() {
                    [ty, interface] => format!("<{ty} as {interface}>"),
                    _ => types.join(""),
                });
            } else if descriptor.kind == Kind::Macro {
                parts.push(format!("{}!", descriptor.name));
            } else {
                parts.push(descriptor.name.to_string());
            }
        }
        parts.join("::")
    }
}

/// A space ends a field; two spaces are one space inside it.
fn space_separated(text: &str) -> Option<(Cow<'_, str>, &str)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut escaped = false;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            if bytes.get(i + 1) == Some(&b' ') {
                escaped = true;
                i += 2;
                continue;
            }
            let field = &text[..i];
            let field = if escaped {
                Cow::Owned(field.replace("  ", " "))
            } else {
                Cow::Borrowed(field)
            };
            return Some((field, &text[i + 1..]));
        }
        i += 1;
    }
    None
}

/// The descriptors `rest`, the end of `symbol`, writes.
fn descriptors<'a>(symbol: &str, mut rest: &'a str) -> Option<Vec<Descriptor<'a>>> {
    let mut parsed = Vec::new();
    while !rest.is_empty() {
        let (name, kind, after) = descriptor(rest)?;
        rest = after;
        let end = symbol.len() - rest.len();
        parsed.push(Descriptor { name, kind, end });
    }
    (!parsed.is_empty()).then_some(parsed)
}

fn descriptor(text: &str) -> Option<(Cow<'_, str>, Kind, &str)> {
    let enclosed = |open: &str, close: char, kind: Kind| {
        let (name, rest) = name(text.strip_prefix(open)?)?;
        Some((name, kind, rest.strip_prefix(close)?))
    };
    if text.starts_with('[') {
        return enclosed("[", ']', Kind::TypeParameter);
    }
    if text.starts_with('(') {
        return enclosed("(", ')', Kind::Parameter);
    }
    let (name, rest) = name(text)?;
    let mut chars = rest.chars();
    let kind = match chars.next()? {
        '/' => Kind::Namespace,
        '#' => Kind::Type,
        '.' => Kind::Term,
        ':' => Kind::Meta,
        '!' => Kind::Macro,
        '(' => {
            let close = rest.find(").")?;
            return Some((name, Kind::Method, &rest[close + 2..]));
        }
        _ => return None,
    };
    Some((name, kind, chars.as_str()))
}

fn name(text: &str) -> Option<(Cow<'_, str>, &str)> {
    if let Some(quoted) = text.strip_prefix('`') {
        let mut end = 0;
        loop {
            end += quoted[end..].find('`')?;
            if quoted[end + 1..].starts_with('`') {
                end += 2;
            } else {
                break;
            }
        }
        let raw = &quoted[..end];
        let name = if raw.contains("``") {
            Cow::Owned(raw.replace("``", "`"))
        } else {
            Cow::Borrowed(raw)
        };
        return Some((name, &quoted[end + 1..]));
    }
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '$')))
        .unwrap_or(text.len());
    (end > 0).then(|| (Cow::Borrowed(&text[..end]), &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Symbol<'_> {
        Symbol::parse(text).unwrap().unwrap()
    }

    #[test]
    fn a_module_symbol_gives_its_package_and_path() {
        let symbol = parse("rust-analyzer cargo my-crate 0.1.0 kinds/epoll/");
        assert_eq!(symbol.package, "my-crate");
        assert_eq!(symbol.version, "0.1.0");
        assert_eq!(symbol.module(), Some(vec!["kinds", "epoll"]));
    }

    #[test]
    fn the_crate_root_is_the_empty_path() {
        assert_eq!(
            parse("rust-analyzer cargo a 0.1.0 crate/").module(),
            Some(vec![])
        );
        assert_eq!(
            parse("rust-analyzer cargo a 0.1.0 Files#").namespaces(),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn items_are_not_modules_but_say_which_module_they_are_in() {
        let symbol = parse("rust-analyzer cargo a 0.1.0 kinds/epoll/impl#[Ready]new().");
        assert_eq!(symbol.module(), None);
        assert_eq!(symbol.namespaces(), ["kinds", "epoll"]);
        assert_eq!(symbol.item_name(), "Ready::new");
    }

    #[test]
    fn an_item_is_placed_in_its_module_its_container_or_an_impl_block() {
        let place = |descriptors: &str| {
            let text = format!("rust-analyzer cargo a 0.1.0 {descriptors}");
            let symbol = parse(&text);
            format!("{:?}", symbol.place())
        };
        assert_eq!(place("kinds/serve()."), "Module");
        assert_eq!(
            place("kinds/Ready#field."),
            r#"In("rust-analyzer cargo a 0.1.0 kinds/Ready#")"#
        );
        assert_eq!(
            place("kinds/Event#Ready#x."),
            r#"In("rust-analyzer cargo a 0.1.0 kinds/Event#Ready#")"#
        );
        assert_eq!(
            place("kinds/impl#[`&'a Ready<T>`][Display]fmt()."),
            r#"Impl(Impl { block: "rust-analyzer cargo a 0.1.0 kinds/impl#[`&'a Ready<T>`][Display]", self_type: "&'a Ready<T>", of_trait: true, is_block: false })"#
        );
        assert_eq!(
            place("impl#[Ready]"),
            r#"Impl(Impl { block: "rust-analyzer cargo a 0.1.0 impl#[Ready]", self_type: "Ready", of_trait: false, is_block: true })"#
        );
        assert!(parse("rust-analyzer cargo a 0.1.0 kinds/Serve#serve().").is_function());
        assert!(!parse("rust-analyzer cargo a 0.1.0 kinds/").is_function());
    }

    #[test]
    fn escaped_names_and_trait_impls() {
        let symbol = parse(
            "rust-analyzer cargo std https://x/std os/impl#[`fs::Metadata`][MetadataExt]mode().",
        );
        assert_eq!(symbol.version, "https://x/std");
        assert_eq!(symbol.item_name(), "<fs::Metadata as MetadataExt>::mode");
        let quoted = parse("rust-analyzer cargo a 0.1.0 `a``b`#");
        assert_eq!(quoted.descriptors[0].name, "a`b");
    }

    #[test]
    fn spaces_in_a_package_name_are_doubled() {
        assert_eq!(
            parse("rust-analyzer cargo my  crate 1 m/").package,
            "my crate"
        );
    }

    #[test]
    fn locals_are_not_symbols_and_garbage_is_an_error() {
        assert!(Symbol::parse("local 12").unwrap().is_none());
        assert!(Symbol::parse("rust-analyzer cargo a 0.1.0 ").is_err());
        assert!(Symbol::parse("rust-analyzer cargo a 0.1.0 x%").is_err());
    }
}
