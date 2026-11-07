// Draws a reference note the server read (explainer.py): prose, a flow
// diagram, a comparison table and code quoted from the pinned diff.
//
// It decides nothing about what is true. Every citation and every quoted line
// arrives already checked against the diff, and a citation is a button that
// asks the page to show that line. An edge the reviewer inferred has no
// citation and is drawn dashed and named, so a picture is not read as a fact.
// All text goes in through textContent; no markup from a note is parsed.

const SVG = "http://www.w3.org/2000/svg";
const NODE_PX = 13; // font sizes of the drawing, which the measurer is asked for
const LABEL_PX = 12;
const LINE = 16;
const WRAP = 22; // characters before a node name wraps
const PAD_X = 14;
const PAD_Y = 9;
const GAP = 22; // between nodes of one layer
const MARGIN = 20;
const PAD = 12; // around everything drawn, so nothing touches the edge of the picture
// Width of a run of text when the caller cannot measure it: a plain estimate.
const estimate = (text, px) => text.length * px * 0.55;
const WAYPOINT = 10; // cross size of the invisible waypoint an edge leaves in a layer it skips
const MIN_SCALE = 0.85; // a wide diagram scrolls rather than shrinks its text below this
const PORT = 12; // between edges that leave or enter one side of a node

export const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const svg = (tag, attrs = {}, text) => {
  const node = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs))
    node.setAttribute(key, value);
  if (text !== undefined) node.textContent = text;
  return node;
};

export function citeLabel(ev) {
  const name = ev.path.split("/").pop();
  const lines = ev.end === ev.start ? ev.start : `${ev.start}-${ev.end}`;
  return `${ev.side === "old" ? "old " : ""}${name}:${lines}`;
}

// A citation as a button that asks the page to show that line in the diff.
export function citeButton(ev, onCite) {
  const button = el("button", "ex-cite", citeLabel(ev));
  button.type = "button";
  const lines = ev.end === ev.start ? ev.start : `${ev.start} to ${ev.end}`;
  button.title = `${ev.side === "old" ? "Before: " : ""}${ev.path}, line ${lines}. Show it in the diff.`;
  button.addEventListener("click", () => onCite(ev));
  return button;
}

// A box that scrolls, so it takes the keyboard: named, focusable, and a region.
const scrollRegion = (node, label) => {
  node.tabIndex = 0;
  node.setAttribute("role", "region");
  node.setAttribute("aria-label", label);
  return node;
};

export function wrapLabel(text, max = WRAP) {
  const lines = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    let rest = word;
    while (rest.length > max) {
      if (line) lines.push(line);
      line = "";
      lines.push(rest.slice(0, max));
      rest = rest.slice(max);
    }
    if (line && line.length + 1 + rest.length > max) {
      lines.push(line);
      line = rest;
    } else {
      line = line ? `${line} ${rest}` : rest;
    }
  }
  if (line) lines.push(line);
  return lines;
}

// Where an edge's label is drawn: the text origin, its anchor and the box it fills.
// Layout and drawing both ask here, so the picture always has room for its labels.
export function labelAt(e, horizontal) {
  const w = e.labelWidth;
  const [x, y] = e.mid;
  if (horizontal) {
    const ty = e.before ? y - 15 : y + 24;
    return {
      x,
      y: ty,
      anchor: "middle",
      box: [x - w / 2, ty - 12, x + w / 2, ty + 4],
    };
  }
  const tx = e.before ? x - 15 : x + 15;
  return {
    x: tx,
    y: y + 4,
    anchor: e.before ? "end" : "start",
    box: e.before ? [tx - w, y - 8, tx, y + 8] : [tx, y - 8, tx + w, y + 8],
  };
}

const mainOf = (n, horizontal) => (horizontal ? n.w : n.h);
const crossOf = (n, horizontal) => (horizontal ? n.h : n.w);

