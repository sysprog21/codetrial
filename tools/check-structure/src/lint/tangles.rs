//! `tangle`: things that know each other in a circle, written in more than one
//! module, or more of them than a reader holds in mind. Where a tangle is
//! written is its members' homes and the modules of the references that tie
//! it, so moving an `impl` block or lifting a member into the parent module's
//! file changes nothing.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Diagnostic, HELD_IN_MIND, Lint, Tie, circle_report, counted, full_name, in_scope, listed,
};
use crate::graph::{Graph, ThingId};
use crate::model::{Model, NodeId};

#[derive(Default)]
struct Written {
    modules: BTreeSet<NodeId>,
    ties: BTreeMap<(ThingId, ThingId), Tie>,
}

pub fn check(model: &Model, graph: &Graph, scope: Option<NodeId>) -> Vec<Diagnostic> {
    let mut written: Vec<Written> = graph
        .tangles
        .iter()
        .map(|members| Written {
            modules: members
                .iter()
                .map(|&member| graph.things[member.index()].home)
                .collect(),
            ties: BTreeMap::new(),
        })
        .collect();
    let tangle_of = |thing: ThingId| graph.tangle_of[thing.index()];
    for usage in &graph.uses {
        let Some(user) = usage.user else { continue };
        let Some(used) = graph.thing_of[usage.item.index()] else {
            continue;
        };
        let Some(tangle) = tangle_of(user).filter(|&t| user != used && tangle_of(used) == Some(t))
        else {
            continue;
        };
        let reference = &model.references[usage.reference];
        let written = &mut written[tangle.index()];
        written.modules.insert(reference.from);
        let tie = written.ties.entry((user, used)).or_default();
        tie.add(model, usage.item, reference, reference.from);
    }
    let mut diagnostics = Vec::new();
    for (index, written) in written.into_iter().enumerate() {
        let members = &graph.tangles[index];
        let concerned = members
            .iter()
            .any(|&member| in_scope(&model.tree, scope, graph.things[member.index()].home));
        let modules = written.modules.len();
        if concerned && (modules > 1 || members.len() > HELD_IN_MIND) {
            diagnostics.push(diagnostic(model, graph, members, written));
        }
    }
    diagnostics
}

