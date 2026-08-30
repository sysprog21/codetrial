// The view layer, kept free of module-scope DOM lookups so `node --test` can
// exercise it with a stub document instead of a real browser. Markup builders
// return strings; the transcript view takes the document and panel it renders
// into. interview.js owns the wiring, this owns the output.

import { escapeHtml, formatTime, orPlaceholder } from "./lib.js";

export function runnerStatusMarkup(status) {
  const text = {
    booting: "Starting the Python runtime...",
    compiling: "Compiling with Compiler Explorer...",
    running: "Running test cases...",
    done: "Run finished.",
  }[status];
  return text ? `<p class="muted small" data-runner-status="${escapeHtml(status)}">${escapeHtml(text)}</p>` : "";
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
  const cases = summary.cases.map((item) => `
      <li>
        <div><span class="${item.pass ? "good" : "critical"}">${item.pass ? "OK" : "FAIL"}</span> ${escapeHtml(item.label)} <span>${escapeHtml(item.timeMs)}ms</span></div>
        ${item.pass ? "" : `<pre>${item.error ? escapeHtml(item.error) : `expected ${escapeHtml(item.expected)}\ngot ${escapeHtml(item.got)}`}</pre>`}
      </li>
    `).join("");
  return {
    label: `Test results · ${summary.passed}/${summary.total}`,
    body: `
    ${statusMarkup}
    <p class="${summary.passed === summary.total ? "good" : "critical"} small"><strong>${summary.passed}/${summary.total} test cases passed</strong></p>
    <ul class="result-list">${cases}</ul>
  `,
  };
}

export function feedbackMarkup(title, section) {
  const list = (items) => orPlaceholder(items).map((item) => `<li>${escapeHtml(item)}</li>`).join("");
  return `<section><h3>${escapeHtml(title)}</h3><h4>Strengths</h4><ul>${list(section.strengths)}</ul><h4>Improve</h4><ul>${list(section.improvements)}</ul></section>`;
}

