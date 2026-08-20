// Run with: node --test tests/browser/recording-consent.test.js
//
// Consent is the one part of recording that is a promise to a person rather
// than a call to a provider, and the only place it is visible is markup and one
// fetch. Nothing here can run `web/interview.js`, so these read both sides as
// text: the disclosure has to say the five things it promises, the step has to
// be reachable by the ids the script queries, and the consent request has to
// come before the token request in `connect`.
//
// Tripwires, not proofs. What actually enforces the ordering is the server
// refusing a recorded room without a persisted consent row, which
// `cargo test --test web interviews_persist_consent_before_egress` covers.

import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";

const page = read("web/interview.html");
const script = read("web/interview.js");

/// Source with line comments removed.
///
/// Every assertion below is a substring search, and this file is full of
/// comments that quote the very strings it searches for. Without this, deleting
/// the call and leaving the comment that explains it keeps the test green,
/// which is the exact failure mode a tripwire cannot afford.
const withoutComments = (source) => source.replace(/^\s*\/\/.*$/gm, "");
const code = withoutComments(script);

test("recording-consent the notice is part of the preflight, not the sidebar", () => {
  // Inside the overlay the candidate cannot skip. A notice in a collapsed
  // panel is a notice nobody read.
  const overlay = page.slice(page.indexOf('id="audio-check"'), page.indexOf('id="ending-overlay"'));
  assert.ok(
    overlay.includes('id="recording-consent-step"'),
    "the consent step belongs inside the media preflight",
  );
  assert.ok(overlay.includes('id="recording-consent"'), "and it needs a control to agree with");
  assert.ok(
    overlay.includes('id="recording-consent-step" hidden'),
    "hidden by default, because most deployments record nothing",
  );
  assert.ok(
    /<label for="recording-consent">/.test(overlay),
    "the checkbox needs a label a screen reader can announce",
  );
});

test("recording-consent the disclosure names what it promises", () => {
  // Whitespace collapsed first. The markup wraps at eighty columns, so a
  // phrase the candidate reads as one sentence is several lines in the file and
  // a naive pattern misses it.
  const disclosure = page
    .slice(
      page.indexOf('id="recording-consent-disclosure"'),
      // The paragraph's own closing tag, not the step container's: slicing to
      // `</div>` would pass on text that had leaked out of the paragraph.
      page.indexOf("</p>", page.indexOf('id="recording-consent-disclosure"')),
    )
    .replace(/\s+/g, " ");
  // One assertion per promise, so a rewrite that drops one fails on that one
  // rather than on an opaque whole-text comparison.
  const promises = {
    "what is captured": /camera, microphone, and this page/i,
    "who receives it": /verified email address/i,
    "how long it is kept": /24 hours/i,
    "that it is deleted": /deleted/i,
    "that consent can be withdrawn": /withdraw consent/i,
    "that local copies cannot be recalled": /cannot be recalled|outside CodeTrial's control/i,
  };
  for (const [promise, pattern] of Object.entries(promises)) {
    assert.match(disclosure, pattern, `the disclosure has to state ${promise}`);
  }
});

test("recording-consent the server decides whether the notice applies", () => {
  // A literal here would be a second answer to "does this server record", and
  // the wrong one shows a candidate no notice at all.
  assert.match(code, /globalThis\.CODETRIAL_RECORDING_ENABLED === true/);
  assert.match(code, /globalThis\.CODETRIAL_CONSENT_VERSION/);
  assert.ok(
    code.includes("nodes.recordingConsentStep.hidden = !recordingEnabled"),
    "the step is shown only where recording is on",
  );
});

test("recording-consent the start button waits for an answer", () => {
  const body = withoutComments(functionBody(script, "consentGiven"));
  assert.ok(
    body.includes("!recordingEnabled || nodes.recordingConsent.checked"),
    "consent is given, or does not apply; there is no third state",
  );
  assert.ok(
    code.includes("const ready = state.ready && consentGiven();"),
    "media readiness alone must not open the Start button on a recording server",
  );
  assert.ok(
    code.includes("nodes.audioJoin.disabled = !ready;"),
    "and the button reads that combined answer",
  );
  assert.ok(
    code.includes('nodes.recordingConsent.addEventListener("change", refresh)'),
    "ticking the box has to repaint, or the button never opens",
  );
});

test("recording-consent the notice's promise of withdrawal is reachable", () => {
  // The disclosure says consent can be withdrawn during the interview. A
  // promise with no control behind it is the worst kind of privacy copy.
  assert.ok(page.includes('id="withdraw-consent"'), "there has to be something to withdraw with");
  assert.ok(
    /id="withdraw-consent"[^>]*hidden/.test(page),
    "hidden where the server records nothing",
  );
  const body = withoutComments(functionBody(script, "withdrawRecordingConsent"));
  assert.match(
    body,
    /fetch\(`\/api\/interviews\/\$\{encodeURIComponent\(state\.interviewId\)\}\/consent`/,
    "and it has to call the route that records the withdrawal",
  );
  assert.match(body, /method: "DELETE"/);
  assert.ok(
    code.includes('nodes.withdrawConsent.addEventListener("click", withdrawRecordingConsent)'),
    "the button has to be wired to it",
  );
  assert.ok(
    code.includes("nodes.withdrawConsent.hidden = false;"),
    "and revealed once there is an interview to withdraw from",
  );

  // The route records a withdrawal. Stopping the provider and scheduling the
  // deletion is the recording lifecycle's work, so the button must not report
  // a completed action it did not perform.
  assert.ok(
    body.includes('"Recording stop requested"'),
    "the button says what happened, which is that the request was recorded",
  );
  assert.ok(
    !/Recording stopped/.test(body),
    "and never claims the recording has already stopped",
  );
});

test("recording-consent is recorded before a token is asked for", () => {
  const body = withoutComments(functionBody(script, "connect"));
  assert.ok(
    body.includes('if (!consentGiven()) throw new Error('),
    "the gate is restated where consent is actually written down, not only on the button",
  );
  const consent = body.indexOf("await recordConsent()");
  const token = body.indexOf('fetch("/api/token"');
  assert.ok(consent !== -1, "connect records consent");
  assert.ok(token !== -1, "connect asks for a token");
  assert.ok(
    consent < token,
    "the token is what precedes an Egress call, so consent is written down first",
  );
  assert.ok(
    /interviewId/.test(body.slice(token)),
    "and the token request carries the interview the consent belongs to",
  );
});

test("recording-consent sends the version it displayed", () => {
  const body = withoutComments(functionBody(script, "recordConsent"));
  assert.ok(body.includes('fetch("/api/interviews"'), "consent is posted, not assumed");
  assert.ok(body.includes("consentVersion"), "and it names the wording that was shown");
  assert.ok(
    body.includes("if (!recordingEnabled) return null;"),
    "a server that records nothing is not asked to record consent",
  );
  assert.ok(
    /throw new Error/.test(body),
    "a refused consent must not fall through into a recorded interview",
  );
});
