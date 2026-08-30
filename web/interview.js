import { loadProblem } from "./problem-data.js";
import {
  MIC_CONFIRM_FRAMES,
  mediaReadiness,
  outputUsable,
  peakLevel,
  videoTrackReady,
  sustainedPeak,
} from "./audio-check.js";
import { highlight } from "./highlight.js";
import { indentSelection } from "./editor.js";
import { createDevicePool } from "./devices.js";
import { createFaceCheck } from "./face-check.js";
import {
  createTranscriptView,
  finalRunnerStatus,
  problemMarkup,
  reportMarkdown,
  reportMarkup,
  resultsMarkup,
  runnerStatusMarkup,
} from "./render.js";
import {
  acceptsReport,
  clamp,
  codeUpdatePayload,
  countdown,
  endInterviewPayload,
  formatTime,
  integrityEventPayload,
  isAgent,
  resumeDeadline,
  sanitizeReport,
  sessionReport,
  testPayload,
  timeWarningPayload,
  topics,
} from "./lib.js";
import {
  attachAvatarAnalyser,
  initAvatarStage,
  isAvatarAnalyserTrack,
  releaseAvatarAnalyser,
  setAvatarExpression,
  setAvatarSpeaking,
  startAvatar,
  stopAvatar,
} from "./avatar/stage.js";
import {
  flushReplay,
  initReplay,
  recordAvatarState,
  recordConsent,
  recordReplay,
  recordStage,
  recordStageTick,
} from "./replay-feed.js";
import {
  AUDIO_OUTPUT_KEY,
  MEET_PRESENTATION_KEY,
  applyAudioOutput,
  initAudioOutput,
  readStored,
  refreshAudioOutputs,
  writeStored,
} from "./audio-output.js";
import {
  PRESENCE_EVENTS,
  initIntegrity,
  monitorIntegrityTracks,
  startIntegrityHeartbeat,
  startIntegrityWorker,
  stopIntegrityWorker,
} from "./integrity.js";
import { initCaptions, showCaptions, updateCaptions } from "./captions.js";
import {
  consentGiven,
  initRecording,
  startRecording,
  withdrawRecordingConsent,
} from "./recording-state.js";
import { saveReportHistory } from "./history.js";
import { createFacePresenceDetector, facePresenceVerdict } from "./face-presence.js";
import { runBrowserTests } from "./runners.js";
import { mountBehavioralReview } from "./behavioral-review.js";

const languages = ["python", "javascript", "c", "cpp", "java"];
let editorInitialized = false;
let codePublishTimer = null;
// The agent reads the editor once per 2s watch tick, so publishing every
// keystroke sends ~10x more full-buffer packets than anyone consumes.
const CODE_PUBLISH_DEBOUNCE_MS = 300;

// From /runtime-config.js, which is the only thing allowed to name what the
// server does. A literal here would be a second answer to "does this server
// record", and the wrong one would show a candidate no notice.
const recordingEnabled = globalThis.CODETRIAL_RECORDING_ENABLED === true;
const consentVersion = globalThis.CODETRIAL_CONSENT_VERSION || "";
/// The envelope version the server accepts. It travels through the runtime
/// config for the same reason the consent version does: two spellings of a
/// version are one deploy away from disagreeing, and this one decides whether
/// every event is refused.
const replayVersion = globalThis.CODETRIAL_REPLAY_VERSION || 1;

const params = new URLSearchParams(window.location.search);
// Start this independent request while the problem data is loading. It has to
// settle rather than reject: nothing awaits it until init() reaches the sign-in
// gate, and a problem that fails to load throws out of the top-level await
// below so nothing ever will, leaving a rejected promise no handler ever
// claims. Null means "no answer", which is not a reason to bounce anyone.
const sessionPromise = fetch("/api/session")
  .then((response) => (response.ok ? response.json() : null))
  .catch(() => null);
// Top-level await: the whole module is written against a known problem, and
// interview.html loads it as a module, so waiting here beats threading a
// promise through every consumer below.
//
// The failure has to be said out loud. Every consumer below reads `problem`
// unconditionally, so an unreachable bank would otherwise leave the page on its
// "Loading interview..." heading forever with a console error nobody sees.
const problem = await loadProblem(params.get("problem")).catch((error) => {
  const title = document.querySelector("#problem-title");
  if (title) title.textContent = "This interview could not load its problem. Reload the page.";
  throw error;
});
const durationMin = clamp(Number.parseInt(params.get("duration") || "45", 10) || 45, 10, 90);
const mode = params.get("mode") === "practice" ? "practice" : "scored";
const interviewProfile = {
  role: params.get("role") || "",
  seniority: params.get("seniority") || "",
  targetCompany: params.get("company") || "",
};
const state = {
  mode,
  paused: false,
  pausedAt: 0,
  codeByLanguage: { ...problem.starterCode },
  language: "python",
  remaining: durationMin * 60,
  // Wall-clock deadline, set when the interview starts. The countdown is
  // derived from it rather than accumulated, so throttling and suspend cannot
  // bend it.
  endsAt: 0,
  phase: "live",
  transcript: null, // createTranscriptView, built in init once nodes exist
  latestSummary: null,
  testStatus: "done",
  report: null,
  room: null,
  connected: false,
  /// The consent this interview is running under, `null` where the server
  /// records nothing. The withdrawal button needs it and has nowhere else to
  /// read it from.
  interviewId: null,
  micEnabled: true,
  localUserStream: null,
  integrityChain: { seq: 0, hash: "" },
  integrityPublish: Promise.resolve(),
  integrityHeartbeatTimer: null,
  integrityTrackStateTimer: null,
  integrityTrackStates: new Map(),
  integrityWorker: null,
  integrityFrameSamplers: [],
  integrityAnalysisDetail: "not_configured",
  // Distinguishes "the interviewer has not arrived" from "the interviewer left".
  sawAgent: false,
  agentIdentity: null,
  // Set once the room is joined and never cleared. Offline practice may score
  // itself; a real interview may not be scored by the browser.
  joinedRoom: false,
  // What face presence last had to say, kept so the interviewer taking the
  // element over defers that warning instead of discarding it.
};

const nodes = {
  title: document.querySelector("#problem-title"),
  meta: document.querySelector("#problem-meta"),
  problemPanel: document.querySelector("#problem-panel"),
  transcriptPanel: document.querySelector("#transcript-panel"),
  problemTab: document.querySelector("#problem-tab"),
  transcriptTab: document.querySelector("#transcript-tab"),
  agentState: document.querySelector("#agent-state"),
  presenceBanner: document.querySelector("#presence-banner"),
  captionsBar: document.querySelector("#captions-bar"),
  captionsText: document.querySelector("#captions-text"),
  timer: document.querySelector("#timer"),
  practiceGuide: document.querySelector("#practice-guide"),

  mic: document.querySelector("#mic"),
  pause: document.querySelector("#pause"),
  retry: document.querySelector("#retry"),
  end: document.querySelector("#end"),
  withdrawConsent: document.querySelector("#withdraw-consent"),
  recordingState: document.querySelector("#recording-state"),
  editor: document.querySelector("#editor"),
  editorHighlight: document.querySelector("#editor-highlight"),
  editorLines: document.querySelector("#editor-lines"),
  compileDisclosure: document.querySelector(".compile-disclosure"),
  run: document.querySelector("#run-tests"),
  resultsLabel: document.querySelector("#results-label"),
  resultsBody: document.querySelector("#results-body"),
  resultsToggle: document.querySelector("#results-toggle"),
  resultsChevron: document.querySelector("#results-chevron"),
  ending: document.querySelector("#ending-overlay"),
  forceReport: document.querySelector("#force-report"),
  leaveRoom: document.querySelector("#leave-room"),
  endingDetail: document.querySelector("#ending-detail"),
  report: document.querySelector("#report-modal"),
  audioCheck: document.querySelector("#audio-check"),
  audioTestTone: document.querySelector("#audio-test-tone"),
  audioHeard: document.querySelector("#audio-heard"),
  audioMeterFill: document.querySelector("#audio-meter-fill"),
  audioStatus: document.querySelector("#audio-check-status"),
  audioOutputState: document.querySelector("#audio-output-state"),
  audioStepOutput: document.querySelector("#audio-step-output"),
  audioStepMic: document.querySelector("#audio-step-mic"),
  audioStepCamera: document.querySelector("#audio-step-camera"),
  cameraState: document.querySelector("#camera-state"),
  cameraIntegrityVideo: document.querySelector("#camera-integrity-video"),
  audioJoin: document.querySelector("#audio-check-join"),
  meetPresentation: document.querySelector("#meet-presentation"),
  recordingConsentStep: document.querySelector("#recording-consent-step"),
  recordingConsent: document.querySelector("#recording-consent"),
  meetMode: document.querySelector("#meet-mode"),
  meetOutputRow: document.querySelector("#meet-output-row"),
  meetOutputSelect: document.querySelector("#meet-output-select"),
  meetOutputNote: document.querySelector("#meet-output-note"),
  jimAvatar: document.querySelector("#jim-avatar"),
  jimAvatarNote: document.querySelector("#jim-avatar-note"),
};

