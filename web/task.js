import {
  TASK_MAX_CODE_BYTES,
  TASK_TOPICS,
  taskCodeTooLarge,
  topics,
  taskActionPayload,
  taskRevisionPayload,
  taskRunPayload,
  taskConnectedPayload,
  taskStartPayload,
  taskEndPayload,
  taskThinkingPayload,
  taskEditPayload,
  boardStreamOptions,
  formatTime,
  countdown as timeLeft,
  languageLabel,
  renderValue,
} from "./lib.js";
import { runBrowserTests } from "./runners.js";
import { initLocalRecording } from "./local-recorder.js";
import { downloadText } from "./download.js";
import { readStored, storageArea, writeStored } from "./audio-output.js";
import {
  applyEditorEdit,
  applyingEditorEdit,
  indentNewline,
  indentSelection,
} from "./editor.js";
import {
  FACE_SAMPLE_UNAVAILABLE,
  createFacePresenceDetector,
  loadFaceAssets,
} from "./face-presence.js";
import {
  calibrate,
  createLookAwayMonitor,
  MIN_PHASE_SAMPLES,
} from "./look-away.js";
import { evaluate, formatResult } from "./calculator.js";
import { mountExampleBoard } from "./example-board.js";
import { mountEditorView } from "./editor-view.js";
import { mountCubeView } from "./cube-view.js";
import { CONSTRAINTS } from "./devices.js";
import { videoTrackReady } from "./audio-check.js";
import { COMPILED_LANGUAGES } from "./compiler-explorer.js";
import {
  NOT_DETECTED,
  acceptTaskState,
  advanceServerClock,
  outcomeSentence,
  rulesInWords,
  serverTimeAt,
  taskLocked,
  taskCodeFileName,
  taskRecovery,
  taskReviewForDisplay,
} from "./task-controller.js";
import {
  CALIBRATION_LINE,
  faceFilesProblem,
  lineTyped,
  typedEnough,
} from "./task-calibration.js";
import { caseLabel, caseLine } from "./task-cases.js";
import { feedbackUnavailableText, reviewSections } from "./task-review.js";
import { st } from "./task-state.js";

// The vendored LiveKit build is a classic script that sets a global, as on the
// interview page; it has no exports for a module to import.
const { Room, RoomEvent, Track } = globalThis.LivekitClient;

const $ = (id) => document.getElementById(id);
const parts = location.pathname.split("/");
// The assignment this page works from: the link's, or, for a learner who goes
// on to their instructor's tasks, the ones they enter.
st.setId = parts[2];
st.taskId = parts[3];
const query = new URLSearchParams(location.search);
st.site = query.get("site") ?? "";
st.version = Number(query.get("version"));
st.admissionKey = crypto.randomUUID();
const hasStarted = () => st.attempt !== "preparing";
const id = () => crypto.randomUUID();
// The microphone and camera are let go together, and a recording of them is
// finalized first, since its audio comes from this stream.
function releaseMedia() {
  const released = st.media;
  st.media = null;
  void localRecording
    .stop()
    .finally(() => released?.getTracks().forEach((track) => track.stop()));
}
const localRecordingHome = $("local-recording").parentElement;
const localRecording = initLocalRecording({
  state: {
    get localUserStream() {
      return st.media;
    },
  },
  nodes: { localRecording: $("local-recording") },
  recordingEnabled: false,
  // Task mode never swaps the microphone under a running recording: preparing
  // devices again stops the recorder first, so the interview page's
  // room-switch hooks (`isRoomLive`, `refreshMicrophone`) have no work here
  // and a microphone that ends stops the recording.
  isRoomLive: () => false,
  stopFocusTarget: () =>
    !$("review").hidden
      ? $("export-review")
      : !$("task").hidden
        ? $("maximize-editor")
        : $("local-recording").querySelector("[data-local-start]"),
  prepareMicrophone: () => {
    if (hasStarted())
      throw new Error("Start recording before starting the task.");
  },
  // The capture picker must close before Start requests fullscreen, and Start
  // waits while the picker is open.
  canStart: () => !st.enteringFullscreen && !hasStarted(),
  // The take lives in this tab alone, so replacing one must not lose it.
  saveReplacedTake: true,
  onSetupChange: (active) => {
    $("start").disabled = active || !startReady();
  },
});

/// How often the camera is sampled while preparing and during an attempt.
const SAMPLE_INTERVAL_MS = 500;
/// How long the reading phase of calibration samples.
const CALIBRATION_PHASE_MS = 10000;
/// The most the typing phase waits for the line; it ends as soon as the line
/// is typed, so a quick typist is not held to the clock.
const CALIBRATION_TYPING_MAX_MS = 40000;
const recoveryKey = () =>
  `codetrial-task-result:${JSON.stringify([st.setId, st.taskId, st.site, st.version])}`;
function rememberAttempt(
  roomName = st.activeRoomName,
  until = st.reviewPollUntil,
  key = recoveryKey(),
  unconfirmed = st.startUnconfirmed,
) {
  writeStored(
    key,
    JSON.stringify({
      room: roomName,
      until,
      ...(unconfirmed && { unconfirmed }),
    }),
    storageArea("sessionStorage"),
  );
}
function forgetAttempt() {
  writeStored(recoveryKey(), "", storageArea("sessionStorage"));
}
async function signedInFlow(
  next = () => (st.wantsInstructor ? showServer() : showUnlock()),
) {
  let saved;
  try {
    saved = JSON.parse(
      readStored(recoveryKey(), storageArea("sessionStorage")),
    );
  } catch {
    /* Ignore invalid storage. */
  }
  if (
    saved &&
    /^[A-Za-z0-9_-]{1,128}$/.test(saved.room) &&
    Number.isFinite(saved.until) &&
    saved.until <= Date.now() + 86400000
  ) {
    st.activeRoomName = saved.room;
    st.reviewPollUntil = saved.until;
    st.startUnconfirmed = saved.unconfirmed === true;
    st.attempt = "lost";
    st.restored = true;
    startAttemptTimers();
    show("task");
    lock();
    $("clock").textContent = "Retrieving the interrupted attempt's result...";
    await pollReview();
    return;
  }
  forgetAttempt();
  return next();
}

