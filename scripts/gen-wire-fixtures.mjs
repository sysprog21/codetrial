#!/usr/bin/env node
// Generates the cross-implementation wire fixtures in tests/fixtures/.
//
// Every data-channel topic has two implementations: a producer in web/lib.js
// and a consumer in src/agent.rs. Each was tested only against itself, which is
// exactly the shape that let the integrity hash diverge and silently empty the
// evidence section of every camera interview while the gate stayed green. A
// fixture is the only thing that feeds one side's real output to the other.
//
// Fixtures are generated, not hand-written, so they cannot drift from the
// producer; and `--check` runs in scripts/test.sh, so a producer change that
// leaves them stale fails the gate rather than waiting for someone to notice.
// Everything here is deterministic: timestamps are fixed values rather than
// clock reads, because a fixture that changes on every run cannot be checked.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURES = join(ROOT, "tests", "fixtures");

const lib = await import(join(ROOT, "web", "lib.js"));

// Read the offered languages out of the producer instead of restating them.
// The point of these fixtures is to catch the two sides disagreeing, and a
// hardcoded list here would be a third place to disagree with. A language the
// tabs offer but src/agent.rs does not recognize reaches no prompt and changes
// no state, so the candidate's click is silently ignored.
function offeredLanguages() {
  const source = readFileSync(join(ROOT, "web", "interview.js"), "utf8");
  const match = source.match(/^const languages = \[(.*?)\];$/m);
  if (!match) throw new Error("could not find the language list in web/interview.js");
  return match[1].split(",").map((part) => part.trim().replace(/^"|"$/g, ""));
}

const CODE = "def two_sum(nums, target):\n    return []\n";
const EDITED = "def two_sum(nums, target):\n    seen = {}\n    return []\n";

// Fixed rather than Date.now(). testPayload stamps itself from the clock, so
// the generator freezes it for the same reason it hardcodes the ISO strings
// below: a fixture nobody can regenerate identically cannot be checked.
const AT_MS = 1_770_000_000_000;

function codeUpdateCases(languages) {
  return [
    // Published on connect. It matches the agent default, so it must not be
    // heard as a language choice; confirming a pick nobody made is worse than
    // staying quiet.
    { name: "opening sync", payload: lib.codeUpdatePayload(CODE, "python") },
    { name: "keystroke", payload: lib.codeUpdatePayload(EDITED, "python", AT_MS) },
    // Every tab the browser actually offers. If src/agent.rs stops recognizing
    // one, the click changes nothing and Jim never acknowledges it.
    ...languages.map((language) => ({
      name: `language tab ${language}`,
      payload: lib.codeUpdatePayload(CODE, language),
    })),
    // A cleared editor is a real thing a candidate does. It is not the same as
    // a packet with no code at all, which must leave the buffer alone.
    { name: "cleared editor", payload: lib.codeUpdatePayload("", "python") },
  ];
}

function controlCases() {
  return [
    { name: "time warning five minutes", payload: lib.timeWarningPayload(300) },
    { name: "time warning one minute", payload: lib.timeWarningPayload(60) },
    {
      name: "end interview",
      payload: lib.endInterviewPayload("candidate_ended", EDITED, "cpp"),
    },
    {
      name: "end interview out of time",
      payload: lib.endInterviewPayload("time_expired", EDITED, "python"),
    },
  ];
}

function testResultsCases() {
  const realNow = Date.now;
  Date.now = () => AT_MS;
  try {
    return [
      {
        name: "all passed",
        payload: lib.testPayload({
          passed: 5,
          total: 5,
          language: "python",
          setupError: null,
          cases: [
            { label: "example 1", pass: true, expected: "[0,1]", got: "[0,1]", timeMs: 3 },
            { label: "example 2", pass: true, expected: "[1,2]", got: "[1,2]", timeMs: 2 },
          ],
        }),
      },
      {
        name: "some failed",
        payload: lib.testPayload({
          passed: 3,
          total: 5,
          language: "python",
          setupError: null,
          cases: [
            { label: "example 1", pass: true, expected: "[0,1]", got: "[0,1]", timeMs: 3 },
            { label: "example 2", pass: false, expected: "[1,2]", got: "[]", timeMs: 2 },
            { label: "example 3", pass: false, expected: "[2,3]", got: "", error: "TypeError: nums is not iterable", timeMs: 1 },
          ],
        }),
      },
      {
        // Five failures against a cap of four, and five sourceEventIds of
        // thirteen digits against caps of four and twelve. A cap no fixture
        // input reaches is a cap `--check` cannot see change.
        name: "more failures than the payload carries",
        payload: lib.testPayload({
          passed: 0,
          total: 5,
          language: "python",
          setupError: null,
          cases: [1, 2, 3, 4, 5].map((index) => ({
            label: `example ${index}`,
            pass: false,
            expected: `[${index}]`,
            got: "[]",
            timeMs: index,
          })),
        }),
      },
      {
        name: "setup error",
        payload: lib.testPayload({
          passed: 0,
          total: 0,
          language: "cpp",
          setupError: "Compiler Explorer returned 503",
          cases: [],
        }),
      },
    ];
  } finally {
    Date.now = realNow;
  }
}

