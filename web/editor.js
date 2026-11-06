import { lineCommentFor, tokenize } from "./tokenizer.js";
import { parseSyntax } from "./syntax-parser.js";

// One level of indentation unless the candidate chose another in the lobby.
const DEFAULT_INDENT = "    ";
// Columns a tab advances, as `tab-size: 4` in styles.css draws it, so
// Shift+Tab under the tab unit still takes back four spaces at most.
const TAB_COLUMNS = 4;

function lastCodeCharacter(value, start, language, parse) {
  const prefix = value.slice(0, start);
  const lineStart = prefix.lastIndexOf("\n") + 1;
  let last = "";
  for (const token of tokenize(prefix, language, parse).tokens) {
    if (token.end <= lineStart || token.kind === "comment") continue;
    const text = prefix.slice(Math.max(token.start, lineStart), token.end);
    if (token.kind !== "code") {
      if (text.trim()) last = "value";
    } else {
      const meaningful = text.trimEnd();
      if (meaningful) last = meaningful.at(-1);
    }
  }
  return last;
}

const BRACKET_PAIRS = { "(": ")", "{": "}", "[": "]" };

export function indentNewline(
  value,
  start,
  end,
  language,
  parse,
  indent = DEFAULT_INDENT,
) {
  const lineStart = start === 0 ? 0 : value.lastIndexOf("\n", start - 1) + 1;
  const before = value.slice(lineStart, start);
  const indentation = before.match(/^[ \t]*/)[0];
  const opener = lastCodeCharacter(value, start, language, parse);
  const closer = BRACKET_PAIRS[opener];
  const nested = Boolean(closer) || (language === "python" && opener === ":");
  // A tab after spaces mixes the two, which Python 3 refuses as a TabError, so
  // a space-indented line nests with spaces whatever the unit.
  const unit =
    indent === "\t" && indentation.includes(" ")
      ? " ".repeat(TAB_COLUMNS)
      : indent;
  const innerIndent = indentation + (nested ? unit : "");
  let insertion = `\n${innerIndent}`;
  const caret = start + insertion.length;
  const after = value.slice(end);
  const trailingSpace = after.match(/^[ \t]*/)[0].length;
  // If the caret is between an opener and a closer, the indentation will be like this:
  //
  // if (a != b) {
  //     |  <-------  caret
  // }
  if (closer && after[trailingSpace] === closer) {
    insertion += `\n${indentation}`;
    end += trailingSpace;
  }
  return {
    value: value.slice(0, start) + insertion + value.slice(end),
    start: caret,
    end: caret,
  };
}

const BRACKET_CLOSERS = new Set(Object.values(BRACKET_PAIRS));

// Prefer the bracket after the caret when two pairs meet, as in `)(`.
// A stray closer leaves enclosing pairs intact; crossed pairs break the chain.
export function matchingBrackets(
  value,
  start,
  end,
  language,
  getTokens = tokenize,
) {
  if (start !== end) return null;
  const isBracket = (character) =>
    Object.hasOwn(BRACKET_PAIRS, character) || BRACKET_CLOSERS.has(character);
  if (!isBracket(value[start]) && !isBracket(value[start - 1])) return null;
  let beforeCaret = null;
  const stack = [];
  let openerCounts = {};
  for (const token of getTokens(value, language).tokens) {
    if (token.kind !== "code") continue;
    for (let index = token.start; index < token.end; index += 1) {
      const character = value[index];
      if (Object.hasOwn(BRACKET_PAIRS, character)) {
        stack.push(index);
        const closer = BRACKET_PAIRS[character];
        openerCounts[closer] = (openerCounts[closer] ?? 0) + 1;
      } else if (BRACKET_CLOSERS.has(character)) {
        const opener = stack.at(-1);
        if (BRACKET_PAIRS[value[opener]] !== character) {
          if (openerCounts[character]) {
            stack.length = 0;
            openerCounts = {};
          }
        } else {
          stack.pop();
          openerCounts[character] -= 1;
          if (opener === start || index === start) return [opener, index];
          if (opener === start - 1 || index === start - 1)
            beforeCaret = [opener, index];
        }
      }
    }
  }
  return beforeCaret;
}

