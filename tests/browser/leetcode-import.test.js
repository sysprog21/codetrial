import { readdir, readFile } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";

// Resolved from this file, not the shell's cwd, so the suite passes from
// anywhere `node --test` is pointed at it.
const repoFile = (path) => new URL(`../../${path}`, import.meta.url);

test("top interview manifest records 150 unique slugs grouped by topic", async () => {
  const manifest = JSON.parse(await readFile(repoFile("scripts/top-interview-150.json"), "utf8"));
  const bank = JSON.parse(await readFile(repoFile("problem-bank/problems.json"), "utf8"));
  const slugs = manifest.sections.flatMap((section) => section.slugs);

  assert.equal(slugs.length, 150);
  assert.equal(new Set(slugs).size, 150);
  assert.equal(manifest.sections.length, 23);
  assert.ok(manifest.sections.every((section) => section.topic && section.slugs.length > 0));
  assert.deepEqual(
    [...new Set(slugs)].sort(),
    bank.filter((problem) => problem.origin !== "original").map((problem) => problem.id).sort(),
  );
});

test("leetcode fetcher never asks GraphQL for statement prose", async () => {
  const source = await readFile(repoFile("scripts/problem_bank/bank.py"), "utf8");
  // Only the query text matters. Scanning the whole file for "content" fails the
  // build the day someone writes the word in a comment.
  const query = source.match(/DETAIL_QUERY = """([\s\S]*?)"""/)?.[1];

  assert.ok(query, "DETAIL_QUERY should still be a triple-quoted literal");
  assert.doesNotMatch(query, /\bcontent\b/);
  assert.match(query, /codeSnippets/);
  assert.match(query, /exampleTestcases/);
  assert.match(query, /metaData/);
});

test("all browser problems pose a scenario and expose only neutral interview metadata", async () => {
  // The variant rules themselves (no source title, a renamed entry point,
  // examples drawn from the judge, neutral case labels) are enforced by the
  // generator and tested in tests/test_gen_problems.py. What is left for here is
  // the shape of what actually ships.
  const bank = JSON.parse(await readFile(repoFile("problem-bank/problems.json"), "utf8"));
  const stages = ["repeat", "example", "algorithm", "coding", "test", "optimizations"];
  const allowed = ["difficulty", "followUpDirections", "reactoStages"];
  // The map is the only file that joins a published id to its page, and it
  // agrees with the pages and judges the browser loads by page name.
  const pageMap = JSON.parse(await readFile(repoFile("web/problem-pages.json"), "utf8"));
  assert.deepEqual(Object.keys(pageMap).sort(), bank.map(({ id }) => id).sort());
  const files = (await readdir(repoFile("web/problems/"))).map((file) => file.slice(0, -".json".length));
  assert.deepEqual(files.sort(), Object.values(pageMap).map(({ page }) => page).sort());
  const judges = (await readdir(repoFile("web/judges/"))).map((file) => file.slice(0, -".json".length));
  assert.deepEqual(judges.sort(), files);

  for (const source of bank) {
    const entry = pageMap[source.id];
    if (source.origin === "original") assert.equal(entry.source, undefined);
    else assert.deepEqual([entry.source, entry.title], [source.title, entry.title]);
    const problem = JSON.parse(await readFile(repoFile(`web/problems/${entry.page}.json`), "utf8"));
    assert.ok(!entry.page.includes(source.id), `${source.id} is served under its published slug`);
    assert.equal(problem.page, entry.page, `${source.id} does not know the page it is served as`);
    assert.equal(problem.title, entry.title);
    // What a scenario link loads names the problem by nothing but its page. A
    // one-word id is exempt, as one-word titles are: `triangle` is also the
    // name of that judge's parameter.
    // The published title is shown small beside the scenario; the id is not.
    assert.equal(problem.source, source.origin === "original" ? undefined : source.title);
    if (!/^[a-z]+$/.test(source.id)) {
      const judge = await readFile(repoFile(`web/judges/${entry.page}.json`), "utf8");
      for (const text of [JSON.stringify(problem), judge]) {
        assert.ok(!text.includes(`"${source.id}"`), `${entry.page} carries the published id`);
      }
    }
    const metadata = problem.interviewMetadata;
    assert.deepEqual(Object.keys(metadata).sort(), allowed);
    assert.equal(metadata.difficulty, source.difficulty);
    assert.deepEqual(metadata.reactoStages, stages);
    assert.deepEqual(metadata.followUpDirections.map(({ stage }) => stage), stages);
    assert.ok(metadata.followUpDirections.every(({ direction }) =>
      direction.startsWith("Ask ") && !/hash|stack|tree|sort|pointer|dynamic programming/i.test(direction)));
    assert.deepEqual(
      Object.keys(problem).sort(),
      ["brief", "difficulty", "examples", "interviewMetadata", "page", "starterCode", "title", ...(source.origin === "original" ? [] : ["source"])].sort(),
      source.id,
    );
    assert.notEqual(problem.title, source.title);
  }
});
