// The view layer, kept free of module-scope DOM lookups so `node --test` can
// exercise it with a stub document instead of a real browser. Markup builders
// return strings; the transcript view takes the document and panel it renders
// into. interview.js owns the wiring, this owns the output.

import {
  FRAMEWORKS,
  escapeHtml,
  formatTime,
  frameworkChecklist,
  loopLabel,
  modeIsWhiteboard,
  modeLabel,
  orPlaceholder,
  surfaceLabel,
} from "./lib.js";

/// The six coding steps as a whiteboard candidate was shown them, in REACTO
/// order, so index `i` here is index `i` of `FRAMEWORKS.coding.steps`.
const BOARD_STEPS = frameworkChecklist("coding", [], "whiteboard").steps;

/// Whether a report is of a whiteboard interview.
///
/// The report's own field where it recorded one. A board handed in by the page
/// says so too, because the offline summary is built on that page before any
/// report exists to carry a mode.
function heldAtBoard(report, board) {
  return modeIsWhiteboard(report.interviewMode) || board !== undefined;
}

/// What a phase is called on this report.
///
/// The report keeps the REACTO vocabulary: the grader scores and plans against
/// it, and evidence rows carry its ids. At a whiteboard the candidate was shown
/// other names for four of the six steps, and a report calling step four
/// Coding names a step they never took. The report spells a phase as an id in
/// evidence and as a label in scores and plans, so either maps.
function phaseName(phase, atBoard) {
  if (!atBoard) return phase;
  const index = FRAMEWORKS.coding.steps.findIndex(
    (step) => step.id === String(phase).toLowerCase(),
  );
  return index < 0 ? phase : BOARD_STEPS[index].label;
}

/// A board image this card may put in a `src`.
///
/// The images come from the page's own canvas rather than from anything a
/// server sent, and they are checked all the same: this shape is the only
/// thing that may go in one.
function boardImage(value) {
  return (
    typeof value === "string" &&
    /^data:image\/(?:jpeg|png);base64,[A-Za-z0-9+/=]+$/.test(value)
  );
}

export function runnerStatusMarkup(status) {
  const text = {
    booting: "Starting the Python runtime...",
    compiling: "Compiling with Compiler Explorer...",
    running: "Running test cases...",
    done: "Run finished.",
  }[status];
  return text
    ? `<p class="muted small" data-runner-status="${escapeHtml(status)}">${escapeHtml(text)}</p>`
    : "";
}

export function finalRunnerStatus(summary, currentStatus) {
  return summary.setupError ? currentStatus : "done";
}

export function resultsMarkup(summary, status = null) {
  const statusMarkup = runnerStatusMarkup(status);
  if (summary.setupError) {
    return {
      label: "Test results",
      body: `${statusMarkup}<p class="critical small"><strong>Couldn't run your code</strong></p><pre>${escapeHtml(summary.setupError)}</pre>`,
    };
  }
  const caseMarkup = (item) => {
    const passed = item.pass === true;
    // `pass === null` is a candidate case with no expectation, so it is an
    // output to read rather than a result to judge. Named once: the three
    // places that used to re-derive it had to agree on what null meant.
    const observed = item.pass === null;
    const input =
      item.input === undefined ? "" : `input ${escapeHtml(item.input)}\n`;
    // Only a case that did not pass shows it, and most cases pass.
    const detail = passed
      ? ""
      : item.error
        ? `${input}${escapeHtml(item.error)}`
        : observed
          ? `${input}got ${escapeHtml(item.got)}`
          : `${input}expected ${escapeHtml(item.expected)}\ngot ${escapeHtml(item.got)}`;
    return `
      <li>
        <div><span class="${passed ? "good" : "critical"}">${observed ? "OUTPUT" : passed ? "OK" : "FAIL"}</span> ${escapeHtml(item.label)} <span>${escapeHtml(item.timeMs)}ms</span></div>
        ${passed ? "" : `<pre>${detail}</pre>`}
      </li>
    `;
  };
  const judgeCases = summary.cases
    .filter((item) => !item.candidate)
    .map(caseMarkup)
    .join("");
  const candidateCases = summary.cases
    .filter((item) => item.candidate)
    .map(caseMarkup)
    .join("");
  return {
    label: `Test results · ${summary.passed}/${summary.total}`,
    body: `
    ${statusMarkup}
    <p class="${summary.passed === summary.total ? "good" : "critical"} small"><strong>${summary.passed}/${summary.total} test cases passed</strong></p>
    <ul class="result-list">${judgeCases}</ul>
    ${candidateCases ? `<h3>Your cases</h3><ul class="result-list">${candidateCases}</ul>` : ""}
  `,
  };
}

