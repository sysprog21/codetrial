import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import {
  generateHarness,
  parseCompilerResults,
} from "../../web/compiler-explorer.js";
import { launchChromium, startStaticServer } from "./source.js";

const message =
  "Returned graph must not contain distinct nodes with the same ID.";
const spec = {
  kind: "function",
  entry: "replicateTopology",
  paramNames: ["node"],
  paramTypes: ["Node"],
  returnType: "Node",
  argTypes: ["graphNode"],
  outputType: "graphNode",
  checker: "exact",
  cases: [
    {
      label: "triangle",
      input: [
        [
          [2, 3],
          [1, 3],
          [1, 2],
        ],
      ],
      expected: [
        [2, 3],
        [1, 3],
        [1, 2],
      ],
    },
  ],
};

// Both duplicate placements are reachable through a later edge, after the
// legitimate object with that ID has already been visited.
function candidate(
  language,
  {
    duplicate,
    invalidId,
    invalidRoot,
    mixed,
    nullList,
    nullNeighbor,
    originalNeighbors,
    root = "a",
  } = {},
) {
  const id = duplicate || 1;
  // "nonRoot" nulls a neighbor of b, reached from the root, so a check that
  // looked only at the root's own list would not pass it.
  const holder = nullNeighbor === "nonRoot" ? "b" : "a";
  if (language === "javascript")
    return `function replicateTopology(node) {
    const a = new Node(1), b = new Node(2), c = new Node(3);
    a.neighbors = [c, b]; b.neighbors = [c, a]; c.neighbors = [b, a];
    ${duplicate ? `const d = new Node(${id}, ${id === 1 ? "a" : "b"}.neighbors.slice()); ${id === 1 ? "b.neighbors = [c, d]" : "c.neighbors = [d, a]"};` : ""}
    ${nullNeighbor ? `${holder}.neighbors[0] = ${nullNeighbor === "undefined" ? "undefined" : "null"};` : ""}
    ${mixed ? "a.neighbors = [node, null];" : ""}
    ${nullList ? "b.neighbors = null;" : ""}
    return ${root};
  }`;
  if (language === "python")
    return `def replicateTopology(node):
    a, b, c = Node(1), Node(2), Node(3)
    a.neighbors, b.neighbors, c.neighbors = [c, b], [c, a], [b, a]
    ${duplicate ? `d = Node(${id})\n    d.neighbors = list(${id === 1 ? "a" : "b"}.neighbors)\n    ${id === 1 ? "b.neighbors = [c, d]" : "c.neighbors = [d, a]"}` : ""}
    ${nullNeighbor ? `${holder}.neighbors[0] = None` : ""}
    ${mixed ? "a.neighbors = [node, None]" : ""}
    ${nullList ? "b.neighbors = None" : ""}
    return ${root}
`;
  if (language === "cpp")
    return `class Solution { public: Node* replicateTopology(Node* node) {
    auto* a = new Node(1); auto* b = new Node(2); auto* c = new Node(3);
    a->neighbors = {c, b}; b->neighbors = {c, a}; c->neighbors = {b, a};
    ${duplicate ? `auto* d = new Node(${id}); d->neighbors = ${id === 1 ? "a" : "b"}->neighbors; ${id === 1 ? "b->neighbors = {c, d}" : "c->neighbors = {d, a}"};` : ""}
    ${nullNeighbor ? `${holder}->neighbors[0] = nullptr;` : ""}
    ${mixed ? "a->neighbors = {node, nullptr};" : ""}
    return ${root};
  }};`;
  if (language === "java")
    return `class Solution { public Node replicateTopology(Node node) {
    Node a = new Node(1), b = new Node(2), c = new Node(3);
    a.neighbors.addAll(Arrays.asList(c, b)); b.neighbors.addAll(Arrays.asList(c, a)); c.neighbors.addAll(Arrays.asList(b, a));
    ${duplicate ? `Node d = new Node(${id}); d.neighbors.addAll(${id === 1 ? "a" : "b"}.neighbors); ${id === 1 ? "b.neighbors.set(1, d)" : "c.neighbors.set(0, d)"};` : ""}
    ${nullNeighbor ? `${holder}.neighbors.set(0, null);` : ""}
    ${mixed ? "a.neighbors.set(0, node); a.neighbors.set(1, null);" : ""}
    ${nullList ? "b.neighbors = null;" : ""}
    return ${root};
  }}`;
  return `struct Node* replicateTopology(struct Node* node) {
    struct Node* a = calloc(1, sizeof(*a)); struct Node* b = calloc(1, sizeof(*b)); struct Node* c = calloc(1, sizeof(*c));
    a->val = 1; b->val = 2; c->val = 3;
    a->numNeighbors = b->numNeighbors = c->numNeighbors = 2;
    a->neighbors = calloc(2, sizeof(*a->neighbors)); b->neighbors = calloc(2, sizeof(*b->neighbors)); c->neighbors = calloc(2, sizeof(*c->neighbors));
    a->neighbors[0] = b; a->neighbors[1] = c; b->neighbors[0] = a; b->neighbors[1] = c; c->neighbors[0] = a; c->neighbors[1] = b;
    ${duplicate ? `struct Node* d = calloc(1, sizeof(*d)); *d = *${id === 1 ? "a" : "b"}; ${id === 1 ? "b->neighbors[0]" : "c->neighbors[1]"} = d;` : ""}
    ${
      invalidId === undefined
        ? ""
        : `struct Node* extra = calloc(1, sizeof(*extra)); extra->val = ${invalidId};
    ${invalidRoot ? `struct Node* root = calloc(1, sizeof(*root)); root->val = ${invalidId}; root->numNeighbors = 2; root->neighbors = calloc(2, sizeof(*root->neighbors)); root->neighbors[0] = a; root->neighbors[1] = extra; return root;` : "a->neighbors = realloc(a->neighbors, 3 * sizeof(*a->neighbors)); a->neighbors[2] = extra; a->numNeighbors = 3;"}`
    }
    ${nullNeighbor ? `${holder}->neighbors[0] = NULL;` : ""}
    ${mixed ? "a->neighbors[0] = node; a->neighbors[1] = NULL;" : ""}
    ${originalNeighbors ? "b->neighbors = node->neighbors[0]->neighbors;" : ""}
    ${nullList ? "b->neighbors = NULL;" : ""}
    return ${root};
  }`;
}

