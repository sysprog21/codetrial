//! The lints. `module-cycle` reads which modules use which, by where code is
//! written; `tangle`, `field-leak` and `shallow-module` read the graph of
//! which things know which, wherever they are written; `wide-directory` reads
//! how the files are laid out.

mod cycles;
mod layering;
mod layout;
mod leaks;
mod shallow;
mod tangles;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::graph::{Graph, ThingId};
use crate::model::{ItemId, Model, NodeId, NodeKind, Reference, Tree};

/// What a reader can hold in mind at once: seven (Miller, "The Magical Number
/// Seven, Plus or Minus Two", 1956). The members of a tangle cannot be learned
/// one at a time, so a tangle of more than seven is too much anywhere; an
/// interface of up to seven things, each with up to seven functions, is small
/// enough to need no justification. A note or a picture lists no more than
/// this either.
pub const HELD_IN_MIND: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lint {
    ModuleCycle,
    Tangle,
    FieldLeak,
    ShallowModule,
    WideDirectory,
}

impl Lint {
    pub fn name(self) -> &'static str {
        match self {
            Lint::ModuleCycle => "module-cycle",
            Lint::Tangle => "tangle",
            Lint::FieldLeak => "field-leak",
            Lint::ShallowModule => "shallow-module",
            Lint::WideDirectory => "wide-directory",
        }
    }

    pub fn parse(name: &str) -> Option<Lint> {
        match name {
            "module-cycle" => Some(Lint::ModuleCycle),
            "tangle" => Some(Lint::Tangle),
            "field-leak" => Some(Lint::FieldLeak),
            "shallow-module" => Some(Lint::ShallowModule),
            "wide-directory" => Some(Lint::WideDirectory),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub lint: Lint,
    /// Stable identifier used in the allow file.
    pub key: String,
    pub message: String,
    pub location: Option<Location>,
    pub notes: Vec<String>,
    /// The notes of a circle's ties past the [`HELD_IN_MIND`] that `notes`
    /// shows, dearest last; the report counts them unless asked for them.
    pub more_ties: Vec<String>,
    pub picture: Picture,
}

/// Why a report is a problem, drawn under its notes: the shape of what it
/// found, at a size a reader takes in at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    /// Members that know each other in a circle, from its cheapest tie: each
    /// step's member knows the next step's, and the last step's the first's.
    /// `rest` says what of the whole the circle leaves out.
    Circle {
        steps: Vec<Step>,
        rest: Option<String>,
    },
    /// A field, `owner`'s, and the modules whose code works on it, with how
    /// many places in each, most first; then what of its other fields is
    /// known outside, and what is kept.
    Spread {
        owner: String,
        field: String,
        modules: Vec<(usize, String)>,
        others: Vec<String>,
    },
    /// How many functions a module shows and hides, and each list it is
    /// learned by that is longer than a reader holds in mind.
    Lists {
        shown: usize,
        hidden: usize,
        lists: Vec<(String, usize)>,
    },
    /// How much there is of what has a most: a directory's files and
    /// directories, a file's lines.
    Bar {
        label: String,
        length: usize,
        most: usize,
    },
}

/// A member of a circle, and its tie to the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub member: String,
    /// Where the tie is first written.
    pub site: Location,
    /// What of the next member the code there uses: `listen uses Stream`.
    pub uses: String,
}

/// Every lint over `model`, optionally only where `scope`, a crate or module,
/// is concerned.
pub fn check(model: &Model, scope: Option<NodeId>) -> Vec<Diagnostic> {
    let graph = Graph::new(model);
    let mut diagnostics = cycles::check(model, scope);
    diagnostics.extend(tangles::check(model, &graph, scope));
    diagnostics.extend(leaks::check(model, &graph, scope));
    diagnostics.extend(shallow::check(model, &graph, scope));
    diagnostics.extend(layout::check(model, scope));
    diagnostics.sort_by(|a, b| a.lint.cmp(&b.lint).then_with(|| a.key.cmp(&b.key)));
    diagnostics
}

pub fn resolve_node(model: &Model, query: &str) -> Option<NodeId> {
    let tree = &model.tree;
    let normalized = query.replace('-', "_");
    tree.ids().find(|&id| {
        tree.node(id).kind != NodeKind::Workspace
            && (tree.display(id) == query || tree.display(id) == normalized)
    })
}

/// The references by which one member of a circle knows another.
#[derive(Default)]
struct Tie {
    items: BTreeSet<ItemId>,
    /// The items whose code holds them.
    holders: BTreeSet<ItemId>,
    sites: BTreeSet<(String, u32)>,
    /// By each module it is written in, as its report counts them, its
    /// first site there.
    written: BTreeMap<NodeId, (String, u32)>,
}