export function isBracketOpenerKeystroke(event) {
  return (
    event.inputType === "insertText" && Object.hasOwn(BRACKET_PAIRS, event.data)
  );
}

// What may follow a bare caret for an opener to get its closer: the end of
// the line, whitespace, a closer, or punctuation that ends an expression. The
// set is VS Code's default autoCloseBefore. Before anything else the candidate
// is opening a group around text that is already there, and a closer typed
// here would sit in front of that text rather than after it (issue 150).
const AUTO_CLOSE_BEFORE = new Set([..." \t\n;:.,=>", ...BRACKET_CLOSERS]);

function isLiteralCharacter(value, at, language, parse = parseSyntax) {
  let usesFallback = true;
  const token = tokenize(value, language, (code, language) => {
    const tree = parse(code, language);
    usesFallback = !tree || tree.rootNode.hasError;
    return tree;
  }).tokens.find((token) => token.start <= at && at < token.end);
  if (token?.kind === "comment") return true;
  if (token?.kind !== "string") return false;
  // The Python fallback scanner cannot separate f-string text from its
  // expressions. Preserve pairing for scanner-classified f-string tokens
  // when the tree is missing or contains errors, matching tokenize's fallback.
  // Recognized syntax regions still distinguish literal text from code.
  if (
    usesFallback &&
    language === "python" &&
    /(?:^|[^\p{ID_Continue}])(?:f|fr|rf)$/iu.test(value.slice(0, token.start))
  )
    return false;
  return true;
}

// Typing an opener around a selection wraps it, the way most editors do;
// typing it at a bare caret inserts an empty pair and steps inside so the
// candidate doesn't have to type the closer themselves. Null, so the caller
// falls through to native insertion, before other text or inside a literal
// or comment.
export function insertBracketPair(value, start, end, opener, language, parse) {
  const closer = BRACKET_PAIRS[opener];
  if (
    start === end &&
    start < value.length &&
    !AUTO_CLOSE_BEFORE.has(value[start])
  )
    return null;
  if (start === end) {
    // Classify the inserted character, not its neighbor: at EOF a comment
    // or unfinished string still owns the input, but a closed literal does
    // not. The shared tokenizer also handles a grammar that is not ready.
    const typed = value.slice(0, start) + opener + value.slice(end);
    if (isLiteralCharacter(typed, start, language, parse)) return null;
  }
  const inner = value.slice(start, end);
  const insertion = `${opener}${inner}${closer}`;
  const caret = start + opener.length + inner.length;
  return {
    value: value.slice(0, start) + insertion + value.slice(end),
    start: start === end ? caret : start + opener.length,
    end: caret,
  };
}

export function isBracketCloserKeystroke(event) {
  return event.inputType === "insertText" && BRACKET_CLOSERS.has(event.data);
}

// The other half of typing over an inserted pair: closing it out of habit
// should step past the closer already there rather than duplicate it. Not
// applicable, so the caller falls through to native insertion, unless the
// caret is collapsed right before that same character.
export function typeOverCloser(value, start, end, closer, language, parse) {
  if (!BRACKET_CLOSERS.has(closer)) return null;
  if (start !== end || value[start] !== closer) return null;
  if (isLiteralCharacter(value, start, language, parse)) return null;
  return { value, start: start + 1, end: start + 1 };
}

// Same content-check-only shape as the bracket predicates above, and for the
// same reason: interview.js decides which events belong to an edit; this
// predicate describes the requested deletion, regardless of isTrusted.
export function isBackspaceKeystroke(event) {
  return event.inputType === "deleteContentBackward";
}

