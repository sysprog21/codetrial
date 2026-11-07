// Draws the diff the server read (diffmodel.py): files, hunks, numbered lines,
// the words that changed, and the runs that moved. It decides nothing about
// what changed; it shows labels the server computed, and it says so when the
// server could not account for every line.
//
// Nothing is left out to keep the page quick. A large diff is drawn in pieces
// across frames, with a count of how far it has got, and every line ends up in
// the page. All text goes in through textContent.

const CHUNK = 400; // rows drawn per frame after the first screenful

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const SIGN = { c: " ", a: "+", d: "-" };
const KIND = { c: "context", a: "added", d: "removed" };

function wordsIn(text, spans, className) {
  const code = el("span", "code");
  if (!spans?.length) {
    code.textContent = text || " ";
    return code;
  }
  let at = 0;
  for (const [start, end] of spans) {
    if (start > at) code.append(text.slice(at, start));
    code.append(el("mark", className, text.slice(start, end)));
    at = end;
  }
  if (at < text.length) code.append(text.slice(at));
  return code;
}

// Whether a row is the passage a question quotes. A one-line quote may be part
// of a line; a quote of several lines marks each row that is one of its lines,
// compared whole, since a short fragment would otherwise match everywhere.
function asks(line, quote) {
  const pieces = quote.split("\n");
  if (pieces.length === 1)
    return line.t.includes(quote) || (SIGN[line.k] + line.t).includes(quote);
  const own = line.t.trim();
  return (
    own !== "" &&
    pieces.some((piece) => piece.replace(/^[ +-]/, "").trim() === own)
  );
}

export class DiffView {
  constructor({ root, notice, summary, position, live }) {
    this.root = root;
    this.notice = notice;
    this.summary = summary;
    this.position = position;
    this.live = live;
    this.model = null;
    this.split = false;
    this.hideWhitespace = false;
    this.generation = 0;
    this.current = -1;
    this.last = null;
    this.drawing = false;
    this.pendingCitation = null;
  }

  setModel(model) {
    this.model = model;
  }

  setSplit(split) {
    this.split = split;
    this.redraw();
  }

  setHideWhitespace(hide) {
    this.hideWhitespace = hide;
    this.redraw();
  }

  redraw() {
    if (this.last) this.show(this.last);
  }

  say(text) {
    this.notice.textContent = text;
    this.notice.hidden = !text;
  }

  // Said on the page, and to a screen reader, both.
  tell(text) {
    this.say(text);
    this.announce(text);
  }

  // A line is announced through a region that is always in the page: one that
  // appears together with its text is often skipped the first time.
  announce(text) {
    if (!this.live) return;
    this.live.textContent = "";
    setTimeout(() => (this.live.textContent = text), 50);
  }

  // Draw the files whose path is `path`, or all of them. Returns how many
  // files matched, so the caller can say when none did.
  show({ path, asked }) {
    this.last = { path, asked };
    this.generation += 1;
    this.drawing = false;
    this.pendingCitation = null;
    const generation = this.generation;
    this.current = -1;
    this.root.replaceChildren();
    this.root.scrollTop = 0;
    const model = this.model;
    if (model.problems.length) return this.raw(model);
    const files = path
      ? model.files.filter((f) => f.path === path)
      : model.files;
    // A file renamed away and a new one at the old path are different files;
    // only if nothing is at the path is it looked for under its old name.
    const chosen =
      path && !files.length
        ? model.files.filter((f) => f.old_path === path)
        : files;
    this.summarize(chosen.length ? chosen : model.files);
    if (path && !chosen.length) {
      this.say(
        "The patch could not be matched to a single file, so the whole diff is shown.",
      );
      return this.draw(model.files, undefined, generation);
    }
    this.say("");
    return this.draw(chosen, asked, generation);
  }

  summarize(files) {
    const sum = (key) => files.reduce((total, f) => total + f[key], 0);
    const parts = [
      `${files.length} ${files.length === 1 ? "file" : "files"}`,
      `+${sum("additions")} −${sum("deletions")}`,
    ];
    if (sum("moved")) parts.push(`${sum("moved")} lines moved`);
    if (sum("whitespace")) parts.push(`${sum("whitespace")} whitespace-only`);
    this.summary.textContent = parts.join(" · ");
  }

  raw(model) {
    this.say(
      `This page could not account for every line of the diff (${model.problems[0]}), so it shows the diff as written.`,
    );
    this.summary.textContent = "";
    const fragment = document.createDocumentFragment();
    for (const text of (model.raw || "").split("\n")) {
      const row = el("div", "row raw");
      if (text.startsWith("+")) row.classList.add("k-a");
      else if (text.startsWith("-")) row.classList.add("k-d");
      else if (text.startsWith("@@")) row.classList.add("hunk");
      row.append(el("span", "code", text || " "));
      fragment.append(row);
    }
    this.root.append(fragment);
    return 0;
  }

