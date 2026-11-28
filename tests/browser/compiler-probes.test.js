import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";

const sourceUrl = new URL("./source.js", import.meta.url).href;

// A fresh host keeps the availability cache independent for each environment.
// The subprocess stubs model installed-but-unusable launchers without a JDK.
function probe({ language = "java", replies = {}, python = false } = {}) {
  const script = `
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
const request = JSON.parse(process.argv[1]);
const calls = [];
childProcess.spawnSync = (command, args) => {
  calls.push({ command, args });
  return { status: 0, stdout: "", stderr: "", ...request.replies[command] };
};
syncBuiltinESMExports();
process.env.PYTHON = "probe-python";
const api = await import(${JSON.stringify(sourceUrl)});
const result = { skip: request.python ? api.pythonSkip() : api.compiledSkip(request.language) };
if (request.python && !result.skip) {
  try { api.runPythonDriver("", { cases: [] }); }
  catch (error) { result.error = { name: error.name, message: error.message }; }
}
result.calls = calls;
console.log(JSON.stringify(result));
`;
  return JSON.parse(
    execFileSync(
      process.execPath,
      [
        "--input-type=module",
        "-e",
        script,
        JSON.stringify({ language, replies, python }),
      ],
      {
        encoding: "utf8",
        timeout: 10000,
      },
    ),
  );
}

for (const [language, command] of [
  ["c", "cc"],
  ["cpp", "g++"],
  ["java", "javac"],
]) {
  test(`${language} skips an installed compiler whose version probe fails`, () => {
    const result = probe({
      language,
      replies: { [command]: { status: 1, stderr: "compiler unavailable" } },
    });
    assert.match(result.skip, /compiler unavailable/);
  });
}

test("Java skips a failing runtime even when javac works", () => {
  const result = probe({
    replies: { java: { status: 1, stderr: "No Java runtime present" } },
  });
  assert.match(result.skip, /No Java runtime present/);
  assert.deepEqual(result.calls, [
    { command: "javac", args: ["-version"] },
    { command: "java", args: ["-version"] },
  ]);
});

test("working compiler and runtime probes do not skip", () => {
  assert.equal(probe().skip, false);
});

test("a missing compiler remains a declared skip", () => {
  const result = probe({
    replies: {
      javac: { status: null, error: { code: "ENOENT", message: "not found" } },
    },
  });
  assert.match(result.skip, /not installed/);
});

test("a compiler that cannot start is also a declared skip", () => {
  const result = probe({
    language: "c",
    replies: {
      cc: {
        status: null,
        error: { code: "EACCES", message: "not executable" },
      },
    },
  });
  assert.match(result.skip, /not executable/);
});

test("Python keeps a failed interpreter probe as a failure", () => {
  const result = probe({
    python: true,
    replies: { "probe-python": { status: 1, stderr: "Python stub" } },
  });
  assert.equal(result.skip, false);
  assert.equal(result.error.name, "AssertionError");
  assert.match(result.error.message, /Python stub/);
  assert.deepEqual(result.calls, [
    { command: "probe-python", args: ["--version"] },
  ]);
});

test("a missing Python interpreter remains a declared skip", () => {
  const result = probe({
    python: true,
    replies: {
      "probe-python": {
        status: null,
        error: { code: "ENOENT", message: "not found" },
      },
    },
  });
  assert.match(result.skip, /not installed/);
  assert.equal(result.error, undefined);
});
