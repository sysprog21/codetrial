#!/usr/bin/env node
// Prints the C or C++ program the page builds for a judge, so the helpers that
// web/compiler-explorer.js keeps inside a template string can be read, compiled
// and run on their own. With no judge it prints the shared prelude, the text
// before the candidate.
//
//   node scripts/dump-harness.mjs [--cpp] [judge-id] > harness.c
//   node scripts/dump-harness.mjs --cpp | clang-format --style=LLVM
//
// The candidate is a stub that returns zero, so the output compiles as it is:
// with -std=c17 for C, and -std=c++20 for C++. Class judges are C++-only and
// have no stub here.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { compilerType, generateHarness } from "../web/compiler-explorer.js";

const webFile = (path) => new URL(`../web/${path}`, import.meta.url);
const MARKER = "CODETRIAL_CANDIDATE_CODE";

/// A candidate that compiles against any signature the harness calls, so this
/// file does not mirror how the harness builds its arguments. In C an empty
/// parameter list is an unprototyped definition, which C23 reads as `(void)`,
/// so compile the result with -std=c17. In C++ the method takes any arguments.
export function stubCandidate(spec, language = "c") {
  const type = Number.isInteger(spec.outputPrefixParam)
    ? "int"
    : compilerType(spec.returnType, language);
  if (language === "cpp") {
    const body = type === "void" ? "{}" : "{ return {}; }";
    return `class Solution { public: template <class... A> ${type} ${spec.entry}(A&&...) ${body} };`;
  }
  if (type === "void") return `void ${spec.entry}() {}`;
  return `${type} ${spec.entry}() { static ${type} zero; return zero; }`;
}

let judges;

/// Every judge spec by id, read once.
export function loadJudges() {
  if (!judges) {
    const pages = JSON.parse(
      readFileSync(webFile("problem-pages.json"), "utf8"),
    );
    judges = Object.fromEntries(
      Object.entries(pages).map(([id, { page }]) => [
        id,
        JSON.parse(readFileSync(webFile(`judges/${page}.json`), "utf8")),
      ]),
    );
  }
  return judges;
}

const shared = {};

/// The helpers every harness carries: everything before the candidate.
export function prelude(language = "c") {
  if (!shared[language]) {
    const spec = Object.values(loadJudges()).find((s) => s.kind === "function");
    const source = generateHarness(language, spec, MARKER);
    shared[language] = source.slice(0, source.indexOf(MARKER));
  }
  return shared[language];
}

function main() {
  const args = process.argv.slice(2);
  const language = args.includes("--cpp") ? "cpp" : "c";
  const id = args.find((arg) => !arg.startsWith("--"));
  const spec = id ? loadJudges()[id] : null;
  if (id && !spec) throw new Error(`No judge named ${id}`);
  if (spec && spec.kind !== "function")
    throw new Error(`${id} is a class judge; only function judges have a stub`);
  let text = spec
    ? generateHarness(language, spec, stubCandidate(spec, language))
    : prelude(language);
  try {
    text = execFileSync("clang-format", ["--style=LLVM"], {
      input: text,
      encoding: "utf8",
      maxBuffer: 1 << 26,
    });
  } catch (error) {
    // Unformatted text is still the program, but a formatter that ran and
    // failed is worth a word, where one that is not installed is not.
    if (error.code !== "ENOENT")
      process.stderr.write(`clang-format failed: ${error.message}\n`);
  }
  process.stdout.write(text);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  main();