export function feedbackMarkup(title, section) {
  const list = (items) =>
    orPlaceholder(items)
      .map((item) => `<li>${escapeHtml(item)}</li>`)
      .join("");
  return `<section><h3>${escapeHtml(title)}</h3><h4>Strengths</h4><ul>${list(section.strengths)}</ul><h4>Improve</h4><ul>${list(section.improvements)}</ul></section>`;
}

/// How one integrity event reads: its label, and the detail line under it.
///
/// Both together, because an event whose reason is already in the label must
/// not repeat it underneath. Deciding that here rather than in each renderer is
/// what keeps the page and the exported markdown saying the same thing.
function integrityEventText(event) {
  if (event.type === "CAMERA_NOT_USED") {
    return {
      label: `Camera not used (${event.detail || "declined"})`,
      detail: null,
    };
  }
  return { label: event.type, detail: event.detail || null };
}

function integrityEvidenceMarkup(report = {}) {
  const rows =
    (report.integrityEvents || [])
      .map((event, index) => {
        const { label, detail } = integrityEventText(event);
        return `
    <li data-integrity-index="${index}"><strong>${escapeHtml(event.severity)}</strong> ${escapeHtml(label)} <span>${escapeHtml(event.at)}</span>${detail ? `<p>${escapeHtml(detail)}</p>` : ""}${sourceEventMarkup(event)}</li>
  `;
      })
      .join("") || "<li>(none captured)</li>";
  return `<section><h3>Integrity Evidence</h3><ul>${rows}</ul><p class="muted small">${escapeHtml(chainSentence(report))}</p></section>`;
}

/// Says out loud that the list above is a subsequence.
///
/// The agent keeps 25 events and verifies every one that arrives, so a busy
/// interview shows a fraction of what was checked. Without this a reader cannot
/// tell a retention gap from a deleted row, which is the difference the chain
/// exists to make visible. A report predating the checkpoint has no numbers and
/// says so rather than implying nothing was dropped.
/// One sentence, so the page and the exported markdown cannot drift apart.
///
/// They were written twice and already disagreed: one said "This report predates
/// chain reporting" and the other "Chain reporting predates this report". The
/// export is the copy that gets forwarded, and two wordings of the same fact in
/// one file is a promise that they will diverge further.
export function chainSentence(report) {
  if (report.integrityChainSeq == null) {
    return "This report predates chain reporting: how much was dropped for space is unknown.";
  }
  const dropped = report.integrityDropped ?? 0;
  return `Chain verified through event ${report.integrityChainSeq}; ${
    dropped === 0
      ? "every event it verified is listed above"
      : `${dropped} more were verified and not kept, for space`
  }.`;
}

function sourceEventMarkup(event) {
  if (!event.sourceEventIds?.length) return "";
  return `<ul>${event.sourceEventIds.map((id) => `<li>source event ${escapeHtml(id)}</li>`).join("")}</ul>`;
}

// The problem panel, here rather than inline in `interview.js` for the reason
// every other builder in this file is here: a markup string assembled beside a
// DOM lookup cannot be handed to `node --test`, so the one guarantee that
// matters about it, that nothing a problem carries becomes live markup, had no
// test while every sibling renderer did.
//
// No constraints section and no source statement: the page carries the scenario
// the interview poses, and the limits and edge-case policies are what the
// candidate asks Jim for, the way they would ask a person. The published title
// is named once, small, so the problem can be found again after the interview.
// The worked examples can be left out, so the Example step starts from cases
// the candidate proposes instead of ones already on screen.
export function problemMarkup(problem, { examples = true } = {}) {
  const example = (example, index) => `
          <section>
            <h2>Example ${index + 1}</h2>
            <pre><span>Input: </span>${escapeHtml(example.input)}
<span>Output: </span>${escapeHtml(example.output)}${
    example.explanation
      ? `
<span>Explanation: </span>${escapeHtml(example.explanation)}`
      : ""
  }</pre>
          </section>
        `;
  return `
    <div class="problem-detail">
      <p class="interview-kicker">Interview exercise</p>
      ${problem.source ? `<p class="problem-source">LeetCode: ${escapeHtml(problem.source)}</p>` : ""}
      ${problem.requestedPage ? `<p class="muted small">The link asked for an exercise this bank does not have, so this is the default exercise.</p>` : ""}
      ${problem.brief.map((text) => `<p>${escapeHtml(text)}</p>`).join("")}
      ${
        examples
          ? `<div class="examples">
        ${problem.examples.map(example).join("")}
      </div>`
          : ""
      }
      <div class="hint-box"><strong>Think out loud.</strong> Jim is listening to your voice and reading your editor in real time. Ask him about input sizes, edge cases and anything the description leaves open, narrate your approach like you would with a human interviewer, and say "can I get a hint?" if you need one.</div>
    </div>
  `;
}