async function api(path, body) {
  const response = await fetch(path, {
    method: body ? "POST" : "GET",
    credentials: "same-origin",
    headers: {
      "content-type": "application/json",
      "x-codetrial-task-request": "1",
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const value = await response.json();
  value.status = response.status;
  if (!response.ok) {
    value.retryAfter = Math.max(
      0,
      Number(response.headers.get("Retry-After")) || 0,
    );
    throw value;
  }
  return value;
}

/// Shows one stage of the flow and hides the others.
function show(section) {
  for (const name of [
    "signin",
    "server",
    "unlock",
    "prepare",
    "task",
    "review",
  ])
    $(name).hidden = name !== section;
}

function error(value) {
  clearTimeout(st.recoveryTimer);
  $("recovery").disabled = Boolean(value.retryAfter);
  if (value.retryAfter)
    st.recoveryTimer = setTimeout(() => {
      $("recovery").disabled = false;
    }, value.retryAfter * 1000);
  $("recovery").hidden = false;
  $("message").hidden = false;
  $("message-text").textContent =
    value.message ?? value.error ?? "The task could not be loaded.";
  // Reloading once an attempt has started would end it as interrupted, so a
  // failure whose answer is a reload, an uncoded one included, is only
  // dismissed then.
  const recovery = taskRecovery(value.code);
  const dismissOnly = hasStarted() && recovery === "Reload";
  $("recovery").textContent = dismissOnly ? "Dismiss" : recovery;
  $("recovery").onclick = () => {
    clearError();
    if (dismissOnly) return;
    if (value.code === "authentication_required") showSignin().catch(error);
    else if (value.code === "credentials_missing") location.reload();
    else if (["invalid_pin", "unlock_required"].includes(value.code)) {
      show("unlock");
      $("pin").focus();
    } else if (value.code === "package_unavailable") show("unlock");
    else if (["set_closed", "task_not_found"].includes(value.code))
      returnToTaskList(value.code);
    else if (["request_key_expired", "setup_failed"].includes(value.code))
      st.admissionKey = id();
    // These clicks are the gesture a permission prompt or fullscreen needs,
    // so they ask again rather than only dismissing the message.
    else if (value.code === "devices_required") {
      $("devices").disabled = false;
      $("devices").click();
    } else if (value.code === "fullscreen_required") {
      if (!$("start").disabled) $("start").click();
    } else if (recovery === "Reload") {
      // A failed load leaves nothing on the page to keep, so it is tried
      // again in place.
      if (!st.task && !value.code) loadTask().catch(error);
      else location.reload();
    }
  };
}

/// Back to the set's tasks after one could not be opened: another task when
/// the set offers one, the PIN otherwise, never the same task again.
function returnToTaskList(code) {
  const others = [...$("task-picker").options].filter(
    (option) => option.value !== st.taskId,
  );
  if (code === "set_closed" || !others.length || hasStarted() || st.room)
    return showUnlock();
  $("task-picker").value = others[0].value;
  show("prepare");
  return $("task-picker").onchange();
}

function clearError() {
  clearTimeout(st.recoveryTimer);
  $("recovery").disabled = false;
  $("message").hidden = true;
}

// --- Sign-in ---------------------------------------------------------------

async function showSignin() {
  show("signin");
  const session = await api("/api/session");
  // The browser sign-in needs the same client ID, and a secret besides, so
  // without the ID there is no other way in to offer.
  $("device-start").hidden = !session.deviceLogin;
  $("no-device-login").hidden = session.deviceLogin;
}

$("device-start").onclick = async () => {
  try {
    const started = await api("/api/github/device", {});
    $("device-link").href = started.verificationUri;
    $("device-link").textContent = started.verificationUri;
    $("device-user-code").textContent = started.userCode;
    $("device-code").hidden = false;
    $("device-start").disabled = true;
    let interval = started.interval;
    const deadline = Date.now() + started.expiresIn * 1000;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, interval * 1000));
      const answer = await api("/api/github/device/poll", {});
      if (answer.signedIn) return signedInFlow();
      interval = answer.interval ?? interval;
    }
    throw { message: "The sign-in code expired. Sign in again." };
  } catch (value) {
    $("device-start").disabled = false;
    $("device-code").hidden = true;
    error(value);
  }
};

// --- Unlock ----------------------------------------------------------------

function showUnlock() {
  show("unlock");
  // A learner who entered the server can go back to it; one who followed a
  // link has nowhere else to go.
  $("unlock-back").hidden = !st.wantsInstructor;
  $("assignment").textContent =
    `Task set ${st.setId}, version ${st.version}, from ${st.site}. Enter the PIN your instructor gave you.`;
}

/// The address of the assignment the page now works from, so a reload comes
/// back to it.
function openAssignment() {
  history.replaceState(
    null,
    "",
    `/t/${encodeURIComponent(st.setId)}/${encodeURIComponent(st.taskId)}?site=${encodeURIComponent(st.site)}&version=${st.version}`,
  );
}

// --- Your instructor's tasks ------------------------------------------------

/// Where a learner who is signed in with GitHub says which server their
/// instructor's tasks are on, before the PIN.
function showServer() {
  show("server");
  $("server-message").textContent = "";
}

/// The default task: the one the session names, with nothing to pick from.
function openDefault() {
  ({
    setId: st.setId,
    taskId: st.taskId,
    site: st.site,
    version: st.version,
  } = st.builtIn);
  st.wantsInstructor = false;
  $("task-picker").replaceChildren(new Option(st.taskId, st.taskId));
  $("task-picker").value = st.taskId;
  openAssignment();
  showPrepare();
  return loadTask();
}

$("instructor-tasks").onclick = async () => {
  try {
    const session = await api("/api/session");
    // Signed in with GitHub already: straight to the server. Otherwise sign in
    // first, which ends at the same place.
    if (session.signedIn && session.githubSignedIn) return showServer();
    st.wantsInstructor = true;
    await showSignin();
  } catch (value) {
    error(value);
  }
};

$("server-form").onsubmit = (event) => {
  event.preventDefault();
  const entered = {
    site: $("server-site").value.trim(),
    set: $("server-set").value.trim(),
    version: Number($("server-version").value),
  };
  // The same checks the server makes of an assignment link, said before the
  // PIN is spent on one that cannot open.
  const problem = !/^https:\/\/[^/\s]+/.test(entered.site)
    ? "The server is an https address, such as https://teacher.github.io/course."
    : !/^[a-z][a-z0-9-]{0,63}$/.test(entered.set)
      ? "The set is lower case letters, digits and hyphens, and starts with a letter."
      : !Number.isSafeInteger(entered.version) || entered.version < 1
        ? "The version is a whole number from 1."
        : "";
  $("server-message").textContent = problem;
  if (problem) return;
  st.wantsInstructor = true;
  ({
    setId: st.setId,
    site: st.site,
    version: st.version,
  } = {
    setId: entered.set,
    site: entered.site,
    version: entered.version,
  });
  st.taskId = "";
  showUnlock();
};
$("server-back").onclick = () => openDefault().catch(error);
$("unlock-back").onclick = () => showServer();

$("pin-form").onsubmit = async (event) => {
  event.preventDefault();
  try {
    const unlocked = await api(
      `/api/task-sets/${encodeURIComponent(st.setId)}/unlock`,
      { site: st.site, version: st.version, pin: $("pin").value },
    );
    $("pin").value = "";
    $("task-picker").replaceChildren(
      ...unlocked.tasks.map((row) => new Option(row.title, row.id)),
    );
    // A set the learner entered names no task, so it opens at its first.
    if (!unlocked.tasks.some((row) => row.id === st.taskId))
      st.taskId = unlocked.tasks[0].id;
    $("task-picker").value = st.taskId;
    openAssignment();
    showPrepare();
    await loadTask();
  } catch (value) {
    error(value);
  }
};

