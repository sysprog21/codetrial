// Run with: node --test tests/browser/replay-render.test.js
//
// What the replay page actually says, read off the render rather than off the
// source.
//
// Every other check on this page reads its text. That is how a dozen ways to put
// an accusation on it were each closed one at a time: a guard that names files,
// functions or variables is a guard somebody writes another around, and the page
// renders through five of them. This drives the render with sentinels in every
// value the server supplies and reads what comes out, which has no such edge.
//
// What it is: a tripwire over the ways a person adds a label without meaning to
// hide it. It reads the panels this page draws into, after every button has been
// pressed, over replays covering every shape a window comes in; it reads text,
// the attributes a browser speaks, whatever went in through a markup property,
// and the dataset, because a stylesheet can draw that. It reads the stylesheet
// separately, because no stub applies one, and the report card's vocabulary
// across every section that card can draw, because that renders here too.
//
// What it is not: a proof. Three commits in a row claimed a word this page
// invents had nowhere left to hide, and each time a reviewer found more places,
// so the claim is retired rather than restated. Text can reach a reader
// through a mechanism no stub models, and the known gaps are written down so the
// next reader inherits them instead of rediscovering them:
//
//  - `select`, `loadEvents` and `loadReport` are never called, and `loadList`
//    runs once at import but only down its failure branch, because the `fetch`
//    installed here refuses everything. Driving their success paths needs a stub
//    with an ordering seam instead. The copy the first three write is held by
//    the literal allowlist in `tests/browser/history.test.js`, which is a source
//    check; what `loadReport` writes is the report card, held by the vocabulary
//    check below, because those words are the report renderer's rather than
//    this page's, and some of them are `web/lib.js`'s.
//  - A source check reads string literals. A regex literal stringified, or a
//    value assembled from character codes, is not one.
//  - `document.title`, and anything else written to the document rather than to
//    a node, is outside what this stub models.
//
// Integrity features produce evidence for human review, not a claim that the
// candidate cheated. That is the rule this file exists to make observable, and
// the list above is what "observable" does not stretch to.

import { test } from "node:test";
import assert from "node:assert/strict";
import { captures, read } from "./source.js";
import { installDocument, importWithout } from "./dom.js";

const dom = installDocument(read("web/replay.html"));
const { render, momentTime } = await import("/replay.js");
const { reportMarkup } = await import("/render.js");
const { sanitizeReport, frameworkPhases } = await import("/lib.js");

/// Every value the server supplies, as a token nothing else could produce. A
/// rendered string is then either first-party copy, a sentinel, or a clock.
const S = {
  code: "Sxcode",
  language: "Sxlang",
  speaker: "Sxwho",
  text: "Sxsaid",
};

/// `responseWindow` is what the producer stamps on the row that opens a window,
/// and on the transcript row whose stream started inside one. Omitting it is not
/// a shortcut here: it is the shape a recording made before that stamping has,
/// and the page says something different about those, so both are fixtures.
const avatar = (at, state, responseWindow) => ({
  kind: "avatar",
  at,
  payload: { state, ...(responseWindow === undefined ? {} : { responseWindow }) },
});
const life = (at, state) => ({ kind: "lifecycle", at, payload: { state } });
const editor = (at) => ({ kind: "editor", at, payload: { code: S.code, language: S.language } });
const tests = (at) => ({ kind: "tests", at, payload: { passed: 3, total: 5 } });
const said = (at, responseWindow) => ({
  kind: "transcript",
  at,
  payload: {
    speaker: S.speaker,
    text: S.text,
    ...(responseWindow === undefined ? {} : { responseWindow }),
  },
});
const stage = (at, mode) => ({ kind: "stage", at, payload: { mode } });

const BASE = 1_770_000_000_000;

