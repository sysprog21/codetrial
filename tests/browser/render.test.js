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
  reportMarkdown,
  reportMarkup,
  resultsMarkup,
  runnerStatusMarkup,
} from "../../web/render.js";

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
    },
    problemTitle: "Two Sum",
    language: "python",
    code: "print(1)\n\n",
  });

  assert.match(body, /class="good">HIRE</);
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
  assert.match(markdown, /^## Verdict: HIRE$/m);
  assert.match(markdown, /^\| Coding \| 82 \/ 100 \|$/m);
  assert.match(markdown, /^\| Hints used \| 2 \|$/m);
  assert.match(markdown, /^## Integrity Evidence$/m);
  assert.match(markdown, /^- 2026-08-15 \[info\] REVIEW_EVENT - &lt;ok&gt;\\\|fine - sources: 1, 2$/m);
  assert.match(markdown, /^\*\*Jim:\*\* Ready\?$/m, "speaker text is trimmed");
  assert.match(markdown, /^\*\*You:\*\* Yes$/m);
  assert.doesNotMatch(markdown, /\*\*You:\*\* *$/m, "blank interim segments are dropped");
  assert.match(markdown, /^```python$/m);
  assert.match(markdown, /^- \(none captured\)$/m, "empty feedback still renders a bullet");
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
