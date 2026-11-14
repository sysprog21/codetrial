//! Where a definition that starts at a line of the buffer ends, from its
//! tree-sitter tree, for a `read_editor` page that starts there.
//!
//! A change review shows the model where a definition begins, and to read it
//! the model pages from that line, a page that runs to the end of the buffer.
//! Knowing where the definition ends lets that page stop there, without a
//! parameter in the declaration, which every generation is billed for; a
//! `symbol` parameter that read a definition by name saved the same bytes in a
//! Live A/B and cost about a hundred tokens on every generation
//! (`docs/evidence-runtime.md`). A buffer that does not parse has no tree and
//! the page answers as before; a buffer with a syntax error still has one,
//! because a candidate mid-edit is the usual state and the definitions around
//! the error are still definitions.
//!
//! Nothing here recurses with the candidate's nesting: the lookup descends from
//! the root to the line, and wrappers and nested objects are looked through one
//! at a time. A worker's stack is the same however deep the code is nested, and
//! the cost is the depth of the tree at that line.

use crate::agent::evidence::{MAX_PARSED_BYTES, ParseCache, grammar_key, parse_with};
use tree_sitter::{Node, Point};

/// The last line of the outermost definition that starts at `line`, 1-based,
/// or `None` when none starts there, the language has no grammar, the buffer
/// is past the parse limit or the parser gave nothing. The tree comes from
/// `cache` when it holds this buffer, as it does once the runtime has observed
/// it, and is parsed afresh otherwise.
pub(crate) fn definition_end(
    language: &str,
    code: &str,
    cache: &ParseCache,
    line: usize,
) -> Option<usize> {
    let grammar = grammar_key(language)?;
    if code.len() > MAX_PARSED_BYTES {
        return None;
    }
    let row = line.checked_sub(1)?;
    let column = code.lines().nth(row)?.find(|c: char| !c.is_whitespace())?;
    let tree = cache
        .tree_for(grammar, code)
        .or_else(|| parse_with(grammar, code, None))?;
    let point = Point { row, column };
    let root = tree.root_node();
    let leaf = root.descendant_for_point_range(point, point)?;
    let mut node = root;
    while node.id() != leaf.id() {
        node = node.child_with_descendant(leaf)?;
        if node.start_position().row != row {
            continue;
        }
        if is_definition(node) {
            return Some(end_line(node));
        }

        if let Some(definition) = later_definition(node, row) {
            return Some(end_line(definition));
        }
    }
    None
}

/// A definition that starts later on `row` than the token the line opens with:
/// a function after `friend`, or a declarator in an export's declaration
/// (`export const f = ...`).
fn later_definition(node: Node<'_>, row: usize) -> Option<Node<'_>> {
    let declared = node.child_by_field_name("declaration");
    on_row(node, row)
        .chain(
            declared
                .into_iter()
                .flat_map(|declaration| on_row(declaration, row)),
        )
        .find(|child| is_definition(*child))
}

/// The named children of `node` that start on `row`, walked once with a
/// cursor: indexing each child rescans the ones before it, and children are in
/// source order, so none past `row` is visited.
fn on_row(node: Node<'_>, row: usize) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    let children = node
        .named_children(&mut cursor)
        .take_while(|child| child.start_position().row <= row)
        .filter(|child| child.start_position().row == row)
        .collect::<Vec<_>>();
    children.into_iter()
}

/// Whether a node of `kind` is a function, a method or a constructor.
fn is_function(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "generator_function_declaration"
            | "constructor_declaration"
            | "method_definition"
            | "method_declaration"
    )
}

/// Whether a node of `kind` is a class, struct, interface or enum.
fn is_type(kind: &str) -> bool {
    matches!(
        kind,
        "class_definition"
            | "class_declaration"
            | "class_specifier"
            | "record_declaration"
            | "struct_specifier"
            | "interface_declaration"
            | "enum_declaration"
            | "enum_specifier"
    )
}

