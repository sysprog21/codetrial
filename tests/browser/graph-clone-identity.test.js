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
    nullNeighbor,
    originalNeighbors,
    root = "a",
  } = {},
) {
  const id = duplicate || 1;
  if (language === "javascript")
    return `function replicateTopology(node) {
    const a = new Node(1), b = new Node(2), c = new Node(3);
    a.neighbors = [c, b]; b.neighbors = [c, a]; c.neighbors = [b, a];
    ${duplicate ? `const d = new Node(${id}, ${id === 1 ? "a" : "b"}.neighbors.slice()); ${id === 1 ? "b.neighbors = [c, d]" : "c.neighbors = [d, a]"};` : ""}
    return ${root};
  }`;
  if (language === "python")
    return `def replicateTopology(node):
    a, b, c = Node(1), Node(2), Node(3)
    a.neighbors, b.neighbors, c.neighbors = [c, b], [c, a], [b, a]
    ${duplicate ? `d = Node(${id})\n    d.neighbors = list(${id === 1 ? "a" : "b"}.neighbors)\n    ${id === 1 ? "b.neighbors = [c, d]" : "c.neighbors = [d, a]"}` : ""}
    return ${root}
`;
  if (language === "cpp")
    return `class Solution { public: Node* replicateTopology(Node* node) {
    auto* a = new Node(1); auto* b = new Node(2); auto* c = new Node(3);
    a->neighbors = {c, b}; b->neighbors = {c, a}; c->neighbors = {b, a};
    ${duplicate ? `auto* d = new Node(${id}); d->neighbors = ${id === 1 ? "a" : "b"}->neighbors; ${id === 1 ? "b->neighbors = {c, d}" : "c->neighbors = {d, a}"};` : ""}
    return ${root};
  }};`;
  if (language === "java")
    return `class Solution { public Node replicateTopology(Node node) {
    Node a = new Node(1), b = new Node(2), c = new Node(3);
    a.neighbors.addAll(Arrays.asList(c, b)); b.neighbors.addAll(Arrays.asList(c, a)); c.neighbors.addAll(Arrays.asList(b, a));
    ${duplicate ? `Node d = new Node(${id}); d.neighbors.addAll(${id === 1 ? "a" : "b"}.neighbors); ${id === 1 ? "b.neighbors.set(1, d)" : "c.neighbors.set(0, d)"};` : ""}
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
    ${nullNeighbor ? "a->neighbors[0] = NULL;" : ""}
    ${originalNeighbors ? "b->neighbors = node->neighbors[0]->neighbors;" : ""}
    return ${root};
  }`;
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
  for (const [duplicate, root] of [
    [0, "a"],
    [1, "a"],
    [2, "a"],
    [0, "b"],
    [0, "c"],
  ]) {
    test(`${language} graph clone ${root !== "a" ? `rejects entry node ${root}` : duplicate ? `rejects duplicate ID ${duplicate}` : "accepts shared nodes and reordered neighbors"}`, async (t) => {
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
            code: candidate(language, { duplicate, root }),
          },
        );
        assert.equal(summary.setupError, undefined);
        assert.equal(summary.total, 1);
        assert.equal(summary.cases.length, 1);
        assert.equal(summary.passed, duplicate || root !== "a" ? 0 : 1);
        if (root !== "a")
          assert.match(summary.cases[0].error, /must start at node 1/);
        if (duplicate)
          assert.match(
            summary.cases[0].error,
            /distinct nodes with the same ID/,
          );
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
    const scenarios = [0, 1, 2].map((duplicate) => ({ duplicate }));
    scenarios.push({ duplicate: 0, root: "b" }, { duplicate: 0, root: "c" });
    if (language === "c")
      for (const invalidId of [0, 1024])
        for (const invalidRoot of [true, false])
          scenarios.push({ duplicate: 0, invalidId, invalidRoot });
    if (language === "c") scenarios.push({ duplicate: 0, nullNeighbor: true });
    if (language === "c")
      scenarios.push({ duplicate: 0, originalNeighbors: true });
    for (const scenario of scenarios) {
      const {
        duplicate,
        invalidId,
        invalidRoot,
        nullNeighbor,
        originalNeighbors,
        root = "a",
      } = scenario;
      test(
        `${language} graph clone ${root !== "a" ? `rejects entry node ${root}` : originalNeighbors ? "reports input nodes before duplicate IDs" : nullNeighbor ? "rejects a NULL neighbor" : invalidId !== undefined ? `rejects out-of-range ${invalidRoot ? "root" : "neighbor"} ID ${invalidId}` : duplicate ? `rejects duplicate ID ${duplicate}` : language === "c" ? "accepts shared nodes" : "accepts shared nodes and reordered neighbors"}`,
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
            else if (originalNeighbors)
              assert.equal(
                results[0].error,
                "Returned graph must be a deep copy.",
              );
            else if (nullNeighbor)
              assert.equal(
                results[0].error,
                "Returned graph has a NULL neighbor.",
              );
            else if (invalidId !== undefined)
              assert.equal(
                results[0].error,
                "Returned graph node IDs must be between 1 and 1023.",
              );
            else if (duplicate) assert.ok(results[0].error.endsWith(message));
            else assert.deepEqual(results[0].actual, spec.cases[0].expected);
          } finally {
            rmSync(dir, { recursive: true, force: true });
          }
        },
      );
    }
  }
});