// Layers by longest path. An edge that closes a cycle is `back`: it is laid out
// as if it ran the other way, and drawn with its arrow at the original end.
function assignLayers(flow, nodes) {
  const out = new Map(nodes.map((n) => [n.id, []]));
  flow.edges.forEach((e, i) => out.get(e.from).push(i));
  const state = new Map();
  const back = new Set();
  const order = [];
  const visit = (id) => {
    state.set(id, 1);
    for (const i of out.get(id)) {
      const to = flow.edges[i].to;
      if (state.get(to) === 1) back.add(i);
      else if (!state.has(to)) visit(to);
    }
    state.set(id, 2);
    order.push(id);
  };
  for (const n of nodes) if (!state.has(n.id)) visit(n.id);
  order.reverse();
  const layer = new Map(order.map((id) => [id, 0]));
  for (const id of order)
    for (const i of out.get(id)) {
      const to = flow.edges[i].to;
      if (!back.has(i))
        layer.set(to, Math.max(layer.get(to), layer.get(id) + 1));
    }
  return { layer, back, depth: Math.max(...layer.values()) };
}

// The rows of items, and each edge as the run of items it passes, from the
// earlier layer to the later. An edge that skips layers gets an invisible
// waypoint in each, which the real nodes of that layer make room for, so no edge
// is drawn through a node.
function buildChains(flow, nodes, { layer, back, depth }, horizontal) {
  const items = new Map(nodes.map((n) => [n.id, n]));
  const rows = Array.from({ length: depth + 1 }, () => []);
  for (const n of nodes) rows[layer.get(n.id)].push(n);
  const chains = flow.edges.map((e, i) => {
    const reversed = back.has(i);
    const [first, last] = reversed ? [e.to, e.from] : [e.from, e.to];
    const ids = [first];
    for (let k = layer.get(first) + 1; k < layer.get(last); k += 1) {
      const dummy = {
        id: `~${i}.${k}`,
        dummy: true,
        w: horizontal ? 0 : WAYPOINT,
        h: horizontal ? WAYPOINT : 0,
      };
      items.set(dummy.id, dummy);
      rows[k].push(dummy);
      ids.push(dummy.id);
    }
    ids.push(last);
    // The gap after which this edge's label sits: the one its first segment crosses.
    const gap = reversed ? layer.get(last) - 1 : layer.get(first);
    return { i, reversed, ids, gap };
  });
  return { items, rows, chains };
}

// Two sweeps: an item sits near the items it connects to, ties keep their order.
function orderRows(rows, chains) {
  const depth = rows.length - 1;
  const above = new Map();
  const below = new Map();
  for (const { ids } of chains)
    ids.slice(1).forEach((id, j) => {
      (below.get(ids[j]) ?? below.set(ids[j], []).get(ids[j])).push(id);
      (above.get(id) ?? above.set(id, []).get(id)).push(ids[j]);
    });
  const sweep = (k, other, links) => {
    const at = new Map(rows[other].map((n, i) => [n.id, i]));
    const mean = (n, i) => {
      const near = (links.get(n.id) ?? []).map((id) => at.get(id));
      return near.length ? near.reduce((x, y) => x + y, 0) / near.length : i;
    };
    rows[k] = rows[k]
      .map((n, i) => [n, mean(n, i), i])
      .sort((x, y) => x[1] - y[1] || x[2] - y[2])
      .map(([n]) => n);
  };
  for (let k = 1; k <= depth; k += 1) sweep(k, k - 1, above);
  for (let k = depth - 1; k >= 0; k -= 1) sweep(k, k + 1, below);
}

// Gives every item its x and y: layers along the flow, each centered across it.
function placeRows(rows, chains, flow, horizontal, measure) {
  const spans = rows.map(
    (row) =>
      row.reduce((sum, n) => sum + crossOf(n, horizontal), 0) +
      GAP * (row.length - 1),
  );
  const widest = Math.max(...spans);
  // Room after a layer for the longest label of an edge that leaves it first.
  const room = (k) => {
    const labels = chains
      .filter((c) => c.gap === k)
      .map((c) => measure(flow.edges[c.i].label, LABEL_PX));
    return horizontal ? Math.max(56, Math.max(0, ...labels) + 28) : 60;
  };
  let at = MARGIN;
  rows.forEach((row, k) => {
    const size = Math.max(
      ...row.map((n) => (n.dummy ? 0 : mainOf(n, horizontal))),
    );
    let across = MARGIN + (widest - spans[k]) / 2;
    for (const n of row) {
      const along = at + (size - mainOf(n, horizontal)) / 2;
      n.x = horizontal ? along : across;
      n.y = horizontal ? across : along;
      across += crossOf(n, horizontal) + GAP;
    }
    at += size + room(k);
  });
}

