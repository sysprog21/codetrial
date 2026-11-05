/// Columns a tab advances to the next multiple of. Matches the `tab-size: 4`
/// the editor layers share, so a tab and four spaces land on the same column.
const TAB = 4;

/// The width when the buffer has no indented block to measure: what editor.js
/// inserts for one level, so the first block a candidate types lines up.
const FALLBACK_WIDTH = 4;

const advance = (column, character) =>
  character === "\t" ? (Math.floor(column / TAB) + 1) * TAB : column + 1;

const columnsOf = (whitespace) => [...whitespace].reduce(advance, 0);

/// Splits leading whitespace at every guide boundary. A boundary that falls
/// inside a tab moves to the tab's end, so a unit never cuts a character.
///
/// With `partial`, whitespace that ends partway through a level makes one more,
/// narrower unit: its guide column is still inside the whitespace, and the
/// guide is drawn on the unit's left edge, so it looks the same as a full one.
/// Without it that remainder is left over as `rest` and gets no guide.
function guideUnits(whitespace, width, partial) {
  const units = [];
  let column = 0;
  let start = 0;
  for (let index = 0; index < whitespace.length; index += 1) {
    column = advance(column, whitespace[index]);
    if (column >= (units.length + 1) * width) {
      units.push(whitespace.slice(start, index + 1));
      start = index + 1;
    }
  }
  if (partial && start < whitespace.length) {
    units.push(whitespace.slice(start));
    start = whitespace.length;
  }
  return { units, rest: whitespace.slice(start) };
}

/// For each line, whether it continues a comment or a string opened on an
/// earlier line. Its indentation is the text's alignment rather than code's:
/// a block comment's ` * ` body sits one column in from its opener. A line
/// that opens a comment or string is indented like code, and is not one.
function continuations(lines, tokens) {
  let token = 0;
  return lines.map(({ start, whitespace, blank }) => {
    if (blank) return false;
    const first = start + whitespace.length;
    while (token < tokens.length && tokens[token].end <= first) token += 1;
    const inside = tokens[token];
    return Boolean(inside && inside.kind !== "code" && inside.start < first);
  });
}

const guide = (text) => `<span class="indent-guide">${text}</span>`;

/// The most common step between one code line's indentation and the next
/// deeper one, so 2-space and 4-space buffers each get a guide per level.
///
/// A line that continues a comment or a string does not count, or every starter
/// with a doc comment would vote for a width of 1. A line that opens one does,
/// which matters because a starter's body is often nothing but a comment. A
/// step of 1 is ignored for the same reason wherever else it comes from, since
/// it is alignment rather than a block. A tie takes the smaller width, because
/// a level drawn twice is easier to read past than a level not drawn at all.
export function detectWidth(lines, tokens) {
  return votedWidth(lines, tokens) ?? FALLBACK_WIDTH;
}

/// The voted step, or null when no line votes: a buffer with no indented block
/// has no width of its own.
function votedWidth(lines, tokens, spacesOnly = false) {
  const votes = new Map();
  const continues = continuations(lines, tokens);
  let previous = null;
  for (const [index, { whitespace, blank }] of lines.entries()) {
    if (blank || continues[index]) continue;
    // A tab-indented line breaks the run, so its steps vote for nothing.
    if (spacesOnly && whitespace.startsWith("\t")) {
      previous = null;
      continue;
    }
    const columns = columnsOf(whitespace);
    const step = columns - (previous ?? columns);
    if (step >= 2 && step <= 8) votes.set(step, (votes.get(step) ?? 0) + 1);
    previous = columns;
  }
  let width = null;
  let best = 0;
  for (const [step, count] of votes) {
    if (count > best || (count === best && step < width)) {
      width = step;
      best = count;
    }
  }
  return width;
}

const linesOf = (code) => {
  let offset = 0;
  return code.split("\n").map((line) => {
    const whitespace = line.match(/^[ \t]*/)[0];
    const start = offset;
    offset += line.length + 1;
    return { start, whitespace, blank: whitespace.length === line.length };
  });
};

/// One level as the code's space-indented lines write it, measured over those
/// lines alone so a tab-indented block in the same code does not vote. Null
/// when no space-indented block is there to measure.
export function bufferIndent(code, tokens) {
  const width = votedWidth(linesOf(code), tokens, true);
  return width === null ? null : " ".repeat(width);
}

/// Returns the guides for `code` as runs for highlight(): one per line that
/// gets any, covering that line's leading whitespace `[start, end)` and holding
/// the HTML that replaces it, with a guide at every guide column inside it.
/// A level the whitespace ends partway through still gets its guide, except on
/// a line continuing a comment or string, whose ` * ` would otherwise carry
/// one beside it on every line of every doc comment.
///
/// A blank line takes at least the shallower of its neighbours' depths, so a
/// guide runs unbroken through a blank line inside a block and stops at one
/// after it. Its own whitespace can take it deeper, up to the deeper
/// neighbour: Enter at the end of a block auto-indents a line whose closer
/// below is shallower, and that line keeps the depth the caret is at, while
/// stray spaces cannot draw guides past the code around them. A line whose
/// whitespace stops short of its last guide's column gets spaces added; they
/// end a line with nothing after them, so no character that follows moves.
export function indentGuides(code, tokens) {
  const lines = linesOf(code);
  const width = detectWidth(lines, tokens);
  const continues = continuations(lines, tokens);
  lines.forEach((entry, index) => {
    Object.assign(
      entry,
      guideUnits(entry.whitespace, width, !continues[index]),
    );
  });

  const depths = lines.map((entry) =>
    entry.blank ? null : entry.units.length,
  );
  const below = new Array(lines.length).fill(0);
  for (let index = lines.length - 1, next = 0; index >= 0; index -= 1) {
    below[index] = next;
    if (depths[index] !== null) next = depths[index];
  }

  const runs = [];
  let above = 0;
  lines.forEach((entry, index) => {
    const end = entry.start + entry.whitespace.length;
    if (!entry.blank) {
      above = depths[index];
      if (entry.units.length) {
        runs.push({
          start: entry.start,
          end,
          html: entry.units.map(guide).join("") + entry.rest,
        });
      }
      return;
    }
    const own = entry.units.length;
    const depth = Math.max(
      Math.min(above, below[index]),
      Math.min(own, Math.max(above, below[index])),
    );
    if (!depth) return;
    let html;
    if (depth <= own) {
      html =
        entry.units.slice(0, depth).map(guide).join("") +
        entry.units.slice(depth).join("") +
        entry.rest;
    } else {
      // Out to the column after its own last guide, then a full level each.
      // A tab can cross two guide columns in one unit at a narrow width, so
      // the line can already be past that column.
      const columns = columnsOf(entry.whitespace);
      html =
        entry.units.map(guide).join("") +
        " ".repeat(Math.max(0, own * width - columns)) +
        guide(" ".repeat(width)).repeat(depth - own);
    }
    runs.push({ start: entry.start, end, html });
  });
  return runs;
}
