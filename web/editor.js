import { tokenize } from "./tokenizer.js";

const INDENT = "    ";

function lastCodeCharacter(value, start, language) {
  const prefix = value.slice(0, start);
  const lineStart = prefix.lastIndexOf("\n") + 1;
  let last = "";
  for (const token of tokenize(prefix, language).tokens) {
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

export function indentNewline(value, start, end, language) {
  const lineStart = start === 0 ? 0 : value.lastIndexOf("\n", start - 1) + 1;
  const before = value.slice(lineStart, start);
  const indentation = before.match(/^[ \t]*/)[0];
  const opener = lastCodeCharacter(value, start, language);
  const closer = BRACKET_PAIRS[opener];
  const nested = Boolean(closer) || (language === "python" && opener === ":");
  const innerIndent = indentation + (nested ? INDENT : "");
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
  return { value: value.slice(0, start) + insertion + value.slice(end), start: caret, end: caret };
}

const BRACKET_CLOSERS = new Set(Object.values(BRACKET_PAIRS));

export function isBracketOpenerKeystroke(event) {
  return event.inputType === "insertText" && Object.hasOwn(BRACKET_PAIRS, event.data);
}

// Typing an opener around a selection wraps it, the way most editors do;
// typing it at a bare caret inserts an empty pair and steps inside so the
// candidate doesn't have to type the closer themselves.
export function insertBracketPair(value, start, end, opener) {
  const closer = BRACKET_PAIRS[opener];
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
export function typeOverCloser(value, start, end, closer) {
  if (!BRACKET_CLOSERS.has(closer)) return null;
  if (start !== end || value[start] !== closer) return null;
  return { value, start: start + 1, end: start + 1 };
}

// Same content-check-only shape as the bracket predicates above, and for the
// same reason: interview.js's reentrancy flag is what keeps this from
// recursing into deleteEmptyPair while its own execCommand("delete") is
// still running, not isTrusted.
export function isBackspaceKeystroke(event) {
  return event.inputType === "deleteContentBackward";
}

// The type-over's counterpart: Backspace right after an auto-inserted empty
// pair should remove both characters in one stroke instead of stranding the
// closer.
export function deleteEmptyPair(value, start, end) {
  if (start !== end || start === 0) return null;
  const closer = BRACKET_PAIRS[value[start - 1]];
  if (!closer || value[start] !== closer) return null;
  return { value: value.slice(0, start - 1) + value.slice(start + 1), start: start - 1, end: start - 1 };
}

export function indentSelection(value, start, end, outdent = false) {
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
  if (start === end && !outdent && !/^[ \t]*$/.test(value.slice(lineStart, start))) {
    return { value: `${value.slice(0, start)}${INDENT}${value.slice(end)}`, start: start + INDENT.length, end: end + INDENT.length };
  }
  const changes = [];
  // `at < end` and not a precomputed block end: `end` is exclusive, so a line
  // starting there is not selected and the loop stops on its own. Deriving a
  // separate bound by backing off a trailing newline cannot tell that newline
  // apart from a selected blank line's own, and skipped it.
  for (let at = lineStart; at < end || at === lineStart;) {
    const indentation = value.slice(at).match(/^\t|^ {1,4}/)?.[0] || "";
    changes.push({ at, remove: outdent ? indentation.length : 0, insert: outdent ? "" : INDENT });
    const next = value.indexOf("\n", at);
    if (next === -1) break;
    at = next + 1;
  }
  const map = (position, includeAtPosition) => changes.reduce((mapped, change) => {
    if (change.at > position || (change.at === position && !includeAtPosition)) return mapped;
    // Inside removed indentation: collapse onto the change, but in mapped
    // coordinates. Returning `change.at` drops every earlier change's delta.
    if (change.remove && position < change.at + change.remove) return mapped + change.insert.length - (position - change.at);
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