impl Tie {
    /// Adds `reference`, which is to `item` or to a wrapper of it, written in
    /// `module`.
    fn add(&mut self, model: &Model, item: ItemId, reference: &Reference, module: NodeId) {
        self.items.insert(item);
        self.holders.extend(reference.holder);
        let path = &model.files[reference.file.index()].path;
        let site = (path.clone(), reference.line);
        let first = self.written.entry(module).or_insert_with(|| site.clone());
        if site < *first {
            *first = site.clone();
        }
        self.sites.insert(site);
    }

    /// How many items it ties by: what cutting it takes.
    fn cost(&self) -> usize {
        self.items.len()
    }

    fn site(&self) -> Location {
        let (file, line) = self.sites.first().expect("a tie has a site");
        Location {
            file: file.clone(),
            line: *line,
        }
    }

    /// `src/kinds.rs:2:1: `kinds` uses `socket` by 1 item: LIMIT`, and the
    /// first site in each other module it is written in.
    fn note(&self, model: &Model, verb: &str, (user, used): &(String, String)) -> String {
        let site = self.site();
        let firsts: BTreeSet<&(String, u32)> = self.written.values().collect();
        let elsewhere: Vec<String> = firsts
            .into_iter()
            .filter(|(file, line)| (file, *line) != (&site.file, site.line))
            .map(|(file, line)| {
                clickable(&Location {
                    file: file.clone(),
                    line: *line,
                })
            })
            .collect();
        let also = if elsewhere.is_empty() {
            String::new()
        } else {
            format!("; also at {}", listed(&elsewhere))
        };
        format!(
            "{}: `{user}` {verb} `{used}` by {}: {}{also}",
            clickable(&site),
            counted(self.items.len(), "item"),
            listed(&names(model, &self.items))
        )
    }

    /// The step of a circle from `member`, whose code it is in: what it
    /// uses of the next, and the item whose code does, when that is one.
    fn step(&self, model: &Model, member: &str) -> Step {
        let items = grouped(&names(model, &self.items));
        let uses = match &names(model, &self.holders)[..] {
            [holder] => format!("{holder} uses {items}"),
            _ => format!("uses {items}"),
        };
        Step {
            member: member.to_owned(),
            site: self.site(),
            uses,
        }
    }
}

/// `Stream::{self, new}, LIMIT`: `names`, with those under one name written
/// once, as a `use` declaration writes them.
fn grouped(names: &[String]) -> String {
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for name in names {
        let (head, tail) = name.split_once("::").unwrap_or((name, "self"));
        match groups.iter_mut().find(|(other, _)| *other == head) {
            Some((_, tails)) => tails.push(tail.to_owned()),
            None => groups.push((head, vec![tail.to_owned()])),
        }
    }
    let written: Vec<String> = groups
        .into_iter()
        .map(|(head, tails)| match &tails[..] {
            [tail] if tail == "self" => head.to_owned(),
            [tail] => format!("{head}::{tail}"),
            _ => format!("{head}::{{{}}}", listed(&tails)),
        })
        .collect();
    listed(&written)
}

fn names(model: &Model, items: &BTreeSet<ItemId>) -> Vec<String> {
    items
        .iter()
        .map(|item| without_generics(&model.items[item.index()].name))
        .collect()
}

/// What is reported of a circle of `members` `noun`s, by the ties between
/// them, each by the names of the two it ties: where its cheapest tie is, a
/// note for each tie, and a picture of the shortest circle through the
/// cheapest. The ties of fewest items come first: they are the cheapest to
/// cut.
fn circle_report(
    model: &Model,
    verb: &str,
    noun: &str,
    members: usize,
    mut ties: Vec<((String, String), &Tie)>,
) -> CircleReport {
    ties.sort_by(|(a, tie_a), (b, tie_b)| (tie_a.cost(), a).cmp(&(tie_b.cost(), b)));
    let mut notes: Vec<String> = ties
        .iter()
        .map(|(names, tie)| tie.note(model, verb, names))
        .collect();
    let more_ties = notes.split_off(notes.len().min(HELD_IN_MIND));
    let circle = shortest_circle(&ties);
    let steps = circle
        .iter()
        .map(|&at| {
            let ((user, _), tie) = &ties[at];
            tie.step(model, user)
        })
        .collect();
    let left_out: Vec<String> = [
        (members - circle.len(), format!("more {noun}")),
        (ties.len() - circle.len(), "more tie".to_owned()),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, noun)| counted(count, &noun))
    .collect();
    let rest = (!left_out.is_empty()).then(|| format!("not drawn: {}", left_out.join(", ")));
    let picture = Picture::Circle { steps, rest };
    CircleReport {
        location: ties.first().map(|(_, tie)| tie.site()),
        notes,
        more_ties,
        picture,
    }
}

/// What `circle_report` found of a circle.
struct CircleReport {
    location: Option<Location>,
    notes: Vec<String>,
    more_ties: Vec<String>,
    picture: Picture,
}

