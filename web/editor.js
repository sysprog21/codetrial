export function indentSelection(value, start, end, outdent = false) {
  if (start === end) {
    if (!outdent) return { value: `${value.slice(0, start)}\t${value.slice(end)}`, start: start + 1, end: end + 1 };
  }
  // Not lastIndexOf alone: a negative fromIndex clamps to 0 and still matches
  // there, so a document that opens with a blank line would resolve the
  // caret at position 0 onto the second line and indent the wrong one.
  const lineStart = start === 0 ? 0 : value.lastIndexOf("\n", start - 1) + 1;
  // An end position at the next line's start does not select that line.
  const firstLineEnd = value.indexOf("\n", lineStart);
  const blockEnd = start === end
    ? (firstLineEnd === -1 ? value.length : firstLineEnd)
    : (end > lineStart && value[end - 1] === "\n" ? end - 1 : end);
  const changes = [];
  for (let at = lineStart; at < blockEnd || (start === end && at === lineStart);) {
    const indentation = value.slice(at).match(/^\t|^ {1,4}/)?.[0] || "";
    changes.push({ at, remove: outdent ? indentation.length : 0, insert: outdent ? "" : "\t" });
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
  return { value, start: map(start, true), end: map(end, false) };
}