async function loadTask() {
  // Only the latest selection's answer is shown, whatever order they arrive.
  const generation = ++st.loadGeneration;
  const selectedTaskId = st.taskId;
  st.task = null;
  $("calibrate").disabled = true;
  $("start").disabled = true;
  $("prepare-title").textContent = "Loading assignment…";
  $("title").textContent = "Loading assignment…";
  $("contract").textContent = "";
  $("public-cases").replaceChildren();
  $("brief").replaceChildren();
  $("code").value = "";
  st.editorView?.paint();
  let loaded;
  try {
    loaded = await api(
      `/api/task-sets/${encodeURIComponent(st.setId)}/tasks/${encodeURIComponent(st.taskId)}?site=${encodeURIComponent(st.site)}&version=${st.version}`,
    );
  } catch (value) {
    if (generation !== st.loadGeneration) return;
    throw value;
  }
  if (generation !== st.loadGeneration) return;
  if (loaded.taskId !== selectedTaskId || st.taskId !== selectedTaskId)
    throw new Error("The loaded task does not match the assignment.");
  st.task = loaded;
  $("calibrate").disabled = !st.media || !st.acknowledgedAt;
  clearError();
  $("title").textContent = st.task.title;
  $("prepare-title").textContent = st.task.title;
  $("contract").textContent = st.task.contract;
  // A task may offer a cube to turn, mounted the first time one does.
  if (st.task.explainers?.includes("cube"))
    st.cubeMounted ??= mountCubeView($("cube-explorer"));
  $("cube-explorer").hidden = !st.task.explainers?.includes("cube");
  $("public-cases").replaceChildren(
    ...(st.task.publicCases?.cases ?? []).map((row, index) => {
      const li = document.createElement("li");
      li.textContent = `${caseLabel(row, index, st.task?.requiredCases)}: input ${renderValue(row.input)}, expected ${renderValue(row.expected)}`;
      return li;
    }),
  );

  $("brief").replaceChildren(
    ...st.task.brief.map((text) => {
      const p = document.createElement("p");
      p.textContent = text;
      return p;
    }),
  );
  // The server offers only the languages it can run, in the instructor's
  // order.
  $("language").replaceChildren(
    ...st.task.languages.map(
      (value) => new Option(languageLabel(value), value),
    ),
  );
  $("language-choice").hidden = st.task.languages.length < 2;
  $("language-note").hidden = !st.task.languages.some((value) =>
    COMPILED_LANGUAGES.includes(value),
  );
  // The notes say what leaves the machine; when the instructor lowered the
  // optimization level they also say how the code is built.
  for (const id of ["language-note", "compiled-note"]) {
    const note = $(id);
    note.dataset.base ??= note.textContent;
    note.textContent = st.task.optimization
      ? `${note.dataset.base} It is built with -${st.task.optimization}, not the default.`
      : note.dataset.base;
  }
  chooseLanguage(st.task.languages[0]);
  const rules = rulesInWords(st.task.rules);
  for (const list of [$("rules"), $("rules-reminder")])
    list.replaceChildren(
      ...rules.map((line) => {
        const li = document.createElement("li");
        li.textContent = line;
        return li;
      }),
    );
  $("not-detected").textContent = NOT_DETECTED;
}

/// Shows the editor as `value` is worked in: its starter, its targets, and
/// whether a run leaves this computer.
function chooseLanguage(value) {
  st.language = value;
  $("language").value = value;
  $("code").value = st.task.starterCode[value];
  st.editorView?.paint();
  $("code-label").textContent = `${languageLabel(value)} implementation`;
  $("compiled-note").hidden = !COMPILED_LANGUAGES.includes(value);
  $("target").replaceChildren(
    new Option("Whole task", ""),
    ...st.task.completionTargets
      .filter((target) => target.language === value)
      .map((target) => new Option(target.goal, target.id)),
  );
}

/// The task and its language name the admission: once a room is joined for
/// them, neither can change.
function lockChoices(locked) {
  $("task-picker").disabled = locked;
  $("language").disabled = locked;
  // So is the acknowledgement the admission records.
  $("acknowledge").disabled = locked;
}

$("language").onchange = () => {
  if (hasStarted() || st.room || !st.task) return;
  // A request key names one task and language; reused for another, the
  // server would refuse it as an override.
  st.admissionKey = id();
  chooseLanguage($("language").value);
};

$("task-picker").onchange = () => {
  // The admission names one task: once a room is joined for it, the page
  // cannot show another.
  if (hasStarted() || st.room) return;
  st.admissionKey = id();
  st.taskId = $("task-picker").value;
  history.replaceState(
    null,
    "",
    `/t/${encodeURIComponent(st.setId)}/${encodeURIComponent(st.taskId)}${location.search}`,
  );
  st.revisionId = "initial";
  return loadTask().catch(error);
};

// --- Preparation -----------------------------------------------------------

function showPrepare() {
  show("prepare");
  // Offered on the default task alone, and a picker is only for a set with a
  // choice in it.
  $("instructor-offer").hidden = !isBuiltIn();
  $("task-picker").closest("label").hidden =
    $("task-picker").options.length < 2;
  st.acknowledgedAt = null;
  $("acknowledge").checked = false;
  $("devices").disabled = true;
}

$("acknowledge").onchange = () => {
  st.acknowledgedAt = $("acknowledge").checked
    ? new Date().toISOString()
    : null;
  if (!st.acknowledgedAt) $("start").disabled = true;
  $("devices").disabled = !st.acknowledgedAt || Boolean(st.media);
  $("calibrate").disabled =
    !st.acknowledgedAt ||
    !st.media ||
    !st.faceReady ||
    !st.task ||
    Boolean(st.room) ||
    st.calibrating;
};

/// How long the first detection may take. It starts the model, which takes
/// seconds on a slow machine.
const DETECTOR_START_MS = 30000;

function detectorStatus(text, percent = null) {
  $("detector-status").textContent = text;
  const bar = $("detector-progress");
  bar.hidden = percent === null;
  if (percent !== null) bar.value = percent;
}

// The model is downloaded as the page opens, with its progress shown, so the
// learner can tell a download in progress from one that failed before they
// spend the calibration on it.
detectorStatus("Downloading the face model…", 0);
const faceFiles = loadFaceAssets({
  onProgress: ({ loaded, total }) =>
    detectorStatus(
      "Downloading the face model…",
      total ? Math.min(99, Math.floor((loaded / total) * 100)) : 0,
    ),
}).then((result) => {
  detectorStatus(
    faceFilesProblem(result) || "Face model downloaded. Allow the camera next.",
  );
  return result;
});

// Permission is granted now, before anything is timed, so no prompt can take
// the page out of fullscreen during the attempt.
$("devices").onclick = async () => {
  // One request at a time, or a second grant would orphan the first stream's
  // tracks, which stopMonitoring never sees.
  $("devices").disabled = true;
  try {
    // The interviewer interrupts itself on the learner's voice here too, so
    // the audio asks for what the interview page asks for.
    st.media = await navigator.mediaDevices.getUserMedia({
      audio: CONSTRAINTS.audio,
      video: { width: 640, height: 480 },
    });
    $("preview").srcObject = st.media;
    $("preview").hidden = false;
    await $("preview")
      .play()
      .catch(() => {});
    // The calibration needs keypoints; a browser's native detector gives
    // only a count.
    st.detector?.close?.();
    st.faceReady = false;
    const problem = faceFilesProblem(await faceFiles);
    if (problem) throw { code: "calibration_required", message: problem };
    detectorStatus("Starting the face detector…");
    st.detector = await createFacePresenceDetector({ native: false });
    if (!st.detector?.available)
      throw {
        code: "calibration_required",
        message:
          "The face detector could not start. Reload the page and try again.",
      };
    // The first detection starts the model, so it is taken now: the learner
    // is told when the detector is running, not only when it was fetched.
    let timer;
    const first = await Promise.race([
      st.detector.detect($("preview")).catch(() => FACE_SAMPLE_UNAVAILABLE),
      new Promise((resolve) => {
        timer = setTimeout(() => resolve(null), DETECTOR_START_MS);
      }),
    ]).finally(() => clearTimeout(timer));
    if (!first?.available)
      throw {
        code: "calibration_required",
        message:
          "The face detector downloaded but is not answering. Reload the page and try again.",
      };
    st.faceReady = true;
    detectorStatus("Face detector ready.");
    $("devices").disabled = true;
    $("calibrate").disabled = !st.task || !st.acknowledgedAt;
  } catch (value) {
    releaseMedia();
    $("devices").disabled = false;
    if (value?.code === "calibration_required") detectorStatus(value.message);
    error(
      value?.code
        ? value
        : {
            code: "devices_required",
            message:
              "The microphone and camera are needed for this task. Allow them and try again.",
          },
    );
  }
};