// Edges that meet one side of a node leave it spread along that side, in the
// order of where they head, so they neither start on one point nor overlap.
// Returns how far each edge's end is shifted, by `${edge}|out` and `${edge}|in`.
function spreadPorts(chains, items, horizontal) {
  const center = (id) => {
    const n = items.get(id);
    return horizontal ? n.y + n.h / 2 : n.x + n.w / 2;
  };
  const ports = new Map();
  chains.forEach(({ ids }, i) => {
    for (const [id, side, next] of [
      [ids[0], "out", ids[1]],
      [ids.at(-1), "in", ids.at(-2)],
    ]) {
      const list = ports.get(`${id}|${side}`) ?? [];
      list.push({ i, side, at: center(next) });
      ports.set(`${id}|${side}`, list);
    }
  });
  const shifts = new Map();
  for (const [key, list] of ports) {
    const step = Math.min(
      PORT,
      (crossOf(items.get(key.split("|")[0]), horizontal) - 10) / list.length,
    );
    list
      .sort((x, y) => x.at - y.at || x.i - y.i)
      .forEach((port, j) =>
        shifts.set(
          `${port.i}|${port.side}`,
          (j - (list.length - 1) / 2) * step,
        ),
      );
  }
  return shifts;
}

// Each edge as a path through its items, with where its number sits and how far
// its curves reach.
function routeEdges(flow, chains, items, shifts, horizontal, measure) {
  // The edges between each pair of nodes, either way round, spread their numbers.
  const pairs = new Map();
  const mates = flow.edges.map((e, i) => {
    const key = [e.from, e.to].sort().join("|");
    const list = pairs.get(key) ?? [];
    list.push(i);
    pairs.set(key, list);
    return list;
  });
  // The control points of the cubic from p to q, with tangents along the flow.
  const tangents = (p, q) => {
    const half = horizontal ? (q[0] - p[0]) / 2 : (q[1] - p[1]) / 2;
    return horizontal
      ? [
          [p[0] + half, p[1]],
          [q[0] - half, q[1]],
        ]
      : [
          [p[0], p[1] + half],
          [q[0], q[1] - half],
        ];
  };
  return flow.edges.map((e, i) => {
    const { reversed, ids } = chains[i];
    const points = ids.map((id, j) => {
      const n = items.get(id);
      const shift = shifts.get(`${i}|${j === 0 ? "out" : "in"}`);
      if (j === 0)
        return horizontal
          ? [n.x + n.w, n.y + n.h / 2 + shift]
          : [n.x + n.w / 2 + shift, n.y + n.h];
      if (j === ids.length - 1)
        return horizontal
          ? [n.x, n.y + n.h / 2 + shift]
          : [n.x + n.w / 2 + shift, n.y];
      return [n.x + n.w / 2, n.y + n.h / 2];
    });
    if (reversed) points.reverse();
    const r = (v) => Math.round(v * 10) / 10;
    const at = (pt) => `${r(pt[0])},${r(pt[1])}`;
    let d = `M${at(points[0])}`;
    const reach = [points[0]];
    let first;
    for (let j = 1; j < points.length; j += 1) {
      const ctl = tangents(points[j - 1], points[j]);
      first ??= ctl;
      d += ` C${[...ctl, points[j]].map(at).join(" ")}`;
      reach.push(...ctl, points[j]);
    }
    // Where the number sits on the first segment: edges between the same two
    // nodes are spread along it, in the order they run, so a pair that goes both
    // ways (A to B and back) does not put both numbers at one place.
    const [p, q] = points;
    const [c1, c2] = first;
    const spread = (mates[i].indexOf(i) - (mates[i].length - 1) / 2) * 0.4;
    const t = Math.min(
      0.85,
      Math.max(0.15, 0.5 + (reversed ? -spread : spread)),
    );
    const u = 1 - t;
    const mid = [0, 1].map(
      (k) =>
        u ** 3 * p[k] +
        3 * u * u * t * c1[k] +
        3 * u * t * t * c2[k] +
        t ** 3 * q[k],
    );
    return {
      ...e,
      number: i + 1,
      d,
      mid,
      reach,
      labelWidth: measure(e.label, LABEL_PX),
    };
  });
}

