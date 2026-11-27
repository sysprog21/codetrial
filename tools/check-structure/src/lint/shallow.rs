//! `shallow-module`: a crate, or a module with child modules, that shows more
//! functions than a reader holds in mind, at least as many as it hides, and a
//! list that long: more things, or more functions of one thing. A reader
//! learns an interface one list at a time, the things it shows and then the
//! functions of one thing, so a few small types pass where one wide type does
//! not. A thing's functions are its methods and the module's free functions
//! that use it; one that uses none of the module's types is a thing of its
//! own. It counts its working functions: those that are not wrappers and not
//! of a trait's `impl` block, each at its thing's home, so a method at its
//! type's. It shows those that code outside it uses, through wrappers; code is
//! outside by its function's home, or by its module where no function holds
//! it. It shows a thing when it shows one of its functions.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{
    Diagnostic, HELD_IN_MIND, Lint, Picture, counted, in_scope, listed, module_location,
    without_generics,
};
use crate::graph::{Graph, ThingId};
use crate::model::{ItemId, ItemKind, Model, NodeId, NodeKind, Tree};

/// What a module hides for each function it shows, at break-even. A module's
/// interface is the cost of using it and what it hides is the benefit; *A
/// Philosophy of Software Design* calls a module shallow when "the benefit of
/// not having to learn about how they work internally is negated by the cost
/// of learning and using their interfaces". The two are equal when it shows
/// as much as it hides.
const BREAK_EVEN: usize = 1;

pub fn check(model: &Model, graph: &Graph, scope: Option<NodeId>) -> Vec<Diagnostic> {
    let tree = &model.tree;
    let judged = judged(tree);
    let home =
        |item: ItemId| graph.thing_of[item.index()].map(|thing| graph.things[thing.index()].home);
    let working = |item: ItemId| {
        let function = &model.items[item.index()];
        function.kind == ItemKind::Function
            && !function.trait_impl
            && !model.wrappers.contains_key(&item)
    };
    let mut total: HashMap<NodeId, usize> = HashMap::new();
    for index in 0..model.items.len() {
        let item = ItemId::new(index);
        if let Some(home) = home(item).filter(|_| working(item)) {
            for module in modules_around(tree, home).filter(|m| judged.contains(m)) {
                *total.entry(module).or_default() += 1;
            }
        }
    }
    let mut shown: HashMap<NodeId, BTreeSet<ItemId>> = HashMap::new();
    for usage in &graph.uses {
        let Some(at) = home(usage.item).filter(|_| working(usage.item)) else {
            continue;
        };
        let reference = &model.references[usage.reference];
        let user = usage
            .user
            .map_or(reference.from, |user| graph.things[user.index()].home);
        for module in modules_around(tree, at) {
            if within(tree, user, module) {
                break;
            }
            if judged.contains(&module) {
                shown.entry(module).or_default().insert(usage.item);
            }
        }
    }
    let known = types_known(model, graph);
    let mut diagnostics = Vec::new();
    for module in judged {
        let total = total.get(&module).copied().unwrap_or(0);
        let Some(shown) = shown.get(&module) else {
            continue;
        };
        let hidden = total.saturating_sub(shown.len());
        let (notes, long) = too_long(&lists(model, graph, &known, module, shown));
        let shallow =
            shown.len() > HELD_IN_MIND && !notes.is_empty() && hidden <= shown.len() * BREAK_EVEN;
        if shallow && in_scope(tree, scope, module) {
            let name = tree.display(module);
            diagnostics.push(Diagnostic {
                lint: Lint::ShallowModule,
                key: name.clone(),
                message: format!(
                    "`{name}` is shallow: it shows {} of its {}",
                    shown.len(),
                    counted(total, "function")
                ),
                location: module_location(model, module),
                notes,
                more_ties: Vec::new(),
                picture: Picture::Lists {
                    shown: shown.len(),
                    hidden,
                    lists: long,
                },
            });
        }
    }
    diagnostics
}