/// How long one detection may take before it counts as unanswered. A hung
/// worker would otherwise stall the monitor on one `await` forever, and its
/// sample-gap rule never runs.
const SAMPLE_TIMEOUT_MS = 2000;

async function sample() {
  const video = $("preview");
  if (!st.detector || video.readyState < 2) return FACE_SAMPLE_UNAVAILABLE;
  let timer;
  try {
    // The detector takes its own frame from the video; copying one here
    // first would make two per sample.
    return await Promise.race([
      st.detector.detect(video),
      new Promise((resolve) => {
        timer = setTimeout(
          () => resolve(FACE_SAMPLE_UNAVAILABLE),
          SAMPLE_TIMEOUT_MS,
        );
      }),
    ]);
  } catch {
    return FACE_SAMPLE_UNAVAILABLE;
  } finally {
    clearTimeout(timer);
  }
}

/// Samples for `ms`, or until `done` says so once enough were taken to judge.
async function samplesFor(ms, done = () => false) {
  const samples = [];
  const until = Date.now() + ms;
  while (
    Date.now() < until &&
    !(samples.length >= MIN_PHASE_SAMPLES && done())
  ) {
    samples.push(await sample());
    await new Promise((resolve) => setTimeout(resolve, SAMPLE_INTERVAL_MS));
  }
  return samples;
}

$("calibrate").onclick = async () => {
  if (
    !st.task ||
    st.task.taskId !== st.taskId ||
    !st.acknowledgedAt ||
    st.calibrating
  )
    return;
  st.calibrating = true;
  lockChoices(true);
  $("calibrate").disabled = true;
  $("start").disabled = true;
  try {
    $("calibration-step").textContent =
      "Look at the screen as you would while reading the task.";
    const screen = await samplesFor(CALIBRATION_PHASE_MS);
    $("calibration-step").textContent =
      `Now type this line below: ${CALIBRATION_LINE}`;
    $("calibration-line").hidden = false;
    $("calibration-line").value = "";
    $("calibration-line").focus();
    const typing = await samplesFor(CALIBRATION_TYPING_MAX_MS, () =>
      lineTyped($("calibration-line").value),
    );
    $("calibration-line").hidden = true;
    // Without typing, the keyboard phase measures the reading posture again,
    // and the limit would leave no room for glancing at the keys.
    if (!typedEnough($("calibration-line").value)) {
      $("calibration-result").textContent =
        "Type the line shown while calibrating, then calibrate again.";
      return;
    }
    const result = calibrate(screen, typing);
    if (!result.ok) {
      $("calibration-result").textContent =
        result.reason === "too_few_samples"
          ? `The camera gave fewer than ${MIN_PHASE_SAMPLES} samples. Check it is not used elsewhere, then calibrate again.`
          : result.reason === "detector_unavailable"
            ? "The face detector is not answering. Reload the page and try again."
            : "Your face was not steadily in view. Face the camera with good light in front of you, then calibrate again.";
      return;
    }
    st.calibration = result;
    $("calibration-result").textContent = "Calibrated. Connecting…";
    await connect();
    clearError();
    $("calibration-result").textContent =
      "Calibrated and connected. Press Start when you are ready.";
    $("start").disabled = false;
  } catch (value) {
    error(value);
  } finally {
    st.calibrating = false;
    $("calibrate").disabled =
      !st.task || !st.acknowledgedAt || Boolean(st.calibration && st.room);
    lockChoices(Boolean(st.room));
  }
};

// --- Connection ------------------------------------------------------------

async function send(topic, message) {
  if (!st.room) throw new Error("Task is not connected.");
  await st.room.localParticipant.publishData(
    new TextEncoder().encode(JSON.stringify(message)),
    { reliable: true, topic },
  );
}

async function connect() {
  if (!st.task || st.task.taskId !== st.taskId)
    throw new Error("Wait for the selected assignment to load.");
  const supported =
    /Firefox|Chrome|Chromium/.test(navigator.userAgent) &&
    !/Mobile|Android/.test(navigator.userAgent) &&
    Boolean(document.fullscreenEnabled);
  const token = await api(
    `/api/task-sets/${encodeURIComponent(st.setId)}/tasks/${encodeURIComponent(st.taskId)}/attempts`,
    {
      site: st.site,
      version: st.version,
      requestKey: st.admissionKey,
      language: st.language,
      supportedPlatform: supported,
      rulesAcknowledgedAt: st.acknowledgedAt,
      calibration: st.calibration,
    },
  );
  lockChoices(true);
  // The room never stops the devices' tracks: before Start a dropped room is
  // only left, and the same microphone is published to the next one.
  st.room = new Room({ stopLocalTrackOnUnpublish: false });
  const thisRoom = st.room;
  const { agentIdentity } = token;
  let setupFailure;
  let prepared = false;
  // Before Start, a lost room or a departed interviewer cannot start the
  // attempt: the page lets go of it, so calibrating again connects afresh.
  const lostBeforeStart = () => {
    setupFailure = {
      code: "setup_failed",
      message:
        "The task connection ended before Start. Calibrate again to reconnect.",
    };
    if (!prepared) return;
    dropConnection();
    error(setupFailure);
  };
  const lost = () => (hasStarted() ? collectResult() : lostBeforeStart());
  st.room.on(RoomEvent.ParticipantDisconnected, (participant) => {
    if (st.room === thisRoom && participant?.identity === agentIdentity) lost();
  });
  st.room.on(RoomEvent.Disconnected, () => {
    if (st.room === thisRoom) lost();
  });
  st.room.on(RoomEvent.TrackSubscribed, (track) => {
    if (st.room !== thisRoom) return;
    if (track.kind === "audio") {
      const audio = track.attach();
      audio.dataset.taskAudio = "1";
      document.body.append(audio);
    }
  });
  st.room.on(RoomEvent.DataReceived, (payload, participant, kind, topic) => {
    if (st.room !== thisRoom) return;
    if (participant?.identity !== agentIdentity) return;
    let message;
    try {
      message = JSON.parse(new TextDecoder().decode(payload));
    } catch {
      return;
    }
    if (topic === TASK_TOPICS.state) renderState(message);
    else if (topic === TASK_TOPICS.review) renderReview(message);
    else if (topic === TASK_TOPICS.error) {
      if (message.code === "setup_failed" && !hasStarted())
        setupFailure = message;
      if (!message.requestId && st.state?.phase === "feedback") return;
      // An error naming no request answers no capture: with none pending
      // either, undefined would match undefined.
      if (
        message.requestId &&
        st.pendingRevision?.requestId === message.requestId
      ) {
        st.pendingRevision.reject(message);
        st.pendingRevision = null;
      }
      error(message);
    }
  });
  try {
    await st.room.connect(token.serverUrl, token.token);
    await st.room.localParticipant.publishTrack(st.media.getAudioTracks()[0], {
      source: Track.Source.Microphone,
    });
    const setupDeadline = Date.now() + st.task.setupTimeoutSeconds * 1000;
    // Sent at least once even if a snapshot already arrived: the runtime
    // accepts Start only after this handshake, and a snapshot it published
    // on its own does not count as one.
    st.state = null;
    st.serverClockOffset = null;
    do {
      if (setupFailure) throw setupFailure;
      if (Date.now() >= setupDeadline)
        throw {
          code: "setup_failed",
          message: "The task connection timed out.",
        };
      await send(TASK_TOPICS.connected, taskConnectedPayload());
      if (!st.state) await new Promise((resolve) => setTimeout(resolve, 1000));
    } while (!st.state);
    if (setupFailure) throw setupFailure;
    // The first handshake can go out before the interviewer has joined and
    // be lost, while the snapshot it publishes on joining ends the loop. It
    // is in the room now, so this one arrives, and before any Start.
    await send(TASK_TOPICS.connected, taskConnectedPayload());
  } catch (value) {
    dropConnection();
    throw value;
  }
  prepared = true;
  st.activeRoomName = token.roomName;
}