// The type-over's counterpart: Backspace right after an auto-inserted empty
// pair should remove both characters in one stroke instead of stranding the
// closer.
export function deleteEmptyPair(value, start, end, language, parse) {
  if (start !== end || start === 0) return null;
  const closer = BRACKET_PAIRS[value[start - 1]];
  if (!closer || value[start] !== closer) return null;
  if (isLiteralCharacter(value, start - 1, language, parse)) return null;
  return {
    value: value.slice(0, start - 1) + value.slice(start + 1),
    start: start - 1,
    end: start - 1,
  };
}

/// One level for the line holding `at`: a tab-indented line keeps tabs and a
/// space-indented one takes `spaces`, whatever the buffer as a whole prefers,
/// so a tab-indented function and a space-indented one in the same file each
/// nest and outdent by their own level. A blank line follows the nearest
/// non-blank line above: a tab there means a tab here, so a blank line inside
/// a tab-indented block does not get spaces. Any other line with no
/// indentation is code of its own and takes `unit`.
export function lineIndentUnit(
  value,
  at,
  unit,
  spaces = unit === "\t" ? " ".repeat(TAB_COLUMNS) : unit,
) {
  const lineStart = at === 0 ? 0 : value.lastIndexOf("\n", at - 1) + 1;
  const lead = value.slice(lineStart).match(/^[ \t]*/)[0];
  if (lead.startsWith("\t")) return "\t";
  if (lead.startsWith(" ")) return spaces;
  const lineEnd = value.indexOf("\n", lineStart);
  if (value.slice(lineStart, lineEnd === -1 ? undefined : lineEnd).trim())
    return unit;
  for (let end = lineStart - 1; end > 0;) {
    const start = value.lastIndexOf("\n", end - 1) + 1;
    const line = value.slice(start, end);
    if (line.trim()) return line.startsWith("\t") ? "\t" : unit;
    end = start - 1;
  }
  return unit;
}

/// The indentation Enter should add after the caret's line when the block it
/// opens already has a body: the extra whitespace of the next code line, if
/// that line is deeper. Null otherwise. Without it, Enter after `def f():`
/// above a pasted four-space body nests at the chosen width and Python rejects
/// the body as an unexpected indent. A comment may sit at any column, so a
/// line holding nothing but comments, line or block, is skipped like a blank
/// one. The tokenizer decides what is a comment, so a C statement that starts
/// with `*` is still code.
export function followingIndent(value, at, language) {
  const comments = lineCommentFor(language)
    ? tokenize(value, language).tokens.filter(
        (token) => token.kind === "comment",
      )
    : [];
  // Every non-space character of [from, to) lies inside a comment.
  const commentOnly = (from, to) => {
    for (let index = from; index < to; index += 1) {
      if (/\s/.test(value[index])) continue;
      const token = comments.find(
        ({ start, end }) => start <= index && index < end,
      );
      if (!token) return false;
      index = token.end - 1;
    }
    return true;
  };
  const lineStart = at === 0 ? 0 : value.lastIndexOf("\n", at - 1) + 1;
  const current = value.slice(lineStart).match(/^[ \t]*/)[0];
  for (let next = value.indexOf("\n", at); next !== -1;) {
    const lineEnd = value.indexOf("\n", next + 1);
    const line = value.slice(next + 1, lineEnd === -1 ? undefined : lineEnd);
    const to = lineEnd === -1 ? value.length : lineEnd;
    if (line.trim() && !commentOnly(next + 1, to)) {
      const lead = line.match(/^[ \t]*/)[0];
      return lead.length > current.length && lead.startsWith(current)
        ? lead.slice(current.length)
        : null;
    }
    next = lineEnd;
  }
  return null;
}

