import { readFile } from "node:fs/promises";
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
  assert.deepEqual([...new Set(slugs)].sort(), bank.map((problem) => problem.id).sort());
});

test("leetcode fetcher never asks GraphQL for statement prose", async () => {
  const source = await readFile(repoFile("scripts/gen-problems.py"), "utf8");
  // Only the query text matters. Scanning the whole file for "content" fails the
  // build the day someone writes the word in a comment.
  const query = source.match(/DETAIL_QUERY = """([\s\S]*?)"""/)?.[1];

  assert.ok(query, "DETAIL_QUERY should still be a triple-quoted literal");
  assert.doesNotMatch(query, /\bcontent\b/);
  assert.match(query, /codeSnippets/);
  assert.match(query, /exampleTestcases/);
  assert.match(query, /metaData/);
});

test("all browser problems expose only ordered neutral interview metadata", async () => {
  const bank = JSON.parse(await readFile(repoFile("problem-bank/problems.json"), "utf8"));
  const stages = ["repeat", "example", "algorithm", "coding", "test", "optimizations"];
  const allowed = ["competencies", "difficulty", "followUpDirections", "reactoStages"];

  for (const source of bank) {
    const problem = JSON.parse(await readFile(repoFile(`web/problems/${source.id}.json`), "utf8"));
    const metadata = problem.interviewMetadata;
    assert.deepEqual(Object.keys(metadata).sort(), allowed);
    assert.equal(metadata.difficulty, source.difficulty);
    assert.deepEqual(metadata.competencies, source.topics);
    assert.deepEqual(metadata.reactoStages, stages);
    assert.deepEqual(metadata.followUpDirections.map(({ stage }) => stage), stages);
    assert.ok(metadata.followUpDirections.every(({ direction }) =>
      direction.startsWith("Ask ") && !/hash|stack|tree|sort|pointer|dynamic programming/i.test(direction)));
    assert.equal("optimal" in metadata, false);
    assert.equal("pitfalls" in metadata, false);
    assert.equal("hintLadder" in metadata, false);
  }
});
