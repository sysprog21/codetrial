#!/usr/bin/env node
// Rejects the timing shapes that made browser tests fail on loaded CI runners
// while passing on a laptop, so a new one cannot land unannounced.
//
//   * A wall-clock bound built from `Date.now() +` and held in a variable named
//     like a deadline, limit, or expiry: it measures how busy the runner is,
//     not the page. Wait with `until` or `animationsFinished` from
//     tests/browser/wait.js, whose timeout is only a backstop that names what
//     hung.
//   * A fixed sleep, `waitForTimeout(...)` or `setTimeout(resolve, N)` with N of
//     50 ms or more, with no comment in the three lines above it or on its
//     line. A fixed sleep either proves an absence by outwaiting it (so it must
//     say what is being outwaited) or papers over a race (so it must not be
//     there).
//   * A wait that bounds a property the test has just set, such as
//     `scrollTop = 900` followed by a wait for `scrollTop > 0`. What follows
//     the value (a scroll listener, a layer painted behind it) catches up a
//     frame later, and the bound can already hold where it stood before, so
//     the wait returns at once on a runner where that frame came early and
//     holds on one where it did not. Wait for the value set, `=== 900`. An
//     exact comparison beside the bound does not tie it down:
//     `code.scrollTop === 900 && layer.scrollTop > 0` holds as soon as the
//     textarea is set, while the layer still sits where it was. A reset the
//     bound cannot meet, such as `count = 0` before a wait for `count >= 3`,
//     is deliberate and carries `timing-ok:`.
//
// A bound that is deliberate carries `timing-ok:` and a reason in a comment on
// its line or the line above. Matching runs over the code with comments and
// string contents blanked, so a call split over several lines is still seen and
// quoted text neither triggers a finding nor passes for a comment or a marker.
// The scanner does not know regular-expression literals, so one holding a quote
// or a comment marker can blank the code after it; none of the tests does.
// The bound rule reads a comparison written on the property itself, either
// way round (`x.scrollTop > 0`, `0 < x.scrollTop`): a predicate that reads the
// property into a local first is not seen, and neither is a wait that does
// not directly follow the assignment.
//
//   check-test-timing.mjs [DIR]    DIR defaults to tests/browser

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const dir = process.argv[2] ?? path.join(root, "tests", "browser");
const problems = [];

const bound =
  /\b\w*(?:deadline|limit|until|expir\w*|giveUp|stopAt)\w*\s*=\s*Date\.now\(\)\s*\+/gi;
const sleep =
  /\bwaitForTimeout\(|\bsetTimeout\(\s*(?:\w+|\(\s*\)\s*=>\s*\w+\(\s*\))\s*,\s*(?:[5-9]\d|\d{3,})\s*,?\s*\)/g;
// `.name =` but not `==`, `===` or `=>`; a compound `+=` has its operator
// before the `=` and is left alone.
const assignment = /\.(\w+)\s*=(?![=>])/g;
const waitCall = /\b(?:waitForFunction|reaches|until)\s*\(/g;
// What may stand between an assignment and the wait tied to it: the rest of
// the assigning statement, closing brackets, and the start of the wait's own
// statement (`await page.`). Any other step between them, awaited or not, or
// the end of one test callback and the start of the next, unties the two.
const tied = /^[^;]*;[\s)}\];,]*(?:await\s+)?(?:[\w$]+\s*\.\s*)*$/;
// A file without any of these cannot produce a finding.
const relevant =
  /Date\.now|waitForTimeout|setTimeout|waitForFunction|reaches|until/;

// The offset of the parenthesis closing the one at `open`.
function closing(code, open) {
  let depth = 0;
  for (let i = open; i < code.length; i++) {
    if (code[i] === "(") depth++;
    else if (code[i] === ")" && --depth === 0) return i;
  }
  return code.length;
}

