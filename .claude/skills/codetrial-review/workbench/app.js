import { DiffView } from "/diffview.js";

const $ = (id) => document.getElementById(id);
const token = new URLSearchParams(location.hash.slice(1)).get("token");
const diffView = new DiffView({
  root: $("diff"),
  notice: $("diff-notice"),
  summary: $("diff-summary"),
  position: $("diff-position"),
});
const MOTIVATION = {
  prompt: "Why is this change needed?",
  hint: "Describe the problem or goal that motivated it, in your own words.",
};
const statusLabels = {
  draft: "Your explanation is in progress",
  ready_to_send: "Ready to send to your reviewer",
  awaiting_review: "Saved and awaiting reviewer feedback",
  reflection_needed: "Compare the reference with your explanation",
  explanation_complete: "Confirmed · Interactive review complete",
};
let session;
let answers = {};
let selected;
let reflectionDigest;
let reflectionSaving = false;
let reflectionDirty = false;
let completionAnnounced = false;
let finished = false;
let finishing = false;
let dirty = false;
let saving = false;
let requestPending = false;
let pollingBlocked = false;
const inputs = new Map();

async function api(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: {
      Authorization: `Bearer ${token || ""}`,
      "Content-Type": "application/json",
    },
  });
  const value = await response.json();
  if (!response.ok)
    throw new Error(value.error || "The request failed. Try again.");
  return value;
}

function showError(error) {
  $("error").textContent = error.message;
  $("error").hidden = false;
}

function addField(container, key, label, hint = "") {
  const wrapper = document.createElement("div");
  wrapper.className = "answer-field";
  const id = `answer-${inputs.size}`;
  const title = document.createElement("label");
  title.htmlFor = id;
  title.textContent = label;
  const input = document.createElement("textarea");
  input.id = id;
  input.name = key;
  input.maxLength = 40000;
  input.value = answers[key] || "";
  input.addEventListener("input", () => {
    answers[key] = input.value;
    dirty = true;
    $("save-status").textContent = "Unsaved changes";
    if (key === "motivation") scheduleCoach();
    renderProgress();
    renderFiles();
    renderFeedback();
  });
  inputs.set(key, input);
  wrapper.append(title);
  if (hint) {
    const helper = document.createElement("p");
    helper.id = `${id}-hint`;
    helper.className = "field-hint";
    helper.textContent = hint;
    input.setAttribute("aria-describedby", helper.id);
    wrapper.append(helper);
  }
  wrapper.append(input);
  container.append(wrapper);
}

function reflectionMatches() {
  return (
    session.reflection &&
    $("self-grade").value === session.reflection.grade &&
    $("correction").value === session.reflection.correction
  );
}

// Reflection edits only matter while the feedback they answer still applies.
function reflectionUnsaved() {
  return (
    reflectionDirty &&
    !dirty &&
    session.feedback?.answer_digest === session.answer_digest &&
    ["reflection_needed", "explanation_complete"].includes(session.status)
  );
}

// What the core answer still lacks, said while it is written rather than after
// it is sent. The server owns the rules (understanding.py); this only asks it,
// once typing pauses, and says the result. Nothing here blocks a save or a send.
let coachTimer = null;
let coachSeq = 0;
let coachLast = null;
function coachActive() {
  const input = inputs.get("motivation");
  return Boolean(
    !finishing &&
    !finished &&
    input &&
    document.body.contains(input) &&
    // An edit after feedback reopens the answer, so it is coached again.
    (dirty || ["draft", "ready_to_send"].includes(session.status)),
  );
}