  draw(files, asked, generation) {
    this.drawing = true;
    const units = [];
    for (const file of files) {
      const section = el("section", "diff-file");
      section.dataset.file = file.path;
      section.append(this.head(file));
      const body = el("div", "file-body");
      section.append(body);
      this.root.append(section);
      for (const unit of this.units(file, asked)) units.push([body, unit]);
    }
    let done = 0;
    const next = (count) => {
      if (generation !== this.generation) return;
      const fragment = new Map();
      for (const [body, unit] of units.slice(done, done + count)) {
        if (!fragment.has(body))
          fragment.set(body, document.createDocumentFragment());
        fragment.get(body).append(unit());
      }
      for (const [body, rows] of fragment) body.append(rows);
      done = Math.min(units.length, done + count);
      if (done < units.length) {
        this.say(
          `Drawing the diff… ${Math.round((100 * done) / units.length)}%`,
        );
        requestAnimationFrame(() => next(CHUNK));
      } else {
        this.drawing = false;
        if (this.notice.textContent.startsWith("Drawing")) this.say("");
        this.finish(asked);
        if (this.pendingCitation) this.reveal(this.pendingCitation);
      }
    };
    next(CHUNK);
    return files.length;
  }

  finish(asked) {
    this.updatePosition();
    const target = this.root.querySelector(".asked");
    if (target && asked) target.scrollIntoView({ block: "center" });
  }

  head(file) {
    const head = el("h3", "file-head");
    const toggle = el("button", "file-toggle", "▾");
    toggle.type = "button";
    toggle.setAttribute("aria-expanded", "true");
    toggle.setAttribute("aria-label", `Collapse ${file.path}`);
    toggle.addEventListener("click", () => {
      const body = head.nextElementSibling;
      const open = body.hidden;
      body.hidden = !open;
      toggle.textContent = open ? "▾" : "▸";
      toggle.setAttribute("aria-expanded", String(open));
      toggle.setAttribute(
        "aria-label",
        `${open ? "Collapse" : "Expand"} ${file.path}`,
      );
    });
    head.append(toggle, el("span", "path", file.path));
    if (file.status === "renamed")
      head.append(el("span", "tag", `renamed from ${file.old_path}`));
    else if (file.status !== "modified")
      head.append(el("span", "tag", file.status));
    head.append(el("span", "counts", `+${file.additions} −${file.deletions}`));
    if (file.label) head.append(el("span", "tag", file.label));
    return head;
  }

  // One function per row, so a file can be drawn in pieces.
  *units(file, asked) {
    if (file.binary) {
      yield () => el("div", "note", "Binary file: its contents are not shown.");
      return;
    }
    if (!file.hunks.length) {
      yield () => el("div", "note", "No line changes in this file.");
      return;
    }
    for (const hunk of file.hunks) {
      yield () => this.hunkHead(hunk);
      yield* this.rows(file, hunk, asked);
    }
  }

  hunkHead(hunk) {
    const row = el("div", "hunk");
    row.append(
      el(
        "span",
        "range",
        `@@ -${hunk.old_start},${hunk.old_count} +${hunk.new_start},${hunk.new_count} @@`,
      ),
    );
    if (hunk.section) row.append(el("span", "section", hunk.section));
    return row;
  }

  // The rows of a hunk, with whitespace-only pairs folded away when asked.
  visible(hunk) {
    const out = [];
    for (const line of hunk.lines) {
      if (this.hideWhitespace && line.ws) {
        if (line.k === "d") continue;
        const partner = hunk.lines[line.p];
        out.push({ ...line, k: "c", o: partner ? partner.o : null });
        continue;
      }
      out.push(line);
    }
    return out;
  }

  *rows(file, hunk, asked) {
    const lines = this.visible(hunk);
    const quote = asked && asked.path === file.path ? asked.quote : null;
    let index = 0;
    let after = "c";
    while (index < lines.length) {
      if (!this.split || lines[index].k === "c") {
        const line = lines[index];
        const start = line.k !== "c" && after === "c";
        after = line.k;
        index += 1;
        if (line.mr) yield () => this.moveNote(line);
        yield () => this.single(line, start, quote);
        continue;
      }
      let end = index;
      while (end < lines.length && lines[end].k !== "c") end += 1;
      const removed = lines.slice(index, end).filter((l) => l.k === "d");
      const added = lines.slice(index, end).filter((l) => l.k === "a");
      for (let i = 0; i < Math.max(removed.length, added.length); i += 1) {
        const pair = [removed[i], added[i]];
        for (const line of pair) if (line?.mr) yield () => this.moveNote(line);
        yield () => this.pair(pair, i === 0, quote);
      }
      index = end;
      after = "c";
    }
  }