/// One question asked and answered: the interviewer speaking, stopping, and
/// speaking again. Four of the replays below are this and nothing else, differing
/// only in when the two ends fall.
const question = (opens, closes, index = 0) => [
  avatar(opens - 1000, "speaking"),
  avatar(opens, "listening", index),
  avatar(closes, "speaking"),
];

/// Every shape a window comes in, so a label that only fires on one of them is
/// still rendered at least once. Branching on a value no fixture sets is the
/// hole this matrix exists to have no room for.
const REPLAYS = {
  "a closed window with an answer": [
    stage(BASE, 0),
    avatar(BASE, "speaking"),
    editor(BASE + 500),
    avatar(BASE + 1000, "listening", 0),
    said(BASE + 4000, 0),
    avatar(BASE + 5000, "speaking"),
    tests(BASE + 6000),
  ],
  "a closed window with no answer": question(BASE + 1000, BASE + 40_000),
  // The same window, from a recording made before the producer stamped its
  // turns. Two of them, because one open window is the single shape a turn can
  // still be placed in without guessing, so a one-window fixture would render
  // the answered case and never reach the sentence this exists for.
  "windows from before turns were stamped": [
    avatar(BASE, "speaking"),
    avatar(BASE + 1000, "listening"),
    said(BASE + 4000),
    avatar(BASE + 5000, "speaking"),
    avatar(BASE + 6000, "listening"),
    said(BASE + 9000),
    avatar(BASE + 10_000, "speaking"),
  ],
  "a window that never closed": [avatar(BASE, "speaking"), avatar(BASE + 1000, "listening", 0), said(BASE + 2000, 0)],
  "a window whose clock ran backwards": question(BASE + 9000, BASE + 4000),
  "a window the interview was paused during": [
    avatar(BASE, "speaking"),
    avatar(BASE + 1000, "listening", 0),
    life(BASE + 2000, "paused"),
    life(BASE + 300_000, "resumed"),
    avatar(BASE + 301_000, "speaking"),
  ],
  "a long window": question(BASE + 1000, BASE + 3_600_000),
  "a window before the first snapshot": [...question(BASE + 1000, BASE + 2000), editor(BASE + 3000)],
  "a replay with no windows at all": [editor(BASE), tests(BASE + 1000), said(BASE + 2000)],
  // `interviewMode` compares against the string, so a number here falls to
  // "Scored" and the fixture rendered the opposite of its own name.
  "the practice mode a legacy report still carries": [
    stage(BASE, "practice"),
    avatar(BASE, "speaking"),
  ],
  // More than one window, and a run of them holding no transcript, which is the
  // shape the page's own note singles out as the one worth opening. Every
  // fixture above yields nought or one, so a label that only fires on a run had
  // nowhere to fire and nothing to fail.
  "a run of windows with no transcript": [
    avatar(BASE, "speaking"),
    ...[1, 2, 3, 4, 5].flatMap((turn) => [
      avatar(BASE + turn * 30_000, "listening", turn - 1),
      avatar(BASE + turn * 30_000 + 26_000, "speaking"),
    ]),
  ],
  "several answered windows and an editor between them": [
    avatar(BASE, "speaking"),
    avatar(BASE + 1000, "listening", 0),
    said(BASE + 4000, 0),
    avatar(BASE + 5000, "speaking"),
    editor(BASE + 6000),
    avatar(BASE + 7000, "listening", 1),
    said(BASE + 9000, 1),
    avatar(BASE + 10_000, "speaking"),
    tests(BASE + 11_000),
  ],
};

/// Every word this page may put in front of a person, and nothing else.
///
/// Kinds and states are here because the page renders them verbatim: a moment
/// button says `editor`, which is the event kind. They are the schema's words,
/// not a judgement, and they are listed so that a kind renamed to something that
/// reads as one fails here.
const ALLOWED = [
  // The response window panel.
  "response window", "duration not recorded", "no candidate transcript recorded",
  "transcript not matched to a window",
  "interview paused during this window", "s",
  // The moment list and the panels beside it.
  "editor", "tests", "Code", "passing", "No test run before this point.",
  // The interview mode a legacy replay carries.
  "Scored", "Practice",
  // Every value the server supplied, which this page passes through and does
  // not author.
  ...Object.values(S),
];