function renderCoach(checks) {
  const shown = coachActive() && checks;
  $("answer-coach").hidden = !shown;
  if (!shown) return;
  const first = checks.state === "empty";
  $("coach-meter").max = checks.minimum;
  $("coach-meter").value = Math.min(checks.words, checks.minimum);
  $("coach-count").textContent = first
    ? `At least ${checks.minimum} words`
    : `${checks.words} of ${checks.minimum} words`;
  if ($("coach-message").textContent !== checks.message)
    $("coach-message").textContent = checks.message;
  const offer =
    ["empty", "short", "ungrounded"].includes(checks.state) &&
    checks.anchors.length;
  $("coach-anchors").hidden = !offer;
  $("coach-chips").replaceChildren(
    ...(offer ? checks.anchors : []).map((name) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "duration-button coach-chip";
      chip.textContent = name;
      chip.title = `Insert \`${name}\` into your answer`;
      chip.addEventListener("click", () => {
        const input = inputs.get("motivation");
        const at = input.selectionEnd ?? input.value.length;
        const text = `\`${name}\``;
        input.setRangeText(text, at, at, "end");
        input.focus();
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      return chip;
    }),
  );
}

// The coach follows the session as well as the keyboard: a load, a save, a send
// or a move to another file changes whether it applies.
let coachKey = "";
function refreshCoach() {
  const key = `${session.identity}|${session.status}|${coachActive()}`;
  if (key === coachKey) return;
  coachKey = key;
  scheduleCoach(true);
}

function scheduleCoach(immediate = false) {
  clearTimeout(coachTimer);
  if (finished || !coachActive()) return renderCoach(null);
  coachTimer = setTimeout(
    async () => {
      const seq = ++coachSeq;
      const text = inputs.get("motivation").value;
      try {
        const reuse =
          coachLast?.text === text && coachLast.identity === session.identity;
        const checks = reuse
          ? coachLast.checks
          : await api("/api/check", {
              method: "PUT",
              body: JSON.stringify({ identity: session.identity, text }),
            });
        // Only the newest question may leave a result behind.
        if (seq !== coachSeq) return;
        coachLast = { text, identity: session.identity, checks };
        renderCoach(checks);
      } catch {
        // A hint that cannot be fetched is not worth an error banner.
        if (seq === coachSeq) renderCoach(null);
      }
    },
    immediate ? 0 : 350,
  );
}

function renderProgress() {
  const count = answers.motivation?.trim() ? 1 : 0;
  $("progress").textContent = `${count} of 1 explanation written`;
  $("review-status").textContent = dirty
    ? "Save to share your latest explanation"
    : reflectionUnsaved()
      ? "Your reflection has unsaved changes"
      : statusLabels[session.status];
}

function renderFiles() {
  const list = $("file-list");
  const focused = document.activeElement?.dataset.path;
  list.replaceChildren();
  for (const file of session.manifest.files) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "file-button";
    button.dataset.path = file.path;
    if (file.path === selected) button.setAttribute("aria-current", "true");
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = file.path;
    const stats = document.createElement("span");
    stats.className = "file-stats";
    stats.textContent = `+${file.additions ?? "?"} / −${file.deletions ?? "?"}`;
    button.append(name, stats);
    button.addEventListener("click", () => selectFile(file.path));
    list.append(button);
    if (file.path === focused) button.focus({ preventScroll: true });
  }
}

function renderDiff(path) {
  const question = session.questions.find((item) => item.path === path);
  diffView.show({
    path,
    asked: question?.quote ? { path, quote: question.quote } : undefined,
  });
  $("diff-heading").textContent = path || "Complete diff";
  $("diff-meta").textContent = path
    ? "Changes in this file"
    : "The entire bundled change, including deletions and renamed files";
}