// Jim's audio element. Named because setSinkId needs a stable target and the
// avatar analyser will need the same one; an anonymous appendChild left neither
// with a handle.
let jimAudio = null;


// Before `init`, because the queue's first producer is inside it. The bindings
// are handed over rather than re-derived: one `state` object, one `nodes` map.
initReplay({ state, nodes, problem, recordingEnabled, consentVersion, replayVersion });
initCaptions({ nodes });
initAvatarStage({ nodes });
initAudioOutput({ nodes, jimAudioElement: () => jimAudio });
initRecording({ state, nodes, recordingEnabled, addTranscript, setBanner });
initIntegrity({
  state,
  nodes,
  updatePresenceBanner,
  publishIntegrityEvent,
  endInterview,
});

init();

async function init() {
  state.transcript = createTranscriptView(document, nodes.transcriptPanel);
  renderRuntimeConfig();
  renderProblem();
  setLanguage("python");
  bindEvents();
  // No enumeration here. Before the preflight grant the browser reports only a
  // blank placeholder, so the list is discarded and repainted a few lines down;
  // the `toggle` handler covers a panel opened while the preflight is still up.
  // Checked before the media gate, not after: /api/token would refuse anyway,
  // and making someone prove their microphone first only to bounce them is a
  // waste of their time.
  if (await signInRequired()) return;
  // Before the media check, not after it. The media check is an overlay over an
  // already laid out shell, so the mount is measurable here, and the candidate
  // is about to spend ten to thirty seconds playing a tone and granting a
  // camera. The avatar needs roughly that long: 10.9 MB of model and a few
  // seconds of glTF parse. Starting it afterwards is why Jim's face turned up a
  // dozen seconds into the interview, behind the neutral panel, which is
  // indistinguishable from an avatar that is never coming.
  startAvatar();
  const preflight = await runAudioCheck();
  // Before the room, so nothing downstream ever sees the camera: the publisher,
  // the watchers and the heartbeat all read the stream, and handing them a
  // track that is about to vanish is what made this a special case everywhere.
  const presenting = nodes.meetPresentation.checked;
  if (presenting) releaseCameraToPresenter(preflight.userStream);
  // Now that the preflight holds a grant, the browser reports real device ids
  // and labels. Before this point the list is empty or a blank placeholder.
  void refreshAudioOutputs();
  // Arms the idle timer for the "Listening..." placeholder too, so the corner
  // is clear by the time the candidate is actually working rather than holding
  // a caption nobody said.
  showCaptions();
  await connect(preflight, presenting);
  publish(topics.code, codeUpdatePayload(currentCode(), state.language));
  state.endsAt = Date.now() + durationMin * 60 * 1000;
  tickTimer();
  setInterval(tickTimer, 1000);
}

function renderRuntimeConfig() {
  // No origin literal here: /runtime-config.js supplies it, and the server is
  // the only place that may name it, because the same value builds the CSP.
  const enabled = globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED !== false
    && globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL !== "";
  nodes.compileDisclosure.textContent = enabled
    ? "C, C++ and Java runs are sent to Compiler Explorer."
    : "C, C++ and Java test runs are disabled by this server.";
}

// Assigning `value` wipes the native undo stack, and a candidate who indents
// once has then lost their whole editing history. Route the change through the
// browser's editing command instead, over just the span that actually changed.
function applyIndent(next) {
  const editor = nodes.editor;
  const previous = editor.value;
  if (next.value === previous) {
    editor.setSelectionRange(next.start, next.end);
    return;
  }
  let head = 0;
  while (previous[head] === next.value[head]) head += 1;
  let tail = 0;
  while (tail < Math.min(previous.length, next.value.length) - head
    && previous[previous.length - 1 - tail] === next.value[next.value.length - 1 - tail]) tail += 1;
  const inserted = next.value.slice(head, next.value.length - tail);
  editor.setSelectionRange(head, previous.length - tail);
  const rewritten = inserted
    ? document.execCommand?.("insertText", false, inserted)
    : document.execCommand?.("delete");
  if (!rewritten) {
    editor.value = next.value;
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  }
  editor.setSelectionRange(next.start, next.end);
}

function bindEvents() {
  nodes.problemTab.addEventListener("click", () => selectTab("problem"));
  nodes.transcriptTab.addEventListener("click", () => selectTab("transcript"));
  nodes.mic.addEventListener("click", toggleMicrophone);
  if (mode === "practice") {
    nodes.pause.hidden = false;
    nodes.retry.hidden = false;
    nodes.practiceGuide.hidden = false;
    nodes.pause.addEventListener("click", togglePause);
    nodes.retry.addEventListener("click", retryPractice);
  } else {
    // Hidden controls are still discoverable DOM and accessibility content.
    // A scored interview has no coaching surface at all.
    nodes.pause.remove();
    nodes.retry.remove();
    nodes.practiceGuide.remove();
  }
  nodes.end.addEventListener("click", () => endInterview("candidate_ended"));
  nodes.withdrawConsent.addEventListener("click", withdrawRecordingConsent);
  nodes.forceReport.addEventListener("click", showReport);
  nodes.leaveRoom.addEventListener("click", leaveRoom);
  nodes.run.addEventListener("click", runTests);
  nodes.meetOutputSelect.addEventListener("change", () => {
    void applyAudioOutput(nodes.meetOutputSelect.value);
  });
  // The checkbox is the only copy of this answer. A mirrored state field had to
  // be written in two places and read in four, and the one reader that forgot
  // it published a wrong camera status for the whole interview.
  nodes.meetPresentation.checked = readStored(MEET_PRESENTATION_KEY) === "1";
  nodes.meetPresentation.addEventListener("change", () => {
    writeStored(MEET_PRESENTATION_KEY, nodes.meetPresentation.checked ? "1" : "0");
  });
  // Device labels and ids stay blank until a getUserMedia grant, so the list
  // built at load is stale by the time anyone opens the panel.
  nodes.meetMode.addEventListener("toggle", () => {
    if (nodes.meetMode.open) void refreshAudioOutputs();
  });
  navigator.mediaDevices?.addEventListener?.("devicechange", () => void refreshAudioOutputs());
  nodes.resultsToggle.addEventListener("click", () => {
    nodes.resultsBody.hidden = !nodes.resultsBody.hidden;
    nodes.resultsChevron.textContent = nodes.resultsBody.hidden ? "^" : "v";
  });
  for (const button of document.querySelectorAll("[data-language]")) {
    button.addEventListener("click", () => setLanguage(button.dataset.language));
  }
  // The overlay does not scroll on its own; it follows the textarea.
  nodes.editor.addEventListener("scroll", () => {
    nodes.editorHighlight.scrollTop = nodes.editor.scrollTop;
    nodes.editorHighlight.scrollLeft = nodes.editor.scrollLeft;
    nodes.editorLines.scrollTop = nodes.editor.scrollTop;
  });
  // Tab indents, so it cannot also move focus. Escape arms the next Tab to do
  // that instead, or a candidate driving the page from the keyboard is stuck
  // in the textarea with no way out.
  let tabLeavesEditor = false;
  nodes.editor.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      tabLeavesEditor = true;
      return;
    }
    // A modifier's own keydown must not disarm it. Shift fires one of its own
    // before Tab does, so without this Escape then Shift+Tab outdents instead
    // of moving focus backwards, and reverse tab order has no way out at all.
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    const indents = event.key === "Tab" && !tabLeavesEditor;
    tabLeavesEditor = false;
    if (!indents) return;
    event.preventDefault();
    applyIndent(indentSelection(nodes.editor.value, nodes.editor.selectionStart, nodes.editor.selectionEnd, event.shiftKey));
  });
  nodes.editor.addEventListener("input", () => {
    state.codeByLanguage[state.language] = nodes.editor.value;
    paintEditor();
    clearTimeout(codePublishTimer);
    codePublishTimer = setTimeout(() => {
      publish(topics.code, codeUpdatePayload(currentCode(), state.language, Date.now()));
      codePublishTimer = null;
    }, CODE_PUBLISH_DEBOUNCE_MS);
  });
  nodes.report.addEventListener("click", (event) => {
    if (event.target.closest("#done")) window.location.assign("/");
    if (event.target.closest("#download-report")) downloadReport();
  });
}

