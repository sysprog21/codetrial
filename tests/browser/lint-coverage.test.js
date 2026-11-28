// Every tracked JavaScript file ESLint reads gets the correctness rules.
//
// eslint.config.mjs gives the correctness rules in a block with no `files`
// key, so a file no other block names still gets them. Before it did, such a
// file was linted with no rule at all and `eslint .` passed it whatever it
// held: the desktop start page, the C and C++ harness audit and a test stub
// went unchecked that way. This holds the config to that shape by asking
// ESLint's own resolution, file by file, rather than reading it by eye.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

test("no linted JavaScript file falls outside the correctness rules", async (t) => {
  // ESLint is optional where the gate runs it, so it is optional here too.
  let ESLint;
  try {
    ({ ESLint } = await import("eslint"));
  } catch {
    t.skip("eslint is not installed: `npm install` enables it");
    return;
  }
  const eslint = new ESLint({ cwd: root });
  const tracked = execFileSync("git", ["ls-files", "*.js", "*.mjs", "*.cjs"], {
    cwd: root,
    encoding: "utf8",
  })
    .split("\n")
    .filter(Boolean);
  const bare = [];
  let linted = 0;
  for (const file of tracked) {
    if (await eslint.isPathIgnored(file)) continue;
    linted += 1;
    const { rules = {} } = await eslint.calculateConfigForFile(file);
    const level = Array.isArray(rules["no-undef"])
      ? rules["no-undef"][0]
      : rules["no-undef"];
    if (level !== 2 && level !== "error") bare.push(file);
  }
  // An ignore that swallowed the tree would leave nothing to hold to the
  // rules; web/ and tests/browser/ alone hold well over two hundred files.
  assert.ok(linted > 200, `ESLint reads only ${linted} tracked files`);
  assert.deepEqual(bare, []);
});