function renderQuestion() {
  // Once the server is being stopped nothing may be typed that it cannot keep.
  if (finishing) return;
  const main = session.questions[0];
  const question =
    session.questions.find((item) => item.path === selected) || main;
  const optional = question && question !== main;
  const key = optional ? `reading:${question.id}` : "motivation";
  inputs.clear();
  $("overall-answers").replaceChildren();
  $("explain-heading").textContent = optional
    ? "Reading focus · Optional"
    : "Your motivation";
  $("question-scope").textContent = optional
    ? "One optional question about this file. Your main explanation stays saved."
    : "One core explanation for the whole change.";
  $("main-question").hidden = !optional;
  $("confidence").hidden = Boolean(optional);
  document.querySelector('label[for="confidence"]').hidden = Boolean(optional);
  addField(
    $("overall-answers"),
    key,
    question?.prompt || MOTIVATION.prompt,
    question
      ? `${question.path}${question.quote ? `\n${question.quote}` : ""}`
      : MOTIVATION.hint,
  );
  // The box was rebuilt, possibly from another tab's save: say how this text
  // stands, not the text it replaced.
  scheduleCoach(true);
}

function selectFile(path) {
  selected = path;
  renderFiles();
  renderDiff(path);
  renderQuestion();
  renderProgress();
  refreshCoach();
}

// While the reviewer holds the answer and has not replied, show that something
// is happening and for how long. Only when a reviewer is actually there: with
// none, the sentence says to resume the review task and motion would mislead.
let waitingSince = null;
let waitingTimer = null;
function renderWaiting() {
  const waiting =
    session.status === "awaiting_review" &&
    ["waiting", "reading"].includes(session.reviewer);
  $("waiting").hidden = !waiting;
  if (!waiting) {
    waitingSince = null;
    clearInterval(waitingTimer);
    waitingTimer = null;
    return;
  }
  if (waitingSince === null) waitingSince = Date.now();
  const tick = () => {
    const seconds = Math.floor((Date.now() - waitingSince) / 1000);
    $("waiting-time").textContent =
      seconds < 60
        ? `${seconds} s`
        : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  };
  tick();
  waitingTimer ??= setInterval(tick, 1000);
}

function renderFeedback() {
  refreshCoach();
  const feedback = session.feedback;
  const current =
    !dirty &&
    feedback?.answer_digest === session.answer_digest &&
    ["reflection_needed", "explanation_complete"].includes(session.status);
  $("feedback-status").textContent = feedback
    ? current
      ? reflectionDirty && session.reflection && !reflectionMatches()
        ? "Your reflection has unsaved changes"
        : statusLabels[session.status]
      : "Earlier feedback is shown below. Your latest answers still need review."
    : session.status === "awaiting_review"
      ? statusLabels.awaiting_review
      : "Choose Send to reviewer when you are ready for feedback on your explanation.";
  if (!dirty) $("confidence").value = answers.confidence || "";
  if (reflectionDigest !== session.feedback_digest) {
    reflectionDigest = session.feedback_digest;
    $("self-grade").value = session.reflection?.grade || "";
    $("correction").value = session.reflection?.correction || "";
    reflectionDirty = false;
  } else if (session.reflection && !reflectionDirty && !reflectionSaving) {
    $("self-grade").value = session.reflection.grade;
    $("correction").value = session.reflection.correction;
  }
  const complete =
    current && session.status === "explanation_complete" && reflectionMatches();
  if (
    (complete || reflectionSaving) &&
    document.activeElement === $("self-grade")
  )
    $("reflect-save").focus();
  $("self-grade").disabled = complete || reflectionSaving;
  $("correction").readOnly = complete || reflectionSaving;
  $("completion-notice").hidden = !complete;
  if (complete && !completionAnnounced) {
    completionAnnounced = true;
    $("completion-notice").scrollIntoView({ block: "nearest" });
  }
  if (complete)
    $("save-status").textContent = "Confirmation saved on this computer";
  $("workspace").classList.toggle("review-complete", complete);
  $("reflect-save").textContent = complete
    ? "Confirmed"
    : reflectionSaving
      ? "Recording…"
      : "Record reflection";
  $("reflect-save").setAttribute(
    "aria-disabled",
    String(dirty || complete || reflectionSaving),
  );
  if (!saving) $("send").setAttribute("aria-disabled", String(complete));
  $("reflection-form").hidden = !current || !session.submission;
  $("reflection-state").textContent = complete
    ? "Interactive review complete. Reflection recorded. This is participation evidence, not code approval."
    : reflectionDirty && session.reflection
      ? "A different reflection was saved elsewhere. Your draft is unchanged; review it before confirming."
      : "";
  const presence = complete
    ? "Your confirmation is saved. No further response is needed for this snapshot."
    : reflectionDirty && session.reflection
      ? "Review your local reflection before confirming it."
      : {
          waiting: "Your review task is waiting for your submission.",
          reading:
            "Your review task received the saved explanations. Feedback is pending.",
          offline:
            "No active review task is waiting. Your explanations stay saved; resume codetrial-review to receive AI feedback.",
          error:
            "The reviewer could not finish. Your explanation is saved. Choose Send to reviewer to retry, or ask your review task to check reviewer.log.",
        }[session.reviewer];
  if ($("reviewer-presence").textContent !== presence)
    $("reviewer-presence").textContent = presence;
  renderWaiting();
  $("feedback-list").replaceChildren();
  for (const note of feedback?.notes || []) {
    const item = document.createElement("li");
    const label = document.createElement("strong");
    const reading = session.questions.find(
      (q) => `reading:${q.id}` === note.field,
    );
    label.textContent = `${reading ? reading.path : "Motivation"} — `;
    item.append(label, document.createTextNode(note.text));
    $("feedback-list").append(item);
  }
}