/// True when nobody is signed in, in which case the candidate is sent back to
/// the home page to sign in before a LiveKit token can be minted.
async function signInRequired() {
  const session = await sessionPromise;
  // Offline practice still works; the token request is the real gate.
  if (!session || session.signedIn || !session.loginRequired) return false;
  nodes.audioCheck.hidden = true;
  nodes.agentState.textContent = "Sign in required";
  addTranscript("interviewer", "Enter your GitHub username on the home page to start an interview.", true);
  window.location.replace("/");
  return true;
}

/// Resolves once the candidate has proven output, microphone, and camera.
/// The tone doubles as the user gesture browsers require before
/// any audio plays, so confirming it also unblocks the interviewer's voice.
function runAudioCheck() {
  return new Promise((resolve) => {
    const mediaDevices = navigator.mediaDevices;
    const browserSupported = Boolean(mediaDevices?.getUserMedia);
    let outputConfirmed = false;
    let meterRetry = null;
    let advanced = false;

    // Built before anything reads it: `refresh` runs on the first frame and
    // asks the pool what the devices are doing.
    const pool = createDevicePool({
      mediaDevices,
      isFinished: () => finished,
      onChange: () => refresh(),
    });
    const trackOf = (kind) => pool.trackOf(kind);
    // Derived, not stored. This was a `let` written inside `sampleReadiness`
    // and read from the face check, so whether the camera worked depended on
    // who had run most recently.
    const cameraReady = () => videoTrackReady(trackOf("video"));
    let micPeak = 0;
    const faceCheck = createFaceCheck({
      video: nodes.cameraIntegrityVideo,
      trackOf: () => trackOf("video"),
      isReady: cameraReady,
      isFinished: () => finished,
      createDetector: createFacePresenceDetector,
      verdictOf: facePresenceVerdict,
      onVerdict: () => refresh(),
    });
    let context = null;
    let hint = null;
    let hintUntil = 0;
    let finished = false;
    const recentPeaks = [];
    // Set once, because refresh() runs on every animation frame: the
    // candidate is the only reliable output sensor, so confirming is
    // accepted whenever they click rather than gated on tone timing.
    nodes.audioHeard.removeAttribute("aria-disabled");
    nodes.audioHeard.classList.remove("waiting");

    // Long enough to read, short enough that it cannot hide the live blocker.
    // refresh() runs on every animation frame, so the status returns by itself.
    const showHint = (text) => {
      hint = text;
      hintUntil = Date.now() + 6000;
      refresh();
    };

    // Reads all eight signals in one call, so a new signal is added here and
    // both callers get it. The camera's error is the one thing still written on
    // the way through: it is the only signal that can only be judged against a
    // track the candidate already granted.
    const sampleReadiness = () => {
      // Keyed on the track, not on the pool's stream. The stream exists from
      // the moment the preflight asks for a device, so testing it here would
      // overwrite the reason the request actually failed -- the permission the
      // candidate denied -- with "no active video track" on every frame, and
      // paint the panel red while the prompt is still on screen.
      const camera = trackOf("video");
      if (camera) pool.setError("video", cameraReady() ? null : "no active video track");
      // An ended track never revives, and while it sits in the stream the retry
      // sees a device of that kind and asks for nothing. The gate has no bypass,
      // so an unplugged device would strand the candidate. A muted track can
      // come back on its own, so only the ended one is dropped.
      //
      // Both kinds, one rule. The camera is dropped here rather than left to the
      // retry tick because its liveness is judged per frame just above; the
      // microphone has no such judgement, because a meter over a device that
      // went away reports silence rather than an error. `dropTrack` fires
      // `onLost`, so whatever was running over the track is torn down with it.
      for (const kind of ["audio", "video"]) {
        const track = trackOf(kind);
        if (track?.readyState !== "ended") continue;
        pool.dropTrack(track);
        pool.retry();
      }
      return mediaReadiness({
        browserSupported,
        outputConfirmed,
        micPeak,
        micError: pool.errorOf("audio"),
        cameraReady: cameraReady(),
        cameraError: pool.errorOf("video"),
        faceReady: faceCheck.ready,
        faceError: faceCheck.error,
      });
    };

    const refresh = () => {
      const state = sampleReadiness();
      if (hint && Date.now() >= hintUntil) hint = null;
      nodes.audioStatus.textContent = hint || state.message;
      nodes.audioOutputState.textContent = state.steps.output ? "Confirmed" : "play a short tone.";
      nodes.cameraState.textContent = state.steps.camera ? "Ready" : "grant access and keep video on.";
      nodes.audioStatus.classList.toggle("critical", ["mic-error", "camera-error", "face-error", "browser"].includes(state.blocker));
      nodes.audioStepOutput.classList.toggle("done", state.steps.output);
      nodes.audioStepMic.classList.toggle("done", state.steps.mic);
      nodes.audioStepCamera.classList.toggle("done", state.steps.camera);
      nodes.audioHeard.disabled = state.steps.output;
      nodes.audioHeard.textContent = state.steps.output ? "Confirmed" : "I heard it";
      // Consent is a separate gate from media readiness on purpose. It is not
      // a device that can be proven, it is an answer, and folding it into
      // `mediaReadiness` would put a legal question inside the function that
      // decides whether a microphone works.
      const ready = state.ready && consentGiven();
      nodes.audioJoin.disabled = !ready;

      // refresh() runs on every animation frame from the meter, so hand over
      // once on the transition rather than stealing focus continuously. The
      // button the candidate just used is now disabled, so focus would
      // otherwise fall back to the document body.
      if (ready && !advanced) {
        advanced = true;
        nodes.audioJoin.classList.add("ready");
        nodes.audioJoin.focus();
      } else if (!ready && advanced) {
        advanced = false;
        nodes.audioJoin.classList.remove("ready");
      }
    };


    const finish = () => {
      if (!sampleReadiness().ready) return;
      finished = true;
      nodes.audioCheck.hidden = true;
      clearTimeout(meterRetry);
      pool.cancelRetry();
      faceCheck.close();
      void context?.close().catch(() => {});
      resolve({ userStream: pool.stream });
    };

    // Browsers keep audio blocked until a user gesture, and the tone is the
    // usual one. It is no longer the only way to confirm output, so both paths
    // run this: a candidate who clicks straight through must not reach the room
    // with the browser still refusing to play the interviewer.
    //
    // A context built inside a gesture is already running, so the common path
    // never awaits. Awaiting first is what left the confirm button disabled
    // while the tone was already audible.
    const unblockOutput = async () => {
      context ||= new (window.AudioContext || window.webkitAudioContext)();
      if (!outputUsable(context.state)) {
        await context.resume().catch(() => {});
      }
      return outputUsable(context.state);
    };

    nodes.audioTestTone.addEventListener("click", async () => {
      if (!(await unblockOutput())) {
        showHint("The browser is still blocking audio. Click the tone again.");
        return;
      }
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      oscillator.frequency.value = 440;
      gain.gain.value = 0.1;
      oscillator.connect(gain).connect(context.destination);
      oscillator.start();
      oscillator.stop(context.currentTime + 0.4);
      hint = null;
      nodes.audioTestTone.textContent = "Play it again";
      refresh();
    });

    // Bound to both events because a mouse click fires each of them and
    // pointerdown is the earliest gesture available to unblock audio, so the
    // second call is expected and returns immediately.
    const confirmOutput = () => {
      if (outputConfirmed) return;
      // Called synchronously so the AudioContext is built inside the gesture.
      void unblockOutput();
      outputConfirmed = true;
      hint = null;
      refresh();
    };
    nodes.audioHeard.addEventListener("pointerdown", confirmOutput);
    nodes.audioHeard.addEventListener("click", confirmOutput);

    nodes.audioJoin.addEventListener("click", finish);

    // Shown only where the server records. A consent step on a server that
    // records nothing asks for permission nobody needs and teaches candidates
    // to click past it.
    nodes.recordingConsentStep.hidden = !recordingEnabled;
    nodes.recordingConsent.addEventListener("change", refresh);

    // One meter at a time. The pool can hand back a replacement microphone
    // now, and both ways back into here -- a fresh track and the meter's own
    // retry -- would otherwise leave the previous AudioContext reading the
    // track that went away and repainting the bar from it every frame.
    let meterGeneration = 0;
    const watchMic = () => {
      const generation = ++meterGeneration;
      return startMediaMeter(
        pool.stream,
        (peak) => {
          pool.setError("audio", null);
          recentPeaks.push(peak);
          if (recentPeaks.length > MIC_CONFIRM_FRAMES) recentPeaks.shift();
          // Latched once proven: the candidate should not have to keep talking
          // to hold the Start button open while they read the screen.
          micPeak = Math.max(micPeak, sustainedPeak(recentPeaks));
          nodes.audioMeterFill.style.width = `${Math.min(100, Math.round(peak * 300))}%`;
          refresh();
        },
        (error) => {
          pool.setError("audio", error);
          // A device that went away has not proven anything about the one that
          // replaces it.
          micPeak = 0;
          recentPeaks.length = 0;
          // The hint outranks the status line on every frame, so a stale one
          // would leave the bar red and still reading "play the test tone".
          hint = null;
          refresh();
          // The gate has no bypass, so it must recover on its own once the
          // candidate grants access or plugs a device back in. Distinct from
          // the pool's retry: this one restarts the level meter over a track we
          // already hold, that one asks the browser for a device again.
          meterRetry = setTimeout(watchMic, 2000);
        },
        () => finished || generation !== meterGeneration,
      );
    };
    // A live track is enough for the microphone: the meter is what proves one
    // actually carries sound, and it runs for the rest of the preflight.
    pool.configure("audio", {
      accept: (track) => Boolean(track),
      onTrack: () => watchMic(),
      onLost: () => {
        meterGeneration += 1;
        micPeak = 0;
        recentPeaks.length = 0;
        clearTimeout(meterRetry);
        nodes.audioMeterFill.style.width = "0%";
        refresh();
      },
    });
    // Granted is not working. A track that arrives already ended or muted would
    // paint the step green over a black square.
    pool.configure("video", {
      accept: (track) => videoTrackReady(track),
      onTrack: () => void faceCheck.start(),
      // The pool drops an ended camera on its own now. A check still bound to
      // that track would keep judging its last frame, and `start` refuses to
      // run while one is already running, so the replacement would never be
      // watched at all.
      onLost: () => faceCheck.reset(),
    });
    pool.start();
    refresh();
  });
}