/// The shortest circle through the first of `ties`, each by the names of the
/// two it ties: the indexes of the ties it takes, in order, the first first.
/// Of two as short, it takes the cheaper ties, which come first.
fn shortest_circle<T>(ties: &[((String, String), T)]) -> Vec<usize> {
    let ((start, end), _) = &ties[0];
    // By member, the tie it was first reached by, going out from `end`.
    let mut reached: BTreeMap<&str, Option<usize>> = BTreeMap::from([(end.as_str(), None)]);
    let mut queue = VecDeque::from([end.as_str()]);
    while let Some(at) = queue.pop_front().filter(|&at| at != start) {
        for (index, ((user, used), _)) in ties.iter().enumerate() {
            if user == at && !reached.contains_key(used.as_str()) {
                reached.insert(used, Some(index));
                queue.push_back(used);
            }
        }
    }
    let mut circle = vec![];
    let mut at = start.as_str();
    while let Some(&Some(index)) = reached.get(at) {
        circle.push(index);
        at = &ties[index].0.0;
    }
    circle.push(0);
    circle.reverse();
    circle
}

/// `file:line:1`, the shape rustc writes, so terminals and editors link it.
fn clickable(location: &Location) -> String {
    format!("{}:{}:1", location.file, location.line)
}

/// Whether `node` is in `scope`'s subtree. A crate's scope is its own code and
/// not a nested workspace member's, which hangs below the same root.
fn in_scope(tree: &Tree, scope: Option<NodeId>, node: NodeId) -> bool {
    scope.is_none_or(|scope| {
        tree.is_within(node, scope)
            && (tree.node(scope).kind == NodeKind::Workspace
                || tree.package_of(node) == tree.package_of(scope))
    })
}

/// `my_crate::kinds::Ready`: where a thing is defined, and its name there.
fn full_name(model: &Model, graph: &Graph, thing: ThingId) -> String {
    let thing = &graph.things[thing.index()];
    let name = without_generics(&model.items[thing.item.index()].name);
    format!("{}::{name}", model.tree.display(thing.home))
}

/// `a, b, c, +2 more`: at most [`HELD_IN_MIND`] of `names`.
fn listed(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(HELD_IN_MIND)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().checked_sub(HELD_IN_MIND) {
        Some(more) if more > 0 => format!("{shown}, +{more} more"),
        _ => shown,
    }
}

/// `1 module`, `2 modules`.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `<Ready as Display>`: a name with generic arguments left out.
fn without_generics(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '<' if depth > 0 || out.ends_with(|c: char| c.is_alphanumeric() || c == '_') => {
                depth += 1
            }
            '>' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => out.push(c),
        }
    }
    out
}

/// The first line of `module`'s primary non-test file, if it has one.
fn module_location(model: &Model, module: NodeId) -> Option<Location> {
    model
        .files
        .iter()
        .filter(|file| !file.test && file.module == module)
        .min_by_key(|file| {
            let path = file.path.as_str();
            let rank = if path.ends_with("/src/lib.rs") || path == "src/lib.rs" {
                0
            } else if path.ends_with("/mod.rs") || path.ends_with("src/main.rs") {
                1
            } else if path.contains("/examples/") {
                3
            } else {
                2
            };
            (rank, path)
        })
        .map(|file| Location {
            file: file.path.clone(),
            line: 1,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_without_its_generic_arguments() {
        assert_eq!(
            without_generics("<Vec<T> as From<(A, B)>>::from"),
            "<Vec as From>::from"
        );
        assert_eq!(without_generics("Ready::new"), "Ready::new");
    }

    #[test]
    fn the_circle_drawn_is_the_shortest_through_the_first_tie_of_the_cheapest() {
        let tie = |user: &str, used: &str| ((user.to_owned(), used.to_owned()), ());
        let ring = [tie("a", "b"), tie("b", "c"), tie("c", "a")];
        assert_eq!(shortest_circle(&ring), [0, 1, 2]);
        let chord = [tie("a", "b"), tie("b", "c"), tie("c", "a"), tie("b", "a")];
        assert_eq!(shortest_circle(&chord), [0, 3]);
        let two_ways = [
            tie("a", "b"),
            tie("b", "d"),
            tie("b", "c"),
            tie("c", "a"),
            tie("d", "a"),
        ];
        assert_eq!(shortest_circle(&two_ways), [0, 1, 4]);
    }

    #[test]
    fn names_under_one_name_are_written_once() {
        let names = |names: &[&str]| -> Vec<String> { names.iter().map(|&n| n.into()).collect() };
        assert_eq!(grouped(&names(&["LIMIT"])), "LIMIT");
        assert_eq!(grouped(&names(&["Stream::new"])), "Stream::new");
        assert_eq!(
            grouped(&names(&[
                "Stream",
                "LIMIT",
                "Stream::new",
                "Stream::Kind::Ready"
            ])),
            "Stream::{self, new, Kind::Ready}, LIMIT"
        );
    }
}