export function reportSaveStatus(result) {
  if (result === null)
    return { message: "Saving report...", className: "muted small" };
  if (result.account === "saved")
    return { message: "Saved to your account", className: "muted small" };
  if (result.local === "saved")
    return { message: "Saved on this device only", className: "muted small" };
  return { message: "Report was not saved", className: "critical small" };
}

/// Only the verdict and the scores differ between an evaluated session and one
/// that produced nothing. Forking the whole card duplicated the header, the
/// evidence section, the code block and the actions row, including the
/// `download-report` and `done` ids that `web/interview.js` binds listeners to.
/// Every expression in this card that emits `/ 100` sits behind the same
/// guard, so "no scores in an incomplete report" is structural rather than
/// something a test has to catch after the fact.
export function reportMarkup({
  report,
  problemTitle,
  language,
  code,
  board,
  boardPhases,
  exampleDrawing,
  saveResult,
}) {
  const atBoard = heldAtBoard(report, board);
  // At a board the coding score is for the drawing, the trace and the cases
  // the candidate named against it, so it is not called Coding there.
  const workTitle = atBoard ? "Board work" : "Coding";
  const hire = report.decision === "HIRE";
  const heading = report.incomplete
    ? "No evaluation"
    : "Your performance packet";
  const practiceLevel = report.incomplete
    ? ""
    : report.practiceLevel
      ? `Judged against a mid-level bar; practiced for: ${escapeHtml(report.practiceLevel)}`
      : "Judged against a mid-level bar";
  const badge = report.incomplete
    ? `<strong class="muted">INCOMPLETE</strong>`
    : `<strong class="${hire ? "good" : "critical"}">${hire ? "HIRE" : "NO HIRE"}</strong>`;
  const scores = report.incomplete
    ? ""
    : `
      <div class="score-grid">
        <div><p>${workTitle}</p><strong>${escapeHtml(report.codingScore)}<span> / 100</span></strong></div>
        <div><p>Interviewer communication</p><strong>${escapeHtml(report.communicationScore)}<span> / 100</span></strong></div>
      </div>
      <p><strong>${escapeHtml(report.hintsUsed)}</strong> hints used during the session</p>`;
  const feedback = report.incomplete
    ? ""
    : `${feedbackMarkup(workTitle, report.codingFeedback)}${feedbackMarkup("Communication", report.communicationFeedback)}`;
  const practiceNext =
    report.incomplete || !report.improvementPlan?.length
      ? ""
      : `<section><h3>Practice next</h3><ol>${report.improvementPlan
          .map(
            (item) => `
      <li><strong>${escapeHtml(phaseName(item.phase, atBoard))} · ${escapeHtml(item.durationMin)} min · ${escapeHtml(item.impact)} impact</strong>
        <p>${escapeHtml(item.drill)}</p>
        <p><strong>Success:</strong> ${escapeHtml(item.successCriterion)}</p>
        <ul>${item.selfReview.map((check) => `<li>${escapeHtml(check)}</li>`).join("")}</ul>
      </li>`,
          )
          .join("")}</ol></section>`;
  const debrief = report.debrief
    ? `<details class="report-debrief"><summary>What the interviewer held back</summary>
      <p>Spaced review will bring this problem back, so your next attempt tests recall.</p>
      ${report.debrief.scenarioContract ? `<p><strong>Scenario contract:</strong> ${escapeHtml(report.debrief.scenarioContract)}</p>` : ""}
      ${report.debrief.approach ? `<p><strong>Approach and complexity:</strong> ${escapeHtml(report.debrief.approach)}</p>` : ""}
      ${report.debrief.pitfalls ? `<p><strong>Common pitfalls:</strong> ${escapeHtml(report.debrief.pitfalls)}</p>` : ""}
      ${report.debrief.hints?.length ? `<h3>Hint ladder</h3><p>Reached hint ${report.debrief.hints.filter((hint) => hint.given).length} of ${report.debrief.hints.length}.</p><ol>${report.debrief.hints.map((hint) => `<li><strong>${hint.given ? "Given" : "Held back"}:</strong> ${escapeHtml(hint.text)}</li>`).join("")}</ol>` : ""}
      ${report.debrief.followUps?.length ? `<h3>Follow-ups this problem offers</h3><ul>${report.debrief.followUps.map((followUp) => `<li>${escapeHtml(followUp)}</li>`).join("")}</ul>` : ""}
    </details>`
    : "";
  const frameworkTimeline = frameworkEvidenceMarkup(
    report.frameworkEvidence,
    atBoard,
  );
  const phaseTable = (group) => {
    const rows = group.rows.map(
      (row) =>
        `<tr><th scope="row">${row.label}</th><td>${phaseScoreText(row.score, escapeHtml)}</td></tr>`,
    );
    return `<table class="phase-scores"><caption>${group.name}</caption><tbody>${rows.join("")}</tbody></table>`;
  };
  // At a board the coding scores sit beside the board they were earned on, in
  // the timeline below the summary, so only a behavioral table is left here.
  const phaseGroups = phaseScoreGroups(report, atBoard).filter(
    (group) => !atBoard || group.kind !== "coding",
  );
  const phaseScores = !phaseGroups.length
    ? ""
    : `<section><h3>Phase scores</h3>${phaseGroups.map(phaseTable).join("")}<p class="muted small">${PHASE_SCORE_NOTE}</p></section>`;
  // Only where the report recorded one, like the mode beside it in the header.
  const loop = report.interviewLoop
    ? ` · ${loopLabel(report.interviewLoop)}`
    : "";
  const surface = report.interviewMode
    ? ` · ${surfaceLabel(report.interviewMode)}`
    : "";
  const rounds = report.rounds?.length
    ? `<section><h3>Interview rounds</h3><ul>${report.rounds.map((round) => `<li>${escapeHtml(round.kind)} · ${escapeHtml(round.budgetMin)} min · ${escapeHtml(round.status)}</li>`).join("")}</ul></section>`
    : "";
  const executionNote =
    report.codeExecution === false
      ? '<p class="muted small">Code execution was disabled for this interview.</p>'
      : "";
  const contract = report.interviewContract
    ? `Contract bundle ${report.interviewContract.bundleVersion} · rubric ${report.interviewContract.rubricVersion} · report schema ${report.interviewContract.reportSchemaVersion}`
    : "Legacy/unversioned contract";
  // Replay renders reports it did not create, so only the interview caller
  // supplies this field and mounts a live region for the pending save.
  const saveStatus =
    saveResult === undefined ? null : reportSaveStatus(saveResult);

  return `
    <div class="report-card">
      <div class="report-header">
        <div><p>${report.mode ? `${modeLabel(report.mode)} ` : ""}interview report${loop}${surface} · ${escapeHtml(problemTitle)}</p><p class="muted small">${escapeHtml(contract)}</p>${practiceLevel ? `<p class="muted small">${practiceLevel}</p>` : ""}<h2>${heading}</h2></div>
        ${badge}
      </div>${scores}
      <section><h3>${report.incomplete ? "What happened" : "Committee summary"}</h3><p>${escapeHtml(report.summary)}</p></section>
      ${atBoard ? boardTimelineMarkup(report, board, boardPhases) : ""}
      ${feedback}
      ${practiceNext}
      ${debrief}
      ${rounds}${executionNote}
      ${phaseScores}
      ${frameworkTimeline}
      ${integrityEvidenceMarkup(report)}
      ${atBoard ? "" : finalCodeMarkup(language, code)}
      ${!atBoard && boardImage(exampleDrawing) ? `<figure class="board-final"><figcaption>Example drawing</figcaption><p class="muted small">Saved from your interview. Example drawings do not affect coding or communication scores.</p><img class="report-board" src="${exampleDrawing}" alt="Your final example drawing"></figure>` : ""}
      ${saveStatus ? `<p id="report-save-status" class="${saveStatus.className}" role="status">${saveStatus.message}</p>` : ""}
      <div class="report-actions"><button id="download-report" type="button">Download report (.md)</button><button id="done" type="button"${saveResult === null ? " disabled" : ""}>Done - back to lobby</button></div>
    </div>
  `;
}