/// `indent` is either one level for every line, as on `main`, or
/// `{ unit, spaces }` for `lineIndentUnit` to resolve line by line, so a
/// selection spanning a tab-indented function and a space-indented one gives
/// each its own level.
export function indentSelection(
  value,
  start,
  end,
  outdent = false,
  indent = DEFAULT_INDENT,
) {
  const levelAt =
    typeof indent === "string"
      ? () => indent
      : (at) => lineIndentUnit(value, at, indent.unit, indent.spaces);
  // Not lastIndexOf alone: a negative fromIndex clamps to 0 and still matches
  // there, so a document that opens with a blank line would resolve the
  // caret at position 0 onto the second line and indent the wrong one.
  const lineStart = start === 0 ? 0 : value.lastIndexOf("\n", start - 1) + 1;
  // A caret past the indentation types an indent where it sits, the way a
  // caret anywhere else types a character. A caret still inside the
  // indentation is asking for another level on the line, so it goes through
  // the block path and the insert lands at column 0. Otherwise the level it
  // builds is off by the column it happened to sit at, and on a line the
  // candidate pasted with tabs the spaces would land between them.
  if (
    start === end &&
    !outdent &&
    !/^[ \t]*$/.test(value.slice(lineStart, start))
  ) {
    const unit = levelAt(lineStart);
    return {
      value: `${value.slice(0, start)}${unit}${value.slice(end)}`,
      start: start + unit.length,
      end: end + unit.length,
    };
  }
  const changes = [];
  // `at < end` and not a precomputed block end: `end` is exclusive, so a line
  // starting there is not selected and the loop stops on its own. Deriving a
  // separate bound by backing off a trailing newline cannot tell that newline
  // apart from a selected blank line's own, and skipped it.
  for (let at = lineStart; at < end || at === lineStart;) {
    const unit = levelAt(at);
    // One level back: a single tab, or up to one unit's width of spaces.
    const level = new RegExp(
      `^\\t|^ {1,${unit === "\t" ? TAB_COLUMNS : unit.length}}`,
    );
    const indentation = value.slice(at).match(level)?.[0] || "";
    changes.push({
      at,
      remove: outdent ? indentation.length : 0,
      insert: outdent ? "" : unit,
    });
    const next = value.indexOf("\n", at);
    if (next === -1) break;
    at = next + 1;
  }
  const map = (position, includeAtPosition) =>
    changes.reduce((mapped, change) => {
      if (
        change.at > position ||
        (change.at === position && !includeAtPosition)
      )
        return mapped;
      // Inside removed indentation: collapse onto the change, but in mapped
      // coordinates. Returning `change.at` drops every earlier change's delta.
      if (change.remove && position < change.at + change.remove)
        return mapped + change.insert.length - (position - change.at);
      return mapped + change.insert.length - change.remove;
    }, position);
  for (let index = changes.length - 1; index >= 0; index -= 1) {
    const change = changes[index];
    value = `${value.slice(0, change.at)}${change.insert}${value.slice(change.at + change.remove)}`;
  }
  // A caret is one position, so it maps once: `includeAtPosition` has to be the
  // same on both ends or a column-0 caret is left behind the indent it just
  // inserted. A real selection keeps the exclusive end it started with.
  return { value, start: map(start, true), end: map(end, start === end) };
}

// Matched on the character and not the key's position: the key that sits
// where a US slash does types `-` on a German layout, and Ctrl with that one
// is the browser's zoom. Shift is left unread for the same layouts, where the
// slash itself is a shifted key. Alt is refused because AltGr reports as
// Ctrl+Alt on Windows and types a character of its own.
export function isToggleCommentKeystroke(event) {
  return (
    event.key === "/" &&
    (event.ctrlKey || event.metaKey) &&
    !event.altKey &&
    !event.isComposing
  );
}