/// The types each free function knows: those whose items its code uses.
fn types_known(model: &Model, graph: &Graph) -> HashMap<ThingId, BTreeSet<ThingId>> {
    let kind = |thing: ThingId| model.items[graph.things[thing.index()].item.index()].kind;
    let mut known: HashMap<ThingId, BTreeSet<ThingId>> = HashMap::new();
    for usage in &graph.uses {
        if let (Some(user), Some(used)) = (usage.user, graph.thing_of[usage.item.index()])
            && kind(user) == ItemKind::Function
            && kind(used) == ItemKind::Type
        {
            known.entry(user).or_default().insert(used);
        }
    }
    known
}

/// The lists a reader learns what `module` shows by, one at a time: each
/// thing it shows, by name, with the names of its functions there, widest
/// first. A method is on its type's list, and a free function on the list
/// of each type of `module` it knows, or on its own if it knows none.
fn lists(
    model: &Model,
    graph: &Graph,
    known: &HashMap<ThingId, BTreeSet<ThingId>>,
    module: NodeId,
    shown: &BTreeSet<ItemId>,
) -> Vec<(String, Vec<String>)> {
    let name = |item: ItemId| without_generics(&model.items[item.index()].name);
    let of_module = |ty: &ThingId| within(&model.tree, graph.things[ty.index()].home, module);
    let mut lists: BTreeMap<ThingId, Vec<String>> = BTreeMap::new();
    for &function in shown {
        let Some(thing) = graph.thing_of[function.index()] else {
            continue;
        };
        let mut on = vec![thing];
        if graph.things[thing.index()].item == function {
            let types = known.get(&thing).into_iter().flatten();
            let types: Vec<ThingId> = types.filter(|ty| of_module(ty)).copied().collect();
            if !types.is_empty() {
                on = types;
            }
        }
        for list in on {
            lists.entry(list).or_default().push(name(function));
        }
    }
    let mut lists: Vec<(String, Vec<String>)> = lists
        .into_iter()
        .map(|(thing, mut functions)| {
            functions.sort();
            (name(graph.things[thing.index()].item), functions)
        })
        .collect();
    lists.sort();
    lists.sort_by_key(|(_, functions)| Reverse(functions.len()));
    lists
}

/// A note for each of `lists` longer than a reader holds in mind, the list
/// of things first, then each thing's functions; and each such list's name
/// and length, `things` for the list of things.
fn too_long(lists: &[(String, Vec<String>)]) -> (Vec<String>, Vec<(String, usize)>) {
    let (mut notes, mut long) = (Vec::new(), Vec::new());
    if lists.len() > HELD_IN_MIND {
        let names: Vec<String> = lists.iter().map(|(thing, _)| thing.clone()).collect();
        let count = counted(lists.len(), "thing");
        notes.push(format!("shows {count}: {}", listed(&names)));
        long.push(("things".to_owned(), lists.len()));
    }
    for (thing, functions) in lists {
        if functions.len() > HELD_IN_MIND {
            let count = counted(functions.len(), "function");
            notes.push(format!("`{thing}` shows {count}: {}", listed(functions)));
            long.push((thing.clone(), functions.len()));
        }
    }
    (notes, long)
}

/// Every crate, and every non-test module with child modules.
fn judged(tree: &Tree) -> BTreeSet<NodeId> {
    let mut parents = BTreeSet::new();
    for id in tree.ids() {
        if let Some(parent) = tree.parent(id).filter(|_| !tree.is_test(id)) {
            parents.insert(parent);
        }
    }
    tree.ids()
        .filter(|&id| {
            !tree.is_test(id)
                && match tree.node(id).kind {
                    NodeKind::Package => true,
                    NodeKind::Module => parents.contains(&id),
                    NodeKind::Workspace => false,
                }
        })
        .collect()
}

/// `node`, then the modules around it, up to its crate.
fn modules_around(tree: &Tree, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let package = tree.package_of(node);
    tree.ancestors(node).take_while(move |&a| {
        tree.package_of(a) == package && tree.node(a).kind != NodeKind::Workspace
    })
}

