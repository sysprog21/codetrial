// Run with: node --test tests/browser/*.test.js
//
// Validates the view layer without a browser. The markup builders are pure
// strings; the transcript view gets a stub document, which is enough to check
// row reuse and escaping. A real browser is only needed for layout and for
// running candidate code, not for this.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  createTranscriptView,
  feedbackMarkup,
  finalRunnerStatus,
  chainSentence,
  problemMarkup,
  reportMarkdown,
  reportMarkup,
  resultsMarkup,
  runnerStatusMarkup,
} from "../../web/render.js";
import { sanitizeReport } from "../../web/lib.js";

const events = [{ type: "CAMERA_STOPPED", at: "00:01", severity: "high", source: "camera", detail: null, sourceEventIds: [] }];

// --- smallest document that satisfies the view -----------------------------
class StubElement {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.className = "";
    this.text = "";
  }
  set textContent(value) {
    this.text = String(value);
    this.children = [];
  }
  get textContent() {
    return this.text;
  }
  append(...kids) {
    this.children.push(...kids);
  }
  replaceChildren(...kids) {
    this.children = kids;
  }
}

const stubDocument = { createElement: (tag) => new StubElement(tag) };
const newPanel = () => {
  const panel = new StubElement("section");
  panel.append(new StubElement("div")); // the empty-state placeholder
  return panel;
};

const rowLabel = (panel, i) => panel.children[i].children[0].children[0].textContent;
const rowText = (panel, i) => panel.children[i].children[0].children[1].textContent;

test("first transcript row replaces the empty-state placeholder", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);
  assert.equal(panel.children.length, 1, "placeholder present up front");

  view.upsert("a", "interviewer", "Hello", false);

  assert.equal(panel.children.length, 1, "placeholder replaced, not appended to");
  assert.equal(rowLabel(panel, 0), "Jim");
  assert.equal(rowText(panel, 0), "Hello...", "interim speech gets the ellipsis");
});

test("streamed chunks patch one row instead of appending", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);

  for (const chunk of ["Walk", "Walk me", "Walk me through it"]) {
    view.upsert("a", "interviewer", chunk, false);
  }

  assert.equal(panel.children.length, 1, "a chunk must not create a row");
  assert.equal(view.size, 1);
  assert.equal(rowText(panel, 0), "Walk me through it...");

  view.upsert("a", "interviewer", "Walk me through it", true);
  assert.equal(rowText(panel, 0), "Walk me through it", "final chunk drops the ellipsis");
});

test("one segment id is one turn, patched in place as the agent republishes it", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);

  // The agent accumulates fragments and republishes the whole turn under a
  // stable id, so the row grows rather than the panel.
  view.upsert("interviewer-0", "interviewer", "Hey, I'm", false, 1_000);
  view.upsert("interviewer-0", "interviewer", "Hey, I'm Jim.", false, 1_500);
  view.upsert("interviewer-0", "interviewer", "Hey, I'm Jim. Today you're looking at Two Sum,", true, 2_000);

  assert.equal(panel.children.length, 1);
  assert.equal(rowText(panel, 0), "Hey, I'm Jim. Today you're looking at Two Sum,");
  assert.deepEqual(
    view.values().map((segment) => `${segment.speaker}:${segment.text}`),
    ["interviewer:Hey, I'm Jim. Today you're looking at Two Sum,"],
  );
});

test("an unfinished turn renders as in progress until the agent closes it", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);

  view.upsert("candidate-0", "you", "I will use a map", false, 1_000);
  assert.equal(rowText(panel, 0), "I will use a map...");

  view.upsert("candidate-0", "you", "I will use a map", true, 1_100);
  assert.equal(rowText(panel, 0), "I will use a map", "closing the turn drops the ellipsis");
});

test("a new turn id opens a new row", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);

  view.upsert("interviewer-0", "interviewer", "Ready?", true, 1_000);
  view.upsert("candidate-0", "you", "Yes.", true, 1_500);
  view.upsert("candidate-1", "you", "I will use a map.", true, 20_000);

  assert.equal(panel.children.length, 3);
  assert.equal(rowText(panel, 0), "Ready?");
  assert.equal(rowText(panel, 1), "Yes.");
  assert.equal(rowText(panel, 2), "I will use a map.");
});

test("a stale republish does not shorten a turn or reopen it", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);

  view.upsert("interviewer-0", "interviewer", "Today you're looking at Two Sum", true, 1_300);
  view.upsert("interviewer-0", "interviewer", "Today", false, 1_200);

  assert.equal(rowText(panel, 0), "Today you're looking at Two Sum");
});

