//! Which tree-sitter node kinds count as control flow, loops, closures,
//! data structures and interfaces, for telling one kind of edit from another.

use super::CLOSURE_PREFIX;

/// Every spelling of a branch or a loop the five grammars use, because a list
/// that holds only the shapes C happens to name is a list that answers for one
/// language. `for (const x of items)` is a `for_in_statement` in JavaScript,
/// `for (int x : a)` is an `enhanced_for_statement` in Java and `for (auto x :
/// v)` is a `for_range_loop` in C++, and none of them is a `for_statement`: the
/// most ordinary loop a candidate writes in three of these tabs was reported as
/// an edit of no particular kind. A `do`/`while` is a `do_statement` in all
/// four C-family grammars and was missing for the same reason.
pub(super) fn control_kind(kind: &str) -> bool {
    loop_kind(kind)
        || matches!(
            kind,
            "if_statement"
                | "elif_clause"
                | "else_clause"
                | "while_statement"
                | "do_statement"
                | "switch_statement"
                | "switch_expression"
                | "case_statement"
                | "match_statement"
                | "try_statement"
                | "catch_clause"
                | "finally_clause"
                | "with_statement"
                | "break_statement"
                | "continue_statement"
        )
}

/// Every comment kind the pinned grammars have. Java has no `comment`, only
/// `line_comment` and `block_comment`, so matching the one word read every
/// Java comment edit as code and moved the review gate on it; JavaScript adds
/// `html_comment` for a `<!--` line.
pub(in crate::agent) fn comment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "line_comment" | "block_comment" | "html_comment"
    )
}

/// The loops that bind something, a subset of [`control_kind`]: `while` and
/// `do` iterate with nothing of their own.
pub(super) fn loop_kind(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement" | "for_in_statement" | "for_range_loop" | "enhanced_for_statement"
    )
}

/// A node kind with its closure nesting stripped, for the questions that do not
/// turn on it.
pub(super) fn unnest(kind: &str) -> &str {
    kind.strip_prefix(CLOSURE_PREFIX).unwrap_or(kind)
}

/// A function the candidate wrote inline to pass somewhere, as opposed to one a
/// caller can find by name. What is inside one is qualified by
/// [`syntax_facts`], and the kinds here are what that walk keys off.
pub(super) fn closure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "lambda" | "lambda_expression" | "arrow_function" | "function_expression"
    )
}

/// A list rather than a chain of alternatives, because the alternatives
/// overlap: every kind containing "dictionary" contains "dict" too, so one of
/// the two terms could never change the answer and nothing could be written to
/// tell whether it was wired up at all.
///
/// The exclusions are the price of matching on substrings. Several grammars
/// spell a syntactic grouping as a list: `argument_list` is what a call puts
/// its arguments in, so extracting a helper and calling it, which is the most
/// ordinary refactor an interview contains, was reported as the candidate
/// changing a data structure. It read as something else again in JavaScript,
/// where the same node is named `arguments`, so the same edit was described
/// differently
/// depending on the tab. A literal that really is a data structure keeps its
/// own name, C's `initializer_list` included.
pub(super) fn data_structure_kind(kind: &str) -> bool {
    const GROUPING: [&str; 6] = [
        "argument_list",
        "parameter_list",
        "expression_list",
        "declaration_list",
        "enumerator_list",
        "type_list",
    ];

    // A destructuring target is spelled as a list or an object in several
    // grammars, `pattern_list` in Python and `array_pattern` in JavaScript, and
    // unpacking `for i, n in enumerate(nums)` builds no structure at all.
    if kind.contains("pattern") || GROUPING.iter().any(|grouping| kind.contains(grouping)) {
        return false;
    }
    ["array", "list", "dict", "object", "map", "set"]
        .iter()
        .any(|word| kind.contains(word))
}

/// A change to something a caller can see: the definition itself, or the
/// parameter list it carries.
///
/// Named in full rather than matched on substrings, which is what the words
/// "function", "method", "class" and "parameter" were. Every call is spelled
/// with one of them - `method_invocation` in Java, `call_expression` and
/// `arrow_function` in JavaScript - so calling a method inside a function body,
/// which is most of what an interview contains, reported the candidate as
/// having changed the signature of the function they called it from.
///
/// Deliberately not "any declaration" either: C spells a local variable
/// `declaration` and Java spells it `local_variable_declaration`, so matching
/// that word reported `int count = 0;` inside a body as a changed signature.
pub(super) fn interface_kind(kind: &str) -> bool {
    matches!(
        kind,
        // The definitions.
        "function_definition"
            | "function_declaration"
            | "function_declarator"
            | "method_declaration"
            | "method_definition"
            | "constructor_declaration"
            | "class_definition"
            | "class_declaration"
            | "class_specifier"
            | "struct_specifier"
            | "interface_declaration"
            | "enum_declaration"
            // The parameter lists they carry, and the parameters in them.
            | "parameters"
            | "parameter_list"
            | "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration"
            | "formal_parameters"
            | "formal_parameter"
            | "spread_parameter"
            | "default_parameter"
            | "typed_parameter"
            | "typed_default_parameter"
    )
}