/// The candidate's code, at the foot of an editor interview's card.
function finalCodeMarkup(language, code) {
  return `<details><summary>Your final code (${escapeHtml(language)})</summary><pre>${escapeHtml(code.trimEnd() || "(editor was empty)")}</pre></details>`;
}

/// A whiteboard interview's work, step by step: the board when each step was
/// completed, what that step scored, and what the interviewer wrote down
/// about it, then the board as the candidate left it.
///
/// Placed under the summary rather than at the foot of the card, where an
/// editor interview keeps its code: the board is the work, and the scores
/// mean little apart from it. A step with nothing to show says so instead of
/// disappearing, because a gap in a six-step list is something to notice.
///
/// The step images are the page's own copies of the checkpoints it sent, and
/// nothing saved carries them, so a report opened from history or replay has
/// the scores and the evidence and says the pictures are not there.
///
/// A step the interviewer recorded as skipped says so. Left out with the
/// observations, it read as "no checkpoint was recorded", which is a different
/// thing: nothing happened, rather than the step being closed on purpose.
function boardTimelineMarkup(report, board, boardPhases) {
  const coding = phaseScoreGroups(report, true).find(
    (group) => group.kind === "coding",
  );
  const images = new Map(
    (Array.isArray(boardPhases) ? boardPhases : []).filter(
      (entry) => Array.isArray(entry) && boardImage(entry[1]),
    ),
  );
  const steps = BOARD_STEPS.map((step, index) => {
    const score = coding?.rows[index].score ?? null;
    const image = images.get(step.id);
    const notes = (report.frameworkEvidence || [])
      .filter((item) => item.phase === step.id)
      .map((item) =>
        item.kind === "skipped"
          ? `<p class="muted small">Skipped: ${escapeHtml(item.summary)}</p>`
          : `<p>${escapeHtml(item.summary)}</p>`,
      );
    const body = `${image ? `<img class="report-board" src="${image}" alt="The board when ${step.label} was completed">` : ""}${notes.join("")}`;
    return `<li class="board-step${body ? "" : " missing"}"><p class="board-step-head"><strong>${index + 1}. ${step.label}</strong>${score === null ? "" : `<span>${phaseScoreText(score, escapeHtml)}</span>`}</p>${body || `<p class="muted small">No checkpoint was recorded for this step.</p>`}</li>`;
  });
  const final =
    board === undefined
      ? `<p class="muted small">Board images are not saved with the report. If this interview was recorded, its replay redraws the board stroke by stroke.</p>`
      : boardImage(board)
        ? `<figure class="board-final"><figcaption>Your final board</figcaption><img class="report-board" src="${board}" alt="The whiteboard as you left it"></figure>`
        : `<p>Your final board</p><p class="muted small">The board could not be read back.</p>`;
  return `<section class="board-timeline"><h3>Your board, step by step</h3><ol>${steps.join("")}</ol>${final}${coding ? `<p class="muted small">${PHASE_SCORE_NOTE}</p>` : ""}</section>`;
}

