import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  behavioralReviewMarkup,
  candidateTranscriptText,
  starRewrite,
} from "../../web/behavioral-review.js";

test("candidate transcript excludes interviewer and preserves the original candidate words", () => {
  const text = candidateTranscriptText([
    { speaker: "interviewer", text: "Tell me about it", final: true },
    { speaker: "candidate", text: "  We had an outage.  ", final: true },
    { speaker: "candidate", text: "I traced the retry loop.", final: true },
    { speaker: "candidate", text: "unfinished invention", final: false },
    { speaker: "candidate", text: "", final: true },
  ]);
  assert.equal(text, "  We had an outage.  \nI traced the retry loop.");
});

test("STAR rewrite contains only candidate fields plus fixed labels", () => {
  const fields = {
    Situation: "The checkout timed out.",
    Task: "I owned diagnosis.",
    Action: "I inspected traces.",
    Result: "Latency recovered.",
  };
  const rewrite = starRewrite(fields);
  assert.equal(rewrite, "Situation: The checkout timed out.\nTask: I owned diagnosis.\nAction: I inspected traces.\nResult: Latency recovered.");
  for (const value of Object.values(fields)) assert.ok(rewrite.includes(value));
  assert.equal(starRewrite({}), "");
});

test("behavioral review escapes hostile transcript and stays absent without candidate speech", () => {
  const markup = behavioralReviewMarkup("<img src=x onerror=alert(1)>");
  assert.doesNotMatch(markup, /<img/);
  assert.match(markup, /&lt;img/);
  assert.equal(behavioralReviewMarkup("  "), "");
  for (const name of ["Situation", "Task", "Action", "Result"]) {
    assert.match(markup, new RegExp(`data-star-field="${name}"`));
  }
  assert.match(markup, /Local self-review only/);
  assert.match(markup, /generated from your fields/);
});

test("review module has reset and dismiss wiring and no provider path", () => {
  const source = readFileSync(new URL("../../web/behavioral-review.js", import.meta.url), "utf8");
  assert.match(source, /#star-reset/);
  assert.match(source, /#star-dismiss/);
  assert.match(source, /\.remove\(\)/);
  assert.doesNotMatch(source, /\bfetch\s*\(|Gemini|\/api\//);
  assert.doesNotMatch(source, /localStorage|saveReportHistory|reportMarkdown/);
});
