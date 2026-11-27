//! Which `mod` declarations build their module only for tests, read from
//! the `#[cfg(..)]` attributes on them, which the index does not show.

use super::Source;
use super::tokens::{Kind, Token};
use crate::scip::Range;

/// A `mod name;` or `mod name { .. }` as its text writes it.
#[derive(Debug)]
pub struct ModuleDeclaration {
    /// Where it writes the module's name.
    pub name: Range,
    /// One of its attributes is a `cfg` that holds only when `test` does:
    /// `#[cfg(test)]`, `#[cfg(all(test, ..))]`, but not `#[cfg(any(test,
    /// ..))]`.
    pub for_tests: bool,
}

impl Source {
    /// Every `mod` declaration, in order.
    pub fn module_declarations(&self) -> Vec<ModuleDeclaration> {
        let tokens = &self.tokens;
        let mut declarations = Vec::new();
        // Whether an attribute read since the last item asks for tests.
        let mut for_tests = false;
        let mut at = 0;
        while at < tokens.len() {
            let token = tokens[at];
            let next = tokens.get(at + 1).copied();
            if self.is(token, "#") && next.is_some_and(|t| self.is(t, "[")) {
                let close = self.matching(at + 1).unwrap_or(tokens.len());
                for_tests |= self.is_cfg_for_tests(&tokens[at + 2..close]);
                at = close + 1;
                continue;
            }
            if self.is(token, "pub") {
                at = match next {
                    Some(open) if self.is(open, "(") => {
                        self.matching(at + 1).map_or(at + 1, |c| c + 1)
                    }
                    _ => at + 1,
                };
                continue;
            }
            if self.is(token, "mod")
                && let Some(name) = next.filter(|t| t.kind == Kind::Ident)
                && tokens
                    .get(at + 2)
                    .is_some_and(|t| self.is(*t, ";") || self.is(*t, "{"))
            {
                declarations.push(ModuleDeclaration {
                    name: self.range(name.start, name.end),
                    for_tests,
                });
            }
            for_tests = false;
            at += 1;
        }
        declarations
    }

    /// Whether the attribute `tokens` write, inside `#[..]`, is a `cfg` that
    /// holds only when `test` does.
    fn is_cfg_for_tests(&self, tokens: &[Token]) -> bool {
        match tokens {
            [cfg, open, predicate @ .., close] => {
                self.is(*cfg, "cfg")
                    && self.is(*open, "(")
                    && self.is(*close, ")")
                    && self.implies_test(predicate)
            }
            _ => false,
        }
    }

    /// Whether the `cfg` predicate `tokens` write holds only when `test`
    /// does: `test`, an `all` with such a predicate in it, an `any` of only
    /// such predicates. A `not` is taken to hold without `test`.
    fn implies_test(&self, tokens: &[Token]) -> bool {
        match tokens {
            [name] => self.is(*name, "test"),
            [operator, open, inner @ .., close] if self.is(*open, "(") && self.is(*close, ")") => {
                let predicates = self.predicates(inner);
                match self.text(*operator) {
                    "all" => predicates.iter().any(|p| self.implies_test(p)),
                    "any" => {
                        !predicates.is_empty() && predicates.iter().all(|p| self.implies_test(p))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// The predicates a comma-separated list writes, by its commas outside
    /// brackets.
    fn predicates<'t>(&self, tokens: &'t [Token]) -> Vec<&'t [Token]> {
        let mut depth = 0i32;
        tokens
            .split(|token| {
                match token.kind {
                    Kind::Open => depth += 1,
                    Kind::Close => depth -= 1,
                    _ => {}
                }
                depth == 0 && self.is(*token, ",")
            })
            .filter(|predicate| !predicate.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each `mod` declaration's name, and whether it builds only for tests.
    fn declared(text: &str) -> Vec<(String, bool)> {
        let source = Source::new(text.into());
        source
            .module_declarations()
            .into_iter()
            .map(|declaration| {
                let start = source.offset(declaration.name.start).unwrap();
                let end = source.offset(declaration.name.end).unwrap();
                (text[start..end].to_owned(), declaration.for_tests)
            })
            .collect()
    }

    #[test]
    fn a_cfg_that_implies_test_builds_a_module_only_for_tests() {
        let text = "#[cfg(test)]\nmod support;\n\
                    #[cfg(all(test, feature = \"slow\"))]\npub(crate) mod soak {\n    mod deep;\n}\n\
                    #[cfg(any(test))]\npub(in crate::a) mod only;\n\
                    #[cfg(test)]\n// a note\n/// docs\n#[path = \"x.rs\"]\nmod moved;\n\
                    #[cfg(all(feature = \"a\", any(test, all(test, unix))))]\nmod nested;\n";
        assert_eq!(
            declared(text),
            [
                ("support".to_owned(), true),
                ("soak".to_owned(), true),
                ("deep".to_owned(), false),
                ("only".to_owned(), true),
                ("moved".to_owned(), true),
                ("nested".to_owned(), true),
            ]
        );
    }

    #[test]
    fn any_not_and_attributes_of_other_items_do_not() {
        let text = "#![cfg(test)]\nmod plain;\n\
                    #[cfg(any(test, feature = \"fuzz\"))]\nmod fuzzing;\n\
                    #[cfg(not(test))]\nmod real;\n\
                    #[cfg(all(not(test), unix))]\nmod unix_only;\n\
                    #[cfg(any())]\nmod never;\n\
                    #[cfg_attr(test, path = \"t.rs\")]\nmod switched;\n\
                    #[cfg(test)]\nfn helper() {}\nmod after;\n\
                    #[cfg(test)]\nuse a::b;\nmod later;\n\
                    macro_rules! m { ($name:ident) => { #[cfg(test)] mod $name; } }\n";
        assert_eq!(
            declared(text),
            [
                ("plain".to_owned(), false),
                ("fuzzing".to_owned(), false),
                ("real".to_owned(), false),
                ("unix_only".to_owned(), false),
                ("never".to_owned(), false),
                ("switched".to_owned(), false),
                ("after".to_owned(), false),
                ("later".to_owned(), false),
            ]
        );
        // Text that stops inside an attribute.
        assert_eq!(declared("mod a;\n#[cfg(test"), [("a".to_owned(), false)]);
    }
}