async function loadSession() {
  if (requestPending || saving) return;
  requestPending = true;
  $("retry").hidden = true;
  try {
    const next = await api("/api/session");
    if (session) {
      if (next.identity !== session.identity)
        throw new Error(
          "The code snapshot changed. Save a local copy of your explanations and reopen a new review session.",
        );
      if (
        dirty &&
        next.revision !== session.revision &&
        next.answer_digest !== session.answer_digest
      )
        throw new Error(
          "This session changed elsewhere. Download your unsaved explanations before reloading the page.",
        );
      session = next;
      if (!dirty) {
        answers = { ...next.answers };
        for (const [key, input] of inputs) input.value = answers[key] || "";
        // Another tab may have saved a different answer; an unchanged one is
        // answered from the last check without asking again.
        scheduleCoach(true);
      }
      renderFiles();
      renderProgress();
      renderFeedback();
      $("error").hidden = true;
      pollingBlocked = false;
      return;
    }
    // Before the session is kept: if the diff cannot be loaded, Retry must
    // start over here and not find a session it believes is already shown.
    const model = await api("/api/diff");
    session = next;
    answers = { ...next.answers };
    const focus = next.questions[0];
    if (focus) selected = focus.path;
    $("change-size").textContent =
      `${next.manifest.files.length} files · ${next.total === null ? "Unknown" : next.total.toLocaleString()} changed lines`;
    $("snapshot").textContent =
      `Review snapshot ${next.manifest.head.slice(0, 12)}`;
    const notes = [];
    if (!next.committed)
      notes.push(
        "Working-tree preview: recheck the committed PR diff before submission.",
      );
    if (!next.coverage_complete)
      notes.push(
        "Coverage is incomplete. Review untracked or binary files separately.",
      );
    if (next.total !== null && next.total > 500)
      notes.push(
        "Over 500 changed lines. All listed files count until exclusions are verified.",
      );
    if (next.large_commits?.length)
      notes.push(
        `Commit threshold reached (at least 200 added + deleted lines): ${next.large_commits.map((c) => `${c.sha.slice(0, 12)} (${c.known_lines} lines)`).join(", ")}. Interactive review is required unless exclusions are verified.`,
      );
    if (!next.commit_coverage_complete)
      notes.push(
        "Commit-size coverage is incomplete. Inspect the full commit series before submission; a small final diff does not rule out a large commit.",
      );
    $("coverage-note").textContent = notes.join(" ");
    $("coverage-note").hidden = !notes.length;
    diffView.setModel(model);
    $("loading").hidden = true;
    $("workspace").hidden = false;
    $("error").hidden = true;
    selectFile(selected || next.manifest.files[0]?.path);
    renderFeedback();
    pollingBlocked = false;
  } catch (error) {
    // A poll that was in flight when the server stopped is not an error.
    if (finished || finishing) return;
    pollingBlocked = true;
    showError(error);
    $("loading").hidden = true;
    $("retry").hidden = false;
  } finally {
    requestPending = false;
  }
}