// Comments the lines the selection touches, or uncomments them when every one
// that has text already is a comment, so a second press undoes the first and
// a block with one commented line in it gains a level rather than losing that
// line's. Null for a language with no line comment, so the caller leaves the
// keystroke alone.
export function toggleLineComment(value, start, end, language) {
  const marker = lineCommentFor(language);
  if (!marker) return null;
  // The same two bounds as indentSelection, for the reasons given there.
  const lineStart = start === 0 ? 0 : value.lastIndexOf("\n", start - 1) + 1;
  const lines = [];
  for (let at = lineStart; at < end || at === lineStart;) {
    const next = value.indexOf("\n", at);
    const text = value.slice(at, next === -1 ? value.length : next);
    const indent = text.match(/^[ \t]*/)[0].length;
    lines.push({ at, indent, blank: indent === text.length });
    if (next === -1) break;
    at = next + 1;
  }
  // Blank lines sit a comment out, or commenting a block would leave a marker
  // and a trailing space on each. When nothing else is selected they are what
  // the candidate asked to comment.
  const written = lines.filter((line) => !line.blank);
  const targets = written.length ? written : lines;
  const commented = targets.every((line) =>
    value.startsWith(marker, line.at + line.indent),
  );
  // One column for the whole block, the shallowest line's, so the markers
  // line up and the code keeps its indentation relative to them.
  const column = Math.min(...targets.map((line) => line.indent));
  const changes = targets.map((line) => {
    if (!commented)
      return { at: line.at + column, remove: 0, insert: `${marker} ` };
    const at = line.at + line.indent;
    // The space is taken back only where there is one: a comment the
    // candidate typed as `#x` is still a comment to remove.
    const spaced = value[at + marker.length] === " ";
    return { at, remove: marker.length + Number(spaced), insert: "" };
  });
  // Changes are in document order, so the first one past the position ends
  // the walk. A position inside a removed marker collapses onto its start.
  const map = (position, includeAtPosition) => {
    let mapped = position;
    for (const { at, remove, insert } of changes) {
      if (at > position || (at === position && !includeAtPosition)) break;
      mapped += insert.length - Math.min(remove, position - at);
    }
    return mapped;
  };
  for (let index = changes.length - 1; index >= 0; index -= 1) {
    const { at, remove, insert } = changes[index];
    value = `${value.slice(0, at)}${insert}${value.slice(at + remove)}`;
  }
  // A caret maps once, as in indentSelection; a selection that ends where a
  // marker goes in still ends before it.
  return { value, start: map(start, true), end: map(end, start === end) };
}

// Move exactly the selected characters; all other text, including empty lines,
// stays intact. The browser's drop position refers to the original buffer.
export function moveSelectedText(value, start, end, target) {
  if (start === end || (target >= start && target <= end))
    return { value, start, end };
  const text = value.slice(start, end);
  const remaining = value.slice(0, start) + value.slice(end);
  const insertion = target > end ? target - text.length : target;
  return {
    value: remaining.slice(0, insertion) + text + remaining.slice(insertion),
    start: insertion,
    end: insertion + text.length,
  };
}

// Set for exactly the duration of an `applyEditorEdit` call, and checked
// first thing in a beforeinput listener, so the synthetic beforeinput the
// edit fires is recognized by when it happens rather than by a property of
// dubious reliability.
let applyingProgrammatic = false;

/// Whether the beforeinput being handled is `applyEditorEdit`'s own.
export function applyingEditorEdit() {
  return applyingProgrammatic;
}

/// Every edit an editor makes on the writer's behalf -- indentation, bracket
/// pairs, closer type-over, paired deletion -- lands here. Assigning `value`
/// wipes the native undo stack, so one such edit would cost the writer their
/// whole editing history. The change goes through the browser's editing
/// command instead, over just the span that actually changed.
export function applyEditorEdit(editor, next) {
  const previous = editor.value;
  if (next.value === previous) {
    editor.setSelectionRange(next.start, next.end);
    return;
  }
  let head = 0;
  while (previous[head] === next.value[head]) head += 1;
  let tail = 0;
  while (
    tail < Math.min(previous.length, next.value.length) - head &&
    previous[previous.length - 1 - tail] ===
      next.value[next.value.length - 1 - tail]
  )
    tail += 1;
  const inserted = next.value.slice(head, next.value.length - tail);
  editor.setSelectionRange(head, previous.length - tail);
  let rewritten;
  applyingProgrammatic = true;
  try {
    rewritten = inserted
      ? document.execCommand?.("insertText", false, inserted)
      : document.execCommand?.("delete");
  } finally {
    applyingProgrammatic = false;
  }
  if (!rewritten) {
    editor.value = next.value;
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  }
  editor.setSelectionRange(next.start, next.end);
}