/// Once Start has been sent, losing the room or the interviewer leaves only
/// the result to collect: the server finalizes the attempt without the page.
/// A Start never acknowledged may not have arrived, which the server's answer
/// settles.
function collectResult() {
  if (attemptFinished()) return;
  st.startUnconfirmed = st.attempt === "starting";
  st.startViolation = null;
  st.attempt = "lost";
  show("task");
  lock();
  stopMonitoring();
  leaveFullscreen();
  st.reviewPollUntil = Date.now() + st.task.reviewDeliverySeconds * 1000;
  rememberAttempt();
  $("message-text").textContent = st.startUnconfirmed
    ? "The connection ended while Start was being confirmed. Checking whether the attempt began."
    : "The room disconnected. Waiting for your result.";
  $("message").hidden = false;
  $("recovery").hidden = true;
  startAttemptTimers();
  pollReview();
}

/// The server holds nothing for a Start it never acknowledged, so it never
/// began: the learner prepares again, with the devices the page let go of.
function startNeverArrived() {
  st.activeRoomName = null;
  st.startUnconfirmed = false;
  forgetAttempt();
  stopAttemptTimers();
  dropConnection();
  st.attempt = "preparing";
  localRecordingHome.append($("local-recording"));
  if (!st.task || st.restored) {
    st.restored = false;
    showUnlock();
    return error({
      code: "unlock_required",
      message: "The task did not start. Enter the PIN to prepare again.",
    });
  }
  show("prepare");
  lock();
  $("devices").disabled = !st.acknowledgedAt || Boolean(st.media);
  error({
    code: "devices_required",
    message:
      "The task did not start. Allow the microphone and camera again, then calibrate again.",
  });
}

/// Lets go of a room that can no longer start the attempt.
function dropConnection() {
  const oldRoom = st.room;
  st.room = null;
  // The interviewer's audio element belongs to the room just left.
  for (const audio of document.querySelectorAll("[data-task-audio]"))
    audio.remove();
  oldRoom?.disconnect(false)?.catch?.(() => {});
  // The old key would replay the admission of the room just left.
  st.admissionKey = id();
  st.state = null;
  // A new room's server may keep another clock; nothing carries over.
  st.serverClockOffset = null;
  st.calibration = null;
  $("start").disabled = true;
  lockChoices(false);
  $("calibrate").disabled = !st.media || !st.acknowledgedAt;
  $("calibration-result").textContent = "";
}

// --- Start and the rules -----------------------------------------------------

const FULLSCREEN_REQUIRED = {
  code: "fullscreen_required",
  message: "Fullscreen is required to start this task.",
};

// Whether Start may be pressed: connected, calibrated and awaiting it.
const startReady = () =>
  Boolean(st.room && st.calibration) &&
  !hasStarted() &&
  st.state?.phase === "ready";

$("start").onclick = () => {
  if (!st.room || !st.calibration || hasStarted() || localRecording.settingUp)
    return;
  // A camera that stopped since calibration would only interrupt the
  // attempt the moment it began.
  if (!videoTrackReady(st.media?.getVideoTracks()[0])) {
    releaseMedia();
    dropConnection();
    $("devices").disabled = !st.acknowledgedAt;
    $("calibrate").disabled = true;
    return error({
      code: "devices_required",
      message:
        "The camera stopped. Allow the microphone and camera again, then calibrate.",
    });
  }
  const thisStart = {};
  st.currentStart = thisStart;
  // A newer Start, or a loss that moved the page on to collecting the
  // result, owns the page from then on.
  const superseded = () =>
    st.currentStart !== thisStart || st.attempt !== "starting";
  const startingRoom = st.room;
  const startingCalibration = st.calibration;
  const startingRoomName = st.activeRoomName;
  const startRecoveryKey = recoveryKey();
  $("start").disabled = true;
  // Fullscreen must be requested synchronously inside this click.
  st.enteringFullscreen = true;
  const fullscreen = $("workspace").requestFullscreen?.();
  Promise.resolve(fullscreen)
    .then(async () => {
      if (st.currentStart === thisStart) st.enteringFullscreen = false;
      if (st.room !== startingRoom || st.calibration !== startingCalibration)
        throw {
          code: "setup_failed",
          message:
            "The connection changed before Start. Calibrate again to reconnect.",
        };
      if (!document.fullscreenElement) throw FULLSCREEN_REQUIRED;
      st.reviewPollUntil =
        Date.now() +
        (st.task.rules.durationMin * 60 + st.task.reviewDeliverySeconds) * 1000;
      const until = st.reviewPollUntil;
      // A reload while Start is on its way cannot tell whether it arrived,
      // so `pagehide` keeps a way back to the result in case it did.
      st.startInFlight = {
        room: startingRoomName,
        until,
        key: startRecoveryKey,
      };
      st.startViolation = null;
      st.attempt = "starting";
      try {
        await send(TASK_TOPICS.start, taskStartPayload());
      } catch {
        st.startInFlight = null;
        if (superseded()) return;
        st.attempt = "preparing";
        if (st.room === startingRoom) dropConnection();
        throw {
          code: "setup_failed",
          message: "Start could not be sent. Calibrate again to reconnect.",
        };
      }
      st.startInFlight = null;
      // Lost while it was sent, the page is already collecting the result;
      // a newer Start owns the page and its record.
      if (superseded()) return;
      rememberAttempt(startingRoomName, until, startRecoveryKey, true);
      const deadline = Date.now() + 10000;
      while (
        !superseded() &&
        st.state?.phase === "ready" &&
        Date.now() < deadline
      )
        await new Promise((resolve) => setTimeout(resolve, 200));
      if (superseded()) return;
      if (!["work", "wrap-up"].includes(st.state?.phase)) {
        // Unacknowledged is not refused: the server may be running the
        // clock, so the page leaves the room and asks for the result.
        collectResult();
        dropConnection();
        return;
      }
      st.attempt = "running";
      rememberAttempt(startingRoomName, until, startRecoveryKey, false);
      startAttemptTimers();
      $("task").prepend($("local-recording"));
      show("task");
      clearError();
      armRules();
      // Leaving fullscreen or the page while Start was being acknowledged
      // fired before the rules were armed; it still counts, even if the
      // learner came back before the acknowledgement.
      const broken = st.startViolation;
      st.startViolation = null;
      if (broken) endAttempt("invalid", broken, 0);
      else if (!document.fullscreenElement)
        endAttempt("invalid", "fullscreen", 0);
      else if (document.hidden) endAttempt("invalid", "visibility", 0);
    })
    .catch((value) => {
      if (st.currentStart !== thisStart) return;
      st.enteringFullscreen = false;
      if (st.room && st.room !== startingRoom) return;
      if (!hasStarted()) leaveFullscreen();
      $("start").disabled = !startReady();
      error(
        st.room !== startingRoom || st.calibration !== startingCalibration
          ? {
              code: "setup_failed",
              message:
                "The connection ended before Start. Calibrate again to reconnect.",
            }
          : value?.code
            ? value
            : FULLSCREEN_REQUIRED,
      );
    });
};

