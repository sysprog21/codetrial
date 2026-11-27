//! Plain-text allowlist for existing architectural debt.
//! One `<lint> <key>` per line, sorted. Fixing an allowed entry never fails the
//! check.

use std::collections::BTreeSet;
use std::path::Path;

use crate::lint::{Diagnostic, Lint};
use crate::{Error, TOOL};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AllowList {
    entries: BTreeSet<(Lint, String)>,
}

impl AllowList {
    pub fn from_diagnostics(diagnostics: &[Diagnostic]) -> AllowList {
        AllowList {
            entries: diagnostics
                .iter()
                .map(|d| (d.lint, d.key.clone()))
                .collect(),
        }
    }

    pub fn read(path: &Path) -> Result<AllowList, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        AllowList::parse(&text).map_err(|line| {
            Error::new(format!(
                "{}:{line}: invalid allowlist entry",
                path.display()
            ))
        })
    }

    pub fn write(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("{}: {e}", parent.display())))?;
        }
        std::fs::write(path, self.to_text())
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))
    }

    pub fn parse(text: &str) -> Result<AllowList, usize> {
        let mut entries = BTreeSet::new();
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((lint_name, key)) = trimmed.split_once(' ') else {
                return Err(index + 1);
            };
            let Some(lint) = Lint::parse(lint_name) else {
                return Err(index + 1);
            };
            if key.trim().is_empty() {
                return Err(index + 1);
            }
            entries.insert((lint, key.trim().to_owned()));
        }
        Ok(AllowList { entries })
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "# Existing architectural debt suppressed by `{TOOL} --allow`.\n\
             # A finding not listed here fails. Fixing a listed one is never an error\n\
             # by itself, but under `--ratchet` it must leave this list in the same\n\
             # change: run `{TOOL} --update-allow` on this file.\n",
        );
        for (lint, key) in &self.entries {
            out += &format!("{} {key}\n", lint.name());
        }
        out
    }

    pub fn is_allowed(&self, diagnostic: &Diagnostic) -> bool {
        self.entries
            .iter()
            .any(|(lint, key)| covers(*lint, key, diagnostic))
    }

    /// Allowed entries that no finding in `diagnostics` has exactly: fixed, or
    /// a circle that has shrunk and is now listed under another key. A shrunk
    /// circle is still allowed, but its entry is out of date.
    pub fn stale_count(&self, diagnostics: &[Diagnostic]) -> usize {
        self.entries
            .iter()
            .filter(|(lint, key)| !diagnostics.iter().any(|d| d.lint == *lint && d.key == *key))
            .count()
    }
}

/// Whether the entry `lint key` allows `diagnostic`. Shrinking an allowed
/// circle is an improvement, never a new error, so an entry allows every
/// tangle or module cycle whose members are among its own.
fn covers(lint: Lint, key: &str, diagnostic: &Diagnostic) -> bool {
    if diagnostic.lint != lint {
        return false;
    }
    if diagnostic.key == key {
        return true;
    }
    let allowed = members(key);
    matches!(lint, Lint::Tangle | Lint::ModuleCycle)
        && members(&diagnostic.key)
            .iter()
            .all(|member| allowed.contains(member))
}

/// The members a circle's key lists. A member's own commas are inside
/// brackets, as in `<(A, B) as Trait>::run`, and split nothing.
fn members(key: &str) -> Vec<&str> {
    let mut members = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, c) in key.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                members.push(&key[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    members.push(&key[start..]);
    members
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::Picture;

    fn tangle(key: &str) -> Diagnostic {
        Diagnostic {
            lint: Lint::Tangle,
            key: key.into(),
            message: String::new(),
            location: None,
            notes: vec![],
            more_ties: vec![],
            picture: Picture::Circle {
                steps: vec![],
                rest: None,
            },
        }
    }

    #[test]
    fn shrinking_an_allowed_tangle_stays_allowed() {
        let allow = AllowList::parse("tangle a::X,a::Y,b::Z\n").unwrap();
        assert!(allow.is_allowed(&tangle("a::X,b::Z")));
        assert!(!allow.is_allowed(&tangle("a::W,a::X,a::Y,b::Z")));
        // Allowed, but the entry is out of date: the circle is smaller.
        assert_eq!(allow.stale_count(&[tangle("a::X,a::Y")]), 1);
        assert_eq!(allow.stale_count(&[tangle("a::X,a::Y,b::Z")]), 0);
        assert_eq!(allow.stale_count(&[]), 1);
    }

    #[test]
    fn shrinking_an_allowed_module_cycle_stays_allowed() {
        let cycle = |key: &str| Diagnostic {
            lint: Lint::ModuleCycle,
            ..tangle(key)
        };
        let allow = AllowList::parse("module-cycle a::x,a::y,a::z\n").unwrap();
        assert!(allow.is_allowed(&cycle("a::x,a::z")));
        assert!(!allow.is_allowed(&cycle("a::w,a::x")));
        assert!(!allow.is_allowed(&tangle("a::x,a::z")));
    }

    #[test]
    fn a_comma_inside_a_members_name_splits_nothing() {
        let allow = AllowList::parse("tangle a::<(A, B) as T>::run,a::X,a::Y\n").unwrap();
        assert!(allow.is_allowed(&tangle("a::<(A, B) as T>::run,a::Y")));
        assert!(!allow.is_allowed(&tangle("a::<(A, C) as T>::run,a::Y")));
        assert!(!allow.is_allowed(&tangle("a::Y,a::Z")));
    }

    #[test]
    fn a_removed_lint_is_an_invalid_entry() {
        assert_eq!(
            AllowList::parse("field-leak a::T::x\nchatty-seam a -> b\n"),
            Err(2)
        );
    }
}
