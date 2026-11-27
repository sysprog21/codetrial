//! Which functions are wrappers, forwarders and accessors, told by the names
//! their code holds and by their text.

use std::collections::HashMap;

use super::forwarders::{self, Name};
use super::{Builder, Named, accessors};
use crate::model::{ItemId, ItemKind, Wrapper};
use crate::scip::{Impl, Kind, Occurrence, Place};

impl<'a> Builder<'a> {
    /// Every forwarder and accessor; also marks the functions that take `self`.
    pub(super) fn wrappers(&mut self) -> HashMap<ItemId, Wrapper> {
        let definitions = self.function_names();
        for function in definitions.taking_self() {
            self.items[function.index()].takes_self = true;
        }
        let mut wrappers = HashMap::new();
        for forwarding in definitions.forwardings() {
            if !reaches(&wrappers, forwarding.target, forwarding.forwarder) {
                let wrapper = Wrapper::Forwarder {
                    target: forwarding.target,
                    fields: forwarding.fields,
                };
                wrappers.insert(forwarding.forwarder, wrapper);
            }
        }
        let mut named: HashMap<ItemId, Vec<(ItemId, bool)>> = HashMap::new();
        for reference in &self.references {
            if let Some(holder) = reference.holder.filter(|h| self.functions.contains(h)) {
                let name = (reference.item, reference.copies);
                named.entry(holder).or_default().push(name);
            }
        }
        for (function, names) in named {
            if !wrappers.contains_key(&function)
                && let Some(accessor) = self.accessor(function, &names)
            {
                wrappers.insert(function, accessor);
            }
        }
        wrappers
    }

    /// What `function` is as an accessor, if it is one; `names` are the items
    /// its code names, and whether it only reads each.
    fn accessor(&self, function: ItemId, names: &[(ItemId, bool)]) -> Option<Wrapper> {
        let item = &self.items[function.index()];
        let inherent = matches!(
            self.places.get(&function),
            Some(Place::Impl(Impl {
                of_trait: false,
                is_block: false,
                ..
            }))
        );
        let own = item.parent.filter(|_| inherent && item.takes_self)?;
        let (at, range) = (*self.definitions.get(&function)?)?;
        let text = self.units[at].source.function(range)?;
        let read_only = |field: ItemId| {
            let mut mentions = names.iter().filter(|(name, _)| *name == field);
            mentions.all(|&(_, copies)| copies)
        };
        let names: Vec<accessors::Named> = names
            .iter()
            .map(|&(name, _)| {
                let named = &self.items[name.index()];
                match named.kind {
                    ItemKind::Field if named.parent == Some(own) => {
                        accessors::Named::OwnField(name)
                    }
                    ItemKind::Type | ItemKind::Module | ItemKind::Impl => accessors::Named::Type,
                    _ => accessors::Named::Other,
                }
            })
            .collect();
        let fields = accessors::fields(&text, &names, |field| {
            match self.definitions.get(&field).copied().flatten() {
                Some((at, range)) => self.units[at].source.field_types(range),
                None => Vec::new(),
            }
        })?;
        let (copied, lent) = fields
            .into_iter()
            .partition(|&field| text.whole || text.returns_copies || read_only(field));
        Some(Wrapper::Accessor {
            lent,
            copied,
            to_read: text.lends_to_read,
        })
    }

    /// What each function's item names, as forwarders are told by it.
    fn function_names(&self) -> forwarders::Definitions {
        let mut definitions = forwarders::Definitions::default();
        for unit in &self.units {
            let kinds: HashMap<&str, u64> = unit
                .document
                .symbols
                .iter()
                .map(|information| (information.symbol, information.kind))
                .collect();
            let mut occurrences: Vec<&Occurrence> = unit.document.occurrences.iter().collect();
            occurrences.sort_by_key(|occurrence| occurrence.range.start);
            for (block, function) in unit.holders.iter() {
                if !self.functions.contains(&function) {
                    continue;
                }
                let defines_function = |occurrence: &Occurrence| {
                    occurrence.definition
                        && self.symbols.get(occurrence.symbol) == Some(&Named::Item(function))
                };
                let start = occurrences.partition_point(|o| o.range.start < block.start);
                let names: Vec<Name> = occurrences[start..]
                    .iter()
                    .copied()
                    .take_while(|occurrence| block.holds(occurrence.range))
                    .skip_while(|occurrence| !defines_function(occurrence))
                    .skip(1)
                    .map(|occurrence| self.name(occurrence, &kinds))
                    .collect();
                let may_forward = matches!(
                    self.places.get(&function),
                    Some(
                        Place::Module
                            | Place::Impl(Impl {
                                of_trait: false,
                                ..
                            })
                    )
                );
                definitions.add(function, may_forward, &names);
            }
        }
        definitions
    }

    /// What `occurrence` names; `kinds` has the kinds of its document's
    /// symbols.
    fn name(&self, occurrence: &Occurrence<'a>, kinds: &HashMap<&str, u64>) -> Name<'a> {
        let symbol = occurrence.symbol;
        let Some(&named) = self.symbols.get(symbol) else {
            return Name::Other;
        };
        match named {
            Named::Local => local_name(symbol, kinds.get(symbol).copied(), occurrence.definition),
            _ if occurrence.definition => Name::Other,
            Named::Item(item) => match self.items[item.index()].kind {
                ItemKind::Module | ItemKind::Type | ItemKind::Impl => Name::Type,
                ItemKind::Field => Name::Field(item),
                ItemKind::Function => Name::Function(Some(item)),
                ItemKind::Variant | ItemKind::Constant | ItemKind::Other => Name::Other,
            },
            Named::Outside(kind) => match kind {
                Kind::Namespace | Kind::Type | Kind::TypeParameter => Name::Type,
                Kind::Method => Name::Function(None),
                Kind::Term | Kind::Parameter | Kind::Meta | Kind::Macro => Name::Other,
            },
        }
    }
}

/// Whether the chain of forwarders from `from` passes `to`.
fn reaches(wrappers: &HashMap<ItemId, Wrapper>, from: ItemId, to: ItemId) -> bool {
    let mut at = from;
    loop {
        if at == to {
            return true;
        }
        match wrappers.get(&at) {
            Some(Wrapper::Forwarder { target, .. }) => at = *target,
            _ => return false,
        }
    }
}

/// `kind`: `scip.proto`'s `SymbolInformation.Kind` of the local, if known.
fn local_name(symbol: &str, kind: Option<u64>, definition: bool) -> Name<'_> {
    const PARAMETER: u64 = 37;
    const SELF_PARAMETER: u64 = 44;
    const TYPE_PARAMETER: u64 = 58;
    match (kind, definition) {
        (Some(TYPE_PARAMETER), _) => Name::Type,
        (Some(PARAMETER), true) => Name::Parameter {
            local: symbol,
            is_self: false,
        },
        (Some(SELF_PARAMETER), true) => Name::Parameter {
            local: symbol,
            is_self: true,
        },
        (_, true) => Name::Other,
        (_, false) => Name::Value(symbol),
    }
}