test("a second speaker appends without disturbing the first", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);
  view.upsert("a", "interviewer", "Ready?", true);
  view.upsert("b", "you", "Sure", true);

  assert.equal(panel.children.length, 2);
  assert.equal(rowLabel(panel, 1), "You");
  assert.equal(rowText(panel, 0), "Ready?", "first row survives");

  view.upsert("a", "interviewer", "Edited", true);
  assert.equal(rowText(panel, 0), "Edited", "interleaved updates hit the right row");
  assert.equal(rowText(panel, 1), "Sure");
  assert.deepEqual(
    view.values().map((segment) => `${segment.speaker}:${segment.text}`),
    ["interviewer:Edited", "you:Sure"],
    "insertion order is preserved for the markdown export",
  );
});

test("remote transcript text cannot inject markup", () => {
  const panel = newPanel();
  const view = createTranscriptView(stubDocument, panel);
  view.upsert("a", "interviewer", "<img src=x onerror=alert(1)>", true);

  assert.equal(rowText(panel, 0), "<img src=x onerror=alert(1)>", "kept as text");
  assert.equal(
    panel.children[0].children[0].children[1].children.length,
    0,
    "textContent must not parse child elements",
  );
});

test("results markup reports counts and marks each case", () => {
  const { label, body } = resultsMarkup({
    passed: 1,
    total: 2,
    cases: [
      { label: "ok case", pass: true, timeMs: 3 },
      { label: "bad case", pass: false, expected: "[0,1]", got: "[1,0]", error: "", timeMs: 4 },
    ],
  });

  assert.equal(label, "Test results · 1/2");
  assert.match(body, /1\/2 test cases passed/);
  assert.match(body, /class="critical small"/, "a partial pass is not styled as good");
  assert.match(body, />OK</);
  assert.match(body, />FAIL</);
  assert.match(body, /expected \[0,1\]\ngot \[1,0\]/, "expected and got sit on separate lines");
});

test("results markup can include runner status without changing counts", () => {
  const { label, body } = resultsMarkup({
    passed: 1,
    total: 1,
    cases: [{ label: "ok case", pass: true, timeMs: 3 }],
  }, "done");

  assert.equal(label, "Test results · 1/1");
  assert.match(body, /data-runner-status="done"/);
  assert.match(body, /Run finished/);
  assert.match(body, /1\/1 test cases passed/);
});

