//! The plain model the graph is built from: the hierarchy, the workspace's
//! files, the items they define and what each is written for, every reference
//! from code to an item outside `use` declarations, and the functions that
//! only wrap others.

mod from_index;
mod tree;

pub use from_index::build;
pub use tree::{NodeId, NodeKind, Tree};

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Model {
    pub tree: Tree,
    /// By [`FileId`].
    pub files: Vec<File>,
    /// By [`ItemId`].
    pub items: Vec<Item>,
    pub references: Vec<Reference>,
    /// What each wrapper hands on. No chain of forwarders loops.
    pub wrappers: HashMap<ItemId, Wrapper>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(u32);

#[derive(Debug)]
pub struct File {
    /// Relative to the workspace's root, `/`-separated.
    pub path: String,
    pub module: NodeId,
    pub test: bool,
}

#[derive(Debug)]
pub struct Item {
    /// How its module writes it: `Ready`, `Ready::new`, `<u64 as
    /// Display>::fmt`.
    pub name: String,
    /// The module it is written in.
    pub owner: NodeId,
    /// The module it is, for a module.
    pub module: Option<NodeId>,
    pub file: Option<FileId>,
    pub kind: ItemKind,
    /// What it is written for: a type, for its fields, variants, associated
    /// items and methods, wherever their `impl` block is; a variant, for its
    /// fields. `None` for a free item, and for what an `impl` block of a type
    /// the crate does not define holds.
    pub parent: Option<ItemId>,
    /// A function of a trait's `impl` block.
    pub trait_impl: bool,
    /// A function with a `self` parameter.
    pub takes_self: bool,
    /// A field of a marker type, `PhantomData` or `PhantomPinned`, which
    /// holds nothing.
    pub holds_nothing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Module,
    /// A struct, enum, union, trait, or type alias.
    Type,
    /// An `impl` block, which code names as `Self`.
    Impl,
    Function,
    Field,
    Variant,
    /// A constant or static.
    Constant,
    Other,
}

#[derive(Debug)]
pub struct Reference {
    pub file: FileId,
    pub line: u32,
    /// The module it is written in.
    pub from: NodeId,
    /// The innermost item whose definition holds it: a function, type, field,
    /// variant or constant. `None` in an `impl` block's header.
    pub holder: Option<ItemId>,
    pub item: ItemId,
    /// For a field: the code only reads its value. It takes a copy, or it
    /// borrows plain data to read, or calls a method on a number; it neither
    /// writes the field nor reaches into it. For a call of an accessor,
    /// directly or through forwarders: the code reads so what it lends.
    pub copies: bool,
    pub test: bool,
}

/// A function that adds nothing of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrapper {
    /// Calls `target` and nothing else, passing its parameters on.
    /// `fields`: the fields of `self` it calls that function on.
    Forwarder { target: ItemId, fields: Vec<ItemId> },
    /// Hands out or overwrites fields of `self`, computing nothing: `lent`
    /// are those it lends or overwrites, `copied` those whose values it only
    /// hands out. `to_read`: it takes `&self` and overwrites nothing, so
    /// what it lends can only be read, and code that calls it does with the
    /// fields what it does with the call.
    Accessor {
        lent: Vec<ItemId>,
        copied: Vec<ItemId>,
        to_read: bool,
    },
}

impl FileId {
    pub fn new(index: usize) -> FileId {
        FileId(u32::try_from(index).expect("fewer than 2^32 files"))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl ItemId {
    pub fn new(index: usize) -> ItemId {
        ItemId(u32::try_from(index).expect("fewer than 2^32 items"))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[cfg(test)]
pub mod tests {
    //! Small models for the graph's and the lints' tests.

    use super::*;

    /// A model of one crate, `p`, with one file for each module.
    pub struct Sample {
        pub model: Model,
    }

    impl Sample {
        pub fn new() -> Sample {
            let mut tree = Tree::new();
            tree.child(tree.root(), NodeKind::Package, "p");
            Sample {
                model: Model {
                    tree,
                    ..Model::default()
                },
            }
        }

        /// The module at `path` below the crate, `""` for the crate itself.
        pub fn module(&mut self, path: &str) -> NodeId {
            let tree = &mut self.model.tree;
            let mut node = tree.child(tree.root(), NodeKind::Package, "p");
            for name in path.split("::").filter(|name| !name.is_empty()) {
                node = tree.child(node, NodeKind::Module, name);
            }
            if !self.model.files.iter().any(|file| file.module == node) {
                self.model.files.push(File {
                    path: if path.is_empty() {
                        "src/lib.rs".to_owned()
                    } else {
                        format!("src/{}.rs", path.replace("::", "/"))
                    },
                    module: node,
                    test: false,
                });
            }
            node
        }

        /// A file of `module`'s at `path`.
        pub fn file(&mut self, module: NodeId, path: &str) {
            self.model.files.push(File {
                path: path.into(),
                module,
                test: false,
            });
        }

        pub fn item(&mut self, owner: NodeId, name: &str, kind: ItemKind) -> ItemId {
            let file = self
                .model
                .files
                .iter()
                .position(|file| file.module == owner);
            self.model.items.push(Item {
                name: name.into(),
                owner,
                module: None,
                file: file.map(FileId::new),
                kind,
                parent: None,
                trait_impl: false,
                takes_self: false,
                holds_nothing: false,
            });
            ItemId::new(self.model.items.len() - 1)
        }

        /// An item of `parent`'s, written in `owner`.
        pub fn member(
            &mut self,
            parent: ItemId,
            owner: NodeId,
            name: &str,
            kind: ItemKind,
        ) -> ItemId {
            let id = self.item(owner, name, kind);
            self.model.items[id.index()].parent = Some(parent);
            id
        }

        /// A reference to `item` in `holder`'s code, on a line of its own.
        pub fn refer(&mut self, holder: ItemId, item: ItemId) {
            let from = self.model.items[holder.index()].owner;
            let file = self.model.files.iter().position(|file| file.module == from);
            self.model.references.push(Reference {
                file: FileId::new(file.expect("a module of the sample's")),
                line: u32::try_from(self.model.references.len()).unwrap() + 1,
                from,
                holder: Some(holder),
                item,
                copies: false,
                test: false,
            });
        }

        /// The same, where the code only takes a copy of `item`'s value.
        pub fn read(&mut self, holder: ItemId, item: ItemId) {
            self.refer(holder, item);
            self.model.references.last_mut().expect("just added").copies = true;
        }
    }
}