/// Whether a node of `kind` wraps a definition: decorators, a template header,
/// a `typedef`.
fn is_wrapper(kind: &str) -> bool {
    matches!(
        kind,
        "decorated_definition" | "template_declaration" | "type_definition"
    )
}

/// Whether `node` defines something: a named function or type, a decorator,
/// template header or `typedef` around one, or a JavaScript binding of a
/// function, a class or an object of functions. Wrappers are looked through
/// one at a time, so a candidate's chain of them costs no stack.
fn is_definition(node: Node<'_>) -> bool {
    let mut node = node;
    while is_wrapper(node.kind()) {
        let mut cursor = node.walk();
        let inner = node.named_children(&mut cursor).find(|child| {
            let kind = child.kind();
            is_function(kind) || is_type(kind) || is_wrapper(kind)
        });
        let Some(inner) = inner else {
            return false;
        };
        node = inner;
    }
    let kind = node.kind();
    if is_function(kind) || is_type(kind) {
        // A C or C++ struct, class or enum without a body is a declaration or a
        // use of the type, not its definition.
        let declared_only =
            kind.ends_with("_specifier") && node.child_by_field_name("body").is_none();
        return !declared_only && is_named(node);
    }
    bound_value(node).is_some_and(defines_by_value)
}

/// The value a JavaScript node binds under a name: a declarator (`const f =
/// () => {}`), a class field or object property (`handler = (e) => {}`), an
/// assignment statement (`f = function () {}`, `exports.f = ...`,
/// `Foo.prototype.f = ...`), or `export default`.
fn bound_value(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "variable_declarator" | "field_definition" | "pair" => {
            ["name", "property", "key"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .filter(|name| name.kind().ends_with("identifier"))?;
            node.child_by_field_name("value")
        }
        "expression_statement" => {
            let assignment = node
                .named_child(0)
                .filter(|child| child.kind() == "assignment_expression")?;
            assignment
                .child_by_field_name("left")
                .filter(|left| matches!(left.kind(), "identifier" | "member_expression"))?;
            assignment.child_by_field_name("right")
        }
        // Only `export default` carries a value.
        "export_statement" => node.child_by_field_name("value"),
        _ => None,
    }
}

/// Whether a JavaScript value is a function or a class.
fn is_function_value(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "generator_function" | "class"
    )
}

/// Whether a JavaScript value defines something: a function, a class, or an
/// object with a function among its members or its nested objects' members.
/// Nested objects wait on a list rather than on the stack, so a candidate's
/// deeply nested literal costs no stack.
fn defines_by_value(value: Node<'_>) -> bool {
    let mut objects = vec![value];
    while let Some(value) = objects.pop() {
        if is_function_value(value.kind()) {
            return true;
        }
        if value.kind() != "object" {
            continue;
        }
        let mut cursor = value.walk();
        for member in value.named_children(&mut cursor) {
            if member.kind() == "method_definition" {
                return true;
            }
            if let Some(inner) = bound_value(member) {
                objects.push(inner);
            }
        }
    }
    false
}

/// Whether a definition is named: by a name, by a declarator (C and C++ put
/// the name at the bottom of one), or, for an anonymous struct, by the
/// `typedef` around it, whose declarator names it however it is shaped
/// (`Node`, `*NodePtr`, `Row[4]`). `struct { int a; } x;` defines no type of
/// its own.
fn is_named(node: Node<'_>) -> bool {
    node.child_by_field_name("name").is_some()
        || node.child_by_field_name("declarator").is_some()
        || node
            .parent()
            .filter(|parent| parent.kind() == "type_definition")
            .and_then(|parent| parent.child_by_field_name("declarator"))
            .is_some()
}

/// The last line a node occupies, 1-based: a node that ends at the start of a
/// line does not own that line. A definition is never empty, so it always
/// ends past its first line's start.
fn end_line(node: Node<'_>) -> usize {
    let end = node.end_position();
    if end.column == 0 {
        end.row
    } else {
        end.row + 1
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/agent/code_map.rs"]
mod tests;