/// What is left of a rendered string once the words somebody chose, the clock,
/// and the punctuation are taken out of it.
/// Longest first, or the unit "s" eats the "s" out of "tests" and the word it
/// was part of stops matching. Sorted once: it is a constant of the run.
const ALLOWED_LONGEST_FIRST = [...ALLOWED].sort((left, right) => right.length - left.length);

/// The clocks one replay holds, formatted by the page's own formatter. Memoized
/// per replay rather than per string: `toLocaleTimeString` is the expensive part
/// and a replay has a handful of distinct timestamps against dozens of strings.
const clocksOf = new Map();
function clocks(events) {
  if (!clocksOf.has(events)) {
    clocksOf.set(events, [...new Set(events.map((event) => event.at))].map(momentTime));
  }
  return clocksOf.get(events);
}

function unaccountedFor(spoken, events) {
  let rest = spoken;
  // The clock first, taken from the page's own formatter for the timestamps this
  // replay holds. A regex for it assumed an English locale: under `zh_TW` the
  // same clock reads with a Chinese meridiem and under `ar-EG` with Arabic-Indic
  // digits, and both failed a test that is not about locales.
  for (const clock of clocks(events)) rest = rest.split(clock).join(" ");
  for (const word of ALLOWED_LONGEST_FIRST) rest = rest.split(word).join(" ");
  return rest.replace(/[\s\d.,:;/·|-]/g, "");
}

test("the replay page says only the words somebody chose", () => {
  // Every panel this page draws words into, the transcript included. Excluding
  // it on the grounds that it only renders the server's words was a claim about
  // the source, which is the kind of claim this file exists to stop making; the
  // sentinels make it cost nothing to read. `#replay-window-note` is absent
  // because `render` only toggles its `hidden`: its text is in the markup, and
  // `tests/browser/history.test.js` pins that.
  const panels = [
    "replay-timeline",
    "replay-status",
    "replay-tests",
    "replay-moment-label",
    "replay-transcript",
    "replay-code",
  ];
  for (const [name, events] of Object.entries(REPLAYS)) {
    for (const id of panels) {
      dom.node(id).replaceChildren();
      dom.node(id).markup.length = 0;
    }
    render(events);

    // Read as rendered, then again after every button has been pressed:
    // `showMoment` and `markCurrent` both write to these panels and neither
    // runs until somebody clicks. Two reads around one round of clicks, not two
    // rounds: nothing observed the second round.
    const readPanels = (pass) => {
      for (const id of panels) {
        for (const spoken of dom.node(id).spoken()) {
          assert.equal(
            unaccountedFor(spoken, events),
            "",
            `${name}, ${pass}: #${id} said something nobody chose: ${JSON.stringify(spoken)}`,
          );
        }
      }
    };
    readPanels("as rendered");
    for (const button of dom.node("replay-timeline").querySelectorAll("[data-moment], [data-window]")) {
      button.click();
    }
    readPanels("after every button is pressed");
  }
});