// Labels of edges that share a gap go to opposite sides, upper half first, so
// they do not print over each other.
function setLabelSides(edges, chains, horizontal) {
  const byGap = new Map();
  edges.forEach((e, i) => {
    const list = byGap.get(chains[i].gap) ?? [];
    list.push(e);
    byGap.set(chains[i].gap, list);
  });
  const axis = horizontal ? 1 : 0;
  for (const list of byGap.values())
    list
      .sort((x, y) => x.mid[axis] - y.mid[axis] || x.number - y.number)
      .forEach((e, j) => {
        // A lone label goes above a horizontal flow and right of a vertical one.
        e.before = list.length < 2 ? horizontal : j < list.length / 2;
      });
}

// The picture is as big as what is drawn in it: nodes, numbers, labels, curves.
function framing(nodes, edges, horizontal) {
  const box = [Infinity, Infinity, -Infinity, -Infinity];
  const grow = (x0, y0, x1, y1) => {
    box[0] = Math.min(box[0], x0);
    box[1] = Math.min(box[1], y0);
    box[2] = Math.max(box[2], x1);
    box[3] = Math.max(box[3], y1);
  };
  for (const n of nodes) grow(n.x, n.y, n.x + n.w, n.y + n.h);
  for (const e of edges) {
    grow(e.mid[0] - 9, e.mid[1] - 9, e.mid[0] + 9, e.mid[1] + 9);
    if (e.label) grow(...labelAt(e, horizontal).box);
    for (const [x, y] of e.reach) grow(x, y, x, y);
  }
  return {
    x: box[0] - PAD,
    y: box[1] - PAD,
    width: box[2] - box[0] + 2 * PAD,
    height: box[3] - box[1] + 2 * PAD,
  };
}

// Places the nodes of a flow in layers along its direction. Pure, so a test can
// check it without a page; `measure(text, px)` is the width of a run of text.
export function layoutFlow(flow, measure = estimate) {
  const horizontal = flow.dir === "LR";
  const nodes = flow.nodes.map((n) => {
    const lines = wrapLabel(n.text);
    const width =
      Math.max(...lines.map((l) => measure(l, NODE_PX))) + 2 * PAD_X;
    return {
      ...n,
      lines,
      w: Math.max(72, width),
      h: lines.length * LINE + 2 * PAD_Y,
    };
  });
  const layers = assignLayers(flow, nodes);
  const { items, rows, chains } = buildChains(flow, nodes, layers, horizontal);
  orderRows(rows, chains);
  placeRows(rows, chains, flow, horizontal, measure);
  const shifts = spreadPorts(chains, items, horizontal);
  const edges = routeEdges(flow, chains, items, shifts, horizontal, measure);
  setLabelSides(edges, chains, horizontal);
  return { nodes, edges, ...framing(nodes, edges, horizontal), horizontal };
}

let figures = 0;

