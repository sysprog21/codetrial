//! Accessors: inherent methods with a `self` parameter whose body only hands
//! out or overwrites fields of `self`. Such a body names one or more fields of
//! the method's own type and nothing else of the workspace but types, computes
//! nothing, and each of its fields' types is written in the signature.
//! `fn items(&self) -> &Vec<u32> { &self.items }`,
//! `fn set_mask(&mut self, set: u64) { self.set = set; }` and
//! `fn with_items<R>(&self, each: impl Fn(&Vec<u32>) -> R) -> R {
//! each(&self.items) }`
//! are accessors; `fn len(&self) -> usize { self.items.len() }` is not. How
//! the body is laid out plays no part: a `let` or a line break hides nothing.

use crate::model::ItemId;
use crate::source::FunctionText;

/// What a method's item names, as far as accessors are told by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// A field of the method's own type.
    OwnField(ItemId),
    /// A type, trait, module, or `Self`.
    Type,
    /// Any other item of the workspace's.
    Other,
}

/// The fields a method hands out or overwrites, if it is an accessor. `text`
/// is what its text says, `names` what its item names, and `types` gives each
/// field's declared type as `Source::field_types` does.
pub fn fields(
    text: &FunctionText,
    names: &[Named],
    types: impl Fn(ItemId) -> Vec<Vec<String>>,
) -> Option<Vec<ItemId>> {
    if text.computes {
        return None;
    }
    let mut fields = Vec::new();
    for name in names {
        match *name {
            Named::OwnField(field) => fields.push(field),
            Named::Type => {}
            Named::Other => return None,
        }
    }
    fields.sort();
    fields.dedup();
    let typed = fields.iter().all(|&field| text.writes_any(&types(field)));
    (!fields.is_empty() && typed).then_some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip::{Position, Range};
    use crate::source::Source;

    const ITEMS: ItemId = ItemId(0);
    const HEAD: ItemId = ItemId(1);

    /// `items: Vec<u32>` and `head: usize`.
    fn types(field: ItemId) -> Vec<Vec<String>> {
        let written = if field == ITEMS {
            "items: Vec<u32>"
        } else {
            "head: usize"
        };
        let source = Source::new(written.into());
        source.field_types(whole(written))
    }

    fn whole(text: &str) -> Range {
        let end = Position {
            line: 0,
            column: text.len() as u32,
        };
        Range {
            start: Position { line: 0, column: 0 },
            end,
        }
    }

    fn accessor(function: &str, names: &[Named]) -> Option<Vec<ItemId>> {
        let source = Source::new(function.into());
        let text = source.function(whole(function)).unwrap();
        fields(&text, names, types)
    }

    #[test]
    fn a_getter_a_setter_and_a_tuple_of_fields_are_accessors() {
        let items = [Named::Type, Named::OwnField(ITEMS)];
        assert_eq!(
            accessor("fn items(&self) -> &Vec<u32> { &self.items }", &items),
            Some(vec![ITEMS])
        );
        assert_eq!(
            accessor("fn items(&self) -> &[u32] { &self.items }", &items),
            Some(vec![ITEMS])
        );
        assert_eq!(
            accessor(
                "fn set(&mut self, head: usize) { self.head = head; }",
                &[Named::OwnField(HEAD)]
            ),
            Some(vec![HEAD])
        );
        let both = [Named::OwnField(ITEMS), Named::OwnField(HEAD)];
        assert_eq!(
            accessor(
                "fn parts(&self) -> (&Vec<u32>, usize) { (&self.items, self.head) }",
                &both
            ),
            Some(vec![ITEMS, HEAD])
        );
    }

    #[test]
    fn what_computes_or_hands_out_something_else_is_no_accessor() {
        let items = [Named::OwnField(ITEMS)];
        assert_eq!(
            accessor("fn len(&self) -> usize { self.items.len() }", &items),
            None
        );
        let both = [Named::OwnField(ITEMS), Named::OwnField(HEAD)];
        assert_eq!(
            accessor(
                "fn backlog(&self) -> usize { self.items.len() - self.head }",
                &both
            ),
            None
        );
        let calls = [Named::OwnField(HEAD), Named::Other];
        assert_eq!(
            accessor("fn head(&self) -> usize { shift(self.head) }", &calls),
            None
        );
        assert_eq!(accessor("fn none(&self) -> usize { 0 }", &[]), None);
        let head = [Named::OwnField(HEAD)];
        assert_eq!(
            accessor(
                "fn next(&self) -> usize { self.head.wrapping_add(1) }",
                &head
            ),
            None
        );
    }

    #[test]
    fn how_a_getter_is_laid_out_plays_no_part() {
        let items = [Named::OwnField(ITEMS)];
        assert_eq!(
            accessor(
                "fn items(&self) -> &Vec<u32> {\n    let items = &self.items;\n    items\n}",
                &items
            ),
            Some(vec![ITEMS])
        );
        assert_eq!(
            accessor(
                "fn with<R>(&self, each: impl Fn(&[u32]) -> R) -> R { each(&self.items) }",
                &items
            ),
            Some(vec![ITEMS])
        );
    }
}
