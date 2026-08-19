// The view layer, kept free of module-scope DOM lookups so `node --test` can
// exercise it with a stub document instead of a real browser. Markup builders
// return strings; the transcript view takes the document and panel it renders
// into. interview.js owns the wiring, this owns the output.

import { escapeHtml, orPlaceholder } from "./lib.js";

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

function integrityEvidenceMarkup(events = []) {
  if (!events.length) return "";
  const rows = events.map((event, index) => `
    <li data-integrity-index="${index}"><strong>${escapeHtml(event.severity)}</strong> ${escapeHtml(event.type)} <span>${escapeHtml(event.at)}</span>${event.detail ? `<p>${escapeHtml(event.detail)}</p>` : ""}${sourceEventMarkup(event)}</li>
  `).join("");
  return `<section><h3>Integrity Evidence</h3><ul>${rows}</ul></section>`;
}

function sourceEventMarkup(event) {
  if (!event.sourceEventIds?.length) return "";
  return `<ul>${event.sourceEventIds.map((id) => `<li>source event ${escapeHtml(id)}</li>`).join("")}</ul>`;
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

  return `
    <div class="report-card">
      <div class="report-header">
        <div><p>Interview report · ${escapeHtml(problemTitle)}</p><h2>${heading}</h2></div>
        ${badge}
      </div>${scores}
      <section><h3>${report.incomplete ? "What happened" : "Committee summary"}</h3><p>${escapeHtml(report.summary)}</p></section>
      ${feedback}
      ${integrityEvidenceMarkup(report.integrityEvents)}
      <details><summary>Your final code (${escapeHtml(language)})</summary><pre>${escapeHtml(code.trimEnd() || "(editor was empty)")}</pre></details>
      <div class="report-actions"><button id="download-report" type="button">Download report (.md)</button><button id="done" type="button">Done - back to lobby</button></div>
    </div>
  `;
}

/// The downloadable report. Pure so the export can be tested without a DOM;
/// `at` is injected because a timestamp would otherwise make it unassertable.
export function reportMarkdown({ report, problemTitle, language, code, transcript, at }) {
  const bullets = (items) => orPlaceholder(items).map((item) => `- ${item}`).join("\n");
  const section = (title, feedbackSection) => `### ${title}\n\n**Strengths**\n${bullets(feedbackSection.strengths)}\n\n**Improvements**\n${bullets(feedbackSection.improvements)}\n`;
  const clean = (value) => escapeHtml(value).replace(/\|/g, "\\|");
  const evidence = (report.integrityEvents || []).map((event) => {
    const sources = event.sourceEventIds?.length ? ` - sources: ${event.sourceEventIds.map(clean).join(", ")}` : "";
    return `- ${clean(event.at)} [${clean(event.severity)}] ${clean(event.type)}${event.detail ? ` - ${clean(event.detail)}` : ""}${sources}`;
  }).join("\n") || "(none captured)";
  const conversation = transcript
    .filter((segment) => segment.final || segment.text.trim())
    .map((segment) => `**${segment.speaker === "interviewer" ? "Jim" : "You"}:** ${segment.text.trim()}`)
    .join("\n\n");
  // The exported copy is the one that outlives the tab and can be forwarded, so
  // it must not carry a verdict the session never earned either. Only the head
  // differs: the tail used to be duplicated per branch and had already drifted
  // four ways, including an untagged code fence and two different names for the
  // same section.
  const head = report.incomplete
    ? [
      "## No evaluation",
      "",
      report.summary || "(none)",
    ]
    : [
      `## Verdict: ${report.decision === "HIRE" ? "HIRE" : "NO HIRE"}`,
      "",
      "| Metric | Score |",
      "|---|---|",
      `| Coding | ${report.codingScore} / 100 |`,
      `| Communication | ${report.communicationScore} / 100 |`,
      `| Hints used | ${report.hintsUsed} |`,
      "",
      "## Committee summary",
      report.summary || "(none)",
      "",
      section("Coding feedback", report.codingFeedback),
      section("Communication feedback", report.communicationFeedback).trimEnd(),
    ];

  return [
    `# Interview Report - ${problemTitle}`,
    `_${at}_`,
    "",
    ...head,
    "",
    // A session with no evaluation can still be one that ended because the
    // camera saw something. Refusing to score it is not a reason to drop the
    // evidence, which `sanitizeReport` deliberately keeps.
    "## Integrity Evidence",
    evidence,
    "",
    `## Final code (${language})`,
    "```" + language,
    code.trimEnd() || "(editor was empty)",
    "```",
    "",
    "## Conversation transcript",
    conversation || "(no speech captured)",
    "",
  ].join("\n");
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