const nullNeighborName = (kind, word) =>
  kind === "undefined"
    ? "an undefined neighbor"
    : `a ${word} neighbor${kind === "nonRoot" ? " of a non-root node" : ""}`;

// A scenario sets one kind at most, and is named for it in the order the
// toolchain suite checks its refusals: entry node, input nodes, null
// neighbor, null list, mixed violation, out-of-range ID, duplicate ID.
function title(language, scenario) {
  const {
    duplicate,
    invalidId,
    invalidRoot,
    mixed,
    nullList,
    nullNeighbor,
    originalNeighbors,
    root = "a",
  } = scenario;
  const browserRun = language === "javascript" || language === "python";
  const nullWord = browserRun ? "null" : "NULL";
  let what;
  if (root !== "a") what = `rejects entry node ${root}`;
  else if (originalNeighbors) what = "reports input nodes before duplicate IDs";
  else if (nullNeighbor)
    what = `rejects ${nullNeighborName(nullNeighbor, nullWord)}`;
  else if (nullList) what = "reads a null neighbors list as empty";
  else if (mixed)
    what = `names the ${language === "c" ? "input node" : `${nullWord} neighbor`} in a mixed violation`;
  else if (invalidId !== undefined)
    what = `rejects out-of-range ${invalidRoot ? "root" : "neighbor"} ID ${invalidId}`;
  else if (duplicate) what = `rejects duplicate ID ${duplicate}`;
  else if (language === "c") what = "accepts shared nodes";
  else what = "accepts shared nodes and reordered neighbors";
  return `${language} graph clone ${what}`;
}

let browser, server, base;
before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    handle: (_request, response, url) => {
      if (url.pathname !== "/judges/graph-clone-identity.json") return false;
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(spec));
      return true;
    },
  }));
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

for (const language of ["javascript", "python"]) {
  for (const scenario of [
    {},
    { duplicate: 1 },
    { duplicate: 2 },
    { root: "b" },
    { root: "c" },
    { nullNeighbor: true },
    { nullNeighbor: "nonRoot" },
    ...(language === "javascript" ? [{ nullNeighbor: "undefined" }] : []),
    { mixed: true },
    { nullList: true },
  ]) {
    const { duplicate, root = "a", nullNeighbor, mixed, nullList } = scenario;
    test(title(language, scenario), async (t) => {
      if (!browser) return t.skip("playwright chromium unavailable");
      const page = await browser.newPage();
      try {
        await page.goto(`${base}/observer.html`);
        const summary = await page.evaluate(
          async ({ language, code }) => {
            const { runBrowserTests } = await import("/runners.js");
            return runBrowserTests("graph-clone-identity", code, language);
          },
          {
            language,
            code: candidate(language, scenario),
          },
        );
        assert.equal(summary.setupError, undefined);
        assert.equal(summary.total, 1);
        assert.equal(summary.cases.length, 1);
        assert.equal(
          summary.passed,
          duplicate || root !== "a" || nullNeighbor || mixed || nullList
            ? 0
            : 1,
        );
        if (root !== "a")
          assert.match(summary.cases[0].error, /must start at node 1/);
        if (duplicate)
          assert.match(
            summary.cases[0].error,
            /distinct nodes with the same ID/,
          );
        if (nullNeighbor || mixed)
          assert.match(
            summary.cases[0].error,
            /Returned graph has a NULL neighbor\./,
          );
        if (nullList) {
          assert.equal(summary.cases[0].error, undefined);
          assert.equal(summary.cases[0].got, "[[2,3],[],[1,2]]");
        }
      } finally {
        await page.close();
      }
    });
  }
}

