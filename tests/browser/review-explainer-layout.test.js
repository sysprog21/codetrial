import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { root } from "./source.js";

const { layoutFlow, wrapLabel, citeLabel, labelAt } = await import(
  pathToFileURL(
    join(root, ".claude/skills/codetrial-review/workbench/explainer.js"),
  ).href
);

const flow = (dir, ...pairs) => {
  const names = [...new Set(pairs.flat())];
  return {
    dir,
    nodes: names.map((text) => ({ id: text, text })),
    edges: pairs.map(([from, to]) => ({ from, to, label: "", ev: null })),
  };
};
const overlaps = (a, b) =>
  a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

test("a chain runs along its direction and its nodes never overlap", () => {
  const lr = layoutFlow(flow("LR", ["a", "b"], ["b", "c"], ["b", "d"]));
  const at = (id) => lr.nodes.find((n) => n.id === id);
  assert.ok(at("a").x < at("b").x && at("b").x < at("c").x);
  assert.equal(at("c").x, at("d").x);
  assert.notEqual(at("c").y, at("d").y);
  const tb = layoutFlow(flow("TB", ["a", "b"], ["b", "c"]));
  assert.ok(tb.nodes[0].y < tb.nodes[1].y && tb.nodes[1].y < tb.nodes[2].y);
  for (const laid of [lr, tb])
    laid.nodes.forEach((a, i) =>
      laid.nodes.slice(i + 1).forEach((b) => assert.ok(!overlaps(a, b))),
    );
});

// Points along the cubic segments of a path written as "Mx,y Cc1 c2 p ...".
const sample = (d) => {
  const n = d.match(/-?\d+(?:\.\d+)?/g).map(Number);
  const points = [];
  for (let i = 2; i + 5 < n.length + 1; i += 6) {
    const [x0, y0] = i === 2 ? [n[0], n[1]] : [n[i - 2], n[i - 1]];
    for (let k = 0; k <= 20; k += 1) {
      const t = k / 20;
      const m = 1 - t;
      const at = (a, b, c, e) =>
        m ** 3 * a + 3 * m * m * t * b + 3 * m * t * t * c + t ** 3 * e;
      points.push([
        at(x0, n[i], n[i + 2], n[i + 4]),
        at(y0, n[i + 1], n[i + 3], n[i + 5]),
      ]);
    }
  }
  return points;
};
const crossed = (laid, edge) =>
  laid.nodes
    .filter((n) => n.id !== edge.from && n.id !== edge.to)
    .filter((n) =>
      sample(edge.d).some(
        ([x, y]) =>
          x > n.x + 1 && x < n.x + n.w - 1 && y > n.y + 1 && y < n.y + n.h - 1,
      ),
    )
    .map((n) => n.text);

test("an edge that skips layers is routed around the nodes it skips", () => {
  // handler -> cache -> database -> caller, and cache -> caller past database.
  for (const dir of ["LR", "TB"]) {
    const laid = layoutFlow(
      flow(
        dir,
        ["handler", "cache"],
        ["cache", "caller"],
        ["cache", "database"],
        ["database", "caller"],
      ),
    );
    for (const e of laid.edges) assert.deepEqual(crossed(laid, e), [], dir);
    assert.ok(laid.width > 0 && laid.height > 0);
  }
});

test("a cycle terminates, keeps its direction and stays clear of the nodes", () => {
  const laid = layoutFlow(flow("LR", ["a", "b"], ["b", "c"], ["c", "a"]));
  assert.equal(laid.edges.length, 3);
  for (const e of laid.edges) {
    assert.match(e.d, /^M[\d.,-]+( C[\d.,\s-]+)+$/);
    assert.ok(e.mid.every(Number.isFinite));
    assert.deepEqual(crossed(laid, e), []);
  }
  // The closing edge ends at "a", on its near side: the arrow points back to it.
  const at = (id) => laid.nodes.find((n) => n.id === id);
  const [, , closing] = laid.edges;
  const end = sample(closing.d).at(-1);
  assert.ok(Math.abs(end[0] - (at("a").x + at("a").w)) < 1.5);
  assert.ok(Math.abs(end[1] - (at("a").y + at("a").h / 2)) < 14);
});

test("edges between the same two nodes are drawn side by side", () => {
  const laid = layoutFlow(flow("LR", ["a", "b"], ["b", "a"]));
  assert.notEqual(laid.edges[0].d, laid.edges[1].d);
  assert.notEqual(laid.edges[0].mid[1], laid.edges[1].mid[1]);
});