  cell(line, quote) {
    const kind = line.k;
    const marks = line.ws ? [] : line.w;
    const code = wordsIn(line.t, marks, kind === "a" ? "word add" : "word del");
    const sign = el("span", "sign", line.m ? "»" : SIGN[kind]);
    sign.title = line.m
      ? `${KIND[kind]}, moved`
      : line.ws
        ? `${KIND[kind]}, whitespace only`
        : KIND[kind];
    return { code, sign };
  }

  decorate(row, line, quote) {
    if (line.m) row.classList.add("moved");
    if (line.ws) row.classList.add("ws");
    if (quote && asks(line, quote)) {
      row.classList.add("asked");
      row.append(el("span", "tag", "asked about"));
    }
  }

  // Where a run of moved lines went or came from, on a row of its own so it is
  // never pushed off the edge by a long line.
  moveNote(line) {
    const note = line.mr;
    const to = line.k === "a" ? "moved from" : "moved to";
    const parts = [`\u00bb ${note.len} lines ${to} ${note.path}:${note.line}`];
    if (note.how === "renamed" && note.renames?.length)
      parts.push(
        `names changed: ${note.renames.map(([a, b]) => `${a} \u2192 ${b}`).join(", ")}`,
      );
    if (note.changed)
      parts.push(
        `${note.changed} ${note.changed === 1 ? "line" : "lines"} changed`,
      );
    return el("div", "moved-note", parts.join(" \u00b7 "));
  }

  single(line, start, quote) {
    const row = el("div", `row k-${line.k}`);
    if (line.o != null) row.dataset.o = line.o;
    if (line.n != null) row.dataset.n = line.n;
    if (start) row.classList.add("change-start");
    const { code, sign } = this.cell(line, quote);
    row.append(
      el("span", "num", line.o ?? ""),
      el("span", "num", line.n ?? ""),
      sign,
      code,
    );
    this.decorate(row, line, quote);
    return row;
  }

  pair([removed, added], first, quote) {
    const row = el("div", "row split");
    if (first) row.classList.add("change-start");
    for (const [line, side] of [
      [removed, "left"],
      [added, "right"],
    ]) {
      const half = el("div", `half ${side}`);
      if (line) {
        if (line.o != null) half.dataset.o = line.o;
        if (line.n != null) half.dataset.n = line.n;
        half.classList.add(`k-${line.k}`);
        const { code, sign } = this.cell(line, quote);
        half.append(
          el("span", "num", (line.k === "d" ? line.o : line.n) ?? ""),
          sign,
          code,
        );
        this.decorate(half, line, quote);
      } else {
        half.classList.add("empty");
      }
      row.append(half);
    }
    return row;
  }

  // Bring cited lines into view and mark them. The rows are drawn in pieces, so
  // lines not drawn yet are waited for. `side` is "new" or "old"; `path` limits
  // the search to that file, since two files can hold the same line number.
  reveal({ path, side, start, end }) {
    this.pendingCitation = { path, side, start, end };
    const key = side === "old" ? "o" : "n";
    for (const old of this.root.querySelectorAll(".cited"))
      old.classList.remove("cited");
    if (this.drawing) return;
    this.pendingCitation = null;
    const scope = path
      ? [...this.root.querySelectorAll(".diff-file")].find(
          (section) => section.dataset.file === path,
        )
      : this.root;
    const rows = [...(scope?.querySelectorAll(`[data-${key}]`) ?? [])].filter(
      (row) =>
        Number(row.dataset[key]) >= start && Number(row.dataset[key]) <= end,
    );
    const where = `${side === "old" ? "old " : ""}line${
      end > start ? `s ${start}\u2013${end}` : ` ${start}`
    }`;
    if (!rows.length) {
      this.tell(
        `${path ? `${path}, ` : ""}${where} is not shown. A filter may be hiding it.`,
      );
      return;
    }
    const body = rows[0].closest(".file-body");
    if (body?.hidden)
      body.previousElementSibling.querySelector(".file-toggle").click();
    for (const row of rows) row.classList.add("cited");
    rows[0].scrollIntoView({ block: "center" });
    // Keyboard and screen-reader users follow the jump: focus lands on the line,
    // and the status line names it.
    rows[0].tabIndex = -1;
    rows[0].focus({ preventScroll: true });
    this.tell(`Showing ${path ? `${path}, ` : ""}${where}.`);
  }

  changes() {
    return [...this.root.querySelectorAll(".change-start")];
  }

  updatePosition() {
    const total = this.changes().length;
    this.position.textContent = total
      ? `${this.current >= 0 ? this.current + 1 : "–"} of ${total} changes`
      : "";
  }

  step(direction) {
    const all = this.changes();
    if (!all.length) return;
    this.current = Math.min(
      all.length - 1,
      Math.max(0, this.current + direction),
    );
    all[this.current].scrollIntoView({ block: "center" });
    this.updatePosition();
  }
}
