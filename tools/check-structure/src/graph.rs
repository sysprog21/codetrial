//! Who knows what. A thing is a type with everything written for it (its
//! fields, variants, associated items and methods, wherever their `impl`
//! blocks are), a free function, or a free constant or static. A use is a
//! reference from non-test code to an item of a non-test thing, seen through
//! wrappers: a use of a forwarder is a use of the function at the end of its
//! chain and a read of the fields of `self` the chain calls on, a use of an
//! accessor is a use of its fields, and what a wrapper's own item names is no
//! one's use. A
//! thing knows another if its code uses one of the other's items; things that
//! know each other in a circle are a tangle.

use crate::model::{ItemId, ItemKind, Model, NodeId, Reference, Wrapper};

pub struct Graph {
    /// By [`ThingId`].
    pub things: Vec<Thing>,
    /// By [`ItemId`]: the thing each item is part of.
    pub thing_of: Vec<Option<ThingId>>,
    pub uses: Vec<Use>,
    /// Each set of two or more things that know each other in a circle,
    /// sorted, by [`TangleId`].
    pub tangles: Vec<Vec<ThingId>>,
    /// By [`ThingId`]: the tangle each thing is in.
    pub tangle_of: Vec<Option<TangleId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThingId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TangleId(u32);

#[derive(Debug)]
pub struct Thing {
    /// The type, function, or constant.
    pub item: ItemId,
    /// The module it is defined in, which is also its methods' home.
    pub home: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Use {
    /// The reference, by its place in the model's.
    pub reference: usize,
    /// The thing whose code holds the reference: the function, type,
    /// field or constant it is in, or that one's type.
    pub user: Option<ThingId>,
    /// What is used, past wrappers.
    pub item: ItemId,
    /// The wrapper the reference names, if it names one.
    pub through: Option<ItemId>,
    /// For a field: the user only reads its value, directly or through
    /// wrappers that hand out nothing else.
    pub copies: bool,
}

impl Graph {
    pub fn new(model: &Model) -> Graph {
        let (things, thing_of) = things(model);
        let uses = uses(model, &thing_of);
        let mut knows: Vec<Vec<usize>> = vec![Vec::new(); things.len()];
        for usage in &uses {
            if let (Some(user), Some(used)) = (usage.user, thing_of[usage.item.index()])
                && user != used
            {
                knows[user.index()].push(used.index());
            }
        }
        let tangles: Vec<Vec<ThingId>> = circles(&knows)
            .into_iter()
            .map(|members| members.into_iter().map(ThingId::new).collect())
            .collect();
        let mut tangle_of = vec![None; things.len()];
        for (index, members) in tangles.iter().enumerate() {
            for member in members {
                tangle_of[member.index()] = Some(TangleId::new(index));
            }
        }
        Graph {
            things,
            thing_of,
            uses,
            tangles,
            tangle_of,
        }
    }
}

/// Each thing, and the thing each item is part of: the item at the top of its
/// chain of parents, if that is a non-test type, function or constant.
fn things(model: &Model) -> (Vec<Thing>, Vec<Option<ThingId>>) {
    let tree = &model.tree;
    let counted = |id: ItemId| {
        let item = &model.items[id.index()];
        item.kind != ItemKind::Module
            && !tree.is_test(item.owner)
            && item.file.is_none_or(|file| !model.files[file.index()].test)
    };
    let mut things = Vec::new();
    let mut of_root: Vec<Option<ThingId>> = vec![None; model.items.len()];
    let mut thing_of = vec![None; model.items.len()];
    for (index, part) in thing_of.iter_mut().enumerate() {
        let id = ItemId::new(index);
        let mut root = id;
        while let Some(parent) = model.items[root.index()].parent {
            root = parent;
        }
        let item = &model.items[root.index()];
        let is_thing = matches!(
            item.kind,
            ItemKind::Type | ItemKind::Function | ItemKind::Constant
        );
        if !is_thing || !counted(id) || !counted(root) {
            continue;
        }
        let thing = *of_root[root.index()].get_or_insert_with(|| {
            things.push(Thing {
                item: root,
                home: item.owner,
            });
            ThingId::new(things.len() - 1)
        });
        *part = Some(thing);
    }
    (things, thing_of)
}

/// The uses of non-test code, seen through wrappers.
fn uses(model: &Model, thing_of: &[Option<ThingId>]) -> Vec<Use> {
    let is_wrapper = |id: ItemId| model.wrappers.contains_key(&id);
    let mut uses = Vec::new();
    for (index, reference) in model.references.iter().enumerate() {
        if reference.test || reference.holder.is_some_and(is_wrapper) {
            continue;
        }
        let user = reference.holder.and_then(|holder| thing_of[holder.index()]);
        let through = Some(reference.item).filter(|&item| is_wrapper(item));
        behind(model, reference, |item, copies| {
            if thing_of[item.index()].is_some() {
                uses.push(Use {
                    reference: index,
                    user,
                    item,
                    through,
                    copies,
                });
            }
        });
    }
    uses
}

/// What `reference` uses, and whether it only reads each: the item it
/// names, or what is behind that if it is a wrapper. A forwarder only reads
/// the fields it calls on: its callers call the function behind it and never
/// see them. What an accessor lends is only read where the code that calls
/// it only reads it.
fn behind(model: &Model, reference: &Reference, mut emit: impl FnMut(ItemId, bool)) {
    let mut at = reference.item;
    loop {
        match model.wrappers.get(&at) {
            Some(Wrapper::Forwarder { target, fields }) => {
                for &field in fields {
                    emit(field, true);
                }
                at = *target;
            }
            Some(Wrapper::Accessor { lent, copied, .. }) => {
                for &field in lent {
                    emit(field, reference.copies);
                }
                for &field in copied {
                    emit(field, true);
                }
                return;
            }
            None => {
                let direct = at == reference.item;
                emit(at, direct && reference.copies);
                return;
            }
        }
    }
}

impl ThingId {
    fn new(index: usize) -> ThingId {
        ThingId(u32::try_from(index).expect("fewer than 2^32 things"))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl TangleId {
    fn new(index: usize) -> TangleId {
        TangleId(u32::try_from(index).expect("fewer than 2^32 tangles"))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

// Strongly connected components (Tarjan's algorithm, with a stack of its own
// instead of recursion, so a long chain cannot overflow the thread's).

/// Each component of two or more nodes, sorted, in order of their first node.
/// `edges[node]` are the nodes `node` reaches in one step.
pub fn circles(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let count = edges.len();
    let mut index = vec![UNSEEN; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut next = 0;
    let mut found = Vec::new();
    for start in 0..count {
        if index[start] != UNSEEN {
            continue;
        }
        let mut work = vec![(start, 0)];
        index[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&(node, edge)) = work.last() {
            if let Some(&to) = edges[node].get(edge) {
                work.last_mut().expect("the node just seen").1 += 1;
                if index[to] == UNSEEN {
                    index[to] = next;
                    low[to] = next;
                    next += 1;
                    stack.push(to);
                    on_stack[to] = true;
                    work.push((to, 0));
                } else if on_stack[to] {
                    low[node] = low[node].min(index[to]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                if component.len() > 1 {
                    component.sort_unstable();
                    found.push(component);
                }
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::Sample;

    #[test]
    fn a_method_written_anywhere_is_part_of_its_type_and_things_that_use_each_other_are_a_tangle() {
        let mut sample = Sample::new();
        let (a, b) = (sample.module("a"), sample.module("b"));
        let tasks = sample.item(a, "Tasks", ItemKind::Type);
        let timers = sample.item(b, "Timers", ItemKind::Type);

        // `impl Timers { fn fire(&mut self, tasks: &mut Tasks) { .. } }`,
        // written in `a`.
        let fire = sample.member(timers, a, "Timers::fire", ItemKind::Function);
        let spawn = sample.member(tasks, a, "Tasks::spawn", ItemKind::Function);
        sample.refer(fire, tasks);
        sample.refer(spawn, timers);
        let graph = Graph::new(&sample.model);
        let of_fire = graph.thing_of[fire.index()].unwrap();
        assert_eq!(graph.things[of_fire.index()].item, timers);
        assert_eq!(graph.things[of_fire.index()].home, b);
        assert_eq!(graph.tangles.len(), 1);
        assert_eq!(graph.tangles[0].len(), 2);
    }

    #[test]
    fn a_use_of_a_wrapper_is_a_use_of_what_is_behind_it_and_its_own_body_is_no_use() {
        let mut sample = Sample::new();
        let (a, b) = (sample.module("a"), sample.module("b"));
        let queue = sample.item(a, "Queue", ItemKind::Type);
        let inner = sample.item(a, "Inner", ItemKind::Type);
        let items = sample.member(inner, a, "Inner::items", ItemKind::Field);
        let field = sample.member(queue, a, "Queue::inner", ItemKind::Field);
        // `fn items(&self) -> &Vec<u32> { &self.items }`
        let getter = sample.member(inner, a, "Inner::items", ItemKind::Function);
        let accessor = Wrapper::Accessor {
            lent: vec![items],
            copied: Vec::new(),
            to_read: true,
        };
        // `fn items(&self) -> &Vec<u32> { self.inner.items() }`
        let forwarder = sample.member(queue, a, "Queue::items", ItemKind::Function);
        let forwarding = Wrapper::Forwarder {
            target: getter,
            fields: vec![field],
        };
        sample.model.wrappers.insert(getter, accessor);
        sample.model.wrappers.insert(forwarder, forwarding);
        sample.refer(getter, items);
        sample.refer(forwarder, field);
        sample.refer(forwarder, getter);
        let report = sample.item(b, "report", ItemKind::Function);
        sample.refer(report, forwarder);
        let graph = Graph::new(&sample.model);
        let used: Vec<(ItemId, Option<ItemId>)> =
            graph.uses.iter().map(|u| (u.item, u.through)).collect();
        assert_eq!(used, [(field, Some(forwarder)), (items, Some(forwarder))]);
        let reporter = graph.thing_of[report.index()];
        assert!(graph.uses.iter().all(|u| u.user == reporter));
    }

    #[test]
    fn finds_each_circle_and_leaves_out_the_rest() {
        // 0 -> 1 -> 2 -> 0, 2 -> 3, 3 -> 4 -> 3, 5 alone.
        let edges = [vec![1], vec![2], vec![0, 3], vec![4], vec![3], vec![]];
        assert_eq!(circles(&edges), [vec![0, 1, 2], vec![3, 4]]);
    }
}
