import { mountPeerArtifacts } from "./peer-artifacts.js";
import {
  acceptsPreview,
  peerApi,
  peerRole,
  previewPayload,
  PEER_PREVIEW_TOPIC,
} from "./peer-lib.js";
import { createMuteToggle, createPeerAudio } from "./peer-audio.js";
import {
  mountWorkspace,
  firstWorkspaceRunnerStatus,
  renderWorkspaceResults,
  renderWorkspaceStatus,
} from "./workspace.js";
import { loadProblem } from "./problem-data.js";
import { runBrowserTests } from "./runners.js";
import { problemMarkup, finalRunnerStatus } from "./render.js";
const byId = (id) => document.getElementById(id);
const id = new URLSearchParams(location.search).get("session");
const base = `/api/peer/sessions/${encodeURIComponent(id ?? "")}`;
const nodes = {
  editor: byId("editor"),
  editorLines: byId("editor-lines"),
  editorHighlight: byId("editor-highlight"),
  run: byId("peer-run"),
  resultsContent: byId("peer-results"),
  resultsLabel: byId("peer-results-label"),
};
let state;
let round = -1;
let roundLoaded = false;
let connectionGeneration = 0;
let revision = 0;
// Sent previews count up from the clock so a reloaded tab outranks its
// predecessor; received ones are ordered against the newest seen this round.
let previewSeq = 0;
let lastPreviewSeq = -1;
let lease;
let previousLease;
let editVersion = 0;
let snapshotRequest = 0;
let language = "python";
let problem;
let tests = null;
let testStatus = null;
let dirty = false;
let saveBusy = false;
let pollBusy = false;
let previewTimer;
let room;
let audioConnected = false;
let offset = 0;
let stopped = false;
let paused = false;
let terminal = false;
let artifacts;
const timers = [];
// A message stays up long enough to read; the poll then falls back to what the
// session's state says. Errors stay longer than passing status.
let noticeUntil = 0;
// The round whose swap this member already confirmed, until the state moves on.
let advanceRequestedRound = -1;
function notice(text, ms = 4000, tone = "info") {
  const line = byId("peer-error");
  line.textContent = text;
  line.dataset.tone = tone;
  noticeUntil = Date.now() + ms;
}
function error(error) {
  notice(error.message ?? String(error), 30000, "error");
  // Another action moved the session on; show where it stands now.
  if (error.code === "version_conflict") void refresh().catch(() => {});
}
function roundPath() {
  return `${base}/rounds/${round}`;
}
const workspace = mountWorkspace({
  nodes,
  readOnly: true,
  onEdit() {
    dirty = true;
    editVersion++;
    tests = null;
    testStatus = null;
    byId("peer-results").textContent = "Run your code to see results.";
    byId("peer-save-status").textContent = "Unsaved changes";
    if (!previewTimer)
      previewTimer = setTimeout(() => {
        previewTimer = null;
        publishPreview();
      }, 300);
  },
  async onRunTests() {
    if (!problem || peerRole(state) !== "candidate" || !lease) return;
    const runRound = round;
    const runEdit = editVersion;
    const runLease = lease;
    const runCode = nodes.editor.value;
    const currentRun = () =>
      !stopped &&
      round === runRound &&
      editVersion === runEdit &&
      lease === runLease &&
      nodes.editor.value === runCode &&
      state.currentRound?.status === "active";
    testStatus = firstWorkspaceRunnerStatus(language);
    renderWorkspaceStatus(nodes, testStatus);
    const result = await runBrowserTests(
      problem.page,
      nodes.editor.value,
      language,
      (status) => {
        if (!currentRun()) return;
        testStatus = status;
        renderWorkspaceStatus(nodes, status);
      },
    );
    if (!currentRun()) return;
    tests = result;
    testStatus = finalRunnerStatus(tests, testStatus);
    renderWorkspaceResults(nodes, tests, { status: testStatus });
    dirty = true;
    await save(false);
  },
});
const audio = createPeerAudio({
  sdk: window.LivekitClient,
  container: byId("peer-audio"),
  enableSound: byId("peer-enable-sound"),
  onError: error,
  async onReconnect() {
    workspace.setReadOnly(true);
    await connect();
    notice("Reconnected.");
  },
  onDisconnect() {
    audioConnected = false;
    room = null;
    updatePrestart();
    if (terminal || stopped || paused) return;
    if (state && !state.currentRound)
      void peerApi(`${base}/ready`, {
        ready: false,
        expectedVersion: state.version,
      }).catch(error);
    workspace.setReadOnly(true);
    notice("Audio disconnected. Connect audio to rejoin.", 30000, "error");
  },
});
function publishPreview() {
  if (!room || peerRole(state) !== "candidate" || !lease) return;
  previewSeq = Math.max(previewSeq + 1, Date.now());
  const packet = previewPayload({
    round,
    revision,
    seq: previewSeq,
    language,
    code: nodes.editor.value,
  });
  void room.localParticipant
    .publishData(new TextEncoder().encode(JSON.stringify(packet)), {
      topic: PEER_PREVIEW_TOPIC,
      reliable: false,
    })
    .catch(error);
}
function leaseLost() {
  previousLease = lease ?? previousLease;
  lease = null;
  workspace.setReadOnly(true);
  updateRecovery();
  byId("peer-download").hidden = !dirty;
  byId("peer-save-status").textContent =
    "Edit access expired or moved to another tab. Recover access or download your unsaved code.";
}
async function save(final) {
  if (!lease || saveBusy) return;
  saveBusy = true;
  const code = nodes.editor.value;
  const savingRound = round;
  const savingLease = lease;
  try {
    const answer = await peerApi(
      `${roundPath()}/snapshot`,
      { lease, expectedRevision: revision, language, code, tests, final },
      "PUT",
    );
    if (stopped || round !== savingRound || lease !== savingLease) return;
    revision = Math.max(revision, answer.revision);
    dirty = nodes.editor.value !== code;
    byId("peer-save-status").textContent = dirty
      ? "Unsaved changes"
      : "Saved to session";
  } catch (failure) {
    if (stopped || round !== savingRound || lease !== savingLease) return;
    if (["lease_lost", "round_sealed"].includes(failure.code)) leaseLost();
    else if (failure.code === "version_conflict") {
      await loadSnapshot();
      if (stopped || round !== savingRound || lease !== savingLease) return;
      byId("peer-save-status").textContent = dirty
        ? "Unsaved changes retained; saving will retry."
        : "Loaded the latest snapshot.";
      return;
    } else {
      byId("peer-save-status").textContent =
        "Could not save. Your code is still in this tab.";
      error(failure);
    }
    throw failure;
  } finally {
    saveBusy = false;
  }
}
function updateRecovery() {
  byId("peer-recover-writer").hidden =
    !previousLease ||
    Boolean(lease) ||
    peerRole(state) !== "candidate" ||
    !["active", "finishing"].includes(state.currentRound?.status);
}
async function loadSnapshot() {
  const loadingRound = round;
  const request = ++snapshotRequest;
  const snapshot = await peerApi(`${roundPath()}/snapshot`);
  if (
    stopped ||
    round !== loadingRound ||
    request !== snapshotRequest ||
    snapshot.revision < revision
  )
    return;
  revision = snapshot.revision;
  if (dirty && peerRole(state) === "candidate") return;
  language = snapshot.language || language;
  tests = snapshot.tests;
  workspace.setLanguage(language);
  workspace.setValue(
    snapshot.revision === 0
      ? (problem?.starterCode?.[language] ?? "")
      : (snapshot.code ?? ""),
  );
  if (tests)
    renderWorkspaceResults(nodes, tests, {
      status: finalRunnerStatus(tests, null),
    });
  else byId("peer-results").textContent = "Run your code to see results.";
}
async function recoverWriter() {
  const recoveringRound = round;
  const token = previousLease;
  if (!token || lease) return;
  byId("peer-recover-writer").disabled = true;
  try {
    const answer = await peerApi(`${roundPath()}/writer`, {
      previousLease: token,
    });
    if (stopped || round !== recoveringRound || previousLease !== token) return;
    lease = answer.lease;
    previousLease = lease;
    await loadSnapshot();
    if (stopped || round !== recoveringRound) return;
    workspace.setReadOnly(state.currentRound?.status !== "active");
    byId("peer-download").hidden = true;
    byId("peer-save-status").textContent = dirty
      ? "Unsaved changes"
      : "Edit access recovered.";
    if (state.currentRound?.status === "finishing") await save(true);
  } catch (failure) {
    error(failure);
  } finally {
    byId("peer-recover-writer").disabled = false;
    updateRecovery();
  }
}
async function loadRound(nextRound) {
  roundLoaded = false;
  round = nextRound;
  editVersion++;
  previousLease = null;
  byId("peer-recover-writer").hidden = true;
  byId("peer-download").hidden = true;
  dirty = false;
  lease = null;
  revision = 0;
  lastPreviewSeq = -1;
  tests = null;
  problem = null;
  byId("peer-notes").value = "";
  byId("peer-results").textContent = "Run your code to see results.";
  const info = await peerApi(`${roundPath()}/problem`);
  problem = await loadProblem(info.page);
  language = state.settings?.language || language;
  byId("peer-problem-title").textContent = info.title;
  byId("peer-problem").innerHTML = problemMarkup(problem, { guidance: false });
  byId("peer-guide-panel").hidden = peerRole(state) !== "interviewer";
  if (peerRole(state) === "interviewer") {
    const guide = await peerApi(`${roundPath()}/guide`);
    byId("peer-guide").textContent = Object.entries(guide)
      .map(
        ([name, value]) =>
          `${name}\n${typeof value === "string" ? value : JSON.stringify(value, null, 2)}`,
      )
      .join("\n\n");
  }
  if (
    peerRole(state) === "candidate" &&
    ["active", "finishing"].includes(state.currentRound.status)
  ) {
    const answer = await peerApi(`${roundPath()}/writer`, {});
    lease = answer.lease;
    previousLease = lease;
  }
  await loadSnapshot();
  roundLoaded = true;
}
function paintClock() {
  const deadline = state?.currentRound?.deadline;
  if (!deadline) {
    byId("peer-timer").textContent = "";
    return;
  }
  const seconds = Math.max(
    0,
    Math.ceil((Number(deadline) * 1000 - (Date.now() + offset)) / 1000),
  );
  byId("peer-timer").textContent =
    `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
async function refreshSavedAudio() {
  byId("peer-audio-capture").hidden = false;
  try {
    const answer = await peerApi(`${base}/audio/status`);
    byId("peer-audio-capture-status").textContent =
      {
        recording: "Saved audio is being recorded.",
        ready: "Saved audio is ready.",
        deleted: "Saved audio has been stopped and deleted.",
        failed: "Audio could not be saved.",
        unavailable: "No saved audio is available.",
      }[answer.status] || "Checking saved audio...";
    byId("peer-stop-saved-audio").hidden = answer.status !== "recording";
  } catch {
    byId("peer-audio-capture-status").textContent =
      "Saved audio status could not be loaded.";
  }
}
function updatePrestart() {
  if (!state) return;
  const prestart =
    !state.currentRound && ["pending", "lobby"].includes(state.status);
  byId("peer-prestart").hidden = !prestart;
  if (!prestart) return;
  const mine = state.members.find((member) => member.slot === state.you.slot);
  const settings = state.settings ?? {};
  byId("peer-prestart-settings").textContent =
    `${settings.topic || "Any topic"} · ${settings.difficulty || ""} · ${settings.language || ""} · ${settings.roundMinutes || 25} minutes per round. Save transcript: ${settings.saveTranscript ? "yes" : "no"}. Save audio: ${settings.saveAudio ? "yes" : "no"}.`;
  byId("peer-prestart-members").textContent = state.members
    .map(
      (member) =>
        `${member.nickname || member.login}: ${member.ready ? "ready" : member.entered ? "entered" : "not entered"}`,
    )
    .join(" · ");
  const consented = state.you.observerConsented === true;
  byId("peer-session-consent-label").hidden = consented;
  byId("peer-session-consented").hidden = !consented;
  byId("peer-session-enter").disabled = Boolean(mine?.entered);
  byId("peer-connect").disabled =
    audioConnected ||
    state.members.length !== 2 ||
    !state.members.every((member) => member.entered);
  byId("peer-session-ready").disabled =
    !audioConnected || !mine?.entered || Boolean(mine?.ready);
  byId("peer-session-start").disabled =
    !audioConnected ||
    state.members.length !== 2 ||
    !state.members.every((member) => member.ready) ||
    !(consented || byId("peer-session-consent").checked);
}
async function refresh(reconnect = false) {
  if (pollBusy || stopped || paused) return;
  pollBusy = true;
  try {
    const next = await peerApi(base);
    if (stopped || paused) return;
    state = next;
    updatePrestart();
    byId("peer-partners").textContent = next.members
      .map((member) => `${member.nickname || member.login} (@${member.login})`)
      .join(" and ");
    const partner = next.members.find(
      (member) => member.slot !== next.you.slot,
    );
    byId("peer-feedback-subtitle").textContent = partner
      ? `Feedback for ${partner.nickname || partner.login} (@${partner.login}) as ${peerRole(next) === "candidate" ? "interviewer" : "candidate"}`
      : "";
    offset = Number(next.serverNow) * 1000 - Date.now();
    byId("peer-title").textContent =
      next.status === "complete"
        ? "Practice complete"
        : next.status === "cancelled"
          ? "Practice cancelled"
          : next.currentRound
            ? `Round ${Number(next.currentRound.index) + 1} of 2`
            : "Prepare your practice";
    byId("peer-role").textContent = !next.currentRound
      ? ""
      : peerRole(next) === "candidate"
        ? "You are the candidate"
        : "You are the interviewer";
    if (
      next.currentRound &&
      (next.currentRound.index !== round || !roundLoaded)
    )
      await loadRound(next.currentRound.index);
    else if (
      round >= 0 &&
      (reconnect ||
        peerRole(next) === "interviewer" ||
        next.currentRound?.status === "feedback")
    )
      await loadSnapshot();
    if (stopped || paused) return;
    updateRecovery();
    const active = next.currentRound?.status === "active";
    workspace.setReadOnly(!active || peerRole(next) !== "candidate" || !lease);
    byId("peer-finish").disabled = !active;
    byId("peer-language").textContent = language;
    const feedback = next.currentRound?.status === "feedback";
    byId("peer-feedback-panel").hidden = !feedback;
    byId("peer-advance").hidden =
      !feedback ||
      ["complete", "cancelled", "finalizing"].includes(next.status);
    byId("peer-advance").textContent =
      advanceRequestedRound === round
        ? "Waiting for your partner to confirm"
        : round === 0
          ? "Confirm swap"
          : "Complete practice";
    if (feedback) await loadFeedback();
    if (next.currentRound?.status === "finishing" && lease && !saveBusy)
      await save(true);
    const stateNotice =
      !next.currentRound &&
      !["complete", "cancelled", "finalizing"].includes(next.status)
        ? "Waiting for both members to confirm the start in the lobby."
        : next.status === "complete"
          ? "Practice complete. Your feedback remains available here."
          : next.status === "cancelled"
            ? "This practice was cancelled. Start another from the lobby."
            : "";
    if (Date.now() >= noticeUntil) {
      byId("peer-error").textContent = stateNotice;
      byId("peer-error").dataset.tone = "info";
    }
    if (
      next.settings?.saveAudio &&
      !["finalizing", "cancelled", "complete"].includes(next.status)
    )
      await refreshSavedAudio();
    paintClock();
    if (["finalizing", "cancelled", "complete"].includes(next.status)) {
      terminal = true;
      byId("peer-audio-capture").hidden = true;
      for (const timer of timers) clearInterval(timer);
      clearTimeout(previewTimer);
      // The partner may submit after this page saw the practice end, so their
      // feedback keeps being looked for until it appears.
      if (feedbackWatch === null && feedbackWatchDone !== round) {
        let polls = 0;
        let reading = false;
        feedbackWatch = setInterval(async () => {
          // A hidden tab neither reads nor spends the budget, and one read
          // at a time keeps an old empty answer from replacing a newer one.
          if (document.hidden || reading) return;
          if (++polls > FEEDBACK_WATCH_POLLS) return finishFeedbackWatch();
          reading = true;
          try {
            await loadFeedback();
          } catch {
            // The next tick tries again.
          } finally {
            reading = false;
          }
        }, FEEDBACK_WATCH_MS);
      }
      lease = null;
      workspace.setReadOnly(true);
      await audio.disconnect();
      room = null;
      byId("peer-connect").disabled = true;
      byId("peer-mute").disabled = true;
      if (!artifacts)
        artifacts = mountPeerArtifacts({
          sessionId: id,
          settings: next.settings ?? {},
          members: next.members,
          onError: error,
          nodes: {
            panel: byId("peer-advice-panel"),
            advice: byId("peer-advice"),
            retry: byId("peer-advice-retry"),
            transcript: byId("peer-download-transcript"),
            transcriptNote: byId("peer-transcript-note"),
            withdrawTranscript: byId("peer-withdraw-transcript"),
            audio: byId("peer-download-audio"),
            audioNote: byId("peer-audio-note"),
            withdraw: byId("peer-withdraw-audio"),
          },
        });
    }
  } finally {
    pollBusy = false;
  }
}
async function connect() {
  const generation = ++connectionGeneration;
  const current = () =>
    generation === connectionGeneration && !stopped && !paused && !terminal;
  byId("peer-connect").disabled = true;
  try {
    const connection = await peerApi(`${base}/token`, {});
    await refresh(true);
    if (!current()) return;
    const connected = await audio.connect(
      connection.url ?? connection.livekitUrl,
      connection.token,
    );
    if (!connected) return;
    if (!current()) {
      await connected.disconnect();
      return;
    }
    room = connected;
    room.on(
      window.LivekitClient.RoomEvent.DataReceived,
      (data, participant, _kind, topic) => {
        if (topic !== PEER_PREVIEW_TOPIC) return;
        try {
          const packet = JSON.parse(new TextDecoder().decode(data));
          if (
            !acceptsPreview(state, participant, packet) ||
            peerRole(state) !== "interviewer"
          )
            return;
          if (packet.seq <= lastPreviewSeq) return;
          lastPreviewSeq = packet.seq;
          if (
            packet.kind === "preview" &&
            typeof packet.code === "string" &&
            packet.revision >= revision
          ) {
            workspace.setLanguage(packet.language);
            workspace.setValue(packet.code);
          } else void loadSnapshot().catch(error);
        } catch {
          /* Invalid disposable previews never change the authoritative state. */
        }
      },
    );
    audioConnected = true;
    updatePrestart();
    byId("peer-mute").disabled = false;
    await listDevices();
    if (!current()) return;
    notice("Audio connected.");
  } finally {
    byId("peer-connect").disabled = terminal;
    if (!terminal) updatePrestart();
  }
}
async function listDevices() {
  const devices = await navigator.mediaDevices.enumerateDevices();
  byId("peer-device").replaceChildren();
  for (const device of devices.filter(
    (device) => device.kind === "audioinput",
  )) {
    const option = document.createElement("option");
    option.value = device.deviceId;
    option.textContent = device.label || "Microphone";
    byId("peer-device").append(option);
  }
}
const dimensions = {
  candidate: [
    "clarification",
    "reasoning",
    "correctness",
    "testing",
    "complexity",
    "technicalExplanation",
  ],
  interviewer: [
    "preparation",
    "questionQuality",
    "hintPacing",
    "followUp",
    "feedbackSpecificity",
  ],
};
let feedbackRound = -1;
let feedbackHydratedRound = -1;
let feedbackEdited = false;
// Looks for the partner's late feedback for twenty minutes, then gives up.
const FEEDBACK_WATCH_MS = 20000;
const FEEDBACK_WATCH_POLLS = 60;
// Kept apart from `timers`, which a terminal refresh clears wholesale.
let feedbackWatch = null;
// The round whose watch ended, so a restored page does not start it again.
let feedbackWatchDone = -1;
function stopFeedbackWatch() {
  clearInterval(feedbackWatch);
  feedbackWatch = null;
}
function finishFeedbackWatch() {
  stopFeedbackWatch();
  feedbackWatchDone = round;
}
let feedbackSubmitted = false;
async function loadFeedback() {
  const loadingRound = round;
  if (feedbackRound !== round) {
    feedbackRound = round;
    feedbackEdited = false;
    feedbackSubmitted = false;
    for (const control of byId("peer-feedback").elements)
      control.disabled = false;
    byId("peer-dimensions").replaceChildren();
    const assessed =
      peerRole(state) === "candidate" ? "interviewer" : "candidate";
    for (const dimension of dimensions[assessed]) {
      const field = document.createElement("fieldset");
      const legend = document.createElement("legend");
      legend.textContent = dimensionLabel(dimension);
      field.append(legend);
      for (const [name, label] of [
        ["score", "Rating"],
        ["example", "Concrete example"],
        ["improvement", "One improvement"],
      ]) {
        const wrapper = document.createElement("label");
        wrapper.textContent = label;
        const input = document.createElement(
          name === "score" ? "select" : "textarea",
        );
        input.name = `${dimension}:${name}`;
        if (name === "score") {
          for (const value of ["not_observed", "1", "2", "3", "4"]) {
            const option = document.createElement("option");
            option.value = value;
            option.textContent =
              value === "not_observed" ? "Not observed" : value;
            input.append(option);
          }
        }
        wrapper.append(input);
        field.append(wrapper);
      }
      byId("peer-dimensions").append(field);
    }
  }
  const answer = await peerApi(`${roundPath()}/feedback`);
  if (stopped || round !== loadingRound) return;
  const own = answer.feedback?.find(
    (entry) => entry.authorSlot === state.you.slot,
  );
  if (feedbackHydratedRound !== round) {
    if (!feedbackEdited && own?.payload?.dimensions) {
      for (const control of byId("peer-feedback").elements) {
        const [dimension, field] = control.name.split(":");
        const value = own.payload.dimensions[dimension]?.[field];
        if (value !== undefined) control.value = String(value);
      }
    }
    feedbackHydratedRound = round;
  }
  feedbackSubmitted = own?.submitted === true;
  for (const control of byId("peer-feedback").elements)
    control.disabled = feedbackSubmitted;
  // Round 0's form is replaced when the roles swap, so it is submitted first.
  byId("peer-advance").disabled =
    (round === 0 && !feedbackSubmitted) || advanceRequestedRound === round;
  renderPartnerFeedback(answer.feedback);
}
function dimensionLabel(dimension) {
  return dimension.replaceAll(/([A-Z])/g, " $1").toLowerCase();
}
function renderPartnerFeedback(entries) {
  const target = byId("peer-feedback-results");
  const theirs = (entries ?? []).filter(
    (entry) => entry.authorSlot !== state.you.slot && entry.payload?.dimensions,
  );
  if (!theirs.length) {
    const waiting = document.createElement("p");
    waiting.className = "muted";
    waiting.textContent =
      "Your partner's feedback appears after both submit, or 24 hours after this round closes.";
    target.replaceChildren(waiting);
    return;
  }
  const heading = document.createElement("h3");
  heading.textContent = "Your partner's feedback";
  const list = document.createElement("dl");
  list.className = "peer-feedback-read";
  finishFeedbackWatch();
  for (const [dimension, answer] of Object.entries(
    theirs[0].payload.dimensions,
  )) {
    const term = document.createElement("dt");
    term.textContent = dimensionLabel(dimension);
    const detail = document.createElement("dd");
    const rating = document.createElement("strong");
    rating.textContent =
      answer.score === undefined || answer.score === "not_observed"
        ? "Not observed"
        : `Rating ${answer.score} of 4`;
    detail.append(rating);
    for (const [field, label] of [
      ["example", "Example"],
      ["improvement", "To improve"],
    ]) {
      if (!answer[field]) continue;
      const line = document.createElement("p");
      line.textContent = `${label}: ${answer[field]}`;
      detail.append(line);
    }
    list.append(term, detail);
  }
  target.replaceChildren(heading, list);
}
byId("peer-feedback").addEventListener("input", () => {
  feedbackEdited = true;
});
byId("peer-feedback").addEventListener("submit", (event) => {
  event.preventDefault();
  if (feedbackSubmitted) return;
  const data = new FormData(event.currentTarget);
  const payload = {};
  for (const [key, value] of data) {
    const [dimension, field] = key.split(":");
    payload[dimension] ??= {};
    payload[dimension][field] =
      field === "score" && value !== "not_observed" ? Number(value) : value;
  }
  void peerApi(
    `${roundPath()}/feedback`,
    { submitted: event.submitter?.value === "submit", dimensions: payload },
    "PUT",
  )
    .then(loadFeedback)
    .catch(error);
});
byId("peer-stop-saved-audio").addEventListener("click", () => {
  byId("peer-stop-saved-audio").disabled = true;
  void peerApi(`${base}/audio`, {}, "DELETE")
    .then(refreshSavedAudio)
    .catch(error)
    .finally(() => {
      byId("peer-stop-saved-audio").disabled = false;
    });
});
byId("peer-session-consent").addEventListener("change", updatePrestart);
for (const [id, action] of [
  ["peer-session-enter", "enter"],
  ["peer-session-ready", "ready"],
  ["peer-session-start", "start"],
]) {
  byId(id).addEventListener("click", () => {
    const payload =
      action === "start"
        ? { expectedVersion: state.version, observerConsent: true }
        : action === "ready"
          ? { ready: true, expectedVersion: state.version }
          : {};
    void peerApi(`${base}/${action}`, payload)
      .then(() => refresh())
      .catch(error);
  });
}
byId("peer-connect").addEventListener(
  "click",
  () => void connect().catch(error),
);
const toggleMute = createMuteToggle({
  mute: (value) => audio.mute(value),
  onChange(value) {
    byId("peer-mute").setAttribute("aria-pressed", String(value));
  },
  onError: error,
});
byId("peer-mute").addEventListener("click", toggleMute);
byId("peer-device").addEventListener(
  "change",
  () => void audio.device(byId("peer-device").value).catch(error),
);
byId("peer-finish").addEventListener("click", () => {
  void peerApi(`${roundPath()}/finish`, { expectedVersion: state.version })
    .then(async () => {
      if (lease) await save(true);
      await refresh();
    })
    .catch(error);
});
byId("peer-advance").addEventListener("click", () => {
  const requested = round;
  advanceRequestedRound = requested;
  byId("peer-advance").disabled = true;
  byId("peer-advance").textContent = "Waiting for your partner to confirm";
  void peerApi(`${base}/advance`, { expectedVersion: state.version })
    .then(() => refresh())
    .catch((failure) => {
      if (advanceRequestedRound === requested) advanceRequestedRound = -1;
      byId("peer-advance").disabled = false;
      error(failure);
    });
});
byId("peer-recover-writer").addEventListener(
  "click",
  () => void recoverWriter(),
);
byId("peer-download").addEventListener("click", () => {
  const url = URL.createObjectURL(
    new Blob([nodes.editor.value], { type: "text/plain" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = `peer-round-${round + 1}-unsaved.txt`;
  link.click();
  URL.revokeObjectURL(url);
});
async function stop() {
  connectionGeneration++;
  stopped = true;
  for (const timer of timers) clearInterval(timer);
  stopFeedbackWatch();
  clearTimeout(previewTimer);
  artifacts?.stop();
  workspace.destroy();
  await audio.disconnect();
}
byId("peer-exit").addEventListener("click", () => {
  void (async () => {
    if (lease && dirty) await save(false);
    await peerApi(`${base}/leave`, {});
    await stop();
    location.assign(`/peer-lobby?session=${encodeURIComponent(id)}`);
  })().catch(error);
});
window.addEventListener("pagehide", (event) => {
  if (!event.persisted) {
    void stop();
    return;
  }
  paused = true;
  connectionGeneration++;
  for (const timer of timers.splice(0)) clearInterval(timer);
  stopFeedbackWatch();
  clearTimeout(previewTimer);
  previewTimer = null;
  audioConnected = false;
  room = null;
  workspace.setReadOnly(true);
  void audio.disconnect().catch(error);
});
window.addEventListener("pageshow", (event) => {
  if (!event.persisted || stopped) return;
  paused = false;
  void refresh(true).then(startTimers).catch(error);
});
window.addEventListener("beforeunload", (event) => {
  if (dirty) {
    event.preventDefault();
    event.returnValue = "";
  }
});
function startTimers() {
  if (terminal || paused || stopped || timers.length) return;
  timers.push(setInterval(() => void refresh().catch(error), 5000));
  timers.push(
    setInterval(() => {
      if (lease && state?.currentRound?.status === "active")
        void save(false).catch(error);
    }, 5000),
  );
  timers.push(
    setInterval(
      () => void peerApi(`${base}/heartbeat`, {}).catch(error),
      15000,
    ),
  );
  timers.push(setInterval(paintClock, 1000));
}
void refresh().catch(error).finally(startTimers);