async function startMediaMeter(stream, onPeak, onError, shouldStop) {
  try {
    const context = new (window.AudioContext || window.webkitAudioContext)();
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    context.createMediaStreamSource(stream).connect(analyser);
    const samples = new Uint8Array(analyser.frequencyBinCount);
    const tick = () => {
      if (shouldStop()) {
        void context.close().catch(() => {});
        return;
      }
      analyser.getByteTimeDomainData(samples);
      onPeak(peakLevel(samples));
      requestAnimationFrame(tick);
    };
    tick();
  } catch (error) {
    onError(String(error?.message || error));
  }
}

async function connect(preflight, presenting = false) {
  setAgentStateLabel("Connecting...");
  try {
    // Before the token, because the token is what precedes an Egress call. The
    // server refuses a recorded room without this row, so the ordering is
    // enforced there rather than resting on this line staying above the next
    // one.
    // The gate the Start button enforces, restated where it is load bearing.
    // The button is UI; this is the function that actually asks the server to
    // write consent down, and it must not be reachable without one.
    if (!consentGiven()) throw new Error("Agree to the recording notice before starting.");
    const interviewId = await recordConsent();
    state.interviewId = interviewId;
    const response = await fetch("/api/token", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ problemId: problem.id, durationMin, interviewId, mode, interviewProfile }),
    });
    if (!response.ok) throw new Error((await response.json()).error || "Failed to create a session.");
    const connection = await response.json();
    await connectLiveKit(connection, preflight, presenting);
    state.connected = true;
    // After the room, not before it. The server refuses to record an interview
    // that has not claimed a room, so starting here rather than alongside the
    // consent request is the difference between a recording and a failure
    // nobody asked for.
    // Both conditions said out loud. `recordConsent` returns null where the
    // server records nothing, so the id alone is enough today, and a reader
    // should not have to know that to see why this is safe.
    if (recordingEnabled && state.interviewId) {
      nodes.withdrawConsent.hidden = false;
      await startRecording();
      // First event of the replay, so a template that joins late knows the
      // interview was already running rather than inferring it from silence.
      recordReplay("lifecycle", { state: "started" });
      // The problem was rendered in the lobby, before this interview existed,
      // so its heading was dropped. Without this the recording shows a blank
      // title for the whole interview.
      recordStage();
      // The starter code, once, so a candidate who never types is not recorded
      // beside an empty editor.
      recordReplay("editor", { code: currentCode(), language: state.language });
    }
  } catch (error) {
    // Swallowed for the candidate, logged for everyone else. Offline practice
    // mode is a reasonable fallback but a terrible diagnosis: it looks the same
    // whether the server has no LiveKit credentials, the token was refused, or
    // the microphone failed to publish after a successful connect.
    console.warn("codetrial connect_failed", error);
    // Whatever threw may have thrown after `connectLiveKit` already joined and
    // published, and dropping the reference does not leave the room. That is
    // how a page ran "offline practice" with a live LiveKit room underneath it,
    // still holding the microphone and still receiving Jim, while nothing on
    // screen could act on either.
    const joined = state.room;
    state.connected = false;
    state.room = null;
    if (joined) void joined.disconnect?.()?.catch?.(() => {});
    stopPreflight(preflight);
    setAgentStateLabel("Offline");
    // Practice mode without a reason reads as the product working. The server
    // says why it refused, in words written for a candidate, and a busy server
    // is a "come back in a few minutes" rather than a "your interview is now a
    // simulation" - so say it, and keep the practice editor underneath it.
    setBanner("connection", `${error?.message || "The interview server could not be reached."} You can keep practising here in the meantime.`);
    addTranscript("interviewer", "Offline practice mode is ready. Talk through your approach and run tests when you are ready.", true);
  }
}