// Whether `predicate` compares `.property` with anything but `===`, the
// property on either side. The `>` of an arrow function (`() => x.top`) is
// not a comparison, and neither are the shifts `>>` and `<<`.
function bounds(predicate, property) {
  const operator = "(===|!==|!=|>=|<=|(?<![=>])>(?!>)|(?<!<)<(?!<))";
  const chain = String.raw`[\w$]+(?:\s*(?:\.\s*[\w$]+|\([^()]*\)|\[[^\]]*\]))*?`;
  const written = [
    new RegExp(String.raw`\.${property}\b\s*${operator}`, "g"),
    new RegExp(String.raw`${operator}\s*${chain}\s*\.\s*${property}\b`, "g"),
  ];
  return written.some((pattern) =>
    [...predicate.matchAll(pattern)].some((compared) => compared[1] !== "==="),
  );
}

// The waits that bound a property the statement before them set, each once
// with the property it bounds: a chained `a.x = b.x = 1` ties both
// assignments to one wait.
function boundWaits(code) {
  const found = new Map();
  for (const set of code.matchAll(assignment)) {
    const after = set.index + set[0].length;
    waitCall.lastIndex = after;
    const call = waitCall.exec(code);
    if (!call) break;
    if (!tied.test(code.slice(after, call.index))) continue;
    const open = call.index + call[0].length - 1;
    const predicate = code.slice(open, closing(code, open) + 1);
    if (bounds(predicate, set[1]) && !found.has(call.index))
      found.set(call.index, set[1]);
  }
  return [...found].map(([offset, property]) => ({ offset, property }));
}

// One global pattern for every comment and string token, so plain code is
// skipped by the regex engine instead of examined a character at a time.
const token =
  /\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`/g;

// Returns `code`, the source with comments and string contents replaced by
// spaces (newlines kept, so offsets and lines still line up), and the sets of
// lines that carry a comment and that carry a `timing-ok:` marker.
function scan(source) {
  const commented = new Set();
  const marked = new Set();
  const blank = (text) => text.replace(/[^\n]/g, " ");
  let code = "";
  let last = 0;
  let line = 0;
  let counted = 0;
  for (const found of source.matchAll(token)) {
    const [text] = found;
    line += source.slice(counted, found.index).split("\n").length - 1;
    counted = found.index;
    code += source.slice(last, found.index);
    if (text[0] === "/") {
      const span = text.split("\n").length;
      for (let k = 0; k < span; k++) {
        commented.add(line + k);
        if (text.includes("timing-ok:")) marked.add(line + k);
      }
      code += blank(text);
    } else {
      code += text[0] + blank(text.slice(1, -1)) + text.at(-1);
    }
    last = found.index + text.length;
  }
  return { code: code + source.slice(last), commented, marked };
}

for (const name of fs.readdirSync(dir).sort()) {
  // wait.js is where the one sanctioned deadline lives.
  if (!name.endsWith(".js") || name === "wait.js") continue;
  const source = fs.readFileSync(path.join(dir, name), "utf8");
  if (!relevant.test(source)) continue;
  const { code, commented, marked } = scan(source);
  const lineOf = (offset) => code.slice(0, offset).split("\n").length - 1;
  const within = (lines, line, before) => {
    for (let k = Math.max(0, line - before); k <= line; k++) {
      if (lines.has(k)) return true;
    }
    return false;
  };
  for (const [pattern, report, needsComment] of [
    [bound, "wall-clock bound; use until() from wait.js", false],
    [sleep, "fixed sleep without a comment saying what it outwaits", true],
  ]) {
    for (const found of code.matchAll(pattern)) {
      const line = lineOf(found.index);
      if (within(marked, line, 1)) continue;
      if (needsComment && within(commented, line, 3)) continue;
      problems.push(`${name}:${line + 1}: ${report}`);
    }
  }
  for (const { offset, property } of boundWaits(code)) {
    const line = lineOf(offset);
    if (within(marked, line, 1)) continue;
    problems.push(
      `${name}:${line + 1}: wait bounds .${property}, which the test just set; wait for the value set`,
    );
  }
}

if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