test("results markup surfaces a setup error instead of case rows", () => {
  const { label, body } = resultsMarkup({ setupError: "SyntaxError: bad <input>" });

  assert.equal(label, "Test results");
  assert.match(body, /Couldn't run your code/);
  assert.match(body, /SyntaxError: bad &lt;input&gt;/, "the error is escaped");
  assert.doesNotMatch(body, /result-list/);
});

test("results markup keeps first-run timeout errors visible", () => {
  const pyodide = resultsMarkup({ setupError: "The Python runtime did not start in time." }, "booting");
  const compiler = resultsMarkup({ setupError: "Compiler Explorer did not respond within 20 seconds. Try again later." }, "compiling");

  assert.match(pyodide.body, /Starting the Python runtime/);
  assert.match(pyodide.body, /The Python runtime did not start in time/);
  assert.match(compiler.body, /Compiling with Compiler Explorer/);
  assert.match(compiler.body, /Compiler Explorer did not respond within 20 seconds/);
});

test("runner status markup names booting compiling running and done", () => {
  assert.match(runnerStatusMarkup("booting"), /Starting the Python runtime/);
  assert.match(runnerStatusMarkup("compiling"), /Compiling with Compiler Explorer/);
  assert.match(runnerStatusMarkup("running"), /Running test cases/);
  assert.match(runnerStatusMarkup("done"), /Run finished/);
});

test("final runner status preserves setup phase for setup errors", () => {
  assert.equal(finalRunnerStatus({ setupError: "The Python runtime did not start in time." }, "booting"), "booting");
  assert.equal(finalRunnerStatus({ setupError: "Compiler Explorer did not respond within 20 seconds." }, "compiling"), "compiling");
  assert.equal(finalRunnerStatus({ passed: 1, total: 1, cases: [] }, "running"), "done");
});

test("results markup escapes everything a candidate's code can produce", () => {
  const { body } = resultsMarkup({
    passed: 0,
    total: 1,
    cases: [
      {
        label: "<script>l</script>",
        pass: false,
        expected: "<script>e</script>",
        got: "<script>g</script>",
        error: "",
        timeMs: 1,
      },
    ],
  });

  assert.doesNotMatch(body, /<script>/, "candidate output must not reach the page as markup");
  assert.match(body, /&lt;script&gt;/);
});

// A candidate who never spoke, whose interviewer died after the greeting, was
// shown "NO HIRE, Coding 40/100, Interviewer communication 70/100". Every one of
// those numbers was invented by the offline fallback: 40 is the literal default
// when no tests ran, and 70 was awarded because the transcript was non-empty,
// which it was because Jim had said hello. A fabricated rejection is the worst
// output a hiring tool can produce, so an empty session now renders no verdict
// and no scores at all.
test("report markup refuses to score a session with nothing in it", () => {
  const body = reportMarkup({
    report: { incomplete: true, summary: "The interviewer disconnected and this session produced no evaluation.", hintsUsed: 0 },
    problemTitle: "Two Sum",
    language: "python",
    code: "def two_sum(): pass",
  });

  assert.match(body, /No evaluation/);
  assert.match(body, /INCOMPLETE/);
  assert.match(body, /produced no evaluation/);
  // No verdict, in either direction, and no invented numbers.
  assert.doesNotMatch(body, /NO HIRE|>HIRE</);
  assert.doesNotMatch(body, /\/ 100/);
  assert.doesNotMatch(body, /\b40\b|\b70\b|\b45\b/);
  // The candidate still gets their code and a way out.
  assert.match(body, /id="download-report"/);
  assert.match(body, /id="done"/);
  assert.match(body, /def two_sum/);
});

test("markdown export of an empty session carries no verdict either", () => {
  const markdown = reportMarkdown({
    report: { incomplete: true, summary: "No interviewer joined and this session produced no evaluation.", hintsUsed: 0 },
    problemTitle: "Two Sum",
    language: "python",
    code: "",
    transcript: [],
    at: "2026-08-17",
  });

  assert.match(markdown, /## No evaluation/);
  // This file outlives the tab and can be forwarded to someone else, so a
  // verdict here is worse than one on screen.
  assert.doesNotMatch(markdown, /Verdict/);
  assert.doesNotMatch(markdown, /HIRE/);
  assert.doesNotMatch(markdown, /\/ 100/);
  assert.match(markdown, /\(editor was empty\)/);
  // One placeholder, shared with the scored export. The incomplete branch used
  // to spell this "(no conversation recorded)" purely because it was a copy.
  assert.match(markdown, /\(no speech captured\)/);
});

test("report markup renders scores, verdict, and escaped feedback", () => {
  const body = reportMarkup({
    report: {
      codingScore: 82,
      communicationScore: 74,
      decision: "HIRE",
      summary: "Solid <session>",
      codingFeedback: { strengths: ["clear"], improvements: [] },
      communicationFeedback: { strengths: [], improvements: ["slow down"] },
      integrityEvents: [{ type: "REVIEW_EVENT", at: "now", severity: "warning", detail: "SCREEN_INTERRUPTION_WITH_FACE_MISSING", sourceEventIds: ["2", "5"] }],
      hintsUsed: 2,
      interviewLoop: "coding_behavioral",
      rounds: [
        { kind: "coding", budgetMin: 37, status: "complete" },
        { kind: "behavioral", budgetMin: 8, status: "started" },
      ],
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n\n",
  });

  assert.match(body, /class="good">HIRE</);
  assert.match(body, /Coding \+ behavioral/);
  assert.match(body, /coding · 37 min · complete/);
  assert.match(body, /behavioral · 8 min · started/);
  assert.match(body, />82<span> \/ 100<\/span>/);
  assert.match(body, /<strong>2<\/strong> hints used/);
  assert.match(body, /Solid &lt;session&gt;/, "summary is escaped");
  assert.match(body, /Integrity Evidence/);
  assert.match(body, /data-integrity-index="0"/, "snapshot actions target integrity rows only");
  assert.match(body, /SCREEN_INTERRUPTION_WITH_FACE_MISSING/);
  assert.match(body, /source event 2/);
  assert.match(body, /<li>\(none captured\)<\/li>/, "empty feedback gets the placeholder");
  assert.match(body, /<pre>print\(1\)<\/pre>/, "trailing blank lines are trimmed");
  assert.match(body, /id="download-report"/);
  assert.match(body, /id="done"/);
});

test("practice-next drills render accessibly in HTML and Markdown", () => {
  const report = {
    codingScore: 70, communicationScore: 70, decision: "HIRE", summary: "Grounded",
    codingFeedback: { strengths: [], improvements: ["Test boundaries"] },
    communicationFeedback: { strengths: [], improvements: [] }, hintsUsed: 0,
    improvementPlan: [{ phase: "Test", weakness: "Test boundaries", impact: "high", frequency: 2, drill: "Build a test table", durationMin: 10, successCriterion: "Cover four case classes", selfReview: ["Predicted outputs", "Included a boundary"] }],
  };
  const session = { report, problemTitle: "Two Sum", language: "python", code: "pass", transcript: [], at: "now" };
  const html = reportMarkup(session);
  const markdown = reportMarkdown(session);
  assert.match(html, /<h3>Practice next<\/h3>/);
  assert.match(html, /Test · 10 min · high impact/);
  assert.match(markdown, /## Practice next/);
  assert.match(markdown, /Success: Cover four case classes/);
});

test("framework phase scores are labeled formative in HTML and Markdown", () => {
  const report = {
    codingScore: 70, communicationScore: 70, decision: "HIRE", summary: "Grounded",
    codingFeedback: { strengths: [], improvements: [] },
    communicationFeedback: { strengths: [], improvements: [] }, hintsUsed: 0,
    frameworkAssessment: { rubricVersion: 1, phases: [] },
  };
  const session = { report, problemTitle: "Two Sum", language: "python", code: "pass", transcript: [], at: "now" };
  for (const output of [reportMarkup(session), reportMarkdown(session)]) {
    assert.match(output, /formative coaching signals, not calibrated hiring evidence/);
  }
});

test("framework evidence timeline distinguishes observed inferred and skipped rows", () => {
  const report = {
    codingScore: 70, communicationScore: 70, decision: "HIRE", summary: "Grounded",
    codingFeedback: { strengths: [], improvements: [] },
    communicationFeedback: { strengths: [], improvements: [] }, hintsUsed: 0,
    frameworkEvidence: [
      { atMs: 65000, phase: "algorithm", source: "candidate_speech", kind: "observed", confidence: 95, summary: "Explained an invariant", frameworkVersion: 1, futurePrivateField: "must-not-render" },
      { atMs: 70000, phase: "test", source: "test_event", kind: "inferred", confidence: 60, summary: "A test suggests coverage", frameworkVersion: 1 },
      { atMs: 300000, phase: "result", source: "session_timing", kind: "skipped", confidence: 100, summary: "Cutoff prevented assessment", frameworkVersion: 1 },
    ],
  };
  const session = { report, problemTitle: "Two Sum", language: "python", code: "pass", transcript: [], at: "now" };
  const html = reportMarkup(session);
  const markdown = reportMarkdown(session);
  assert.match(html, /id="framework-evidence-title"/);
  assert.match(html, /01:05 · observed · candidate_speech · 95% confidence · v1/);
  for (const kind of ["observed", "inferred", "skipped"]) {
    assert.match(html, new RegExp(kind));
    assert.match(markdown, new RegExp(kind));
  }
  assert.match(markdown, /## Framework evidence/);
  assert.doesNotMatch(html, /must-not-render/);
  assert.doesNotMatch(markdown, /must-not-render/);
});

test("report markup marks a no-hire and an empty editor", () => {
  const body = reportMarkup({
    report: {
      codingScore: 0,
      communicationScore: 0,
      decision: "NO_HIRE",
      summary: "",
      codingFeedback: { strengths: [], improvements: [] },
      communicationFeedback: { strengths: [], improvements: [] },
      hintsUsed: 0,
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "   ",
  });

  assert.match(body, /class="critical">NO HIRE</);
  assert.match(body, /\(editor was empty\)/);
});

test("problem markup escapes every field a problem carries", () => {
  // Not all problem text is written in this repository: `leetcode-import.js`
  // brings statements, examples and constraints in from outside, and all three
  // land in the same panel.
  const body = problemMarkup({
    statement: ["<script>s</script>"],
    examples: [
      {
        input: "<script>i</script>",
        output: "<script>o</script>",
        explanation: "<script>x</script>",
      },
    ],
    constraints: ["<script>c</script>"],
  });

  assert.doesNotMatch(body, /<script>/, "no problem field may become live markup");
  for (const marker of ["s", "i", "o", "x", "c"]) {
    assert.match(body, new RegExp(`&lt;script&gt;${marker}&lt;/script&gt;`));
  }
});

test("problem markup omits the explanation line rather than printing undefined", () => {
  const body = problemMarkup({
    statement: ["one"],
    examples: [{ input: "a", output: "b" }],
    constraints: ["c"],
  });

  assert.doesNotMatch(body, /Explanation/);
  assert.doesNotMatch(body, /undefined/);
});

test("feedback markup escapes list items", () => {
  const body = feedbackMarkup("Coding", {
    strengths: ["<b>bold</b>"],
    improvements: ["fine"],
  });

  assert.match(body, /&lt;b&gt;bold&lt;\/b&gt;/);
  assert.doesNotMatch(body, /<b>/);
});

test("markdown export carries the whole session", () => {
  const markdown = reportMarkdown({
    report: {
      codingScore: 82,
      communicationScore: 74,
      decision: "HIRE",
      summary: "Solid.",
      codingFeedback: { strengths: ["clear"], improvements: [] },
      communicationFeedback: { strengths: [], improvements: ["slow down"] },
      integrityEvents: [{ type: "REVIEW_EVENT", at: "2026-08-15", severity: "info", detail: "<ok>|fine", sourceEventIds: ["1", "2"] }],
      hintsUsed: 2,
      interviewLoop: "coding_only",
      rounds: [
        { kind: "coding", budgetMin: 45, status: "complete" },
        { kind: "behavioral", budgetMin: 0, status: "not_configured" },
      ],
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n",
    transcript: [
      { speaker: "interviewer", text: " Ready? ", final: true },
      { speaker: "you", text: "Yes", final: true },
      { speaker: "you", text: "   ", final: false },
    ],
    at: "2026-01-01",
  });

  assert.match(markdown, /^# Interview Report - Two Sum$/m);
  assert.match(markdown, /^Loop: Coding only$/m);
  assert.match(markdown, /^Rounds: coding \(45 min, complete\); behavioral \(0 min, not_configured\)$/m);
  assert.match(markdown, /^## Verdict: HIRE$/m);
  assert.match(markdown, /^\| Coding \| 82 \/ 100 \|$/m);
  assert.match(markdown, /^\| Hints used \| 2 \|$/m);
  assert.match(markdown, /^## Integrity Evidence$/m);
  // Markdown escapes, not HTML entities: `\<` and `\|` render back as the
  // characters the candidate typed, where `&lt;` renders as literal "&lt;" in a
  // file nothing parses as HTML. The `<` is escaped rather than left alone
  // because a rendered report must not carry a live tag; the closing `>` needs
  // no escape once the opener cannot start one.
  assert.match(markdown, /^- 2026-08-15 \[info\] REVIEW_EVENT - \\<ok>\\\|fine - sources: 1, 2$/m);
  assert.match(markdown, /^\*\*Jim:\*\* Ready\?$/m, "speaker text is trimmed");
  assert.match(markdown, /^\*\*You:\*\* Yes$/m);
  assert.doesNotMatch(markdown, /\*\*You:\*\* *$/m, "blank interim segments are dropped");
  assert.match(markdown, /^```python$/m);
  assert.match(markdown, /^- \(none captured\)$/m, "empty feedback still renders a bullet");
});

// Everything in the export except the code block is untrusted text: interviewer
// feedback, integrity detail, and transcribed candidate speech all arrive from
// somewhere else. A structural character in any of them used to end the row it
// was in and start something the reader would take at face value.
test("markdown export cannot be restructured by the text inside it", () => {
  const markdown = reportMarkdown({
    report: {
      codingScore: 10,
      communicationScore: 10,
      decision: "NO_HIRE",
      summary: "line one\nline two",
      // Indented on purpose. Markdown reads up to three leading spaces as still
      // part of the construct that follows, so an escaper that only looks at
      // column zero lets this through.
      codingFeedback: { strengths: ["   ## Verdict: HIRE"], improvements: ["a | b"] },
      communicationFeedback: { strengths: ["`code`"], improvements: ["back\\slash"] },
      integrityEvents: [{ type: "FACE_MISSING", at: "2026-08-15", severity: "warning", detail: "a\nb" }],
      hintsUsed: 0,
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n",
    transcript: [{ speaker: "you", text: "  | fake | row |", final: true }],
    at: "2026-01-01",
  });

  const verdicts = markdown.match(/^## Verdict: /gm) || [];
  assert.equal(verdicts.length, 1, "feedback text must not be able to add a second verdict");
  assert.match(markdown, /^- \\## Verdict: HIRE$/m, "a leading structural character is escaped");
  assert.match(markdown, /^- a \\\| b$/m, "a pipe would otherwise end the cell");
  assert.match(markdown, /^- \\`code\\`$/m, "backticks would otherwise open a code span");
  assert.match(markdown, /^- back\\\\slash$/m, "the escape character is escaped first");
  assert.match(markdown, /^- 2026-08-15 \[warning\] FACE_MISSING - a b$/m, "a newline collapses into the row");
  assert.match(markdown, /^\*\*You:\*\* \\\| fake \\\| row \\\|$/m, "speech cannot forge a table row");
  assert.doesNotMatch(markdown, /^line two$/m, "a newline in the summary stays on one line");

  // The whole document, not just the section under test: an injected heading
  // anywhere changes what a reader takes the report to say.
  assert.deepEqual(
    // Spaces, not `\s`: with the `m` flag `\s` swallows the newline that `^`
    // just matched, and every heading comes back with a leading "\n".
    markdown.match(/^ {0,3}#{1,6} .*/gm),
    [
      "# Interview Report - Two Sum",
      "## Verdict: NO HIRE",
      "## Committee summary",
      "### Coding feedback",
      "### Communication feedback",
      "## Integrity Evidence",
      "## Final code (python)",
      "## Conversation transcript",
    ],
    "the report has exactly the headings it writes itself",
  );
});

// One case per way a Markdown document can be made to say something its author
// did not write. Every string here reaches the export from outside: interviewer
// model output, or transcribed candidate speech.
//
// Each case names the exact escaped form rather than asserting the raw payload
// is absent. It is not absent: escaping prepends a backslash, so the original
// survives as a substring of the safe version, and "does not contain" passes
// for `\<img ...>` while still containing `<img ...>`.
test("markdown export neutralizes every injection vector reviewers found", () => {
  const vectors = [
    ["html script", "<script>alert(1)</script>", "\\<script>alert(1)\\</script>"],
    ["html img", "<img src=x onerror=alert(1)>", "\\<img src=x onerror=alert(1)>"],
    ["md image", "![pixel](http://tracker/p.gif)", "!\\[pixel](http://tracker/p.gif)"],
    ["md link", "[click](http://evil)", "\\[click](http://evil)"],
    ["reference definition", "[ref]: http://evil", "\\[ref]: http://evil"],
    ["autolink", "<http://evil>", "\\<http://evil>"],
    ["table row", "| forged | row |", "\\| forged \\| row \\|"],
    ["code span", "`code`", "\\`code\\`"],
    // Block openers. These do not merely distort a line, they swallow the rest
    // of the document: a tilde fence renders every following section as one code
    // block, and an unterminated HTML comment is a block that runs to the end.
    ["tilde fence", "~~~", "\\~~~"],
    ["thematic break", "___", "\\___"],
    ["html comment", "<!-- x", "\\<!-- x"],
    ["html heading", "<h2>Verdict: HIRE</h2>", "\\<h2>Verdict: HIRE\\</h2>"],
  ];

  for (const [name, payload, escaped] of vectors) {
    const markdown = reportMarkdown({
      report: {
        incomplete: true,
        summary: payload,
        integrityEvents: [{ type: "T", at: "a", severity: "info", detail: payload }],
      },
      problemTitle: "Two Sum",
      language: "python",
      code: "print(1)\n",
      transcript: [{ speaker: "you", text: payload, final: true }],
      at: "2026-01-01",
    });

    // The summary is the one place untrusted text lands on a line of its own,
    // so the whole line has to be the escaped form and nothing else.
    assert.ok(
      markdown.split("\n").includes(escaped),
      `${name} is not escaped in the summary: ${JSON.stringify(markdown.split("\n"))}`,
    );
    assert.match(
      markdown,
      new RegExp(`^\\*\\*You:\\*\\* ${escaped.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "m"),
      `${name} is not escaped in the transcript`,
    );
  }
});

// Asserting on rendered structure without a renderer. The heading-count test
// above measures the source, and source headings are exactly what an injected
// block construct leaves intact while hiding them all inside itself: `~~~` in a
// summary produced a document whose eight `#` lines rendered as two headings and
// one enormous code block. This states the property that implies the rendering:
// no line may begin a block construct the document did not write itself.
test("markdown export starts no block construct it did not write", () => {
  const payloads = ["~~~", "~~~~~~", "```", "<!-- x", "<div>", "___", "---", "===", "    indented"];

  for (const payload of payloads) {
    const markdown = reportMarkdown({
      report: {
        incomplete: true,
        summary: payload,
        integrityEvents: [{ type: "T", at: "a", severity: "info", detail: payload }],
      },
      problemTitle: "Two Sum",
      language: "python",
      code: "print(1)\n",
      transcript: [{ speaker: "you", text: payload, final: true }],
      at: "2026-01-01",
    });

    // Everything the document legitimately opens: its own headings, its own
    // fence around the candidate's code, its bullets, and its bold speaker
    // labels. Anything else at the start of a line came from the payload.
    const own = /^(#{1,6} |`{3,}(python)?$|- |\*\*|_2026-01-01_$|\| |\|-)/;
    for (const line of markdown.split("\n")) {
      if (!line.trim() || own.test(line) || line === "print(1)") continue;
      assert.doesNotMatch(
        line,
        /^ {0,3}(~{3,}|`{3,}|<|_{3,}|-{3,}|={2,})|^ {4,}\S/,
        `${JSON.stringify(payload)} opened a block construct: ${JSON.stringify(line)}`,
      );
    }
  }
});

// A grader writes numbered feedback. An escape has to be invisible once
// rendered, and `\3` is not: a digit is not escapable punctuation, so the
// backslash survives into the reader's copy.
test("markdown export escapes list delimiters without leaving the backslash visible", () => {
  const markdown = reportMarkdown({
    report: {
      incomplete: true,
      summary: "3. Use a hash map",
      integrityEvents: [],
      codingFeedback: { strengths: ["12) Rename the variable"], improvements: [] },
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n",
    transcript: [],
    at: "2026-01-01",
  });

  assert.match(markdown, /^3\\\. Use a hash map$/m, "the delimiter carries the escape, not the digit");
  assert.doesNotMatch(markdown, /\\\d/, "a backslash before a digit renders literally");
});

// A `===` line would underline the paragraph above it into a heading, but no
// caller can produce one on its own line: feedback and evidence are list items,
// transcript turns carry a speaker prefix, and the summary is separated by a
// blank line, which ends the paragraph a setext underline would need.
test("markdown export gives untrusted text no bare line to underline", () => {
  const markdown = reportMarkdown({
    report: {
      incomplete: true,
      summary: "===",
      integrityEvents: [{ type: "T", at: "a", severity: "info", detail: "===" }],
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n",
    transcript: [
      { speaker: "you", text: "make me a heading", final: true },
      { speaker: "you", text: "===", final: true },
    ],
    at: "2026-01-01",
  });

  const lines = markdown.split("\n");
  for (const [index, line] of lines.entries()) {
    if (!/^[=-]+$/.test(line.trim()) || !line.trim()) continue;
    const above = lines[index - 1] ?? "";
    assert.equal(above.trim(), "", `a setext underline landed under ${JSON.stringify(above)}`);
  }
});

// The info string is the only value this document emits unescaped, so it takes
// only what a language tag can be.
test("markdown export cannot have its code fence opened by the language", () => {
  const markdown = reportMarkdown({
    report: { incomplete: true, summary: "s", integrityEvents: [] },
    problemTitle: "Two Sum",
    language: "python\n```\n## Injected",
    code: "print(1)\n",
    transcript: [],
    at: "2026-01-01",
  });

  assert.match(markdown, /^```pythonInjected$/m, "the fence header keeps only tag characters");
  assert.doesNotMatch(markdown, /^ {0,3}## Injected$/m, "the language cannot write a heading");
});

// Candidate code is the one thing here that must not be escaped, so a run of
// backticks inside it has to be answered by a longer fence. Otherwise a ``` in
// a comment closes the block and the rest of the report is read as prose.
test("markdown export fences code that contains backticks", () => {
  const markdown = reportMarkdown({
    report: {
      incomplete: true,
      summary: "Nothing to grade.",
      integrityEvents: [],
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "# ```\nprint(1)\n",
    transcript: [],
    at: "2026-01-01",
  });

  assert.match(markdown, /^````python$/m, "the fence outgrows the longest run inside it");
  assert.match(markdown, /^# ```$/m, "the code itself is left exactly as the candidate wrote it");
  assert.match(markdown, /^## Conversation transcript$/m, "the sections after the code survive");
});

test("markdown export handles an empty session", () => {
  const markdown = reportMarkdown({
    report: {
      codingScore: 0,
      communicationScore: 0,
      decision: "NO_HIRE",
      summary: "",
      codingFeedback: { strengths: [], improvements: [] },
      communicationFeedback: { strengths: [], improvements: [] },
      hintsUsed: 0,
    },
    problemTitle: "Two Sum",
    language: "javascript",
    code: "",
    transcript: [],
    at: "2026-01-01",
  });

  assert.match(markdown, /^## Verdict: NO HIRE$/m);
  assert.match(markdown, /^\(none\)$/m, "missing summary falls back");
  assert.match(markdown, /^\(editor was empty\)$/m);
  assert.match(markdown, /^\(no speech captured\)$/m);
});

test("the report says the event list is a subsequence, or says it cannot tell", () => {
  // Retention makes the list a subsequence with holes, so a reader who is not
  // told cannot distinguish "the agent dropped 380 for space" from "rows were
  // deleted from the stored report". That distinction is what the chain is for.

  const dropped = reportMarkup({ report: sanitizeReport({
    decision: "HIRE", integrityEvents: events, integrityChainSeq: 412, integrityDropped: 380,
  }), problemTitle: "Two Sum", language: "python", code: "" });
  assert.match(dropped, /Chain verified through event 412/);
  assert.match(dropped, /380 more were verified and not kept/);

  const complete = reportMarkup({ report: sanitizeReport({
    decision: "HIRE", integrityEvents: events, integrityChainSeq: 1, integrityDropped: 0,
  }), problemTitle: "Two Sum", language: "python", code: "" });
  assert.match(complete, /every event it verified is listed above/);

  // A report with no checkpoint must not read as "nothing was dropped", which is
  // the one answer it cannot support. Both shapes reach this: a report stored
  // before the field existed carries nothing, and the agent sends an explicit
  // null when the chain never advanced. `Number(null)` is 0, so the second one
  // used to render as "verified through event 0, every event is listed above".
  for (const absent of [{}, { integrityChainSeq: null, integrityDropped: null }]) {
    const legacy = reportMarkup({
      report: sanitizeReport({ decision: "HIRE", integrityEvents: events, ...absent }),
      problemTitle: "Two Sum", language: "python", code: "",
    });
    assert.match(legacy, /predates chain reporting/, JSON.stringify(absent));
    assert.doesNotMatch(legacy, /every event it verified is listed above/, JSON.stringify(absent));
  }
});

test("the page and the exported markdown tell the same chain story", () => {
  // They were two copies of the same three branches and already worded
  // differently. The export is the copy that gets forwarded, so a reader
  // comparing it against the page must not find a different claim.
  const shapes = [
    { integrityChainSeq: 412, integrityDropped: 380 },
    { integrityChainSeq: 7, integrityDropped: 0 },
    {},
  ];
  // Both list shapes. Nothing retained is not a rare case: it is what a report
  // looks like when everything the chain verified was dropped for space, and it
  // is the case where the page used to stay silent while the export spoke.
  for (const [report, list] of shapes.flatMap((r) => [[r, events], [r, []]])) {
    const sentence = chainSentence(sanitizeReport({ decision: "HIRE", ...report }));
    const session = {
      report: sanitizeReport({ decision: "HIRE", integrityEvents: list, ...report }),
      problemTitle: "Two Sum", language: "python", code: "",
    };
    assert.ok(reportMarkup(session).includes(sentence), sentence);
    assert.ok(reportMarkdown({ ...session, transcript: [] }).includes(sentence), sentence);
  }
});

test("report views identify active and legacy scoring contracts", () => {
  const active = sanitizeReport({ incomplete: true, interviewContract: {
    bundleVersion: 3, livePromptVersion: 1, reportPromptVersion: 3,
    rubricVersion: 1, reportSchemaVersion: 1,
  } });
  const session = { report: active, problemTitle: "Two Sum", language: "python", code: "" };
  assert.match(reportMarkup(session), /Contract bundle 3 · rubric 1 · report schema 1/);
  assert.match(reportMarkdown({ ...session, transcript: [] }), /Contract: bundle 3; live prompt 1; report prompt 3; rubric 1; report schema 1/);

  const legacy = { ...session, report: sanitizeReport({ incomplete: true }) };
  assert.match(reportMarkup(legacy), /Legacy\/unversioned contract/);
  assert.match(reportMarkdown({ ...legacy, transcript: [] }), /Contract: legacy\/unversioned/);
});