function armRules() {
  navigator.wakeLock
    ?.request("screen")
    .then((lock) => {
      // Granted after the attempt already ended, it is let go at once.
      if (monitoring()) st.wakeLock = lock;
      else lock.release?.().catch(() => {});
    })
    .catch(() => {
      if (!monitoring()) return;
      $("results").textContent =
        "This browser would not keep the screen awake; keep it from locking yourself.";
    });
  const monitor = createLookAwayMonitor({
    limit: st.calibration.limit,
    metric: st.calibration.metric,
    thresholdMs: st.task.rules.lookAwaySeconds * 1000,
  });
  // A muted track delivers frozen or black frames, which would read as a
  // face held still or as nobody there; either way it is not evidence.
  for (const track of st.media.getVideoTracks()) {
    for (const event of ["ended", "mute"])
      track.addEventListener(event, () =>
        endAttempt("interrupted", "camera", 0),
      );
    // One that stopped before these listeners existed fires nothing.
    if (!videoTrackReady(track)) endAttempt("interrupted", "camera", 0);
  }
  if (!monitoring()) return;
  const tick = async () => {
    if (!monitoring()) return;
    const verdict = monitor.update(await sample(), performance.now());
    if (!monitoring()) return;
    if (verdict.state === "violation")
      return endAttempt("invalid", "look_away", verdict.awayMs);
    if (verdict.state === "interrupted")
      return endAttempt("interrupted", "detector", 0);
    countdown(verdict);
    st.monitorTimer = setTimeout(tick, SAMPLE_INTERVAL_MS);
  };
  st.monitorTimer = setTimeout(tick, SAMPLE_INTERVAL_MS);
}

function monitoring() {
  return st.attempt === "running" && st.state?.phase !== "feedback";
}

/// Stops watching. Once an attempt has started, stopping is for good, so the
/// camera and microphone are let go too and their light goes out.
function stopMonitoring() {
  if (hasStarted()) {
    if (st.attempt === "lost" && st.startUnconfirmed) {
      // The server may send us back to preparation if Start never arrived.
      void localRecording.stop();
      $("task").prepend($("local-recording"));
    } else localRecording.finish($("task"));
    st.media?.getTracks().forEach((track) => track.stop());
    st.media = null;
    st.detector?.close?.();
    st.detector = null;
    st.faceReady = false;
    st.beepContext?.close().catch(() => {});
    st.beepContext = null;
  }
  clearTimeout(st.monitorTimer);
  st.monitorTimer = null;
  $("countdown").hidden = true;
  st.wakeLock?.release?.().catch(() => {});
  st.wakeLock = null;
}

// The countdown is heard as well as seen: a learner looking away from the
// screen cannot read it.
function countdown(verdict) {
  if (verdict.state !== "warning") {
    $("countdown").hidden = true;
    return;
  }
  const left = Math.max(
    0,
    Math.ceil((st.task.rules.lookAwaySeconds * 1000 - verdict.awayMs) / 1000),
  );
  $("countdown").textContent =
    `Face the screen: the attempt ends in ${left} seconds.`;
  $("countdown").hidden = false;
  try {
    st.beepContext ??= new AudioContext();
    const tone = st.beepContext.createOscillator();
    tone.frequency.value = 880;
    tone.connect(st.beepContext.destination);
    tone.start();
    tone.stop(st.beepContext.currentTime + 0.15);
  } catch {
    /* The visible countdown remains. */
  }
}

/// The attempt is over: from now the result is due within the delivery
/// window, however much of the task's time went unused.
function attemptOver() {
  st.attempt = "ending";
  st.reviewPollUntil = Date.now() + st.task.reviewDeliverySeconds * 1000;
  rememberAttempt();
  stopMonitoring();
}

/// Ends the attempt under a rule or a failure. Monitoring stops first, so
/// CodeTrial leaving fullscreen afterwards is not itself reported.
function endAttempt(outcome, cause, durationMs) {
  if (!monitoring()) return;
  attemptOver();
  lock();
  // An ending that cannot be sent must not leave the attempt running
  // unwatched: leaving the room makes the server end it as interrupted.
  send(TASK_TOPICS.end, taskEndPayload(id(), outcome, cause, durationMs)).catch(
    () => st.room?.disconnect()?.catch?.(() => {}),
  );
  leaveFullscreen();
}

function leaveFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
}

function ruleBroken(cause) {
  if (st.attempt === "starting") st.startViolation ??= cause;
  endAttempt("invalid", cause, 0);
}
document.addEventListener("fullscreenchange", () => {
  if (!document.fullscreenElement) ruleBroken("fullscreen");
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) ruleBroken("visibility");
});
window.addEventListener("pagehide", (event) => {
  // A page kept for back/forward navigation comes back as it was, so its
  // recorder stays usable; only a page that is really going away lets go.
  if (!event.persisted) localRecording.dispose();
  if (st.startInFlight)
    rememberAttempt(
      st.startInFlight.room,
      st.startInFlight.until,
      st.startInFlight.key,
      true,
    );
  endAttempt("interrupted", "reload", 0);
});

// --- The work ----------------------------------------------------------------

function renderState(incoming) {
  const accepted = acceptTaskState(st.state, incoming);
  if (accepted === st.state) return;
  st.state = accepted;
  st.serverClockOffset = advanceServerClock(
    st.serverClockOffset,
    st.state.serverNow,
    performance.now(),
    Date.now(),
  );
  // A new deadline shows now, not at the next tick.
  if (st.attemptTimers) paintClock();
  $("phase").textContent = `${st.state.phase} - Jim ${st.state.interviewer}`;
  $("checks").replaceChildren(
    ...st.state.checks.map((check) => {
      const li = document.createElement("li");
      const description =
        st.task.understandingChecks.find((row) => row.id === check.id)
          ?.question ?? check.id;
      li.textContent = `${description}: ${check.state}`;
      return li;
    }),
  );
  $("hint").textContent =
    `Hint (${Math.max(0, st.state.hintRungsMax - st.state.hintRungsUsed)} remaining)`;
  // The server stops honoring "thinking" once the work is frozen; the box,
  // locked from here on, says the same.
  if (st.state.finalRevisionId) $("thinking").checked = false;
  if (
    st.pendingRevision &&
    st.state.acknowledgedCaptureRequestId === st.pendingRevision.requestId
  ) {
    st.pendingRevision.resolve();
    st.pendingRevision = null;
  }
  // A capture the attempt's end overtook is refused without a word, the state
  // being the answer: it settles here rather than as a timeout over the review.
  if (st.pendingRevision && st.state.phase === "feedback") {
    st.pendingRevision.reject(ATTEMPT_ENDED);
    st.pendingRevision = null;
  }
  if (st.state.phase === "feedback" && st.attempt === "running") {
    // The server ended it (Finish or time): stop watching before leaving
    // fullscreen.
    attemptOver();
    leaveFullscreen();
  }
  lock();
}

// Every reason the learner may not change the code right now.
function editorLocked() {
  return (
    ["ending", "lost", "done"].includes(st.attempt) ||
    st.actionPending ||
    taskLocked(st.state)
  );
}
function lock() {
  const locked = editorLocked();
  st.board?.refresh();
  // A Finish confirmed after the attempt ended would act on nothing.
  if (
    $("confirm").open &&
    (["ending", "lost", "done"].includes(st.attempt) || taskLocked(st.state))
  )
    $("confirm").close();
  $("code").disabled = locked;
  for (const control of [
    "run",
    "hint",
    "discuss",
    "finish",
    "target",
    "thinking",
  ])
    $(control).disabled =
      locked ||
      (control === "run" && st.runBusy) ||
      (["hint", "discuss"].includes(control) &&
        st.state?.interviewer !== "available");
}