async function saveAnswers() {
  if (saving || requestPending) return false;
  saving = true;
  $("save").setAttribute("aria-disabled", "true");
  $("save-status").textContent = "Saving…";
  const submitted = { ...answers };
  try {
    session = await api("/api/answers", {
      method: "PUT",
      body: JSON.stringify({
        identity: session.identity,
        revision: session.revision,
        answers: submitted,
      }),
    });
    dirty = JSON.stringify(answers) !== JSON.stringify(submitted);
    $("save-status").textContent = dirty
      ? "Saved. You have newer unsaved changes."
      : "Saved on this computer";
    $("error").hidden = true;
    pollingBlocked = false;
    renderProgress();
    renderFeedback();
    return true;
  } catch (error) {
    showError(error);
    $("save-status").textContent =
      "Not saved. Your explanations remain in the form.";
    return false;
  } finally {
    saving = false;
    $("save").removeAttribute("aria-disabled");
  }
}
$("answers-form").addEventListener("submit", (event) => {
  event.preventDefault();
  saveAnswers();
});
$("send").addEventListener("click", async () => {
  if (
    saving ||
    requestPending ||
    (!dirty && session.status === "explanation_complete")
  )
    return;
  if (dirty && !(await saveAnswers())) return;
  if (dirty) return;
  if (session.missing.length) {
    showError(new Error("Explain your motivation before sending."));
    if (session.questions[0]) selectFile(session.questions[0].path);
    inputs.get("motivation")?.focus();
    return;
  }
  saving = true;
  $("send").setAttribute("aria-disabled", "true");
  try {
    session = await api("/api/review", {
      method: "PUT",
      body: JSON.stringify({
        identity: session.identity,
        revision: session.revision,
      }),
    });
    $("save-status").textContent = "Sent to your review task";
    $("error").hidden = true;
    pollingBlocked = false;
    renderProgress();
    renderFeedback();
  } catch (error) {
    showError(error);
  } finally {
    saving = false;
    $("send").setAttribute(
      "aria-disabled",
      String(!dirty && session.status === "explanation_complete"),
    );
  }
});
$("confidence").addEventListener("change", () => {
  if ($("confidence").value) answers.confidence = $("confidence").value;
  else delete answers.confidence;
  dirty = true;
  $("save-status").textContent = "Unsaved changes";
  renderProgress();
  renderFeedback();
});
for (const id of ["self-grade", "correction"]) {
  $(id).addEventListener("input", () => {
    reflectionDirty = true;
    renderProgress();
    renderFeedback();
  });
}
$("reflection-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (
    dirty ||
    saving ||
    requestPending ||
    (session.status === "explanation_complete" && reflectionMatches())
  )
    return;
  saving = true;
  reflectionSaving = true;
  renderFeedback();
  try {
    session = await api("/api/reflection", {
      method: "PUT",
      body: JSON.stringify({
        identity: session.identity,
        revision: session.revision,
        feedback_digest: reflectionDigest,
        grade: $("self-grade").value,
        correction: $("correction").value,
      }),
    });
    reflectionDirty = false;
    renderProgress();
    renderFeedback();
    $("error").hidden = true;
  } catch (error) {
    showError(error);
  } finally {
    saving = false;
    reflectionSaving = false;
    renderFeedback();
  }
});
$("main-question").addEventListener("click", () => {
  selectFile(session.questions[0].path);
  inputs.get("motivation").focus();
});
$("refresh").addEventListener("click", loadSession);
$("retry").addEventListener("click", loadSession);
$("all-diff").addEventListener("click", () => selectFile());
$("download").addEventListener("click", () => {
  const text = [
    "# My change explanation",
    `Snapshot: ${session.manifest.head}`,
    `\nReview record:\n${JSON.stringify({ localReflection: { feedback_digest: reflectionDigest, grade: $("self-grade").value, correction: $("correction").value, unsaved: reflectionDirty }, questions: session.questions, submission: session.submission, feedback: session.feedback, reflection: session.reflection, history: session.history }, null, 2)}`,
    ...Object.entries(answers).map(([key, value]) => `\n## ${key}\n\n${value}`),
  ].join("\n");
  const url = URL.createObjectURL(
    new Blob([text], { type: "text/markdown;charset=utf-8" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = "change-explanation.md";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
});
window.addEventListener("beforeunload", (event) => {
  if (dirty || reflectionUnsaved()) {
    event.preventDefault();
    event.returnValue = "";
  }
});
await loadSession();
setInterval(() => {
  if (
    session &&
    !finished &&
    !finishing &&
    !pollingBlocked &&
    document.visibilityState === "visible"
  )
    loadSession();
}, 5000);

// Finishing ends the session: the server stops, the token leaves the address
// bar, and the page says what comes next. A browser only lets a script close a
// window the script opened, so when the tab stays, the page says it is safe to
// close rather than leaving the button looking broken.
$("close-window").addEventListener("click", async () => {
  if (finished || $("close-window").getAttribute("aria-disabled") === "true")
    return;
  // Nothing typed since the last save may be left behind by a closed server.
  if (dirty) {
    showError(new Error("Save your latest edit before finishing."));
    return;
  }
  $("close-window").setAttribute("aria-disabled", "true");
  finishing = true;
  clearTimeout(coachTimer);
  const boxes = [...inputs.values(), $("confidence")];
  for (const box of boxes) {
    box.readOnly = true;
    box.disabled = true;
  }
  try {
    await api("/api/finish", { method: "PUT", body: "{}" });
  } catch (error) {
    finishing = false;
    $("close-window").setAttribute("aria-disabled", "false");
    for (const box of boxes) {
      box.readOnly = false;
      box.disabled = false;
    }
    showError(error);
    return;
  }
  finished = true;
  history.replaceState(null, "", location.pathname + location.search);
  $("error").hidden = true;
  $("workspace").hidden = true;
  $("finished").hidden = false;
  // The button that had focus went away with the workspace.
  $("finished").tabIndex = -1;
  $("finished").focus();
  $("finished").scrollIntoView({ block: "start" });
  window.close();
  setTimeout(() => {
    $("close-hint").textContent =
      "This browser kept the tab open; you can close it yourself.";
  }, 300);
});

// How the diff is drawn is the reader's choice and is kept between visits.
function setSplit(split) {
  $("diff-mode").setAttribute("aria-pressed", String(split));
  $("diff-mode").textContent = split ? "Unified" : "Side by side";
  try {
    localStorage.setItem("codetrial-diff-split", split ? "1" : "0");
  } catch {
    // Storage may be off; the choice then lasts until the page closes.
  }
  diffView.setSplit(split);
}
$("diff-mode").addEventListener("click", () =>
  setSplit($("diff-mode").getAttribute("aria-pressed") !== "true"),
);
$("diff-ws").addEventListener("change", (event) =>
  diffView.setHideWhitespace(event.target.checked),
);
$("diff-prev").addEventListener("click", () => diffView.step(-1));
$("diff-next").addEventListener("click", () => diffView.step(1));
$("diff").addEventListener("keydown", (event) => {
  if (event.key === "]" || event.key === "n") diffView.step(1);
  else if (event.key === "[" || event.key === "p") diffView.step(-1);
});
try {
  if (localStorage.getItem("codetrial-diff-split") === "1") setSplit(true);
} catch {
  // Unreadable storage leaves the default view.
}
