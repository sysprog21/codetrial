//! Which type the items of each `impl` block are for. The index names an
//! `impl` block's type as it is written there, which may be an alias or a
//! path, so the block's own text decides where it can, and else the type of
//! that name nearest in the crate.

use std::cmp::Reverse;
use std::collections::HashMap;

use super::{Builder, Named, lines};
use crate::model::{ItemId, ItemKind, NodeId};
use crate::scip::{Impl, Place};
use crate::source::Source;

impl<'a> Builder<'a> {
    /// Sets what each item is written for: the item its symbol is declared in,
    /// or the type its `impl` block is for.
    pub(super) fn place_in_types(&mut self) {
        for (id, parent) in self.parents() {
            self.items[id.index()].parent = Some(parent);
        }
    }

    /// Each item written for another, and that other.
    fn parents(&self) -> Vec<(ItemId, ItemId)> {
        let mut blocks: HashMap<&'a str, (&Impl<'a>, Vec<ItemId>)> = HashMap::new();
        let mut parents = Vec::new();
        for (&id, place) in &self.places {
            match place {
                Place::Module => {}
                Place::In(container) => {
                    if let Some(&Named::Item(container)) = self.symbols.get(container) {
                        parents.push((id, container));
                    }
                }
                Place::Impl(block) => {
                    let (_, members) = blocks.entry(block.block).or_insert((block, Vec::new()));
                    members.push(id);
                }
            }
        }
        let types = self.types_by_name();
        for (block, mut members) in blocks.into_values() {
            members.sort();
            let owner = self.items[members[0].index()].owner;
            let written = members
                .iter()
                .find_map(|member| self.written_type(*member))
                .unwrap_or_else(|| self.same_named_type(&types, owner, &block.self_type));
            if let Some(ty) = written {
                parents.extend(members.iter().map(|&member| (member, ty)));
            }
        }
        parents
    }

    /// The type that the text of `member`'s `impl` block writes it for, if
    /// that text is found: `Some(None)` for a type the crate does not define.
    fn written_type(&self, member: ItemId) -> Option<Option<ItemId>> {
        let (at, range) = (*self.definitions.get(&member)?)?;
        let unit = &self.units[at];
        let block = unit
            .impl_blocks
            .iter()
            .filter(|block| block.block.holds(range))
            .min_by_key(|block| (lines(block.block), Reverse(block.block.start)))?;
        let Some(name) = block.self_type else {
            return Some(None);
        };
        let ty = unit
            .document
            .occurrences
            .iter()
            .find(|occurrence| occurrence.range == name && !occurrence.definition)
            .and_then(|occurrence| match self.symbols.get(occurrence.symbol) {
                Some(&Named::Item(ty)) => Some(ty),
                _ => None,
            })
            .filter(|&ty| {
                let tree = &self.tree;
                self.items[ty.index()].kind == ItemKind::Type
                    && tree.package_of(self.items[ty.index()].owner)
                        == tree.package_of(self.items[member.index()].owner)
            });
        Some(ty)
    }

    /// The crate's types that are written in a module, by their package and
    /// name.
    fn types_by_name(&self) -> HashMap<(NodeId, &str), Vec<ItemId>> {
        let mut types: HashMap<(NodeId, &str), Vec<ItemId>> = HashMap::new();
        for (id, place) in &self.places {
            let item = &self.items[id.index()];
            if item.kind == ItemKind::Type
                && *place == Place::Module
                && let Some(package) = self.tree.package_of(item.owner)
            {
                types.entry((package, &item.name)).or_default().push(*id);
            }
        }
        types
    }

    /// The type named as `written` closest to `owner` in its crate.
    fn same_named_type(
        &self,
        types: &HashMap<(NodeId, &str), Vec<ItemId>>,
        owner: NodeId,
        written: &str,
    ) -> Option<ItemId> {
        let name = Source::type_name(written)?;
        let package = self.tree.package_of(owner)?;
        let tree = &self.tree;
        types
            .get(&(package, name.as_str()))?
            .iter()
            .copied()
            .max_by_key(|&ty| {
                let home = self.items[ty.index()].owner;
                let meet = tree.common_ancestor(home, owner);

                // A type in the impl's own module and one in a child both meet
                // it at the impl's module; the one declared nearer wins.
                (
                    tree.node(meet).depth,
                    Reverse(tree.node(home).depth),
                    Reverse(ty),
                )
            })
    }
}