function integrityEvidenceMarkup(report = {}) {
  const rows = (report.integrityEvents || []).map((event, index) => `
    <li data-integrity-index="${index}"><strong>${escapeHtml(event.severity)}</strong> ${escapeHtml(event.type)} <span>${escapeHtml(event.at)}</span>${event.detail ? `<p>${escapeHtml(event.detail)}</p>` : ""}${sourceEventMarkup(event)}</li>
  `).join("") || "<li>(none captured)</li>";
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
// test while every sibling renderer did. Problem text is not all first-party:
// `leetcode-import.js` brings statements in from outside.
export function problemMarkup(problem) {
  const example = (example, index) => `
          <section>
            <h2>Example ${index + 1}</h2>
            <pre><span>Input: </span>${escapeHtml(example.input)}
<span>Output: </span>${escapeHtml(example.output)}${example.explanation ? `
<span>Explanation: </span>${escapeHtml(example.explanation)}` : ""}</pre>
          </section>
        `;
  return `
    <div class="problem-detail">
      ${problem.statement.map((text) => `<p>${escapeHtml(text)}</p>`).join("")}
      <div class="examples">
        ${problem.examples.map(example).join("")}
      </div>
      <h2>Constraints</h2>
      <ul>${problem.constraints.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>
      <div class="hint-box"><strong>Think out loud.</strong> Jim is listening to your voice and reading your editor in real time - narrate your approach like you would with a human interviewer, and say "can I get a hint?" if you need one.</div>
    </div>
  `;
}

/// Only the verdict and the scores differ between an evaluated session and one
/// that produced nothing. Forking the whole card duplicated the header, the
/// evidence section, the code block and the actions row, including the
/// `download-report` and `done` ids that `web/interview.js` binds listeners to.
/// Exactly one expression in this file emits `/ 100`, and it sits behind the
/// guard, so "no scores in an incomplete report" is structural rather than
/// something a test has to catch after the fact.
export function reportMarkup({ report, problemTitle, language, code }) {
  const hire = report.decision === "HIRE";
  const heading = report.incomplete ? "No evaluation" : "Your performance packet";
  const badge = report.incomplete
    ? `<strong class="muted">INCOMPLETE</strong>`
    : `<strong class="${hire ? "good" : "critical"}">${hire ? "HIRE" : "NO HIRE"}</strong>`;
  const scores = report.incomplete
    ? ""
    : `
      <div class="score-grid">
        <div><p>Coding</p><strong>${escapeHtml(report.codingScore)}<span> / 100</span></strong></div>
        <div><p>Interviewer communication</p><strong>${escapeHtml(report.communicationScore)}<span> / 100</span></strong></div>
      </div>
      <p><strong>${escapeHtml(report.hintsUsed)}</strong> hints used during the session</p>`;
  const feedback = report.incomplete
    ? ""
    : `${feedbackMarkup("Coding", report.codingFeedback)}${feedbackMarkup("Communication", report.communicationFeedback)}`;
  const practiceNext = report.incomplete || !report.improvementPlan?.length
    ? ""
    : `<section><h3>Practice next</h3><ol>${report.improvementPlan.map((item) => `
      <li><strong>${escapeHtml(item.phase)} · ${escapeHtml(item.durationMin)} min · ${escapeHtml(item.impact)} impact</strong>
        <p>${escapeHtml(item.drill)}</p>
        <p><strong>Success:</strong> ${escapeHtml(item.successCriterion)}</p>
        <ul>${item.selfReview.map((check) => `<li>${escapeHtml(check)}</li>`).join("")}</ul>
      </li>`).join("")}</ol></section>`;
  const frameworkTimeline = frameworkEvidenceMarkup(report.frameworkEvidence);
  const frameworkCalibration = report.frameworkAssessment
    ? `<p class="muted small">REACTO/STAR phase scores are formative coaching signals, not calibrated hiring evidence.</p>`
    : "";
  const loopLabel = report.interviewLoop === "coding_only" ? "Coding only" : "Coding + behavioral";
  const rounds = report.rounds?.length ? `<section><h3>Interview rounds</h3><ul>${report.rounds.map((round) => `<li>${escapeHtml(round.kind)} · ${escapeHtml(round.budgetMin)} min · ${escapeHtml(round.status)}</li>`).join("")}</ul></section>` : "";
  const contract = report.interviewContract
    ? `Contract bundle ${report.interviewContract.bundleVersion} · rubric ${report.interviewContract.rubricVersion} · report schema ${report.interviewContract.reportSchemaVersion}`
    : "Legacy/unversioned contract";

  return `
    <div class="report-card">
      <div class="report-header">
        <div><p>${report.mode === "practice" ? "Practice" : "Scored"} interview report · ${loopLabel} · ${escapeHtml(problemTitle)}</p><p class="muted small">${escapeHtml(contract)}</p><h2>${heading}</h2></div>
        ${badge}
      </div>${scores}
      <section><h3>${report.incomplete ? "What happened" : "Committee summary"}</h3><p>${escapeHtml(report.summary)}</p></section>
      ${feedback}
      ${practiceNext}
      ${rounds}
      ${frameworkCalibration}
      ${frameworkTimeline}
      ${integrityEvidenceMarkup(report)}
      <details><summary>Your final code (${escapeHtml(language)})</summary><pre>${escapeHtml(code.trimEnd() || "(editor was empty)")}</pre></details>
      <div class="report-actions"><button id="download-report" type="button">Download report (.md)</button><button id="done" type="button">Done - back to lobby</button></div>
    </div>
  `;
}

/// The downloadable report. Pure so the export can be tested without a DOM;
/// `at` is injected because a timestamp would otherwise make it unassertable.
export function reportMarkdown({ report, problemTitle, language, code, transcript, at, mode }) {
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
  const mdText = (value) => String(value ?? "")
    .replace(/\\/g, "\\\\")
    .replace(/([|`<\[])/g, "\\$1")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^([#>*+=~_-])/, "\\$1")
    // The delimiter, not the digits: a digit is not escapable punctuation, so
    // `\3.` rendered a literal backslash and corrupted numbered feedback, which
    // is the shape a grader writes in. `)` is an ordered-list delimiter too.
    .replace(/^(\d+)([.)])/, "$1\\$2");
  const bullets = (items) => orPlaceholder(items).map((item) => `- ${mdText(item)}`).join("\n");
  const section = (title, feedbackSection) => `### ${title}\n\n**Strengths**\n${bullets(feedbackSection.strengths)}\n\n**Improvements**\n${bullets(feedbackSection.improvements)}\n`;
  const evidence = (report.integrityEvents || []).map((event) => {
    const sources = event.sourceEventIds?.length ? ` - sources: ${event.sourceEventIds.map(mdText).join(", ")}` : "";
    return `- ${mdText(event.at)} [${mdText(event.severity)}] ${mdText(event.type)}${event.detail ? ` - ${mdText(event.detail)}` : ""}${sources}`;
  }).join("\n") || "(none captured)";
  const practiceNext = report.incomplete || !report.improvementPlan?.length
    ? []
    : [
      "## Practice next",
      "",
      ...report.improvementPlan.flatMap((item, index) => [
        `${index + 1}. **${mdText(item.phase)} · ${mdText(item.durationMin)} min · ${mdText(item.impact)} impact**`,
        `   - Drill: ${mdText(item.drill)}`,
        `   - Success: ${mdText(item.successCriterion)}`,
        ...item.selfReview.map((check) => `   - Check: ${mdText(check)}`),
      ]),
      "",
    ];
  const frameworkTimeline = report.frameworkEvidence?.length
    ? [
      "## Framework evidence",
      "",
      ...report.frameworkEvidence.map((item) =>
        `- ${frameworkTime(item.atMs)} · **${mdText(item.phase)}** · ${mdText(item.kind)} · ${mdText(item.source)} · ${mdText(item.confidence)}% · v${mdText(item.frameworkVersion)} — ${mdText(item.summary)}`),
      "",
    ]
    : [];
  const chainNote = mdText(chainSentence(report));
  const conversation = transcript
    .filter((segment) => segment.final || segment.text.trim())
    .map((segment) => `**${segment.speaker === "interviewer" ? "Jim" : "You"}:** ${mdText(segment.text.trim())}`)
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
  const body = code.trimEnd() || "(editor was empty)";
  const longestRun = (body.match(/`+/g) || []).reduce((longest, run) => Math.max(longest, run.length), 0);
  const fence = "`".repeat(Math.max(2, longestRun) + 1);
  // The info string is the one place this document emits a value unescaped, so
  // it takes the only characters a language tag can be. Today `language` comes
  // from the tabs and is allowlisted on both sides, but a newline here would
  // end the fence line and start emitting the candidate's code as prose.
  const info = String(language).replace(/[^a-zA-Z0-9_+-]/g, "");
  // The exported copy is the one that outlives the tab and can be forwarded, so
  // it must not carry a verdict the session never earned either. Only the head
  // differs: the tail used to be duplicated per branch and had already drifted
  // four ways, including an untagged code fence and two different names for the
  // same section.
  const head = report.incomplete
    ? [
      "## No evaluation",
      "",
      mdText(report.summary) || "(none)",
    ]
    : [
      `## Verdict: ${report.decision === "HIRE" ? "HIRE" : "NO HIRE"}`,
      "",
      "| Metric | Score |",
      "|---|---|",
      `| Coding | ${mdText(report.codingScore)} / 100 |`,
      `| Communication | ${mdText(report.communicationScore)} / 100 |`,
      `| Hints used | ${mdText(report.hintsUsed)} |`,
      "",
      "## Committee summary",
      mdText(report.summary) || "(none)",
      "",
      section("Coding feedback", report.codingFeedback),
      section("Communication feedback", report.communicationFeedback).trimEnd(),
    ];

  return [
    `# Interview Report - ${mdText(problemTitle)}`,
    `_${at}_`,
    `Mode: ${mode === "practice" || report.mode === "practice" ? "Practice" : "Scored"}`,
    `Loop: ${report.interviewLoop === "coding_only" ? "Coding only" : "Coding + behavioral"}`,
    report.interviewContract
      ? `Contract: bundle ${report.interviewContract.bundleVersion}; live prompt ${report.interviewContract.livePromptVersion}; report prompt ${report.interviewContract.reportPromptVersion}; rubric ${report.interviewContract.rubricVersion}; report schema ${report.interviewContract.reportSchemaVersion}`
      : "Contract: legacy/unversioned",
    ...(report.rounds?.length ? ["Rounds: " + report.rounds.map((round) => `${round.kind} (${round.budgetMin} min, ${round.status})`).join("; ")] : []),
    ...(report.frameworkAssessment ? ["REACTO/STAR phase scores are formative coaching signals, not calibrated hiring evidence."] : []),
    "",
    ...head,
    "",
    ...practiceNext,
    ...frameworkTimeline,
    // A session with no evaluation can still be one that ended because the
    // camera saw something. Refusing to score it is not a reason to drop the
    // evidence, which `sanitizeReport` deliberately keeps.
    "## Integrity Evidence",
    evidence,
    "",
    chainNote,
    "",
    `## Final code (${mdText(language)})`,
    fence + info,
    body,
    fence,
    "",
    "## Conversation transcript",
    conversation || "(no speech captured)",
    "",
  ].join("\n");
}

function frameworkTime(atMs) {
  return formatTime(Math.max(0, Math.floor(Number(atMs) / 1000)));
}

function frameworkEvidenceMarkup(items) {
  if (!items?.length) return "";
  return `<section aria-labelledby="framework-evidence-title">
    <h3 id="framework-evidence-title">Framework evidence</h3>
    <ol class="framework-timeline">${items.map((item) => `<li>
      <strong>${escapeHtml(item.phase)}</strong>
      <span>${frameworkTime(item.atMs)} · ${escapeHtml(item.kind)} · ${escapeHtml(item.source)} · ${escapeHtml(item.confidence)}% confidence · v${escapeHtml(item.frameworkVersion)}</span>
      <p>${escapeHtml(item.summary)}</p>
    </li>`).join("")}</ol>
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