/// Whether `node` is in `module`'s subtree, its crate's code and not another
/// crate's.
fn within(tree: &Tree, node: NodeId, module: NodeId) -> bool {
    tree.is_within(node, module) && tree.package_of(node) == tree.package_of(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Wrapper;
    use crate::model::tests::Sample;

    /// `store`, with a child module, whose `Store` has `shown` methods that
    /// `app` calls and `hidden` that only `Store` calls.
    fn store(shown: usize, hidden: usize) -> Sample {
        let mut sample = Sample::new();
        let (store, app) = (sample.module("store"), sample.module("app"));
        sample.module("store::table");
        let ty = sample.item(store, "Store", ItemKind::Type);
        let run = sample.item(app, "run", ItemKind::Function);
        let helpers: Vec<ItemId> = (0..hidden)
            .map(|i| sample.member(ty, store, &format!("Store::helper{i}"), ItemKind::Function))
            .collect();
        for i in 0..shown {
            let method = sample.member(ty, store, &format!("Store::m{i}"), ItemKind::Function);
            sample.refer(run, method);
            for &helper in &helpers {
                sample.refer(method, helper);
            }
        }
        sample
    }

    /// `store`, with a child module, that hides nothing and shows to `app` a
    /// type `T{i}` with `widths[i]` methods for each `i`, and `free` free
    /// functions `f{i}`.
    fn things(widths: &[usize], free: usize) -> Sample {
        let mut sample = Sample::new();
        let (store, app) = (sample.module("store"), sample.module("app"));
        sample.module("store::table");
        let run = sample.item(app, "run", ItemKind::Function);
        for (t, &width) in widths.iter().enumerate() {
            let ty = sample.item(store, &format!("T{t}"), ItemKind::Type);
            for i in 0..width {
                let method = sample.member(ty, store, &format!("T{t}::m{i}"), ItemKind::Function);
                sample.refer(run, method);
            }
        }
        for i in 0..free {
            let function = sample.item(store, &format!("f{i}"), ItemKind::Function);
            sample.refer(run, function);
        }
        sample
    }

    /// Adds to `store` a free function `name` that `app` calls and whose code
    /// uses each of `types`.
    fn knowing(sample: &mut Sample, name: &str, types: &[&str]) {
        let store = sample.module("store");
        let function = sample.item(store, name, ItemKind::Function);
        let run = named(sample, "run");
        sample.refer(run, function);
        for ty in types {
            let ty = named(sample, ty);
            sample.refer(function, ty);
        }
    }

    fn named(sample: &Sample, name: &str) -> ItemId {
        let items = &sample.model.items;
        ItemId::new(items.iter().position(|item| item.name == name).unwrap())
    }

    fn keys(sample: &Sample) -> Vec<String> {
        let graph = Graph::new(&sample.model);
        check(&sample.model, &graph, None)
            .into_iter()
            .map(|d| d.key)
            .collect()
    }

    fn notes(sample: &Sample) -> Vec<String> {
        let graph = Graph::new(&sample.model);
        let mut diagnostics = check(&sample.model, &graph, None);
        assert_eq!(diagnostics.len(), 1);
        diagnostics.remove(0).notes
    }

    #[test]
    fn a_module_that_shows_more_than_it_hides_and_more_than_seven_is_shallow() {
        assert_eq!(keys(&store(8, 8)), ["p::store"]);
        let sample = store(8, 8);
        let graph = Graph::new(&sample.model);
        assert_eq!(
            check(&sample.model, &graph, None)[0].picture,
            Picture::Lists {
                shown: 8,
                hidden: 8,
                lists: vec![("Store".into(), 8)],
            }
        );
        assert_eq!(keys(&store(8, 9)), Vec::<String>::new());
        assert_eq!(keys(&store(7, 0)), Vec::<String>::new());
    }

    #[test]
    fn seven_is_a_limit_on_each_list_not_on_their_sum() {
        assert_eq!(keys(&things(&[5, 5, 5], 0)), Vec::<String>::new());
        assert_eq!(keys(&things(&[5, 4, 3], 1)), Vec::<String>::new());
        assert_eq!(keys(&things(&[7; 7], 0)), Vec::<String>::new());
        assert_eq!(keys(&things(&[], 7)), Vec::<String>::new());
        assert_eq!(keys(&things(&[], 8)), ["p::store"]);
        assert_eq!(keys(&things(&[8], 0)), ["p::store"]);
    }

    #[test]
    fn the_notes_name_each_list_that_is_too_long_the_things_first_then_the_widest() {
        assert_eq!(
            notes(&things(&[2, 8], 5)),
            [
                "`T1` shows 8 functions: T1::m0, T1::m1, T1::m2, T1::m3, T1::m4, T1::m5, T1::m6, +1 more"
            ]
        );
        assert_eq!(
            notes(&things(&[8, 9, 9], 5)),
            [
                "shows 8 things: T1, T2, T0, f0, f1, f2, f3, +1 more",
                "`T1` shows 9 functions: T1::m0, T1::m1, T1::m2, T1::m3, T1::m4, T1::m5, T1::m6, +2 more",
                "`T2` shows 9 functions: T2::m0, T2::m1, T2::m2, T2::m3, T2::m4, T2::m5, T2::m6, +2 more",
                "`T0` shows 8 functions: T0::m0, T0::m1, T0::m2, T0::m3, T0::m4, T0::m5, T0::m6, +1 more",
            ]
        );
        assert_eq!(
            notes(&things(&[1], 7)),
            ["shows 8 things: T0, f0, f1, f2, f3, f4, f5, +1 more"]
        );
    }

    #[test]
    fn a_free_function_is_on_the_list_of_each_type_of_the_module_it_knows() {
        let mut sample = things(&[6], 0);
        knowing(&mut sample, "fa", &["T0"]);
        knowing(&mut sample, "fb", &["T0"]);
        assert_eq!(keys(&sample), ["p::store"]);
        assert_eq!(keys(&things(&[6], 2)), Vec::<String>::new());

        let mut sample = things(&[6, 6], 0);
        knowing(&mut sample, "both", &["T0", "T1"]);
        knowing(&mut sample, "first", &["T0"]);
        knowing(&mut sample, "second", &["T1"]);
        assert_eq!(
            notes(&sample),
            [
                "`T0` shows 8 functions: T0::m0, T0::m1, T0::m2, T0::m3, T0::m4, T0::m5, both, +1 more",
                "`T1` shows 8 functions: T1::m0, T1::m1, T1::m2, T1::m3, T1::m4, T1::m5, both, +1 more",
            ]
        );
    }

    #[test]
    fn a_free_function_that_knows_no_type_of_the_module_is_a_thing_of_its_own() {
        let mut sample = things(&[], 7);
        let app = sample.module("app");
        sample.item(app, "Outside", ItemKind::Type);
        knowing(&mut sample, "e", &["Outside"]);
        assert_eq!(
            notes(&sample),
            ["shows 8 things: e, f0, f1, f2, f3, f4, f5, +1 more"]
        );
    }

    #[test]
    fn lists_that_share_a_free_function_never_make_a_module_shallow_that_shows_seven() {
        let mut sample = things(&[0; 8], 0);
        let types: Vec<String> = (0..8).map(|t| format!("T{t}")).collect();
        let types: Vec<&str> = types.iter().map(String::as_str).collect();
        knowing(&mut sample, "build", &types);
        assert_eq!(keys(&sample), Vec::<String>::new());
    }

    #[test]
    fn wrappers_count_for_nothing_and_a_method_counts_at_its_types_home() {
        let mut sample = store(8, 8);
        let ty = ItemId::new(0);
        let (store, app) = (sample.module("store"), sample.module("app"));
        // Eight private forwarders do not hide anything.
        for i in 0..8 {
            let forwarder = sample.member(ty, store, &format!("Store::pad{i}"), ItemKind::Function);
            let target = ItemId::new(3);
            let wrapper = Wrapper::Forwarder {
                target,
                fields: Vec::new(),
            };
            sample.model.wrappers.insert(forwarder, wrapper);
        }
        // A method written in `app` is still the store's.
        let moved = sample.member(ty, app, "Store::moved", ItemKind::Function);
        let run = ItemId::new(1);
        sample.refer(run, moved);
        let graph = Graph::new(&sample.model);
        let diagnostics = check(&sample.model, &graph, None);
        assert_eq!(
            diagnostics[0].message,
            "`p::store` is shallow: it shows 9 of its 17 functions"
        );
    }
}