// `code` inside a run of prose; nothing else is read as markup.
function inline(parent, text) {
  text.split(/(`[^`]+`)/).forEach((piece) => {
    if (!piece) return;
    if (piece.startsWith("`") && piece.endsWith("`") && piece.length > 2)
      parent.append(el("code", "", piece.slice(1, -1)));
    else parent.append(piece);
  });
}

// Prose with citations. **bold** may wrap a citation, so it is read across the
// pieces; an unpaired ** is left as the characters it is.
function parts(parent, items, onCite) {
  // A backtick span is code: a `**kwargs` in it is neither a bold mark nor
  // split by one.
  const pieces = (part) => part.split(/(`[^`]+`)/);
  const isCode = (piece) =>
    piece.startsWith("`") && piece.endsWith("`") && piece.length > 2;
  const marks = items.reduce(
    (n, part) =>
      n +
      (typeof part === "string"
        ? pieces(part).reduce(
            (m, piece) =>
              m + (isCode(piece) ? 0 : piece.split("**").length - 1),
            0,
          )
        : 0),
    0,
  );
  const pairs = marks % 2 === 0;
  let target = parent;
  for (const part of items) {
    if (typeof part !== "string") {
      target.append(citeButton(part, onCite));
      continue;
    }
    for (const chunk of pieces(part)) {
      if (isCode(chunk) || !pairs) {
        if (chunk) inline(target, chunk);
        continue;
      }
      for (const piece of chunk.split(/(\*\*)/)) {
        if (piece === "**")
          target =
            target === parent ? parent.appendChild(el("strong")) : parent;
        else if (piece) inline(target, piece);
      }
    }
  }
  return parent;
}

// Text width as the browser will draw it, with the page's own font.
let measureText;
function measurer() {
  if (measureText) return measureText;
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return undefined;
  const family = getComputedStyle(document.body).fontFamily;
  measureText = (text, px) => {
    ctx.font = `${px}px ${family}`;
    return ctx.measureText(text).width;
  };
  return measureText;
}

// The two arrowheads, a cited arrow's and an inferred one's, for one figure.
function arrowDefs(id) {
  const defs = svg("defs");
  for (const kind of ["solid", "inferred"]) {
    const marker = svg("marker", {
      id: `${id}-${kind}`,
      viewBox: "0 0 10 10",
      refX: "9",
      refY: "5",
      markerWidth: "7",
      markerHeight: "7",
      orient: "auto-start-reverse",
    });
    marker.append(
      svg("path", {
        d: "M0 0L10 5L0 10z",
        class: kind === "solid" ? "ex-arrow" : "ex-arrow ex-inferred",
      }),
    );
    defs.append(marker);
  }
  return defs;
}

function nodeShape(n) {
  const group = svg("g", { class: "ex-node" });
  group.append(svg("rect", { x: n.x, y: n.y, width: n.w, height: n.h, rx: 8 }));
  n.lines.forEach((line, i) =>
    group.append(
      svg(
        "text",
        {
          x: n.x + n.w / 2,
          y: n.y + PAD_Y + LINE * i + LINE * 0.75,
          "text-anchor": "middle",
        },
        line,
      ),
    ),
  );
  return group;
}

// An edge's number, and its label beside it.
function badgeShape(e, horizontal) {
  const [x, y] = e.mid;
  const group = svg("g", {
    class: `ex-badge${e.inferred ? " ex-inferred" : ""}`,
  });
  group.append(
    svg("circle", { cx: x, cy: y, r: 9 }),
    svg("text", { x, y: y + 3.5, "text-anchor": "middle" }, String(e.number)),
  );
  if (e.label) {
    const at = labelAt(e, horizontal);
    group.append(
      svg(
        "text",
        { x: at.x, y: at.y, "text-anchor": at.anchor, class: "ex-label" },
        e.label,
      ),
    );
  }
  return group;
}

function flowCanvas(laid, id, label) {
  const canvas = svg("svg", {
    viewBox: `${laid.x} ${laid.y} ${laid.width} ${laid.height}`,
    role: "img",
    "aria-label": label,
  });
  // Through the CSSOM: the page's policy refuses an inline style attribute.
  canvas.style.maxWidth = `${laid.width}px`;
  canvas.style.minWidth = `${Math.round(laid.width * MIN_SCALE)}px`;
  canvas.append(arrowDefs(id));
  for (const e of laid.edges)
    canvas.append(
      svg("path", {
        d: e.d,
        class: `ex-edge${e.inferred ? " ex-inferred" : ""}`,
        "marker-end": `url(#${id}-${e.inferred ? "inferred" : "solid"})`,
      }),
    );
  for (const n of laid.nodes) canvas.append(nodeShape(n));
  for (const e of laid.edges) canvas.append(badgeShape(e, laid.horizontal));
  return canvas;
}