/// The board timeline, in the exported document.
///
/// The steps and what was said about each, without the pictures. A markdown
/// file with a hundred kilobytes of base64 per step is a file nothing renders
/// and no reader can scroll past. It outlives the card that showed them and
/// a recording may not exist, so it says the pictures are not in it rather
/// than where they are.
function boardStepsMarkdown(report, mdText) {
  const notes = (id) =>
    (report.frameworkEvidence || [])
      .filter((item) => item.phase === id)
      .map(
        (item) =>
          `   - ${item.kind === "skipped" ? "Skipped: " : ""}${mdText(item.summary)}`,
      );
  return [
    "## Your board, step by step",
    "",
    ...BOARD_STEPS.flatMap((step, index) => [
      `${index + 1}. **${step.label}**`,
      ...notes(step.id),
    ]),
    "",
    "The board images are not part of this file. If this interview was recorded, its replay redraws the board stroke by stroke.",
    "",
  ];
}

/// The downloadable report. Pure so the export can be tested without a DOM;
/// `at` is injected because a timestamp would otherwise make it unassertable.
export function reportMarkdown({
  report,
  problemTitle,
  language,
  code,
  board,
  transcript,
  at,
}) {
  const atBoard = heldAtBoard(report, board);
  const workTitle = atBoard ? "Board work" : "Coding";
  // One rule for every untrusted string in this document, where there used to
  // be three. Evidence rows ran through escapeHtml, feedback bullets and
  // transcript turns ran through nothing. escapeHtml was the wrong escaper for
  // a file nobody renders as HTML: it turned a candidate's `<` into `&lt;` and
  // left the characters that actually break Markdown structure alone.
  //
  // The escaped set is structure and safety, not cosmetics. `|` ends a table
  // cell and a backtick opens a code span. `<` is the one this file used to get
  // right by accident: the old evidence rows ran through escapeHtml, so an
  // interviewer model emitting `<img src=x onerror=...>` came out inert, and
  // collapsing three escapers into one briefly handed that back. It also closes
  // autolinks. `[` closes inline links, images, and reference definitions, so
  // `![pixel](http://tracker)` in feedback cannot phone home from a rendered
  // report. Emphasis characters are deliberately left alone mid-line: `*` and
  // `_` can only make text italic there, and escaping every cosmetic character
  // makes the raw .md unreadable, which is the form a human actually opens.
  //
  // At line start they are structure, not decoration, so the leading rule below
  // takes a wider set: `~~~` opens a fence the backtick rule never sees, `___`
  // is a thematic break, and `=` would underline the line above into a heading.
  // A `~~~` in one summary rendered every section after it as one code block.
  //
  // Backslash first, or the escapes get escaped. All whitespace collapses to
  // single spaces because every use below is one line of a table row, a list
  // item, or an attributed transcript turn, and a newline in any of them ends
  // the row and starts something else. Then trim, because Markdown reads up to
  // three leading spaces as still being part of the construct that follows: an
  // earlier version collapsed only newlines, so `   ## Verdict: HIRE` kept its
  // indent, the leading-character rule below never saw the `#`, and feedback
  // text could still write a heading. The leading-character rule runs last,
  // once trimming has decided what the line actually starts with.
  const mdText = (value) =>
    String(value ?? "")
      .replace(/\\/g, "\\\\")
      .replace(/([|`<\[])/g, "\\$1")
      .replace(/\s+/g, " ")
      .trim()
      .replace(/^([#>*+=~_-])/, "\\$1")
      // The delimiter, not the digits: a digit is not escapable punctuation, so
      // `\3.` rendered a literal backslash and corrupted numbered feedback, which
      // is the shape a grader writes in. `)` is an ordered-list delimiter too.
      .replace(/^(\d+)([.)])/, "$1\\$2");
  const bullets = (items) =>
    orPlaceholder(items)
      .map((item) => `- ${mdText(item)}`)
      .join("\n");
  const section = (title, feedbackSection) =>
    `### ${title}\n\n**Strengths**\n${bullets(feedbackSection.strengths)}\n\n**Improvements**\n${bullets(feedbackSection.improvements)}\n`;
  const evidence =
    (report.integrityEvents || [])
      .map((event) => {
        const sources = event.sourceEventIds?.length
          ? ` - sources: ${event.sourceEventIds.map(mdText).join(", ")}`
          : "";
        const { label, detail } = integrityEventText(event);
        return `- ${mdText(event.at)} [${mdText(event.severity)}] ${mdText(label)}${detail ? ` - ${mdText(detail)}` : ""}${sources}`;
      })
      .join("\n") || "(none captured)";
  const practiceNext =
    report.incomplete || !report.improvementPlan?.length
      ? []
      : [
          "## Practice next",
          "",
          ...report.improvementPlan.flatMap((item, index) => [
            `${index + 1}. **${mdText(phaseName(item.phase, atBoard))} · ${mdText(item.durationMin)} min · ${mdText(item.impact)} impact**`,
            `   - Drill: ${mdText(item.drill)}`,
            `   - Success: ${mdText(item.successCriterion)}`,
            ...item.selfReview.map((check) => `   - Check: ${mdText(check)}`),
          ]),
          "",
        ];
  const debrief = report.debrief
    ? [
        "## What the interviewer held back",
        "",
        "Spaced review will bring this problem back, so your next attempt tests recall.",
        ...(report.debrief.scenarioContract
          ? [
              `**Scenario contract:** ${mdText(report.debrief.scenarioContract)}`,
            ]
          : []),
        ...(report.debrief.approach
          ? [`**Approach and complexity:** ${mdText(report.debrief.approach)}`]
          : []),
        ...(report.debrief.pitfalls
          ? [`**Common pitfalls:** ${mdText(report.debrief.pitfalls)}`]
          : []),
        ...(report.debrief.hints?.length
          ? [
              "",
              "### Hint ladder",
              "",
              `Reached hint ${report.debrief.hints.filter((hint) => hint.given).length} of ${report.debrief.hints.length}.`,
              "",
              ...report.debrief.hints.map(
                (hint) =>
                  `- **${hint.given ? "Given" : "Held back"}:** ${mdText(hint.text)}`,
              ),
            ]
          : []),
        ...(report.debrief.followUps?.length
          ? [
              "",
              "### Follow-ups this problem offers",
              "",
              ...report.debrief.followUps.map(
                (followUp) => `- ${mdText(followUp)}`,
              ),
            ]
          : []),
        "",
      ]
    : [];
  const phaseGroups = phaseScoreGroups(report, atBoard);
  const phaseScores = !phaseGroups.length
    ? []
    : [
        "## Phase scores",
        "",
        ...phaseGroups.flatMap((group) => [
          `| ${group.name} | Score |`,
          "|---|---|",
          ...group.rows.map(
            (row) => `| ${row.label} | ${phaseScoreText(row.score, mdText)} |`,
          ),
          "",
        ]),
        PHASE_SCORE_NOTE,
        "",
      ];
  const frameworkTimeline = report.frameworkEvidence?.length
    ? [
        "## Framework evidence",
        "",
        ...report.frameworkEvidence.map(
          (item) =>
            `- ${frameworkTime(item.atMs)} · **${mdText(phaseName(item.phase, atBoard))}** · ${mdText(item.kind)} · ${mdText(item.source)} · ${mdText(item.confidence)}% · v${mdText(item.frameworkVersion)} — ${mdText(item.summary)}`,
        ),
        "",
      ]
    : [];
  const chainNote = mdText(chainSentence(report));
  // Missing storage is not evidence of silence: only a saved, empty transcript
  // can say that no speech was captured. Omitted fields have the same meaning.
  const conversation =
    transcript == null
      ? "(transcript was not saved)"
      : transcript
          .filter((segment) => segment.final || segment.text.trim())
          .map(
            (segment) =>
              `**${segment.speaker === "interviewer" ? "Jim" : "You"}:** ${mdText(segment.text.trim())}`,
          )
          .join("\n\n");
  // Candidate code is the one field here that must not be escaped, so when it
  // contains a run of backticks the fence is what has to give. Without this, a
  // ``` in a comment closes the block early and the rest of the report is read
  // as prose.
  //
  // Folded rather than spread into `Math.max`: one argument per backtick run is
  // one argument per two characters of pasted code, and past roughly a hundred
  // thousand runs the call blows the argument limit and throws a RangeError, so
  // the candidate's own download button would do nothing.
  const body =
    code == null
      ? "(final code was not saved)"
      : code.trimEnd() || "(editor was empty)";
  const longestRun = (body.match(/`+/g) || []).reduce(
    (longest, run) => Math.max(longest, run.length),
    0,
  );
  const fence = "`".repeat(Math.max(2, longestRun) + 1);
  // The info string is the one place this document emits a value unescaped, so
  // it takes the only characters a language tag can be. Today `language` comes
  // from the tabs and is allowlisted on both sides, but a newline here would
  // end the fence line and start emitting the candidate's code as prose.
  const info = String(language ?? "").replace(/[^a-zA-Z0-9_+-]/g, "");
  // The exported copy is the one that outlives the tab and can be forwarded, so
  // it must not carry a verdict the session never earned either. Only the head
  // differs: the tail used to be duplicated per branch and had already drifted
  // four ways, including an untagged code fence and two different names for the
  // same section.
  const head = report.incomplete
    ? ["## No evaluation", "", mdText(report.summary) || "(none)"]
    : [
        `## Verdict: ${report.decision === "HIRE" ? "HIRE" : "NO HIRE"}`,
        "",
        report.practiceLevel
          ? `Judged against a mid-level bar; practiced for: ${mdText(report.practiceLevel)}`
          : "Judged against a mid-level bar",
        "",
        "| Metric | Score |",
        "|---|---|",
        `| ${workTitle} | ${mdText(report.codingScore)} / 100 |`,
        `| Communication | ${mdText(report.communicationScore)} / 100 |`,
        `| Hints used | ${mdText(report.hintsUsed)} |`,
        "",
        "## Committee summary",
        mdText(report.summary) || "(none)",
        "",
        section(`${workTitle} feedback`, report.codingFeedback),
        section(
          "Communication feedback",
          report.communicationFeedback,
        ).trimEnd(),
      ];

  return [
    `# Interview Report - ${mdText(problemTitle)}`,
    `_${at}_`,
    ...(report.mode ? [`Mode: ${modeLabel(report.mode)}`] : []),
    ...(report.interviewLoop
      ? [`Loop: ${loopLabel(report.interviewLoop)}`]
      : []),
    ...(report.interviewMode
      ? [`Held at: ${surfaceLabel(report.interviewMode)}`]
      : []),
    ...(report.codeExecution === false
      ? ["Code execution was disabled for this interview."]
      : []),
    report.interviewContract
      ? `Contract: bundle ${report.interviewContract.bundleVersion}; live prompt ${report.interviewContract.livePromptVersion}; report prompt ${report.interviewContract.reportPromptVersion}; rubric ${report.interviewContract.rubricVersion}; report schema ${report.interviewContract.reportSchemaVersion}`
      : "Contract: legacy/unversioned",
    ...(report.rounds?.length
      ? [
          "Rounds: " +
            report.rounds
              .map(
                (round) =>
                  `${round.kind} (${round.budgetMin} min, ${round.status})`,
              )
              .join("; "),
        ]
      : []),
    "",
    ...head,
    "",
    ...phaseScores,
    ...practiceNext,
    ...debrief,
    ...frameworkTimeline,
    // A session with no evaluation can still be one that ended because the
    // camera saw something. Refusing to score it is not a reason to drop the
    // evidence, which `sanitizeReport` deliberately keeps.
    "## Integrity Evidence",
    evidence,
    "",
    chainNote,
    "",
    ...(atBoard
      ? boardStepsMarkdown(report, mdText)
      : [
          `## Final code (${mdText(language ?? "not recorded")})`,
          ...(code == null ? [body] : [fence + info, body, fence]),
          "",
        ]),
    "## Conversation transcript",
    conversation || "(no speech captured)",
    "",
  ].join("\n");
}

function frameworkTime(atMs) {
  return formatTime(Math.max(0, Math.floor(Number(atMs) / 1000)));
}

const PHASE_SCORE_NOTE =
  "REACTO/STAR phase scores are formative coaching signals, not calibrated hiring evidence. They are not weighted parts of the Coding or Communication score.";

const phaseScoreText = (score, escape) =>
  score === null ? "Not assessed" : `${escape(score)} / 100`;

/// The per-phase scores the grader already returns, grouped by framework. An
/// all-null framework is shown only when its round ran: STAR is otherwise all
/// null in a session that asked no behavioral question. A round that did not
/// run is one `reportRounds` calls skipped or not configured; naming those two
/// rather than the ones that ran keeps a status added there from hiding scores.
/// Labels come from FRAMEWORKS, not the report, so only the score is untrusted;
/// at a board they are the names the candidate was shown for the same steps.
/// Empty for an incomplete report, and when no framework qualifies, so a caller
/// emits no heading over nothing.
function phaseScoreGroups(report, atBoard = false) {
  if (report.incomplete || !report.frameworkAssessment) return [];
  const scores = new Map(
    report.frameworkAssessment.phases.map((item) => [item.phase, item.score]),
  );
  const ran = new Set(
    (report.rounds || [])
      .filter((round) => !["skipped", "not_configured"].includes(round.status))
      .map((round) => round.kind),
  );
  return Object.entries(FRAMEWORKS)
    .map(([kind, framework]) => {
      const shown = frameworkChecklist(
        kind,
        [],
        atBoard ? "whiteboard" : "coding",
      );
      return {
        kind,
        ran: ran.has(kind),
        name: shown.name,
        rows: framework.steps.map((step, index) => ({
          label: shown.steps[index].label,
          score: scores.get(step.label) ?? null,
        })),
      };
    })
    .filter(
      (group) => group.ran || group.rows.some((row) => row.score !== null),
    );
}

function frameworkEvidenceMarkup(items, atBoard) {
  if (!items?.length) return "";
  return `<section aria-labelledby="framework-evidence-title">
    <h3 id="framework-evidence-title">Framework evidence</h3>
    <ol class="framework-timeline">${items
      .map(
        (item) => `<li>
      <strong>${escapeHtml(phaseName(item.phase, atBoard))}</strong>
      <span>${frameworkTime(item.atMs)} · ${escapeHtml(item.kind)} · ${escapeHtml(item.source)} · ${escapeHtml(item.confidence)}% confidence · v${escapeHtml(item.frameworkVersion)}</span>
      <p>${escapeHtml(item.summary)}</p>
    </li>`,
      )
      .join("")}</ol>
  </section>`;
}

/// Owns transcript segments and their rows together, because the two were
/// previously separate structures keyed by the same id.
///
/// One id is one turn. The agent accumulates the fragments Gemini emits and
/// republishes the whole turn under a stable `lk.segment_id`, so this only has
/// to patch a row in place. It used to reassemble sentences here by guessing
/// from timestamps, which could not see turn boundaries and never reached the
/// copy of the transcript that feeds the report prompt.
export function createTranscriptView(document, panel) {
  const rows = [];
  const rowOf = new Map();

  function render(row) {
    if (!row.body) {
      // The panel ships with an empty-state placeholder; the first real row
      // replaces it.
      if (rows.length === 1) {
        panel.replaceChildren();
      }
      const element = document.createElement("div");
      element.className = `transcript-row ${row.speaker === "interviewer" ? "alex" : "you"}`;
      const inner = document.createElement("div");
      const who = document.createElement("p");
      who.textContent = row.speaker === "interviewer" ? "Jim" : "You";
      row.body = document.createElement("p");
      inner.append(who, row.body);
      element.append(inner);
      panel.append(element);
    }
    // textContent, not innerHTML: transcript text is remote input.
    row.body.textContent = row.final ? row.text : `${row.text}...`;
  }

  return {
    upsert(id, speaker, text, final, at = Date.now()) {
      let row = rowOf.get(id);
      if (!row) {
        row = { speaker, text: "", final, at, body: null };
        rows.push(row);
        rowOf.set(id, row);
      }
      // A late republish of an earlier state must not shorten the row or
      // reopen a turn the agent already closed. Timestamps come from the
      // browser clock and two updates can land in the same millisecond, so
      // a closed turn is also final against anything still calling itself
      // in progress.
      if (at < row.at || (row.final && !final)) return;
      Object.assign(row, { text: text.trim(), final, at });
      render(row);
    },
    get size() {
      return rows.length;
    },
    values() {
      return [...rows];
    },
  };
}
