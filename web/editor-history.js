// Selection belongs to an edit, not just to the current buffer: moving the
// caret after an edit must not change what undo or redo will restore.
export function editorSnapshot(editor) {
  return {
    value: editor.value,
    start: editor.selectionStart,
    end: editor.selectionEnd,
    direction: editor.selectionDirection,
  };
}

export function createEditorHistory({ limit = 200, groupDelay = 750 } = {}) {
  if (!Number.isInteger(limit) || limit < 1)
    throw new RangeError("History limit must be a positive integer");
  const languages = new Map();
  let active = null;
  let group = null;

  const copy = (snapshot) => ({ ...snapshot });
  const same = (left, right) =>
    left.value === right.value &&
    left.start === right.start &&
    left.end === right.end &&
    left.direction === right.direction;

  function select(language, snapshot, outgoing = null) {
    group = null;
    if (active && outgoing) active.current = copy(outgoing);
    if (!languages.has(language))
      languages.set(language, {
        entries: [],
        cursor: 0,
        current: copy(snapshot),
      });
    active = languages.get(language);
    return copy(active.current);
  }

  function record(before, after, kind = null, now = Date.now()) {
    if (!active) throw new Error("Select a language before recording edits");
    active.current = copy(after);
    if (before.value === after.value) return;
    const previous = active.entries[active.cursor - 1];
    const merge =
      kind !== null &&
      group?.kind === kind &&
      now - group.time >= 0 &&
      now - group.time <= groupDelay &&
      active.cursor === active.entries.length &&
      previous &&
      same(previous.after, before);
    active.entries.splice(active.cursor);
    if (merge) previous.after = copy(after);
    else {
      active.entries.push({ before: copy(before), after: copy(after) });
      if (active.entries.length > limit) active.entries.shift();
    }
    active.cursor = active.entries.length;
    group = kind === null ? null : { kind, time: now };
  }

  function undo() {
    group = null;
    if (!active || active.cursor === 0) return null;
    active.current = copy(active.entries[--active.cursor].before);
    return copy(active.current);
  }

  function redo() {
    group = null;
    if (!active || active.cursor === active.entries.length) return null;
    active.current = copy(active.entries[active.cursor++].after);
    return copy(active.current);
  }

  return {
    select,
    record,
    undo,
    redo,
    breakGroup: () => (group = null),
  };
}