fn diagnostic(model: &Model, graph: &Graph, members: &[ThingId], written: Written) -> Diagnostic {
    let mut names: Vec<String> = members
        .iter()
        .map(|&member| full_name(model, graph, member))
        .collect();
    names.sort();
    let ties = written
        .ties
        .iter()
        .map(|(&(user, used), tie)| {
            let names = (full_name(model, graph, user), full_name(model, graph, used));
            (names, tie)
        })
        .collect();
    let report = circle_report(model, "knows", "thing", members.len(), ties);
    let mut notes = vec![format!("members: {}", listed(&names))];
    if written.modules.len() > 1 {
        let mut modules: Vec<String> = written
            .modules
            .iter()
            .map(|&module| model.tree.display(module))
            .collect();
        modules.sort();
        notes.push(format!("written in: {}", listed(&modules)));
    }
    notes.extend(report.notes);
    let count = names.len();
    let message = match written.modules.iter().collect::<Vec<_>>()[..] {
        [only] => format!(
            "{count} things know each other in a circle, all in `{}`",
            model.tree.display(*only)
        ),
        _ => format!(
            "{count} things know each other in a circle, across {}",
            counted(written.modules.len(), "module")
        ),
    };
    Diagnostic {
        lint: Lint::Tangle,
        key: names.join(","),
        message,
        location: report.location,
        notes,
        more_ties: report.more_ties,
        picture: report.picture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::Picture;
    use crate::model::tests::Sample;
    use crate::model::{ItemId, ItemKind};

    fn keys(sample: &Sample) -> Vec<String> {
        let graph = Graph::new(&sample.model);
        check(&sample.model, &graph, None)
            .into_iter()
            .map(|d| d.key)
            .collect()
    }

    #[test]
    fn two_things_in_a_circle_are_reported_when_their_code_is_in_two_modules() {
        let mut sample = Sample::new();
        let (kernel, timers) = (sample.module("kernel"), sample.module("kernel::timers"));
        let tasks = sample.item(kernel, "Tasks", ItemKind::Type);
        let timer = sample.item(timers, "Timers", ItemKind::Type);
        let spawn = sample.member(tasks, kernel, "Tasks::spawn", ItemKind::Function);
        let fire = sample.member(timer, timers, "Timers::fire", ItemKind::Function);
        sample.refer(spawn, timer);
        sample.refer(fire, tasks);
        assert_eq!(
            keys(&sample),
            ["p::kernel::Tasks,p::kernel::timers::Timers"]
        );
    }

    #[test]
    fn a_circle_in_one_module_is_reported_only_when_it_is_too_big_to_hold_in_mind() {
        let mut sample = Sample::new();
        let module = sample.module("ring");
        let ring = |sample: &mut Sample, size: usize| {
            let things: Vec<ItemId> = (0..size)
                .map(|i| sample.item(module, &format!("T{size}x{i}"), ItemKind::Type))
                .collect();
            for i in 0..size {
                sample.refer(things[i], things[(i + 1) % size]);
            }
        };
        ring(&mut sample, HELD_IN_MIND);
        assert_eq!(keys(&sample), Vec::<String>::new());
        ring(&mut sample, HELD_IN_MIND + 1);
        assert_eq!(keys(&sample).len(), 1);
    }

    #[test]
    fn the_picture_is_the_shortest_circle_through_the_cheapest_tie() {
        let mut sample = Sample::new();
        let (a_module, b_module) = (sample.module("a"), sample.module("b"));
        let a = sample.item(a_module, "A", ItemKind::Type);
        let a_x = sample.member(a, a_module, "A::x", ItemKind::Function);
        let b = sample.item(b_module, "B", ItemKind::Type);
        let b_run = sample.member(b, b_module, "B::run", ItemKind::Function);
        let c = sample.item(b_module, "C", ItemKind::Type);
        // The cheapest way round, from `A` by one item, is back by two.
        sample.refer(a, b);
        sample.refer(b_run, c);
        sample.refer(c, a);
        sample.refer(b_run, a);
        sample.refer(b_run, a_x);
        let graph = Graph::new(&sample.model);
        let diagnostics = check(&sample.model, &graph, None);
        let Picture::Circle { steps, rest } = &diagnostics[0].picture else {
            panic!("a tangle draws a circle");
        };
        let drawn: Vec<(&str, &str)> = steps
            .iter()
            .map(|step| (step.member.as_str(), step.uses.as_str()))
            .collect();
        assert_eq!(
            drawn,
            [
                ("p::a::A", "A uses B"),
                ("p::b::B", "B::run uses A::{self, x}")
            ]
        );
        assert_eq!(
            rest.as_deref(),
            Some("not drawn: 1 more thing, 2 more ties")
        );
    }

    #[test]
    fn an_impl_block_written_elsewhere_ties_its_type_from_there() {
        let mut sample = Sample::new();
        let (scheduling, elsewhere) = (sample.module("scheduling"), sample.module("elsewhere"));
        let tasks = sample.item(scheduling, "Tasks", ItemKind::Type);
        let timers = sample.item(scheduling, "Timers", ItemKind::Type);
        let spawn = sample.member(tasks, scheduling, "Tasks::spawn", ItemKind::Function);
        let fire = sample.member(timers, elsewhere, "Timers::fire", ItemKind::Function);
        sample.refer(spawn, timers);
        sample.refer(fire, tasks);
        let graph = Graph::new(&sample.model);
        let diagnostics = check(&sample.model, &graph, None);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message,
            "2 things know each other in a circle, across 2 modules"
        );
        assert_eq!(
            diagnostics[0].notes[1],
            "written in: p::elsewhere, p::scheduling"
        );
    }

    #[test]
    fn a_tie_written_in_two_modules_is_shown_in_both() {
        let mut sample = Sample::new();
        let (tail, writer) = (sample.module("tail"), sample.module("writer"));
        let wal_tail = sample.item(tail, "WalTail", ItemKind::Type);
        let scanned = sample.item(tail, "Scanned", ItemKind::Type);
        let scan_bytes = sample.member(wal_tail, tail, "WalTail::scan_bytes", ItemKind::Function);
        let scan_file = sample.member(wal_tail, writer, "WalTail::scan_file", ItemKind::Function);
        let records = sample.member(scanned, tail, "Scanned::records", ItemKind::Field);
        sample.refer(scan_bytes, scanned);
        sample.refer(scan_file, records);
        sample.refer(scanned, wal_tail);
        let graph = Graph::new(&sample.model);
        let notes = &check(&sample.model, &graph, None)[0].notes;
        assert_eq!(notes[1], "written in: p::tail, p::writer");
        assert!(
            notes[3].ends_with("; also at src/writer.rs:2:1"),
            "{}",
            notes[3]
        );
    }
}
