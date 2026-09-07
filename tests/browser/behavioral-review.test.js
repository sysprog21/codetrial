import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  STAR_FIELDS,
  behavioralReviewMarkup,
  candidateTranscriptText,
  mountBehavioralReview,
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

// What the module must NOT do, which is the one property here that no drive can
// express: an absence is over the whole file, and a test that calls the two
// exported entry points would say nothing about a fetch on a third path.
//
// The three `assert.match` tripwires that used to sit above these -- for
// `#star-reset`, `#star-dismiss` and `.remove()` -- are gone, because the tests
// that drive `mountBehavioralReview` below strictly dominate them. Renaming the
// reset id in the mount fails the drive and passed all three, since `/#star-reset/`
// matches `#star-reset-draft`; deleting the wiring outright fails both.
test("the review is a local rewrite of the candidate's own words and nothing else", () => {
  const source = readFileSync(new URL("../../web/behavioral-review.js", import.meta.url), "utf8");
  // No provider: a suggestion that reached a model would be the app putting
  // words in the candidate's mouth, which is the thing this feature is not.
  assert.doesNotMatch(source, /\bfetch\s*\(|Gemini|\/api\//);
  // And no persistence: the draft is a scratchpad, so it must not reach the
  // stored report, the history, or the exported markdown.
  assert.doesNotMatch(source, /localStorage|saveReportHistory|reportMarkdown/);
});

test("the tagging form is laid out, not left as running text", () => {
  // The markup was always semantic: a label wrapping its own textarea. With no
  // rule for it the pair laid out inline, so each box sat between its own name
  // and the next one and the fourth wrapped alone onto a second line. Asserted
  // here because nothing else in the suite looks at how this renders.
  const css = readFileSync(new URL("../../web/styles.css", import.meta.url), "utf8");
  assert.match(css, /\.star-fields \{[^}]*display: grid;/s);
  assert.match(css, /\.star-field \{[^}]*flex-direction: column;/s);
  assert.match(css, /#star-suggestion label \{[^}]*display: block;/s);
  assert.match(css, /\.star-field textarea,\s*#star-suggestion textarea \{[^}]*width: 100%;/s);
});

/// A fragment-level stand-in for the part of a document `mountBehavioralReview`
/// touches, built by scanning the markup the module itself produces rather than
/// by hand: a node exists here exactly when the markup declares it, so the two
/// cannot drift into agreement about an id neither renders.
///
/// Flat, like `tests/browser/dom.js` and for the same reason -- a tree parsed
/// wrong models a page no browser would. It is enough because every node this
/// module queries is a descendant of the one section it inserts. Selectors are
/// matched against id and attribute only, and anything else throws rather than
/// answering `null`, so a query this cannot honour fails the test instead of
/// quietly taking the module's `?.` branch.
function reportRoot() {
  const nodes = [];
  const build = (markup) => {
    for (const [, tag, attributes] of markup.matchAll(/<([a-zA-Z][\w-]*)((?:\s+[\w-]+="[^"]*")*)/g)) {
      const attrs = Object.fromEntries([...attributes.matchAll(/([\w-]+)="([^"]*)"/g)].map((it) => [it[1], it[2]]));
      nodes.push({
        tag,
        attrs,
        removed: false,
        value: "",
        listeners: {},
        addEventListener(name, handler) {
          (this.listeners[name] ||= []).push(handler);
        },
        fire(name) {
          for (const handler of this.listeners[name] || []) handler();
        },
        remove() {
          this.removed = true;
        },
        querySelector: (selector) => find(selector),
        querySelectorAll: (selector) => findAll(selector),
      });
    }
  };
  const findAll = (selector) => {
    const match = selector.match(/^#([\w-]+)$|^\[([\w-]+)(?:="([^"]*)")?\]$|^\.([\w-]+)$/);
    if (!match) throw new Error(`the fragment stub does not implement the selector: ${selector}`);
    const [, id, attribute, attributeValue, className] = match;
    return nodes.filter((node) => {
      if (node.removed) return false;
      if (id !== undefined) return node.attrs.id === id;
      if (attribute !== undefined) {
        if (!(attribute in node.attrs)) return false;
        return attributeValue === undefined || node.attrs[attribute] === attributeValue;
      }
      return (node.attrs.class || "").split(/\s+/).includes(className);
    });
  };
  const find = (selector) => findAll(selector)[0] ?? null;
  // The anchor the module inserts before. It is the page's, not the markup's,
  // so it is declared here rather than scanned.
  build(`<div class="report-actions">`);
  const inserted = [];
  find(".report-actions").insertAdjacentHTML = (position, markup) => {
    inserted.push({ position, markup });
    build(markup);
  };
  return { root: { querySelector: find }, find, findAll, inserted };
}

// Every assertion above this point reads the module as text or checks a pure
// function, so the wiring `mountBehavioralReview` installs was pinned by
// `assert.match(source, /#star-reset/)` and nothing else. That survives three
// mutations that break the feature outright: a reset handler that writes an
// empty string instead of the rewrite, a dismiss button bound to "mouseover",
// and the per-field "input" listener deleted. Driving it catches all three,
// because it asks what the buttons do rather than whether their names appear.
test("the review draft is rebuilt from the fields on every edit and on reset", () => {
  const { root, find, findAll, inserted } = reportRoot();

  assert.equal(mountBehavioralReview(root, [
    { speaker: "candidate", text: "We lost the checkout queue.", final: true },
  ]), true);
  assert.deepEqual(inserted.map((it) => it.position), ["beforebegin"],
    "the section goes in before the report actions, once");

  const rewrite = find("#star-rewrite");
  assert.equal(rewrite.value, "", "nothing is drafted before the candidate tags anything");

  const fields = findAll("[data-star-field]");
  assert.deepEqual(fields.map((node) => node.attrs["data-star-field"]), STAR_FIELDS);

  // Typing in one field redraws the whole draft, so a handler that fires but
  // reads the wrong node, or writes a constant, is visible here.
  fields[0].value = "The checkout queue stalled.";
  fields[0].fire("input");
  assert.equal(rewrite.value, "Situation: The checkout queue stalled.");

  fields[2].value = "I drained it by hand.";
  fields[2].fire("input");
  assert.equal(rewrite.value, "Situation: The checkout queue stalled.\nAction: I drained it by hand.");

  // Reset is the same rebuild, so an edit the candidate made to the draft goes
  // back to what their fields say and no further.
  rewrite.value = "invented prose nobody said";
  find("#star-reset").fire("click");
  assert.equal(rewrite.value, "Situation: The checkout queue stalled.\nAction: I drained it by hand.");
});

test("dismissing the suggestion removes it and leaves the transcript standing", () => {
  const { root, find } = reportRoot();
  mountBehavioralReview(root, [{ speaker: "candidate", text: "We lost the queue.", final: true }]);

  assert.notEqual(find("#star-suggestion"), null);
  find("#star-dismiss").fire("click");
  assert.equal(find("#star-suggestion"), null, "the generated draft is what goes away");
  // The candidate's own words are evidence and are not the app's to withdraw.
  assert.notEqual(find("#star-original"), null);
  assert.notEqual(find("#behavioral-review"), null);
});

// Nothing is mounted for a candidate who never spoke, and the caller is told so
// rather than left to discover an empty section on the page.
test("a session with no candidate speech mounts nothing", () => {
  const { root, find, inserted } = reportRoot();
  assert.equal(mountBehavioralReview(root, [
    { speaker: "interviewer", text: "Tell me about a conflict.", final: true },
    { speaker: "candidate", text: "still typing", final: false },
  ]), false);
  assert.deepEqual(inserted, []);
  assert.equal(find("#behavioral-review"), null);
});