// The replay: what the candidate was looking at, sent to the server so the
// recording template can render it and the replay page can play it back. The
// video is the provider's; this is the part CodeTrial owns.
//
// Batched, because an event per keystroke is a request per keystroke. Thirty
// two events and a second are both the server's limits, not guesses.
/// `presenting` is threaded through explicitly. It was read here while only
/// `connect` had it in scope, so every single interview threw
/// `ReferenceError: presenting is not defined` immediately after joining the
/// room, fell into the offline-practice catch, and nulled `state.room` while
/// the real LiveKit room stayed connected underneath. The candidate saw Jim
/// greet them and then had no working interview: no code sync, no integrity
/// events, no end-of-interview control message, and a status pill reading
/// "Waiting" forever.
async function connectLiveKit(connection, preflight, presenting = false) {
  const livekit = window.LivekitClient;
  if (!livekit?.Room) throw new Error("LiveKit browser SDK is unavailable.");
  const room = new livekit.Room({ adaptiveStream: true, dynacast: true });

  room.on(livekit.RoomEvent.DataReceived, (payload, participant, kind, topic) => {
    if (topic === topics.control && isAgent(participant)) {
      receiveControl(payload);
      return;
    }
    if (!acceptsReport(topic, participant)) return;
    void receiveReport(room, payload);
  });
  room.on(livekit.RoomEvent.ParticipantAttributesChanged, updateAgentState);
  room.on(livekit.RoomEvent.ParticipantConnected, updateAgentState);
  // Both of these were missing, and the pill showed "Waiting" through a whole
  // interview while Jim talked: the label was written once at connect, before
  // the agent's participant record existed, and nothing ever rewrote it. An
  // agent that leaves now updates the label too, instead of leaving it claiming
  // an interviewer who is gone.
  room.on(livekit.RoomEvent.ParticipantDisconnected, updateAgentState);
  room.on(livekit.RoomEvent.TrackSubscribed, updateAgentState);
  // The room's own connection had no handlers at all, so a mid-interview drop
  // left `state.connected` true, the UI silent, and every `publish` quietly
  // discarding code updates, test results and integrity events. LiveKit retries
  // on its own, so the candidate is told to wait rather than to restart.
  room.on(livekit.RoomEvent.Reconnecting, () => {
    state.connected = false;
    setBanner("connection", "Reconnecting to the interview. Keep working; your code is safe.");
  });
  room.on(livekit.RoomEvent.Reconnected, () => {
    state.connected = true;
    setBanner("connection", "");
    // Everything held during the gap, in order, then the current buffer on top
    // so Jim is reading what the candidate is actually looking at rather than
    // whatever the last queued keystroke said.
    flushPendingPublishes();
    publish(topics.code, codeUpdatePayload(currentCode(), state.language));
    updateAgentState();
  });
  room.on(livekit.RoomEvent.Disconnected, () => {
    state.connected = false;
    // Terminal, not transient: LiveKit fires Reconnecting for a recoverable
    // blip and only reaches here when it has given up. Dropping the reference
    // is what makes the banner's own advice possible: `endInterview` hides the
    // offline-report button and skips the local report whenever `state.room` is
    // truthy, so leaving a dead room in place told the candidate to end the
    // interview and then left them waiting on an overlay for 55 seconds.
    state.room = null;
    setBanner("connection", "The interview connection dropped. End the interview to get your report.");
    console.warn("codetrial room_disconnected");
  });
  // The participant is needed to tell Jim from any other remote audio, so both
  // handlers take the full signature rather than just the track.
  room.on(livekit.RoomEvent.TrackSubscribed, (track, publication, participant) =>
    playRemoteAudio(track, participant));
  room.on(livekit.RoomEvent.TrackUnsubscribed, dropRemoteAudio);
  if (typeof room.registerTextStreamHandler === "function") {
    room.registerTextStreamHandler(topics.transcript, (reader, participant) =>
      consumeTranscript(room, reader, participant));
  }

  await room.connect(connection.serverUrl, connection.token);
  try {
    await publishPreflightTracks(room, preflight);
  } catch (error) {
    await room.disconnect().catch(() => {});
    throw error;
  }
  state.room = room;
  state.connected = true;
  // Sticky, unlike `state.room`, which a disconnect clears. This is what says
  // "an interviewer was involved in this session", and it is the only thing
  // allowed to decide whether the browser may produce a score.
  state.joinedRoom = true;
  state.localUserStream = preflight.userStream;
  nodes.mic.textContent = "Mic on";
  updateAgentState();
  monitorIntegrityTracks();
  await publishIntegrityEvent({ type: "SESSION_START", source: "media", severity: "info" });
  // The camera is named, not judged. Nothing here can tell a virtual camera
  // from a real one: `devices.js` asks for `video: true` with no device id, so
  // Chrome opens whatever is default, and a virtual camera satisfies the face
  // check with a composited feed this page never captured. Blocking on a list
  // of vendor names would be a list to maintain and a way to lock out a working
  // setup, so the label goes in the signed trail and a reader decides. Same
  // rule as the test counts: safe to narrate, unsafe to score.
  await publishIntegrityEvent({
    type: "MEDIA_PREFLIGHT_PASSED",
    source: "preflight",
    severity: "info",
    detail: `camera=${preflight.userStream?.getVideoTracks?.()[0]?.label || "none"}`,
  });
  startIntegrityHeartbeat();
  // Keyed on what the candidate chose, not on whether a camera happens to be
  // in the stream. A camera that died between the preflight and here leaves an
  // empty stream too, and recording that as a deliberate release would put a
  // claim in the signed trail that the candidate never made.
  if (presenting) {
    // Published as evidence rather than swallowed: a report with no face
    // events must not read like one where the camera watched and saw nothing.
    state.integrityAnalysisDetail = "camera_released";
    await publishIntegrityEvent({
      type: "CAMERA_RELEASED_TO_PRESENTER",
      source: "camera",
      severity: "warning",
      detail: "meet_presentation",
    });
  } else {
    // Unchanged for every other interview, including one whose camera failed:
    // the worker starts, the sampler finds no track, and the analyser detail
    // says so instead of claiming a release.
    startIntegrityWorker();
  }
}

/// Meet cannot open a camera CodeTrial is holding, and the preflight has
/// already proved the device works and that the candidate was in front of it.
///
/// The track is removed from the stream, not just stopped. "Released" and "not
/// in `getVideoTracks()`" are the same fact, and representing it twice meant
/// every reader of the camera had to learn about the flag separately. One of
/// them did not: the integrity heartbeat kept publishing `camera=ended` every
/// five seconds for the whole session. With the track gone the existing null
/// guards in the publisher, the watcher and the poller all do the right thing
/// on their own, and the next reader added will too.
function releaseCameraToPresenter(stream) {
  const camera = stream?.getVideoTracks?.()[0];
  if (!camera) return;
  camera.stop();
  stream.removeTrack?.(camera);
  nodes.cameraIntegrityVideo.srcObject = null;
}