test("the replay page renders a window for every question and none for anything else", () => {
  // The counterpart to the copy check: that the panel says the right things is
  // only worth asserting if it says them about the right rows.
  const windows = (events) => {
    dom.node("replay-timeline").replaceChildren();
    render(events);
    return dom
      .node("replay-timeline")
      .querySelectorAll("[data-window]")
      .map((button) => button.textContent);
  };

  assert.equal(windows(REPLAYS["a closed window with an answer"]).length, 1);
  assert.equal(windows(REPLAYS["a replay with no windows at all"]).length, 0);
  assert.match(windows(REPLAYS["a closed window with no answer"])[0], /no candidate transcript recorded/);
  // The two sentences an empty window comes in, told apart by the recording
  // rather than by the interview. Both fixtures hold a window with no turn on
  // it; only one of them is a recording that could have said so.
  const legacy = windows(REPLAYS["windows from before turns were stamped"]);
  assert.equal(legacy.length, 2);
  assert.match(legacy[1], /transcript not matched to a window/);
  assert.doesNotMatch(legacy[1], /no candidate transcript recorded/);
  assert.match(windows(REPLAYS["a window that never closed"])[0], /duration not recorded/);
  assert.match(windows(REPLAYS["a window whose clock ran backwards"])[0], /duration not recorded/);
  assert.match(windows(REPLAYS["a window the interview was paused during"])[0], /interview paused during this window/);
  assert.match(windows(REPLAYS["a long window"])[0], /3599\.0 s/);
});