const run = promisify(execFile);
describe("graph clone toolchain harnesses", { concurrency: 4 }, () => {
  for (const [language, compiler, flags, extension] of [
    ["c", "cc", ["-std=c17"], "c"],
    ["cpp", "g++", ["-std=c++20"], "cpp"],
    ["java", "javac", [], "java"],
  ]) {
    let available = true;
    try {
      execFileSync(compiler, [language === "java" ? "-version" : "--version"], {
        stdio: "ignore",
      });
      if (language === "java")
        execFileSync("java", ["-version"], { stdio: "ignore" });
    } catch {
      available = false;
    }
    const scenarios = [{}, { duplicate: 1 }, { duplicate: 2 }];
    scenarios.push({ root: "b" }, { root: "c" });
    if (language === "c")
      for (const invalidId of [0, 1024])
        for (const invalidRoot of [true, false])
          scenarios.push({ invalidId, invalidRoot });
    // A copied node whose neighbors are [an input node, null] breaks two
    // rules at once, and which one is named depends on the walk: C checks
    // each edge in order and meets the input node first, while C++, Java,
    // JavaScript and Python refuse a null while queueing every neighbor of
    // the node before any of them is visited. Only a single violation reads
    // the same in all five; these scenarios pin C, C++ and Java, and the
    // browser suite above pins JavaScript and Python.
    scenarios.push(
      { nullNeighbor: true },
      { nullNeighbor: "nonRoot" },
      { mixed: true },
    );
    if (language === "c") scenarios.push({ originalNeighbors: true });
    // C++ holds neighbors in a vector, which cannot be null. In C the null
    // list keeps its numNeighbors of two, so the harness must not trust it.
    if (language === "c" || language === "java")
      scenarios.push({ nullList: true });
    for (const scenario of scenarios) {
      const {
        duplicate,
        invalidId,
        mixed,
        nullList,
        nullNeighbor,
        originalNeighbors,
        root = "a",
      } = scenario;
      test(
        title(language, scenario),
        {
          skip: available
            ? false
            : language === "java"
              ? "javac/java are not available"
              : `${compiler} is not available`,
        },
        async () => {
          const dir = mkdtempSync(join(tmpdir(), "codetrial-graph-"));
          try {
            const source = join(
              dir,
              language === "java" ? "Main.java" : `graph.${extension}`,
            );
            const binary = join(dir, "graph");
            writeFileSync(
              source,
              generateHarness(language, spec, candidate(language, scenario)),
            );
            await run(
              compiler,
              language === "java" ? [source] : [...flags, source, "-o", binary],
            );
            const output =
              language === "java"
                ? await run("java", ["-cp", dir, "Main"])
                : await run(binary);
            const results = parseCompilerResults(output.stdout).results;
            assert.equal(results.length, 1);
            if (root !== "a")
              assert.match(results[0].error, /must start at node 1/);
            else if (originalNeighbors || (mixed && language === "c"))
              assert.equal(
                results[0].error,
                "Returned graph must be a deep copy.",
              );
            else if (nullNeighbor || mixed)
              assert.ok(
                results[0].error.endsWith(
                  "Returned graph has a NULL neighbor.",
                ),
                results[0].error,
              );
            else if (invalidId !== undefined)
              assert.equal(
                results[0].error,
                "Returned graph node IDs must be between 1 and 1023.",
              );
            else if (duplicate) assert.ok(results[0].error.endsWith(message));
            else if (nullList) {
              assert.equal(results[0].error, undefined);
              assert.deepEqual(results[0].actual, [[2, 3], [], [1, 2]]);
            } else assert.deepEqual(results[0].actual, spec.cases[0].expected);
          } finally {
            rmSync(dir, { recursive: true, force: true });
          }
        },
      );
    }
  }
});