async function publishPreflightTracks(room, preflight) {
  const livekit = window.LivekitClient;
  const source = livekit?.Track?.Source || {};
  for (const track of preflight.userStream?.getAudioTracks?.() || []) {
    await room.localParticipant.publishTrack(track, { source: source.Microphone });
  }
  // A camera handed to Meet is already out of the stream, so this loop is
  // empty and LiveKit never holds the device open for the interview.
  for (const track of preflight.userStream?.getVideoTracks?.() || []) {
    await room.localParticipant.publishTrack(track, { source: source.Camera });
  }
}

function stopPreflight(preflight) {
  preflight?.userStream?.getTracks?.().forEach((track) => track.stop());
}

function setLocalAudioEnabled(enabled) {
  for (const track of state.localUserStream?.getAudioTracks?.() || []) {
    track.enabled = enabled;
  }
}


function stopLocalMedia() {
  stopIntegrityWorker();
  stopPreflight({ userStream: state.localUserStream });
  state.localUserStream = null;
  clearInterval(state.integrityHeartbeatTimer);
  state.integrityHeartbeatTimer = null;
  clearInterval(state.integrityTrackStateTimer);
  state.integrityTrackStateTimer = null;
  state.integrityTrackStates.clear();
}

async function receiveReport(room, payload) {
  try {
    state.report = sanitizeReport(JSON.parse(new TextDecoder().decode(payload)));
    state.phase = "report";
    setLocalAudioEnabled(false);
    await saveHistory();
    // renderReport releases the devices for every report path, so there is no
    // separate stopLocalMedia here.
    renderReport();
    void room.disconnect().catch(() => {});
    state.room = null;
    state.connected = false;
  } catch (error) {
    // A malformed report is not worth tearing the session down over, but it is
    // worth saying so: this catch also covers saveHistory and renderReport, so
    // swallowing it silently leaves the candidate on the "Still working" overlay
    // until a 55 second timer offers them a way out, with no stated reason.
    console.warn("codetrial report_render_failed", error);
  }
}

// One element for the whole session. LiveKit re-subscribes on reconnect, and
// track.attach() with no argument mints a fresh element every time: two of
// them play the same track into the shared tab, audibly doubled, while
// setSinkId only ever reaches the newest.
function playRemoteAudio(track, participant) {
  if (track.kind !== "audio" || typeof track.attach !== "function" || !isCurrentAgent(participant)) return;
  const element = jimAudio ?? document.createElement("audio");
  track.attach(element);
  element.autoplay = true;
  element.id = "jim-audio";
  jimAudio = element;
  if (!element.isConnected) document.body.appendChild(element);
  void element.play?.().catch(() => {});
  // The candidate may have chosen an output before Jim's track arrived.
  void applyAudioOutput(readStored(AUDIO_OUTPUT_KEY));
  attachAvatarAnalyser(track, participant);
}

function dropRemoteAudio(track) {
  if (track.kind !== "audio") return;
  // Cleared only if the element that went away was Jim's. Any other remote
  // audio track unsubscribing -- a second participant leaving -- would
  // otherwise drop the handle while the element is still playing, and
  // setSinkId would have nothing to route for the rest of the session.
  for (const element of track.detach?.() || []) {
    element.remove();
    if (element === jimAudio) jimAudio = null;
  }
  // Only if it was the analyser's own track. The guard above already refuses to
  // drop Jim's element for someone else's track; without the same guard here, a
  // second participant leaving killed lip sync for the rest of the session, and
  // on reconnect a late unsubscribe for the OLD publication tore down the
  // analyser that had just been built for the new one.
  if (!isAvatarAnalyserTrack(track)) return;
  // The mouth is driven by an analyser whose track just went away. Left alone
  // it would freeze mid-syllable on whatever the last window held.
  setAvatarSpeaking(false);
  releaseAvatarAnalyser();
}

/// Presentation only. The renderer is imported here and nowhere else, so a
/// session that never reaches this point never downloads Three.js, and a
/// browser without WebGL, or a model that cannot be fetched or verified, lands on
/// the neutral panel that web/interview.html already carries.
async function consumeTranscript(room, reader, participant) {
  const attrs = reader.info?.attributes || {};
  const id = attrs["lk.segment_id"] || reader.info?.id || randomId();
  const speaker = participant?.identity === room.localParticipant.identity ? "you" : "interviewer";
  if (speaker === "interviewer" && !isCurrentAgent(participant)) return;
  const final = attrs["lk.transcription_final"] === "true";
  let text = "";
  try {
    for await (const chunk of reader) {
      text += chunk;
      updateTranscriptSegment(id, speaker, text, final);
      // Not `text.trim()`: this runs per chunk and would copy the whole turn so
      // far just to ask whether it is empty. Non-empty is enough here, and the
      // post-loop write below does the trim once.
      if (text) updateCaptions(speaker, text, id);
    }
  } catch {
    // Keep whatever arrived before the stream broke.
  }
  if (text.trim()) {
    updateTranscriptSegment(id, speaker, text, final);
    updateCaptions(speaker, text, id);
    // Once per turn, at the end of the stream, not once per chunk: a chunk is a
    // few words and a turn is a sentence, and the replay is read as sentences.
    recordReplay("transcript", { speaker, text: text.trim() });
  }
}

async function toggleMicrophone() {
  state.micEnabled = !state.micEnabled;
  if (state.localUserStream) {
    setLocalAudioEnabled(state.micEnabled);
    if (!state.micEnabled) {
      void publishIntegrityEvent({
        type: "MICROPHONE_STOPPED",
        source: "microphone",
        severity: "warning",
      });
    }
  } else if (state.room) {
    await state.room.localParticipant.setMicrophoneEnabled(state.micEnabled).catch(() => {
      state.micEnabled = !state.micEnabled;
    });
  }
  nodes.mic.textContent = state.micEnabled ? "Mic on" : "Muted";
}

function renderProblem() {
  nodes.title.textContent = problem.title;
  nodes.meta.textContent = `${problem.difficulty} · ${problem.topics.join(", ")}`;
  // The heading the recording shows. Sent from here rather than assembled in
  // the template, so the two pages name the problem the same way. This render
  // happens in the lobby, before there is an interview to attach it to, which
  // is why `connect` sends it again once there is one.
  recordStage();
  nodes.problemPanel.innerHTML = problemMarkup(problem);
}

function selectTab(tab) {
  const transcript = tab === "transcript";
  nodes.problemPanel.hidden = transcript;
  nodes.transcriptPanel.hidden = !transcript;
  nodes.problemTab.classList.toggle("selected", !transcript);
  nodes.transcriptTab.classList.toggle("selected", transcript);
}

function setLanguage(language) {
  if (!languages.includes(language)) return;
  if (editorInitialized) {
    state.codeByLanguage[state.language] = nodes.editor.value;
  }
  state.language = language;
  nodes.editor.value = state.codeByLanguage[language];
  paintEditor();
  editorInitialized = true;
  for (const button of document.querySelectorAll("[data-language]")) {
    button.classList.toggle("selected", button.dataset.language === language);
  }
  // Debounced like a keystroke, and for the same reason. Publishing on every
  // click meant a candidate trying three tabs in a row produced three language
  // changes, and the agent speaks a confirmation for each, so Jim talked over
  // himself. Coalescing here fixes it at the source rather than teaching the
  // agent to ignore packets the browser should not have sent.
  clearTimeout(codePublishTimer);
  codePublishTimer = setTimeout(() => {
    publish(topics.code, codeUpdatePayload(currentCode(), state.language));
    codePublishTimer = null;
  }, CODE_PUBLISH_DEBOUNCE_MS);
}

/// One banner element, several owners, ranked. Each owner keeps its own slot,
/// so a warning raised while a higher-ranked one is showing is deferred rather
/// than thrown away, and every owner is a row here instead of another flag and
/// another branch. The two-owner version this replaces needed a boolean lock, a
/// deferred-text field that only one of the two owners got, and a lock-breaking
/// special case for session-ending events.
const BANNER_RANK = { session: 4, connection: 3, interviewer: 2, face: 1 };
const banners = { session: "", connection: "", interviewer: "", face: "" };