test("the stylesheet puts no words on the page either", () => {
  // Not reachable by rendering: a stub document applies no stylesheet, and
  // `::after { content: … }` is text a browser draws that no node holds.
  //
  // Scoped to the declarations that draw, rather than to every quoted string in
  // the file. The wider version failed on `input[type="text"]`, which is a
  // selector and not speech, and having to allowlist `"ready"` for the same
  // reason was the tell that it was reading the wrong thing. Comments out first:
  // an apostrophe in a sentence is not a quoted value.
  const styles = read("web/styles.css").replace(/\/\*[\s\S]*?\*\//g, "");
  // Every property that can draw text, not only `content`. A list marker does it
  // too, and both `#replay-timeline` and `#replay-transcript` are lists.
  // Whitespace before the colon is allowed by CSS and was allowed past an
  // earlier version of this: `content : "..."` matched nothing.
  const draws = /(?:^|[;{\s])(content|list-style-type|list-style)\s*:\s*([^;}]+)/gim;
  assert.deepEqual(
    new Set([...styles.matchAll(draws)].map((match) => match[2].trim())),
    new Set(['"\u2713"', "none"]),
    "a stylesheet that spells a word is a stylesheet saying something",
  );
  // `attr()` draws an attribute, so a dataset value becomes text a browser
  // shows. The render check reads the dataset for the same reason; this refuses
  // the mechanism outright, because the two together are what make it speech.
  assert.doesNotMatch(styles, /attr\s*\(/i, "an attribute drawn is an attribute spoken");
  // And nothing smuggled through a data URI, which carries its text unquoted.
  assert.doesNotMatch(styles, /url\(\s*['"]?data:/i, "an inline image can hold a sentence");
});

test("the report card this page renders names no finding either", () => {
  // `loadReport` writes `reportMarkup(...)` into `#replay-report` as markup, and
  // that markup comes from `web/render.js`, which the render above never
  // reaches: `render` does not call it, and the fetch this file installs refuses
  // everything.
  //
  // Held at every word rather than at the headings. Headings alone was the
  // earlier version and it was wrong twice over: three of them only render on
  // branches the fixtures did not reach, and the prose between them is not all
  // the interviewer's own assessment. `chainSentence`, "(none captured)" and the
  // sentences in the Integrity Evidence section are CodeTrial's words, in the
  // section a reviewer reads for exactly this.
  //
  // `web/render.js` is shared with the interview page, so this is a second
  // reader of it rather than its owner: a legitimate change to report copy adds
  // a word here, which is the point.
  // The raw shape the agent sends, which is what `sanitizeReport` reads. An
  // earlier version passed the sanitized field names instead, so the base card
  // rendered its placeholders and every branch below sanitized down to it.
  const sentinels = ["Sxsum", "Sxstr", "Sximp", "Sxtitle", "Sxlang", "Sxcode", "Sxnote", "Sxwhy", "Sxplan", "Sxev"];
  const base = {
    codingScore: 70,
    communicationScore: 70,
    decision: "HIRE",
    summary: "Sxsum",
    codingFeedback: { strengths: ["Sxstr"], improvements: ["Sximp"] },
    communicationFeedback: { strengths: ["Sxstr"], improvements: [] },
    hintsUsed: 0,
  };
  // Shapes `sanitizeReport` accepts, taken from tests/browser/render.test.js.
  // A branch list is a claim, and the section assertion below is what makes this
  // one check itself: a fixture the sanitizer rejects renders the default card
  // and looks exactly like a branch that worked.
  const branches = [
    {},
    { hintsUsed: 2 },
    { decision: "NO HIRE" },
    { decision: "LEAN HIRE" },
    {
      rounds: [
        { kind: "coding", budgetMin: 37, status: "complete" },
        { kind: "behavioral", budgetMin: 8, status: "started" },
      ],
    },
    {
      codingFeedback: { strengths: ["Sxstr"], improvements: ["Sximp"] },
      improvementPlan: [
        {
          phase: "Test",
          weakness: "Sximp",
          impact: "high",
          frequency: 2,
          drill: "Sxplan",
          durationMin: 10,
          successCriterion: "Sxwhy",
          selfReview: ["Sxnote"],
        },
      ],
    },
    {
      frameworkEvidence: [
        {
          atMs: 65_000,
          phase: "algorithm",
          source: "candidate_speech",
          kind: "observed",
          confidence: 95,
          summary: "Sxev",
          frameworkVersion: 1,
        },
      ],
    },
    {
      integrityEvents: [
        { type: "FACE_MISSING", severity: "warning", at: 1, detail: "Sxnote", sourceEventIds: ["7"] },
      ],
      integrityChainSeq: 2,
      integrityChainVerified: true,
    },
    // The other arm of the chain sentence, which only renders when something
    // was dropped for space.
    { integrityEvents: [{ type: "FACE_MISSING", severity: "warning", at: 1 }], integrityChainSeq: 2, integrityDropped: 3 },
    { incomplete: true, summary: "Sxsum" },
    // The header, the contract line and the calibration note, none of which any
    // earlier version of this list reached. The valid contract has to be the
    // active one or the card says the report is unsupported instead, so both
    // shapes are here: they render different sentences.
    { mode: "practice", interviewLoop: "coding_only" },
    {
      interviewContract: {
        bundleVersion: 4,
        livePromptVersion: 1,
        reportPromptVersion: 4,
        reportSchemaVersion: 1,
        rubricVersion: 1,
      },
    },
    { interviewContract: { bundleVersion: 1 } },
    {
      // Every phase, or `sanitizeReport` drops the assessment and the
      // calibration note with it. The list is read from `frameworkPhases` so a
      // phase added there does not silently shorten this fixture.
      frameworkAssessment: {
        rubricVersion: 1,
        phases: frameworkPhases.map((phase) => ({ phase, score: 60, weaknessTags: [] })),
      },
    },
    { code: "" },
  ];
  const said = new Set();
  const rendered = [];
  for (const extra of branches) {
    const report = sanitizeReport({ ...base, ...extra });
    const markup = reportMarkup({
      report,
      problemTitle: "Sxtitle",
      language: "Sxlang",
      code: "code" in extra ? extra.code : "Sxcode",
    });
    rendered.push(markup);
    for (const word of markup.replace(/<[^>]*>/g, " ").split(/\s+/)) {
      if (word && !sentinels.includes(word)) said.add(word);
    }
  }

  // Every section the card can draw appeared at least once, and the list of
  // sections is read out of the card rather than typed here. Typing it was the
  // previous version and it was the same defect one layer up: the list named six
  // sections, nothing checked that it named them all, and the section holding
  // the calibration note fell through both it and the vocabulary set. A list
  // derived from the source cannot be short of the source.
  // `h3` only, and literal `h3` at that. The card's `<section>` titles all use
  // one; its `h2` is the verdict and its `h4`s are the two feedback lists, none
  // of which is a section, and the `h2`s further down the file belong to the
  // problem statement rather than to this card. What the pattern cannot see is a
  // heading built from an interpolation, and the card has two: the summary
  // section, whose two arms are named in the sentence list below, and the two
  // feedback sections, whose titles are the words "Coding" and "Communication"
  // in the vocabulary set instead.
  const headings = [...read("web/render.js").matchAll(/<h3[^>]*>([^<{]+)<\/h3>/g)].map(
    (match) => match[1],
  );
  assert.ok(headings.length >= 4, `only ${headings.length} headings found in web/render.js`);
  for (const section of headings) {
    assert.ok(
      rendered.some((markup) => markup.includes(section)),
      `no branch rendered the ${section} section, so its words are not in this set`,
    );
  }
  // And the sentences that are not under a heading of their own. Each is
  // first-party copy on a branch that has to be reached deliberately, and the
  // branch list above is where reaching them is arranged.
  //
  // Hand-maintained, and said so rather than claimed to be derived. The heading
  // list above is read out of the source and cannot be short of it; this one
  // cannot be, because the copy it names lives inside template interpolations
  // that a source scan does not recover. What the two together give is a
  // tripwire, not a proof, which is what the header says.
  for (const sentence of [
    "Committee summary",
    "What happened",
    "No evaluation",
    "formative coaching signals",
    "Contract bundle",
    "Legacy/unversioned contract",
    "cannot be scored by this version",
    "Chain verified through event",
    "every event it verified is listed above",
    "were verified and not kept, for space",
    "predates chain reporting",
    "source event",
    "(editor was empty)",
    "Practice interview report",
  ]) {
    assert.ok(
      rendered.some((markup) => markup.includes(sentence)),
      `no branch rendered "${sentence}", so its words are not in this set`,
    );
  }

  // The valid-contract branch has to carry the contract this build calls active,
  // or `sanitizeReport` marks it unsupported and the branch collapses into the
  // invalid one below it. `activeContract` is function-local and cannot be read
  // from here, so the branch is checked by what it renders instead: a card that
  // names its bundle and does not say it cannot be scored. Stating the
  // precondition in a comment and not checking it is how the branch list went
  // wrong the last two times.
  assert.ok(
    rendered.some(
      (markup) => markup.includes("Contract bundle") && !markup.includes("cannot be scored"),
    ),
    "no branch rendered a supported contract, so the active contract has moved",
  );
  // Numbers, enum values and phase names are here because the card renders them
  // verbatim: they are the schema's words, the same category as `editor` on the
  // timeline, and listing them means one renamed to something that reads as a
  // judgement fails here.
  assert.deepEqual(
    said,
    new Set([
      "(.md)", "(Sxlang)", "(editor", "(none", "-", "/", "0", "01:05", "1", "10",
      "100", "2", "2;", "3", "37", "4", "7", "70", "8", "95%", "Chain",
      "CodeTrial.", "Coding", "Committee", "Communication", "Contract", "Done",
      "Download", "Evidence", "FACE_MISSING", "Framework", "HIRE", "INCOMPLETE",
      "Improve", "Integrity", "Interview", "Interviewer", "Legacy/unversioned",
      "NO", "No", "Practice", "REACTO/STAR", "Strengths", "Success:", "Test",
      "This", "What", "Your", "above.", "algorithm", "an", "and", "are", "back",
      "be", "behavioral", "bundle", "by", "calibrated", "candidate_speech",
      "cannot", "captured)", "chain", "coaching", "code", "coding",
      "communication", "complete", "confidence", "contract", "dropped", "during",
      "empty)", "evaluation", "event", "every", "evidence", "evidence.", "final",
      "for", "formative", "happened", "high", "hints", "hiring", "how", "impact",
      "interview", "is", "it", "kept,", "listed", "lobby", "malformed", "min",
      "more", "much", "next", "not", "observed", "of", "only", "or", "packet",
      "performance", "phase", "predates", "report", "reporting:", "rounds",
      "rubric", "schema", "scored", "scores", "session", "signals,", "source",
      "space", "space.", "started", "summary", "the", "this", "through", "to",
      "unknown.", "unsupported", "used", "uses", "v1", "verified", "version",
      "warning", "was", "were", "\u00b7",
    ]),
    "a word on the report card is a word somebody chose",
  );
});

test("a missing id is caught, except for the ids nothing drives", async () => {
  // Measured rather than described. `querySelector` answering `null` for an
  // undeclared id is only worth having if something then fails, and which ids
  // that covers depends on where each node is first dereferenced: at import, in
  // `render`, inside `showMoment`, or in a function no test calls. Four attempts
  // to write that down in a comment produced four wrong sentences, so it is an
  // assertion now.
  //
  // The five below are written only on the fetch-driven paths, which this file
  // does not drive, so removing one from the page is invisible here and fatal in
  // a browser. That is the same gap the header declares, with the ids measured
  // rather than named by hand.
  //
  // Which functions those are is deliberately not listed. Naming them is what
  // this comment did a moment ago, and the list was wrong within the hour:
  // `#replay-media` is written by `loadEvents` as well as by `select`. The set
  // below is measured and the functions behind it are not, because a fact
  // nothing checks is a fact that goes stale.
  const markup = read("web/replay.html");
  const declared = captures(markup, /id="([^"]+)"/g);
  assert.ok(declared.length >= 10, `only ${declared.length} ids declared`);
  const missed = [];
  for (const id of declared) {
    const outcome = await importWithout(markup, "/replay.js", id, ({ render: renderAgain }) =>
      renderAgain(REPLAYS["a closed window with an answer"]),
    );
    if (!outcome.caught) missed.push(id);
  }
  assert.deepEqual(
    new Set(missed),
    new Set(["replay-list", "replay-empty", "replay-title", "replay-media", "replay-report"]),
  );

  // The loop above installs a fetch that never settles, once per id. A test that
  // awaited that stub would not fail, it would hang, and the run would time out
  // in whichever file happened to come next. So the undo is asserted here rather
  // than trusted: the stub is gone by the time this line runs.
  const answered = await Promise.race([
    Promise.resolve(globalThis.fetch("/anything")).then(
      () => "answered",
      () => "answered",
    ),
    new Promise((resolve) => setTimeout(() => resolve("hung"), 200)),
  ]);
  assert.equal(answered, "answered", "importWithout left a fetch that never settles");
});

test("exactly one moment button is current", () => {
  dom.node("replay-timeline").replaceChildren();
  render(REPLAYS["several answered windows and an editor between them"]);
  const buttons = dom.node("replay-timeline").querySelectorAll("[data-moment]");
  const current = () => buttons.filter((button) => button.current);

  // The last moment is opened when the page renders, before anything is pressed.
  assert.deepEqual(
    current().map((button) => button.dataset.moment),
    [String(Math.max(...buttons.map((b) => Number(b.dataset.moment ?? -1))))],
  );

  for (const button of buttons) {
    button.click();
    assert.deepEqual(
      current(),
      [button],
      `pressing ${JSON.stringify(button.dataset)} left ${current().length} buttons current`,
    );
  }
  assert.equal(dom.node("replay-timeline").querySelectorAll("[data-window]")[0].tag, "div");
});

test("a response window does not replace the final editor with missing history", () => {
  dom.node("replay-timeline").replaceChildren();
  render([
    avatar(BASE, "speaking"),
    avatar(BASE + 1000, "listening"),
    avatar(BASE + 2000, "speaking"),
    editor(BASE + 3000),
  ]);
  const code = dom.node("replay-code").textContent;
  dom.node("replay-timeline").querySelectorAll("[data-window]")[0].click();
  assert.equal(dom.node("replay-code").textContent, code);
});