// What a capture settles with when the attempt ended before acknowledging it;
// nothing is left to report or retry.
const ATTEMPT_ENDED = Object.freeze({ attemptEnded: true });

/// Whether the attempt is over for good: its result is shown, or its room is
/// lost. A send still in flight then fails on the room that is gone, which is
/// no news to the learner looking at the result.
const attemptFinished = () => st.attempt === "done" || st.attempt === "lost";

const CODE_TOO_LARGE = Object.freeze({
  code: "action_rejected",
  message: `The code is longer than ${TASK_MAX_CODE_BYTES} bytes; shorten it to continue.`,
});

async function capture(trigger) {
  if (st.pendingRevision)
    throw new Error("A revision is still awaiting acknowledgement.");
  if (taskCodeTooLarge($("code").value)) throw CODE_TOO_LARGE;
  const captured = { id: st.revisionId, code: $("code").value };
  clearTimeout(st.editTimer);
  const requestId = id();
  const promise = new Promise((resolve, reject) => {
    st.pendingRevision = { requestId, resolve, reject };
  });
  const timer = setTimeout(() => {
    st.pendingRevision?.reject(
      new Error("Revision acknowledgement timed out."),
    );
    st.pendingRevision = null;
  }, 10000);
  // The server holds a run's revision only once it has received it; a gap
  // reported for one it never saw is refused.
  let sent = false;
  try {
    await send(
      TASK_TOPICS.revision,
      taskRevisionPayload(requestId, captured.id, captured.code, trigger),
    );
    sent = true;
    await promise;
    return captured;
  } catch (value) {
    // A send that never left settles nothing, so the slot is freed here or
    // every later capture refuses as still awaiting this one.
    if (st.pendingRevision?.requestId === requestId) st.pendingRevision = null;
    if (
      trigger === "run" &&
      sent &&
      !attemptFinished() &&
      value !== ATTEMPT_ENDED
    )
      await sendRunGap(
        captured.id,
        "Capture acknowledgement missing; no practice cases ran.",
      ).catch(() => {});
    throw value;
  } finally {
    clearTimeout(timer);
  }
}
// A run that produced no summary, reported so the server releases the
// revision it held for it; a telemetry gap, never a failed case.
function sendRunGap(revision, diagnostics) {
  return send(
    TASK_TOPICS.run,
    taskRunPayload(id(), id(), revision, 0, 0, diagnostics),
  );
}
async function action(action, revision = null) {
  await send(
    TASK_TOPICS.action,
    taskActionPayload(
      id(),
      action,
      revision,
      ["hint", "discuss"].includes(action) ? $("target").value || null : null,
    ),
  );
}

$("code").oninput = () => {
  st.revisionId = id();
  clearTimeout(st.editTimer);
  st.editTimer = setTimeout(sendCurrentEdit, 250);
};
function sendCurrentEdit() {
  if (!st.room || editorLocked()) return;
  // The server refuses it anyway; saying so here beats a silent drop.
  if (taskCodeTooLarge($("code").value)) return error(CODE_TOO_LARGE);
  send(
    topics.code,
    taskEditPayload(st.revisionId, $("code").value, st.language),
  ).catch(error);
}
$("code").onkeydown = (event) => {
  if (event.key === "F2") {
    event.preventDefault();
    st.tabLeavesEditor = true;
    return;
  }
  if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
  const indents = event.key === "Tab" && !st.tabLeavesEditor;
  st.tabLeavesEditor = false;
  // An input method confirming a candidate, or a chord the page does not
  // own, keeps its key. Some input methods end composition before the
  // confirming keydown, which still carries the legacy code 229.
  if (
    !indents ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    event.isComposing ||
    event.keyCode === 229
  )
    return;
  event.preventDefault();
  const editor = $("code");
  applyEditorEdit(
    editor,
    indentSelection(
      editor.value,
      editor.selectionStart,
      editor.selectionEnd,
      event.shiftKey,
    ),
  );
};
// A line break arrives as input rather than as a key, which leaves out
// composition and chords that type no newline.
$("code").addEventListener("beforeinput", (event) => {
  if (
    applyingEditorEdit() ||
    event.isComposing ||
    !event.cancelable ||
    event.inputType !== "insertLineBreak"
  )
    return;
  event.preventDefault();
  const editor = $("code");
  applyEditorEdit(
    editor,
    indentNewline(
      editor.value,
      editor.selectionStart,
      editor.selectionEnd,
      st.language,
    ),
  );
});
$("target").onchange = () => {
  const target = st.task.completionTargets.find(
    (row) => row.id === $("target").value,
  );
  if (!target) return;
  const at = $("code").value.indexOf(target.marker);
  if (at >= 0) {
    $("code").focus();
    $("code").setSelectionRange(at, at + target.marker.length);
  }
};
$("hint").onclick = () => action("hint").catch(error);
// Discuss and Finish name the revision they captured, so the editor stays
// locked until the action is sent.
async function captureThen(trigger) {
  st.actionPending = true;
  lock();
  try {
    const captured = await capture(trigger);
    await action(trigger, captured.id);
  } catch (value) {
    if (value !== ATTEMPT_ENDED && !attemptFinished()) error(value);
  } finally {
    st.actionPending = false;
    lock();
  }
}
$("discuss").onclick = () => captureThen("discuss");
$("finish").onclick = () => $("confirm").showModal();
$("cancel-finish").onclick = () => $("confirm").close();
$("confirm-finish").onclick = () => {
  $("confirm").close();
  if (editorLocked()) return;
  return captureThen("finish");
};
$("run").onclick = async () => {
  // The captured revision until its summary is sent.
  let unreported;
  st.runBusy = true;
  lock();
  try {
    const captured = await capture("run");
    unreported = captured.id;
    const results = await runBrowserTests(
      st.taskId,
      captured.code,
      st.language,
      (text) => {
        $("results").textContent = text;
      },
      [],
      { ...st.task.publicCases, optimization: st.task.optimization },
    );
    $("results").textContent = results.runnerUnavailable
      ? `Practice runner unavailable: ${results.setupError}. No cases executed; this is a telemetry gap.`
      : `${results.passed}/${results.total} public cases passed (learner-reported practice evidence)\n${
          results.setupError ??
          results.cases
            .map((row, index) => caseLine(row, index, st.task?.requiredCases))
            .join("\n")
        }`;
    await send(
      TASK_TOPICS.run,
      taskRunPayload(
        id(),
        id(),
        captured.id,
        results.runnerUnavailable ? 0 : results.passed,
        results.runnerUnavailable ? 0 : results.total,
        String(
          results.setupError ??
            (results.total ? "" : "Public runner produced no test results."),
        ).slice(0, 4096),
      ),
    );
    unreported = null;
  } catch (value) {
    if (value !== ATTEMPT_ENDED && !attemptFinished()) error(value);
  } finally {
    if (unreported && !attemptFinished())
      await sendRunGap(
        unreported,
        "Practice runner did not complete; this is a telemetry gap.",
      ).catch(error);
    st.runBusy = false;
    lock();
  }
};
$("thinking").onchange = () =>
  send(TASK_TOPICS.thinking, taskThinkingPayload($("thinking").checked)).catch(
    error,
  );

st.editorView = mountEditorView({
  stack: $("code").closest(".editor-stack"),
  editor: $("code"),
  language: () => st.language,
});