function setBanner(source, text) {
  if (!nodes.presenceBanner) return;
  banners[source] = text || "";
  const top = Object.keys(banners)
    .filter((key) => banners[key])
    .sort((left, right) => BANNER_RANK[right] - BANNER_RANK[left])[0];
  nodes.presenceBanner.textContent = top ? banners[top] : "";
  nodes.presenceBanner.hidden = !top;
}

function updatePresenceBanner(eventType) {
  const entry = PRESENCE_EVENTS[eventType];
  if (!entry) return;
  // A session-ending event is the last thing the candidate will be told, so it
  // outranks the interviewer notice rather than sitting behind it.
  setBanner(entry.endsInterview ? "session" : "face", entry.clears ? "" : entry.banner || "");
}


/// A repaint, not a clock. `state.remaining` used to be decremented once per
/// firing, so a hidden or minimised tab, which browsers throttle to roughly one
/// timer per minute, showed a countdown drifting arbitrarily far from reality
/// and never reached zero. Meet presentation mode steers candidates into a
/// second tab, so that is the normal case, not the exotic one. An OS suspend or
/// a lid close stopped it entirely; this file already reasons about exactly
/// that hazard for the face sampler.
function tickTimer() {
  if (state.phase !== "live" || state.paused) return;
  const tick = countdown(state.remaining, state.endsAt, Date.now());
  state.remaining = tick.remaining;
  nodes.timer.textContent = formatTime(tick.remaining);
  nodes.timer.classList.toggle("urgent", tick.urgent);
  recordStageTick(tick.remaining);
  if (tick.warn) publish(topics.control, timeWarningPayload(tick.remaining));
  if (tick.expired) endInterview("time_up");
}

function togglePause() {
  if (mode !== "practice" || state.phase !== "live") return;
  publish(topics.control, { type: "pause_interview", paused: !state.paused });
  // Offline practice has no authoritative room to acknowledge the change.
  if (!state.room && !state.joinedRoom) applyPause(!state.paused);
}

function receiveControl(bytes) {
  try {
    const message = JSON.parse(new TextDecoder().decode(bytes));
    if (message.type === "pause_state" && typeof message.paused === "boolean") {
      applyPause(message.paused);
    }
  } catch {
    // Untrusted data packets that are not valid controls are ignored.
  }
}

function applyPause(paused) {
  if (mode !== "practice" || paused === state.paused) return;
  state.paused = paused;
  if (paused) {
    state.pausedAt = Date.now();
  } else {
    state.endsAt = resumeDeadline(state.endsAt, state.pausedAt, Date.now());
    state.pausedAt = 0;
  }
  nodes.pause.textContent = paused ? "Resume practice" : "Pause practice";
  nodes.editor.disabled = paused;
  nodes.run.disabled = paused;
  recordReplay("lifecycle", { state: paused ? "paused" : "resumed", mode });
  recordStage();
  tickTimer();
}

function retryPractice() {
  if (mode !== "practice") return;
  const retry = new URL(window.location.href);
  retry.searchParams.set("retry", Date.now().toString());
  window.location.href = retry.toString();
}

async function runTests() {
  flushPendingCodePublish();
  nodes.run.disabled = true;
  nodes.run.textContent = "Running...";
  nodes.resultsBody.hidden = false;
  setTestStatus(firstRunnerStatus(state.language));
  const summary = await runBrowserTests(problem.id, currentCode(), state.language, setTestStatus);
  state.latestSummary = summary;
  state.testStatus = finalRunnerStatus(summary, state.testStatus);
  renderResults(summary);
  publish(topics.tests, testPayload(summary));
  recordReplay("tests", {
    passed: summary.passed,
    failed: Math.max(0, summary.total - summary.passed),
    total: summary.total,
  });
  if (!state.room) {
    addTranscript("you", "I ran the tests.", true);
    addTranscript("interviewer", summary.setupError ? "I could not run that yet. Check the setup error and keep going." : `${summary.passed}/${summary.total} tests passed. Explain what changed.`, true);
  }
  nodes.run.disabled = false;
  nodes.run.textContent = "Run tests";
}

function firstRunnerStatus(language) {
  if (language === "python") return "booting";
  if (["c", "cpp", "java"].includes(language)) return "compiling";
  return "running";
}

function setTestStatus(status) {
  state.testStatus = status;
  nodes.resultsLabel.textContent = "Test results";
  nodes.resultsBody.innerHTML = runnerStatusMarkup(status);
}

function renderResults(summary) {
  const { label, body } = resultsMarkup(summary, state.testStatus);
  nodes.resultsLabel.textContent = label;
  nodes.resultsBody.innerHTML = body;
}

function addTranscript(speaker, text, final) {
  updateTranscriptSegment(randomId(), speaker, text, final);
}

function updateTranscriptSegment(id, speaker, text, final) {
  state.transcript.upsert(id, speaker, text, final);
}

function randomId() {
  return Math.random().toString(36).slice(2);
}

function flushPendingCodePublish() {
  if (!codePublishTimer) return;
  clearTimeout(codePublishTimer);
  codePublishTimer = null;
  publish(topics.code, codeUpdatePayload(currentCode(), state.language, Date.now()));
  // The same debounce the agent gets. An event per keystroke would be the
  // whole per-interview budget spent on the first ten minutes of typing.
  recordReplay("editor", { code: currentCode(), language: state.language });
}

function endInterview(reason) {
  if (state.phase !== "live") return;
  state.phase = "ending";
  // Last event, and sent rather than queued: the page is about to stop being
  // the kind of page that flushes timers, and an "ended" nobody sent leaves a
  // replay that just stops.
  recordReplay("lifecycle", { state: "ended", reason });
  void flushReplay();
  // The end_interview payload carries the final buffer, so drop any debounced
  // code_update still in flight rather than racing it.
  clearTimeout(codePublishTimer);
  nodes.end.disabled = true;
  nodes.ending.hidden = false;
  nodes.forceReport.hidden = Boolean(state.room);
  if (state.room) {
    // Longer than the agent's worst case, not shorter: REPORT_TIMEOUT in
    // src/livekit.rs is 45s, and a timer-driven end spends WRAP_UP_WAIT ahead
    // of it. Offering "leave the room" before that elapses invites the
    // candidate to walk out on a report that is still coming, and leaving
    // never saves it.
    setTimeout(() => {
      if (state.phase === "ending") nodes.endingDetail.textContent = "Still working. A slow grader can take up to a minute.";
    }, 8000);
    setTimeout(() => {
      if (state.phase === "ending") nodes.leaveRoom.hidden = false;
    }, 55000);
  }
  publish(topics.control, endInterviewPayload(reason, currentCode(), state.language));
  if (!state.room) setTimeout(showReport, 300);
}

function leaveRoom() {
  void state.room?.disconnect?.();
  stopAvatar();
  stopLocalMedia();
  window.location.href = "/";
}

async function showReport() {
  if (state.phase === "report") return;
  state.phase = "report";
  nodes.ending.hidden = true;
  const passed = state.latestSummary?.passed || 0;
  const total = state.latestSummary?.total || 0;
  // Only the candidate's own turns count as having communicated. Jim's greeting
  // is a row in the same transcript, so `transcript.size` was non-zero the
  // instant he said hello, and a candidate who never spoke was scored 70 for
  // communication on the strength of being greeted.
  const candidateTurns = state.transcript
    .values()
    .filter((row) => row.speaker !== "interviewer" && row.text.trim())
    .length;

  // `state.joinedRoom`, not `state.room`: whether an interviewer was ever
  // present is a different fact from whether the connection survived, and only
  // the first one decides if this browser may score anybody. The rule and the
  // failure it came from live in `sessionReport`.
  state.report = { ...sessionReport({
    joinedRoom: state.joinedRoom,
    passed,
    total,
    candidateTurns,
  }), mode };
  await saveHistory();
  renderReport();
}