test("long names wrap and a citation reads as a short label", () => {
  assert.deepEqual(wrapLabel("cache lookup before the database", 12), [
    "cache lookup",
    "before the",
    "database",
  ]);
  assert.deepEqual(wrapLabel("x".repeat(30), 12), [
    "x".repeat(12),
    "x".repeat(12),
    "x".repeat(6),
  ]);
  const ev = { path: "src/a/cache.rs", side: "new", start: 4, end: 9 };
  assert.equal(citeLabel(ev), "cache.rs:4-9");
  assert.equal(citeLabel({ ...ev, side: "old", end: 4 }), "old cache.rs:4");
});

test("labels of edges that share a gap go to opposite sides", () => {
  const laid = layoutFlow({
    ...flow("LR", ["a", "b"], ["a", "c"]),
    edges: [
      { from: "a", to: "b", label: "up", ev: null },
      { from: "a", to: "c", label: "down", ev: null },
    ],
  });
  assert.notEqual(laid.edges[0].before, laid.edges[1].before);
  const lone = layoutFlow(flow("LR", ["a", "b"]));
  assert.equal(lone.edges[0].before, true);
  assert.equal(layoutFlow(flow("TB", ["a", "b"])).edges[0].before, false);
});

test("numbers of edges between the same two nodes do not overlap", () => {
  const laid = layoutFlow(flow("LR", ["a", "b"], ["a", "b"], ["a", "b"]));
  const gap = ([x, y], [u, v]) => Math.hypot(x - u, y - v);
  const [one, two, three] = laid.edges.map((e) => e.mid);
  for (const [p, q] of [
    [one, two],
    [two, three],
    [one, three],
  ])
    assert.ok(gap(p, q) >= 18, `badges ${gap(p, q)}px apart`);
});

// Everything drawn must lie inside the viewBox, or the picture clips it.
const inside = (laid, [x0, y0, x1, y1]) =>
  x0 >= laid.x - 0.01 &&
  y0 >= laid.y - 0.01 &&
  x1 <= laid.x + laid.width + 0.01 &&
  y1 <= laid.y + laid.height + 0.01;
const clipped = (laid) => {
  const bad = [];
  for (const n of laid.nodes)
    if (!inside(laid, [n.x, n.y, n.x + n.w, n.y + n.h]))
      bad.push(`node ${n.id}`);
  for (const e of laid.edges) {
    const [x, y] = e.mid;
    if (!inside(laid, [x - 9, y - 9, x + 9, y + 9]))
      bad.push(`badge ${e.number}`);
    if (e.label && !inside(laid, labelAt(e, laid.horizontal).box))
      bad.push(`label ${e.number}`);
    // A cubic stays inside the hull of its end and control points.
    for (const [px, py] of e.reach)
      if (!inside(laid, [px, py, px, py])) bad.push(`curve ${e.number}`);
  }
  return bad;
};

test("labels, badges and nodes never fall outside the picture", () => {
  // Cases the reviewers reproduced.
  const long = "x".repeat(48);
  for (const dir of ["LR", "TB"]) {
    const one = flow(dir, ["a", "b"]);
    one.edges[0].label = long;
    assert.deepEqual(
      clipped(layoutFlow(one)),
      [],
      `${dir}: a 48-character label`,
    );
    const many = flow(dir, ...Array.from({ length: 8 }, () => ["a", "b"]));
    assert.deepEqual(
      clipped(layoutFlow(many)),
      [],
      `${dir}: eight parallel edges`,
    );
  }
  // Seeded random graphs, 2 to 12 nodes, both directions, labels of any length.
  let seed = 7;
  // mulberry32: every bit is usable, unlike the low bits of a power-of-two LCG.
  const rand = (n) => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) % n;
  };
  for (let round = 0; round < 400; round += 1) {
    const size = 2 + rand(11);
    const pairs = Array.from({ length: 1 + rand(16) }, () => {
      const a = rand(size);
      return [`n${a}`, `n${(a + 1 + rand(size - 1)) % size}`];
    });
    const g = flow(round % 2 ? "LR" : "TB", ...pairs);
    g.edges.forEach((e) => (e.label = "w".repeat(rand(49))));
    assert.deepEqual(clipped(layoutFlow(g)), [], `round ${round}`);
  }
});

test("a pair of edges that run both ways keeps its numbers apart", () => {
  const laid = layoutFlow(flow("LR", ["a", "b"], ["b", "a"]));
  const [one, two] = laid.edges.map((e) => e.mid);
  assert.ok(Math.hypot(one[0] - two[0], one[1] - two[1]) >= 18);
});