async function publishBoard(blob, strokes) {
  if (!st.room || !blob) return false;
  const bytes = new Uint8Array(await blob.arrayBuffer());
  st.boardSequence += 1;
  const writer = await st.room.localParticipant.streamBytes(
    boardStreamOptions(st.boardSequence, strokes, bytes.byteLength),
  );
  try {
    await writer.write(bytes);
  } catch (value) {
    // A writer left open keeps the agent's reader, and one of its few slots,
    // open.
    await writer.close().catch(() => {});
    throw value;
  }
  await writer.close();
  return true;
}
// The drawing is not part of a code capture, so a Run, Hint or Discuss in
// flight does not take the pen from the learner; only the attempt's end does.
const boardLocked = () =>
  ["ending", "lost", "done"].includes(st.attempt) || taskLocked(st.state);
st.board = mountExampleBoard($("task-board"), {
  locked: boardLocked,
  publish: publishBoard,
  connected: () => Boolean(st.room),
});

// The brief and cases beside the editor, or the editor alone across the page.
// The button stays on screen in both views, so focus never leaves it.
$("maximize-editor").onclick = () => {
  const maximized =
    $("maximize-editor").getAttribute("aria-pressed") !== "true";
  $("maximize-editor").setAttribute("aria-pressed", String(maximized));
  $("task-grid").dataset.maximized = String(maximized);
};

// A result belongs to the expression and the unit it was computed from.
for (const [id, event] of [
  ["calculator-angle", "onchange"],
  ["calculator-input", "oninput"],
])
  $(id)[event] = () => {
    $("calculator-result").textContent = "";
  };

$("calculator").onsubmit = (event) => {
  event.preventDefault();
  try {
    $("calculator-result").textContent = formatResult(
      evaluate($("calculator-input").value, {
        angle: $("calculator-angle").value,
      }),
    );
  } catch (value) {
    $("calculator-result").textContent = value.message;
  }
};

// --- The result --------------------------------------------------------------

function renderReview(value, awaitingResult = false) {
  value = taskReviewForDisplay(value);
  clearError();
  st.review = value;
  st.attempt = "done";
  stopMonitoring();
  stopAttemptTimers();
  lock();
  // The result is the room's last message; nothing else comes from it.
  if (st.room) dropConnection();
  $("clock").textContent = "Task finished";
  $("review").hidden = false;
  localRecording.finish($("review"));
  $("outcome").textContent = outcomeSentence(value.taskAssessment?.outcome);
  const container = $("review-content");
  container.replaceChildren();
  $("new-attempt").hidden = awaitingResult;
  const finalCode = value.taskAssessment?.finalCode;
  // A restored tab has only what the server kept; without its code there is
  // nothing to download.
  const codeLost = st.restored && typeof finalCode !== "string";
  $("export-code").disabled = codeLost;
  if (value.feedbackUnavailable) {
    container.textContent = feedbackUnavailableText(awaitingResult, codeLost);
    return;
  }
  if (!value.taskAssessment?.dimensions) return;
  function appendFeedback(label, text) {
    const section = document.createElement("section");
    section.className = "review-item";
    const heading = document.createElement("h3");
    heading.textContent = label;
    const p = document.createElement("p");
    p.textContent = text;
    section.append(heading, p);
    container.append(section);
  }
  for (const [label, text] of reviewSections(value.taskAssessment)) {
    appendFeedback(label, text);
  }
}
$("new-attempt").onclick = () => {
  forgetAttempt();
  location.reload();
};
$("export-code").onclick = () =>
  downloadText(
    st.review?.taskAssessment?.finalCode ?? $("code").value,
    taskCodeFileName(
      st.taskId,
      st.review?.taskAssessment?.language ?? st.language,
    ),
    "text/plain",
  );
/// What the exported file says about itself, so it is not passed along as
/// something it is not.
const EXPORT_NOTICE =
  "Generated by CodeTrial on the learner's own computer: learning feedback, not an authenticated submission or a secure grade.";
$("export-review").onclick = () =>
  downloadText(
    JSON.stringify({ notice: EXPORT_NOTICE, ...st.review }, null, 2),
    `${st.taskId}-result.json`,
    "application/json",
  );
// No warning while the rules are armed: a browser leaves fullscreen to show
// the dialog, so even "Stay" would end the attempt as invalid, where leaving
// outright is only an interruption. After the attempt ends, it guards the
// result still on its way.
window.addEventListener("beforeunload", (event) => {
  const resultDue =
    hasStarted() &&
    !st.restored &&
    !["starting", "done"].includes(st.attempt) &&
    !monitoring();
  // Unsaved media outlives the result: the take stays in this tab alone.
  const mediaUnsaved =
    localRecording.unsaved && st.attempt !== "starting" && !monitoring();
  if (resultDue || mediaUnsaved) {
    event.preventDefault();
    event.returnValue = "";
  }
});
function paintClock() {
  if (!st.review && st.state?.deadlineAt)
    $("clock").textContent = `${formatTime(
      timeLeft(
        Date.parse(st.state.deadlineAt),
        serverTimeAt(st.serverClockOffset, performance.now(), Date.now()),
      ).remaining,
    )} remaining`;
}
function startAttemptTimers() {
  st.attemptTimers ??= [
    setInterval(paintClock, 1000),
    setInterval(pollReview, 5000),
  ];
}
function stopAttemptTimers() {
  for (const timer of st.attemptTimers ?? []) clearInterval(timer);
  st.attemptTimers = null;
}
async function pollReview() {
  if (
    !st.activeRoomName ||
    st.attempt === "done" ||
    !hasStarted() ||
    st.reviewPolling
  )
    return;
  // A live room delivers the result itself; the server only has to be asked
  // once the attempt is over or the room is gone.
  if (monitoring() || st.attempt === "starting") return;
  st.reviewPolling = true;
  let value;
  let refused = false;
  try {
    value = await api(
      `/api/task-reviews/${encodeURIComponent(st.activeRoomName)}`,
    );
  } catch (refusal) {
    value = refusal;
    refused = true;
  } finally {
    st.reviewPolling = false;
  }
  // The room may have delivered the result while this was on its way.
  if (st.attempt === "done") return;
  if (refused) {
    if (value.code !== "result_unavailable") return error(value);
    if (st.startUnconfirmed) return startNeverArrived();
    st.activeRoomName = null;
    forgetAttempt();
    renderReview({ assessmentMode: "task", feedbackUnavailable: true });
  } else if (!value.pending) renderReview(value);
  else if (Date.now() >= st.reviewPollUntil)
    renderReview({ assessmentMode: "task", feedbackUnavailable: true }, true);
}

// --- Boot --------------------------------------------------------------------

const isBuiltIn = () =>
  Boolean(st.builtIn) &&
  st.setId === st.builtIn.setId &&
  st.site === st.builtIn.site &&
  st.version === st.builtIn.version;

api("/api/session")
  .then((session) => {
    st.builtIn = session.builtInTask ?? null;
    // The default task needs no sign-in, no server and no PIN: straight to
    // preparing for it, unless an attempt at it is waiting to be collected.
    if (isBuiltIn()) return signedInFlow(openDefault);
    if (!st.site || !Number.isSafeInteger(st.version) || st.version < 1)
      return error({
        code: "site_invalid",
        message:
          "Open this task from the assignment link on your instructor's page; it names the site and version.",
      });
    // A result names a GitHub account, so one signed in under a name of its
    // own choosing signs in again here.
    return session.signedIn && session.githubSignedIn
      ? signedInFlow()
      : showSignin();
  })
  .catch(error);