// The text equivalent of the picture, and where each arrow is grounded.
function edgeList(edges, name, onCite) {
  const list = el("ol", "ex-edges");
  for (const e of edges) {
    const item = el("li");
    item.append(
      el("span", "ex-num", String(e.number)),
      el("span", "ex-step", `${name.get(e.from)} → ${name.get(e.to)}`),
    );
    if (e.label) item.append(el("span", "ex-note", e.label));
    item.append(
      e.ev
        ? citeButton(e.ev, onCite)
        : el("span", "ex-tag", "inferred · no diff line cited"),
    );
    list.append(item);
  }
  return list;
}

function flowFigure(flow, onCite) {
  const laid = layoutFlow(flow, measurer());
  figures += 1;
  const name = new Map(flow.nodes.map((n) => [n.id, n.text]));
  const named = flow.edges
    .map((e) => `${name.get(e.from)} to ${name.get(e.to)}`)
    .join("; ");
  const scroll = el("div", "ex-scroll");
  scrollRegion(scroll, "Flow diagram");
  scroll.append(flowCanvas(laid, `ex${figures}`, `Flow diagram: ${named}`));
  const figure = el("figure", "ex-figure");
  figure.append(
    scroll,
    edgeList(laid.edges, name, onCite),
    el(
      "figcaption",
      "ex-caption",
      "Solid arrows cite a line of the diff. Dashed arrows are the reviewer’s inference.",
    ),
  );
  return figure;
}

function codeFigure(block, onCite) {
  const figure = el("figure", "ex-code");
  const head = el("figcaption", "ex-code-head");
  head.append(
    citeButton(block.ev, onCite),
    el(
      "span",
      "ex-tag",
      block.ev.side === "old" ? "before the change" : "after the change",
    ),
  );
  const body = el("div", "ex-code-body");
  scrollRegion(body, `Code from ${block.ev.path}`);
  for (const row of block.rows) {
    const line = el("div", `ex-row k-${row.k}`);
    line.append(
      el("span", "num", (block.ev.side === "old" ? row.o : row.n) ?? ""),
      el("span", "sign", { a: "+", d: "-", c: " " }[row.k]),
      el("span", "code", row.t || " "),
    );
    body.append(line);
  }
  figure.append(head, body);
  return figure;
}

function tableBlock(block, onCite) {
  const wrap = el("div", "ex-table-wrap");
  scrollRegion(wrap, "Comparison table");
  const table = el("table", "ex-table");
  const head = el("tr");
  for (const items of block.head) {
    const cell = el("th");
    cell.scope = "col";
    head.append(parts(cell, items, onCite));
  }
  const thead = el("thead");
  thead.append(head);
  table.append(thead);
  const body = el("tbody");
  for (const row of block.rows) {
    const tr = el("tr");
    row.forEach((items, i) => {
      const cell = el(i === 0 ? "th" : "td");
      if (i === 0) cell.scope = "row";
      tr.append(parts(cell, items, onCite));
    });
    body.append(tr);
  }
  table.append(body);
  wrap.append(table);
  return wrap;
}

// The note as the page shows it. `onCite(ev)` is asked to show a cited line.
export function renderNote(blocks, onCite) {
  const root = el("div", "ex-note-body");
  for (const block of blocks) {
    if (block.t === "p") root.append(parts(el("p"), block.parts, onCite));
    else if (block.t === "h")
      root.append(parts(el("h4", "ex-heading"), block.parts, onCite));
    else if (block.t === "ul") {
      const list = el("ul");
      for (const item of block.items)
        list.append(parts(el("li"), item, onCite));
      root.append(list);
    } else if (block.t === "flow") root.append(flowFigure(block, onCite));
    else if (block.t === "code") root.append(codeFigure(block, onCite));
    else if (block.t === "table") root.append(tableBlock(block, onCite));
  }
  return root;
}
