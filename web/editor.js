const INDENT = "    ";

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