function renderReport() {
  // The interview is over. Both report paths, agent-sent and offline, land
  // here, so this is the one place that has to stop the render loop and the
  // one place that has to release the devices. Only the agent-sent path used
  // to call stopLocalMedia, so an offline or forced report left the microphone
  // and camera live, and the integrity heartbeat still sampling, while the
  // candidate was looking at a screen telling them the interview had ended.
  stopAvatar();
  stopLocalMedia();
  nodes.report.hidden = false;
  nodes.report.innerHTML = reportMarkup({
    report: state.report,
    problemTitle: problem.title,
    language: state.language,
    code: currentCode(),
  });
  mountBehavioralReview(nodes.report, state.transcript.values());
}

function saveHistory() {
  // The interview id travels with the report so the replay page can put the
  // two beside each other. Reports are keyed by their own id and recordings by
  // theirs, and without this the only thing relating them is the clock.
  const entry = { id: randomId(), date: new Date().toISOString(), interviewId: state.interviewId, problemId: problem.id, problemTitle: problem.title, difficulty: problem.difficulty, language: state.language, durationMin, mode, report: state.report };
  return saveReportHistory(entry);
}

function downloadReport() {
  const markdown = buildMarkdown();
  const url = URL.createObjectURL(new Blob([markdown], { type: "text/markdown" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `interview-report-${problem.id}-${new Date().toISOString().slice(0, 10)}.md`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

function buildMarkdown() {
  return reportMarkdown({
    report: state.report,
    problemTitle: problem.title,
    language: state.language,
    code: currentCode(),
    transcript: state.transcript.values(),
    at: new Date().toLocaleString(),
    mode,
  });
}

/// Gated on `state.connected`, which until now had five assignments and no
/// readers. `state.room` stays set through a reconnect, so gating on it meant
/// the outage window silently discarded test results, the time warning and
/// integrity events while the banner correctly said the connection was down.
///
/// Held rather than dropped. The queue is bounded because an outage long enough
/// to overflow it is one the interview will not survive anyway, and an
/// unbounded one turns a network problem into a memory problem.
const PUBLISH_QUEUE_MAX = 64;
const pendingPublishes = [];

function publish(topic, payload) {
  if (!state.room) return;
  if (!state.connected) {
    if (pendingPublishes.length < PUBLISH_QUEUE_MAX) {
      pendingPublishes.push({ topic, payload });
    } else {
      console.warn("codetrial publish_queue_full", topic);
    }
    return;
  }
  return sendData(topic, payload);
}

function sendData(topic, payload) {
  return state.room.localParticipant
    .publishData(new TextEncoder().encode(JSON.stringify(payload)), { reliable: true, topic })
    .catch((error) => {
      // `end_interview` travels this path, and it is the only signal that
      // produces a real report. Swallowing its failure left the candidate on
      // the "Still working" overlay for 55 seconds with nothing anywhere
      // saying why.
      console.warn("codetrial publish_failed", topic, error);
      throw error;
    });
}

function flushPendingPublishes() {
  if (!state.room || !state.connected) return;
  const queued = pendingPublishes.splice(0, pendingPublishes.length);
  for (const { topic, payload } of queued) void sendData(topic, payload).catch(() => {});
}

async function publishIntegrityEvent(input) {
  state.integrityPublish = state.integrityPublish.then(async () => {
    if (!state.room) return;
    const event = await integrityEventPayload(input, state.integrityChain);
    await state.room.localParticipant.publishData(new TextEncoder().encode(JSON.stringify(event)), {
      reliable: true,
      topic: topics.integrity,
    });
    state.integrityChain = { seq: event.seq, hash: event.hash };
  }).catch((error) => {
    // The chain cannot resynchronize, so a dropped event costs every later one.
    // Worth a line rather than a shrug.
    console.warn("codetrial integrity_publish_failed", error);
  });
  return state.integrityPublish;
}

function updateAgentState() {
  const participants = [...(state.room?.remoteParticipants?.values?.() || [])];
  const agent = participants.find((participant) => participant.identity === state.agentIdentity)
    || (!state.agentIdentity && participants.find(isAgent));
  if (!agent) {
    setAgentStateLabel("Waiting", false);
    // "Waiting" is honest but useless on its own: it looks identical whether
    // the interviewer has not arrived yet or arrived, greeted, and died. Once
    // Jim has been seen, his absence is a failure the candidate is entitled to
    // know about, rather than a frozen page they keep talking into.
    if (state.sawAgent) {
      setBanner("interviewer",
        "The interviewer disconnected. Nothing you typed is lost; end the interview to get your report.");
    }
    // Logged on the edge, not on the level: this runs for every participant
    // event, including the entirely normal window before the agent arrives.
    // The identities are the one thing that distinguishes "nobody is here"
    // from "somebody is here and isAgent does not recognise them", and no
    // server log can answer that question for the browser.
    if (state.sawAgent) {
      console.warn("codetrial no_interviewer", participants.map((each) => each.identity));
    }
    // Deliberately NOT muting the mouth here. Not finding an agent participant
    // is exactly the condition that pinned Jim's jaw shut for whole interviews,
    // and muting on it re-creates that bug in the one case the audio-driven
    // mouth exists to survive. Silence closes the mouth on its own, because the
    // analyser reports no amplitude; only the real interrupt paths mute.
    return;
  }
  // An agent that comes back releases its own slot. Whatever face presence or
  // the connection last said is still in theirs, so the banner falls back to it
  // rather than to blank: the interviewer returning is not evidence about the
  // camera or the network.
  state.sawAgent = true;
  state.agentIdentity ||= agent.identity;
  setBanner("interviewer", "");
  // The published attribute and the label are two different questions. The pill
  // has to say something even when the attribute is missing, so it falls back;
  // the mouth must not, because a missing attribute is not a claim of silence.
  const published = agent?.attributes?.["lk.agent.state"];
  const value = published || "listening";
  const labels = { listening: "Listening", thinking: "Thinking...", speaking: "Speaking" };
  setAgentStateLabel(labels[value] || "Listening", value === "listening");
  // The same three states the pill shows. `questioning`, `encouraging`, and
  // `challenging` wait for src/agent.rs to publish lk.avatar.state; inventing
  // them here would be the avatar guessing at the interviewer's intent.
  setAvatarExpression(value);
  // On the change, not on the tick. This runs for every participant event, and
  // an interviewer who stays in one state for a minute is one event, not sixty.
  recordAvatarState(value);
  // Only an agent that actually publishes its state may close the jaw. Muting
  // on the "listening" fallback is the same bug as muting on a missing agent
  // participant: an interviewer whose attribute never arrives, or arrives
  // stale, would talk through the whole interview with his mouth shut. Audio is
  // the signal that cannot lie, and silence closes the mouth on its own.
  if (published) setAvatarSpeaking(published === "speaking");
}

function isCurrentAgent(participant) {
  if (!isAgent(participant)) return false;
  if (!state.agentIdentity) state.agentIdentity = participant.identity;
  return participant.identity === state.agentIdentity;
}

function setAgentStateLabel(label, ready = false) {
  nodes.agentState.textContent = label;
  nodes.agentState.closest(".agent-pill")?.classList.toggle("ready", ready);
}

function paintEditor() {
  nodes.editorHighlight.firstElementChild.innerHTML = highlight(currentCode(), state.language);
  nodes.editorLines.textContent = Array.from({ length: currentCode().split("\n").length }, (_, index) => index + 1).join("\n");
}

function currentCode() {
  return nodes.editor.value;
}
