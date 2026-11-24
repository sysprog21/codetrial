// Chisel lines colored from a tree-sitter-scala parse: Scala's keywords,
// literals, comments and types, and on top of them what makes the code
// hardware: Chisel's constructors and control (Wire, RegInit, when, Mux...),
// its hardware types, the width and literal suffixes (3.U, 32.W) and its
// connection and comparison operators (:=, <>, ===, =/=).
//
// The output is DOM nodes built with textContent, never HTML, since the lines
// are the learner's own code. Until the grammar has loaded, or if it cannot
// load, a line is plain text.

import { parseSyntax, prepareLanguage } from "./syntax-parser.js";

/// Chisel's hardware constructors, control and helpers.
const CHISEL_CALLS = new Set([
  "Module",
  "IO",
  "Wire",
  "WireDefault",
  "WireInit",
  "Reg",
  "RegInit",
  "RegNext",
  "RegEnable",
  "Mem",
  "SyncReadMem",
  "Mux",
  "MuxCase",
  "MuxLookup",
  "Mux1H",
  "PriorityMux",
  "Cat",
  "Fill",
  "VecInit",
  "when",
  "elsewhen",
  "otherwise",
  "switch",
  "is",
  "Input",
  "Output",
  "Flipped",
  "Decoupled",
  "Valid",
  "DontCare",
  "printf",
  "assert",
  "chiselTypeOf",
]);

/// Chisel's hardware types, colored as types wherever they appear.
const CHISEL_TYPES = new Set([
  "UInt",
  "SInt",
  "Bool",
  "Bits",
  "Clock",
  "Reset",
  "AsyncReset",
  "Vec",
  "Bundle",
]);

/// The suffixes that make a Scala literal a hardware literal or a width.
const LITERAL_SUFFIXES = new Set(["U", "S", "B", "W"]);

/// Chisel's operators: connection, bulk connection, hardware equality.
const HARDWARE_OPERATORS = new Set([":=", "<>", "===", "=/=", "##", ":<>="]);

/// Nodes colored as one range, whatever they hold.
const WHOLE = {
  comment: "tok-comment",
  block_comment: "tok-comment",
  string: "tok-string",
  interpolated_string_expression: "tok-string",
  character_literal: "tok-string",
};

/// How long one parse of an item's lines may take, in milliseconds.
const SNIPPET_BUDGET_MS = 1000;

let loading = null;

/// Starts loading the grammar; resolves true once lines can be colored.
/// Until then the parser has no grammar and every line is plain.
export function loadChisel() {
  loading ??= prepareLanguage("scala").then(
    () => true,
    () => false,
  );
  return loading;
}

/// The class of a leaf, or null for plain text.
function leafClass(node) {
  const { type, parent } = node;
  if (!node.isNamed) {
    // `true`, `false` and `null` are words inside their literal's node.
    if (/^(boolean|null)_literal$/.test(parent?.type ?? ""))
      return "tok-literal";
    if (/^[a-z]+$/.test(type)) return "tok-keyword";
    return null;
  }
  if (type === "integer_literal" || type === "floating_point_literal")
    return "tok-number";
  if (type === "boolean_literal" || type === "null_literal")
    return "tok-literal";
  if (type === "operator_identifier")
    return HARDWARE_OPERATORS.has(node.text) ? "tok-hardware-op" : null;
  if (type === "type_identifier")
    return CHISEL_TYPES.has(node.text) ? "tok-chisel-type" : "tok-type";
  if (type === "identifier") {
    if (CHISEL_TYPES.has(node.text)) return "tok-chisel-type";
    if (CHISEL_CALLS.has(node.text)) return "tok-chisel";
    // `3.U`, `false.B`, `32.W`, `"h1F".U`: the field of a literal.
    if (
      LITERAL_SUFFIXES.has(node.text) &&
      parent?.type === "field_expression" &&
      /literal$|^string$/.test(parent.firstChild?.type ?? "")
    )
      return "tok-number";
  }
  return null;
}

/// The colored ranges of `code`, in order and without overlaps.
export function chiselRanges(code) {
  // A defense's lines are parsed once per redraw, so they get far more than
  // the editor's keystroke budget: a cold first parse on a slow machine
  // would otherwise be cancelled and leave them plain.
  const tree = parseSyntax(code, "scala", SNIPPET_BUDGET_MS);
  if (!tree) return [];
  const ranges = [];
  const visit = (node) => {
    const whole = WHOLE[node.type];
    if (whole) {
      ranges.push({ start: node.startIndex, end: node.endIndex, kind: whole });
      return;
    }
    if (node.childCount === 0) {
      const kind = leafClass(node);
      if (kind)
        ranges.push({ start: node.startIndex, end: node.endIndex, kind });
      return;
    }
    for (const child of node.children) visit(child);
  };
  try {
    visit(tree.rootNode);
  } finally {
    tree.delete();
  }
  return ranges;
}

/// Each of `lines` as a list of nodes, colored when the grammar is loaded.
/// web-tree-sitter's indexes are UTF-16 offsets, as JavaScript's are.
export function chiselLines(lines) {
  const code = lines.join("\n");
  let ranges = [];
  try {
    ranges = chiselRanges(code);
  } catch {
    // A parse that throws colors nothing; the lines stay text.
  }
  const out = [];
  let lineStart = 0;
  let cursor = 0;
  for (const line of lines) {
    const lineEnd = lineStart + line.length;
    const nodes = [];
    const plain = (from, to) => {
      if (to > from) nodes.push(document.createTextNode(code.slice(from, to)));
    };
    let at = lineStart;
    while (cursor < ranges.length && ranges[cursor].end <= lineStart)
      cursor += 1;
    for (
      let i = cursor;
      i < ranges.length && ranges[i].start < lineEnd;
      i += 1
    ) {
      const start = Math.max(ranges[i].start, lineStart);
      const end = Math.min(ranges[i].end, lineEnd);
      if (end <= start) continue;
      plain(at, start);
      const span = document.createElement("span");
      span.className = ranges[i].kind;
      span.textContent = code.slice(start, end);
      nodes.push(span);
      at = end;
    }
    plain(at, lineEnd);
    out.push(nodes);
    lineStart = lineEnd + 1;
  }
  return out;
}