// The `at` values are fixed strings, not clock reads, so the hash chain is
// reproducible. Keep one detail at the 80-character bound: that is the case
// that broke, and a fixture without it cannot catch the regression.
const INTEGRITY_INPUTS = [
  { type: "SESSION_START", source: "media", severity: "info", at: "2026-08-18T06:36:26.727Z" },
  { type: "MEDIA_PREFLIGHT_PASSED", source: "preflight", severity: "info", at: "2026-08-18T06:36:26.734Z" },
  {
    type: "INTEGRITY_HEARTBEAT", source: "media", severity: "info", at: "2026-08-18T06:36:56.812Z",
    detail: "analyzer=source=camera;analysis=face_detect,tracking;frames=3;transport=ImageBitmap",
  },
  {
    type: "FACE_MISSING", source: "camera", severity: "warning", durationMs: 2000,
    at: "2026-08-18T06:37:14.005Z", detail: "faces=0",
  },
  {
    type: "CAMERA_RELEASED_TO_PRESENTER", source: "media", severity: "info",
    at: "2026-08-18T06:37:41.330Z", detail: "analyzer=camera_released",
  },
  {
    type: "INTEGRITY_HEARTBEAT", source: "media", severity: "info", at: "2026-08-18T06:38:26.901Z",
    detail: "analyzer=source=camera;analysis=tracking;frames=1;transport=ImageBitmap",
  },
  // A real detector failure, which is the one event whose detail is an
  // arbitrary runtime error string: `web/face-worker.js` fills it from
  // `error.message`. Parentheses and quotes are ordinary there, and an agent
  // that drops a detail carrying them hashes different bytes from the producer.
  {
    type: "FACE_DETECTOR_UNAVAILABLE", source: "media", severity: "warning",
    at: "2026-08-18T06:39:02.114Z",
    detail: "face detection unavailable: Cannot read properties of undefined (reading 'a')",
  },
  // The mirror case: characters both sides keep, so a browser that starts
  // stripping something the agent would have accepted fails here too. One
  // fixture cannot catch a divergence in a direction it never exercises.
  {
    type: "INTEGRITY_HEARTBEAT", source: "media", severity: "info",
    at: "2026-08-18T06:39:44.702Z",
    detail: "az AZ 09 _-=/;:,.",
  },
  // A camera label, which is the candidate's to name and is printed in the
  // interviewer's report. Localized text stays, because stripping it would leave
  // the evidence unreadable for the people most likely to need it. The
  // right-to-left override and the zero-width space do not: they reorder or hide
  // what is printed around them, and this fixture is what proves the browser and
  // the agent remove the identical set. If either side keeps one, the hashes
  // disagree here rather than in somebody's interview.
  {
    type: "MEDIA_PREFLIGHT_PASSED", source: "preflight", severity: "info",
    at: "2026-08-18T06:39:52.400Z",
    detail: `camera=FaceTime HD \u039A\u03AC\u03BC\u03B5\u03C1\u03B1 \u0645\u06CC\u200C\u0631 \u202Egnitautis\u200B (05AC:8514)`,
  },
  // The only input carrying sourceEventIds, so the conditional key in
  // `integrityEventPayload` (`if (event.sourceEventIds.length)`) and its mirror
  // in `sanitize_integrity_event` are exercised at all. A key that is present on
  // one side and absent on the other is a canonical-JSON shape difference, which
  // is the most divergence-prone construct in the whole hash. Five ids of
  // thirteen digits, against caps of four and twelve.
  {
    type: "MULTIPLE_FACES", source: "camera", severity: "critical", durationMs: 4000,
    at: "2026-08-18T06:40:11.006Z", detail: "faces=2",
    sourceEventIds: ["1111111111111", "2222222222222", "3333333333333", "4444444444444", "5555555555555"],
  },
];

async function integrityChain() {
  const events = [];
  let previous = { seq: 0, hash: "" };
  for (const input of INTEGRITY_INPUTS) {
    const event = await lib.integrityEventPayload(input, previous);
    events.push(event);
    previous = { seq: event.seq, hash: event.hash };
  }
  return events;
}

const languages = offeredLanguages();
const files = {
  "code-update.json": { topic: lib.topics.code, cases: codeUpdateCases(languages) },
  "control.json": { topic: lib.topics.control, cases: controlCases() },
  "test-results.json": { topic: lib.topics.tests, cases: testResultsCases() },
  "integrity-chain.json": await integrityChain(),
};

const check = process.argv.includes("--check");
let stale = [];

mkdirSync(FIXTURES, { recursive: true });
for (const [name, value] of Object.entries(files)) {
  const path = join(FIXTURES, name);
  const wanted = `${JSON.stringify(value, null, 2)}\n`;
  if (check) {
    let found = null;
    try {
      found = readFileSync(path, "utf8");
    } catch {
      found = null;
    }
    if (found !== wanted) stale.push(name);
  } else {
    writeFileSync(path, wanted);
  }
}

if (check && stale.length) {
  console.error(
    `wire fixtures are stale: ${stale.join(", ")}\n` +
      "A producer in web/lib.js changed and the fixture the Rust consumer is\n" +
      "tested against did not. Run scripts/gen-wire-fixtures.mjs and re-run\n" +
      "cargo test --test agent, which is where a real divergence will show.",
  );
  process.exit(1);
}
