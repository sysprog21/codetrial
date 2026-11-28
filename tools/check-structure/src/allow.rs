//! Plain-text allowlist for existing architectural debt.
//! One `<lint> <key> # <reason>` per line, sorted. Fixing an allowed entry
//! never fails the check. Every entry says why it is allowed, and the tool
//! never lists a finding no entry already covered: a new one goes onto the
//! list by hand, with its reason, or is fixed. A circle that splits into two
//! smaller ones is listed as both, each with the reason it had.

use std::collections::BTreeMap;
use std::path::Path;

use crate::lint::{Diagnostic, Lint};
use crate::{Error, TOOL};

/// What separates an entry's key from its reason. A key is a path of Rust
/// names, which can hold `#` only as `r#`, never after a space.
const REASON: &str = " # ";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AllowList {
    /// Each entry's reason, by lint and key.
    entries: BTreeMap<(Lint, String), String>,
}

/// Why a line of an allowlist cannot be read.
#[derive(Debug, PartialEq, Eq)]
pub struct Invalid {
    pub line: usize,
    pub why: &'static str,
}

impl AllowList {
    /// The list `--update-allow` writes: this one's entries that `diagnostics`
    /// still has, each keeping its reason, or the findings that no entry
    /// covers. A circle that has shrunk keeps the reason of the entry it
    /// shrank from, under its new key; a finding new to the list is never
    /// added, because only a person can say why it is allowed.
    pub fn updated<'a>(
        &self,
        diagnostics: &'a [Diagnostic],
    ) -> Result<AllowList, Vec<&'a Diagnostic>> {
        let mut entries = BTreeMap::new();
        let mut new = Vec::new();
        for diagnostic in diagnostics {
            match self.reason_for(diagnostic) {
                Some(reason) => {
                    entries.insert((diagnostic.lint, diagnostic.key.clone()), reason.to_owned());
                }
                None => new.push(diagnostic),
            }
        }
        if new.is_empty() {
            Ok(AllowList { entries })
        } else {
            Err(new)
        }
    }

    /// The list at `path`; none when there is no such file.
    pub fn read_or_empty(path: &Path) -> Result<AllowList, Error> {
        if path.exists() {
            AllowList::read(path)
        } else {
            Ok(AllowList::default())
        }
    }

    pub fn read(path: &Path) -> Result<AllowList, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        AllowList::parse(&text).map_err(|Invalid { line, why }| {
            Error::new(format!(
                "{}:{line}: invalid allowlist entry: {why}",
                path.display()
            ))
        })
    }

    /// Whether the list was written for another workspace, where every entry
    /// would read as fixed: it names crates and `knows` recognizes none of
    /// them. A directory (`src/`) is in most workspaces, so it decides only
    /// for a list that names no crate. One crate that left still leaves the
    /// others recognized.
    ///
    /// `knows` is asked about a directory key (`src/web/`) as written, and
    /// about the crate of each member of any other key, so a tangle across two
    /// crates is known while either is.
    pub fn names_nothing_known(&self, knows: impl Fn(&str) -> bool) -> bool {
        let keys = self.entries.keys().map(|(_, key)| key.as_str());
        let (directories, items): (Vec<&str>, Vec<&str>) = keys.partition(|k| k.ends_with('/'));
        let judged: Vec<&str> = if items.is_empty() {
            directories
        } else {
            items
                .iter()
                .flat_map(|key| members(key))
                .map(|member| member.split_once("::").map_or(member, |(name, _)| name))
                .collect()
        };
        !judged.is_empty() && !judged.into_iter().any(knows)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn write(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("{}: {e}", parent.display())))?;
        }
        std::fs::write(path, self.to_text())
            .map_err(|e| Error::new(format!("{}: {e}", path.display())))
    }

    pub fn parse(text: &str) -> Result<AllowList, Invalid> {
        let mut entries = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let invalid = |why| Invalid {
                line: index + 1,
                why,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((entry, reason)) = trimmed.split_once(REASON) else {
                return Err(invalid(
                    "no reason; end the entry with ` # <why it is allowed>`",
                ));
            };
            let Some((lint_name, key)) = entry.split_once(' ') else {
                return Err(invalid("expected `<lint> <key>`"));
            };
            let Some(lint) = Lint::parse(lint_name) else {
                return Err(invalid("no such lint"));
            };
            if key.trim().is_empty() {
                return Err(invalid("the key is empty"));
            }
            let duplicate = entries
                .insert((lint, key.trim().to_owned()), reason.trim().to_owned())
                .is_some();
            if duplicate {
                return Err(invalid("listed twice"));
            }
        }
        Ok(AllowList { entries })
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "# Existing architectural debt suppressed by `{TOOL} --allow`.\n\
             # A finding not listed here fails. Fixing a listed one is never an error\n\
             # by itself, but under `--ratchet` it must leave this list in the same\n\
             # change: run `{TOOL} --update-allow` on this file, which adds nothing new.\n\
             # Each entry ends with ` # ` and why it is allowed; a new one is added\n\
             # by hand, with its reason, in a commit that says why.\n",
        );
        for ((lint, key), reason) in &self.entries {
            out += &format!("{} {key}{REASON}{reason}\n", lint.name());
        }
        out
    }

    pub fn is_allowed(&self, diagnostic: &Diagnostic) -> bool {
        self.reason_for(diagnostic).is_some()
    }

    /// The reason of the entry that allows `diagnostic`, if one does: its own
    /// entry when it has one, so a broader circle listed before it does not
    /// lend its reason, and otherwise one that covers it.
    fn reason_for(&self, diagnostic: &Diagnostic) -> Option<&str> {
        self.entries
            .get(&(diagnostic.lint, diagnostic.key.clone()))
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|((lint, key), _)| covers(*lint, key, diagnostic))
                    .map(|(_, reason)| reason)
            })
            .map(String::as_str)
    }

    /// Allowed entries that no finding in `diagnostics` has exactly: fixed, or
    /// a circle that has shrunk and is now listed under another key. A shrunk
    /// circle is still allowed, but its entry is out of date.
    pub fn stale_count(&self, diagnostics: &[Diagnostic]) -> usize {
        self.entries
            .keys()
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
        let allow = AllowList::parse("tangle a::X,a::Y,b::Z # old\n").unwrap();
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
        let allow = AllowList::parse("module-cycle a::x,a::y,a::z # old\n").unwrap();
        assert!(allow.is_allowed(&cycle("a::x,a::z")));
        assert!(!allow.is_allowed(&cycle("a::w,a::x")));
        assert!(!allow.is_allowed(&tangle("a::x,a::z")));
    }

    #[test]
    fn a_comma_inside_a_members_name_splits_nothing() {
        let allow = AllowList::parse("tangle a::<(A, B) as T>::run,a::X,a::Y # old\n").unwrap();
        assert!(allow.is_allowed(&tangle("a::<(A, B) as T>::run,a::Y")));
        assert!(!allow.is_allowed(&tangle("a::<(A, C) as T>::run,a::Y")));
        assert!(!allow.is_allowed(&tangle("a::Y,a::Z")));
    }

    #[test]
    fn a_removed_lint_is_an_invalid_entry() {
        assert_eq!(
            AllowList::parse("field-leak a::T::x # why\nchatty-seam a -> b # why\n")
                .unwrap_err()
                .line,
            2
        );
    }

    #[test]
    fn every_entry_says_why_it_is_allowed() {
        for (text, why) in [
            ("field-leak a::T::x\n", "no reason"),
            // A line is trimmed first, so a reason of blanks is no reason.
            ("field-leak a::T::x #  \n", "no reason"),
            (
                "field-leak a::T::x # why\nfield-leak a::T::x # again\n",
                "listed twice",
            ),
            ("field-leak # why\n", "expected"),
            ("field-leak  # why\n", "the key is empty"),
            ("tangel a::X # why\n", "no such lint"),
        ] {
            let error = AllowList::parse(text).unwrap_err();
            assert!(error.why.starts_with(why), "{text:?}: {error:?}");
        }
        let allow = AllowList::parse("field-leak a::r#type::x # a raw name keeps its #\n");
        assert_eq!(allow.map(|a| a.len()), Ok(1));
    }

    #[test]
    fn an_update_keeps_reasons_and_never_adds_an_entry() {
        let allow = AllowList::parse(
            "tangle a::X,a::Y,b::Z # predates the gate\ntangle c::P,c::Q # being split\n",
        )
        .unwrap();

        // A fixed entry leaves; a shrunk one keeps its reason under its new
        // key.
        let updated = allow.updated(&[tangle("a::X,b::Z")]).unwrap();
        assert_eq!(
            updated.to_text().lines().last(),
            Some("tangle a::X,b::Z # predates the gate")
        );
        assert_eq!(updated.len(), 1);
        // A finding no entry covers is refused, not written.
        let findings = [tangle("a::X,b::Z"), tangle("d::M,d::N")];
        let refused = allow.updated(&findings).unwrap_err();
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].key, "d::M,d::N");
    }

    #[test]
    fn a_list_is_foreign_only_when_no_entry_is_known() {
        let allow =
            AllowList::parse("tangle a::X,a::Y # one\nwide-directory src/ # two\n").unwrap();
        assert!(allow.names_nothing_known(|_| false));
        // Any crate has a src/, so it does not make the list this workspace's.
        assert!(allow.names_nothing_known(|key| key == "src/"));
        assert!(!allow.names_nothing_known(|name| name == "a"));
        // Any member's crate makes a circle known, not only the first one's.
        let across = AllowList::parse("tangle a::X,b::Y,b::Z # three\n").unwrap();
        assert!(!across.names_nothing_known(|name| name == "b"));
        assert!(across.names_nothing_known(|name| name == "c"));
        let directories = AllowList::parse("wide-directory src/ # two\n").unwrap();
        assert!(!directories.names_nothing_known(|key| key == "src/"));
        assert!(directories.names_nothing_known(|_| false));
        assert!(!AllowList::default().names_nothing_known(|_| false));
    }

    #[test]
    fn an_exact_entry_keeps_its_own_reason_beside_a_broader_one() {
        // The broader circle sorts first and covers the finding too; the
        // finding's own entry is still the one whose reason it keeps.
        let allow = AllowList::parse(
            "tangle a::W,a::X,a::Y # the broader circle\ntangle a::X,a::Y # its own\n",
        )
        .unwrap();
        let findings = [tangle("a::X,a::Y")];
        let updated = allow.updated(&findings).unwrap();
        assert_eq!(
            updated.to_text().lines().last(),
            Some("tangle a::X,a::Y # its own")
        );
    }

    #[test]
    fn what_it_writes_it_reads_back() {
        let allow =
            AllowList::parse("tangle a::X,a::Y # because\nfield-leak a::T::x # held\n").unwrap();
        assert_eq!(AllowList::parse(&allow.to_text()), Ok(allow));
    }

    #[test]
    fn the_committed_debt_list_parses() {
        let text = include_str!("../debt.allow");
        if let Err(error) = AllowList::parse(text) {
            panic!(
                "tools/check-structure/debt.allow:{}: {}",
                error.line, error.why
            );
        }
    }
}
