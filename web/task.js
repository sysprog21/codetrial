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
  formatTime,
  countdown as timeLeft,
  languageLabel,
  renderValue,
} from "./lib.js";
import { runBrowserTests } from "./runners.js";
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
} from "./face-presence.js";
import {
  calibrate,
  createLookAwayMonitor,
  MIN_PHASE_SAMPLES,
} from "./look-away.js";
import { evaluate } from "./calculator.js";
import { CONSTRAINTS } from "./devices.js";
import { videoTrackReady } from "./audio-check.js";
import { COMPILED_LANGUAGES } from "./compiler-explorer.js";
import {
  NOT_DETECTED,
  acceptTaskState,
  outcomeSentence,
  rulesInWords,
  taskLocked,
  taskCodeFileName,
  taskRecovery,
  taskReviewForDisplay,
} from "./task-controller.js";

// The vendored LiveKit build is a classic script that sets a global, as on the
// interview page; it has no exports for a module to import.
const { Room, RoomEvent, Track } = globalThis.LivekitClient;

const $ = (id) => document.getElementById(id);
const parts = location.pathname.split("/");
const setId = parts[2];
let taskId = parts[3];
const query = new URLSearchParams(location.search);
const site = query.get("site") ?? "";
const version = Number(query.get("version"));
let task, state, room, review;
let revisionId = "initial";
// The language chosen for this attempt: the task's first until the learner
// picks another, and fixed once a room is joined for it.
let language = "python";
let admissionKey = crypto.randomUUID();
let pendingRevision;
// Where the attempt stands; every guard asks this rather than a flag of its
// own:
//   "preparing"  before Start is sent;
//   "starting"   Start is on its way and not yet acknowledged: the server may
//                already be running the clock;
//   "running"    started, with the rules armed;
//   "ending"     an ending is under way, reported by the page or the server,
//                so CodeTrial leaving fullscreen itself, or a second rule
//                firing, is not reported again;
//   "lost"       started, or perhaps started, but the room is gone: the
//                server finishes the attempt and the page only collects the
//                result;
//   "done"       the result is shown.
let attempt = "preparing";
const hasStarted = () => attempt !== "preparing";
let runBusy = false;
// A Discuss or Finish between its capture and its action: an edit sent in
// between would move the server past the revision the action names.
let actionPending = false;
let editTimer;
let activeRoomName;
let reviewPollUntil;
// The room was lost before Start was acknowledged, so the server may hold an
// attempt or nothing at all; nothing retained then means it never began.
let startUnconfirmed = false;
// A rule broken while Start was being acknowledged, before the rules were
// armed; it still ends the attempt once it runs.
let startViolation = null;
let reviewPolling = false;
let recoveryTimer;
let acknowledgedAt = null;
let media = null;
let detector = null;
let calibration = null;
let calibrating = false;
let monitorTimer = null;
let wakeLock = null;
let beepContext = null;
const id = () => crypto.randomUUID();

/// How often the camera is sampled while preparing and during an attempt.
const SAMPLE_INTERVAL_MS = 500;
/// How long each calibration phase samples.
const CALIBRATION_PHASE_MS = 10000;
const recoveryKey = () =>
  `codetrial-task-result:${JSON.stringify([setId, taskId, site, version])}`;
// A Start sent but not yet confirmed: its room, polling deadline and storage
// key, kept so a reload in between still finds its way back to the result.
let startInFlight = null;
// This tab was reloaded into an attempt it can only collect the result of.
let restored = false;
function rememberAttempt(
  roomName = activeRoomName,
  until = reviewPollUntil,
  key = recoveryKey(),
  unconfirmed = startUnconfirmed,
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
async function signedInFlow() {
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
    activeRoomName = saved.room;
    reviewPollUntil = saved.until;
    startUnconfirmed = saved.unconfirmed === true;
    attempt = "lost";
    restored = true;
    startAttemptTimers();
    show("task");
    lock();
    $("clock").textContent = "Retrieving the interrupted attempt's result...";
    await pollReview();
    return;
  }
  forgetAttempt();
  return showUnlock();
}

/// The line the learner types during calibration.
const CALIBRATION_LINE = "for item in items: total += item";

/// Whether the typing phase saw typing: most of the line, ignoring spacing.
function typedEnough(text) {
  const compact = (value) => value.replace(/\s+/g, "");
  return compact(text).length >= compact(CALIBRATION_LINE).length / 2;
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
  for (const name of ["signin", "unlock", "prepare", "task", "review"])
    $(name).hidden = name !== section;
}

function error(value) {
  clearTimeout(recoveryTimer);
  $("recovery").disabled = Boolean(value.retryAfter);
  if (value.retryAfter)
    recoveryTimer = setTimeout(() => {
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
      admissionKey = id();
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
      if (!task && !value.code) loadTask().catch(error);
      else location.reload();
    }
  };
}

/// Back to the set's tasks after one could not be opened: another task when
/// the set offers one, the PIN otherwise, never the same task again.
function returnToTaskList(code) {
  const others = [...$("task-picker").options].filter(
    (option) => option.value !== taskId,
  );
  if (code === "set_closed" || !others.length || hasStarted() || room)
    return showUnlock();
  $("task-picker").value = others[0].value;
  show("prepare");
  return $("task-picker").onchange();
}

function clearError() {
  clearTimeout(recoveryTimer);
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
  $("assignment").textContent =
    `Task set ${setId}, version ${version}, from ${site}. Enter the PIN your instructor gave you.`;
}

$("pin-form").onsubmit = async (event) => {
  event.preventDefault();
  try {
    const unlocked = await api(
      `/api/task-sets/${encodeURIComponent(setId)}/unlock`,
      { site, version, pin: $("pin").value },
    );
    $("pin").value = "";
    $("task-picker").replaceChildren(
      ...unlocked.tasks.map((row) => new Option(row.title, row.id)),
    );
    $("task-picker").value = taskId;
    showPrepare();
    await loadTask();
  } catch (value) {
    error(value);
  }
};

let loadGeneration = 0;
async function loadTask() {
  // Only the latest selection's answer is shown, whatever order they arrive.
  const generation = ++loadGeneration;
  const selectedTaskId = taskId;
  task = null;
  $("calibrate").disabled = true;
  $("start").disabled = true;
  $("prepare-title").textContent = "Loading assignment…";
  $("title").textContent = "Loading assignment…";
  $("contract").textContent = "";
  $("public-cases").replaceChildren();
  $("brief").replaceChildren();
  $("code").value = "";
  let loaded;
  try {
    loaded = await api(
      `/api/task-sets/${encodeURIComponent(setId)}/tasks/${encodeURIComponent(taskId)}?site=${encodeURIComponent(site)}&version=${version}`,
    );
  } catch (value) {
    if (generation !== loadGeneration) return;
    throw value;
  }
  if (generation !== loadGeneration) return;
  if (loaded.taskId !== selectedTaskId || taskId !== selectedTaskId)
    throw new Error("The loaded task does not match the assignment.");
  task = loaded;
  $("calibrate").disabled = !media || !acknowledgedAt;
  clearError();
  $("title").textContent = task.title;
  $("prepare-title").textContent = task.title;
  $("contract").textContent = task.contract;
  $("public-cases").replaceChildren(
    ...(task.publicCases?.cases ?? []).map((row, index) => {
      const li = document.createElement("li");
      li.textContent = `${caseLabel(row, index)}: input ${renderValue(row.input)}, expected ${renderValue(row.expected)}`;
      return li;
    }),
  );

  $("brief").replaceChildren(
    ...task.brief.map((text) => {
      const p = document.createElement("p");
      p.textContent = text;
      return p;
    }),
  );
  // The server offers only the languages it can run, in the instructor's
  // order.
  $("language").replaceChildren(
    ...task.languages.map((value) => new Option(languageLabel(value), value)),
  );
  $("language-choice").hidden = task.languages.length < 2;
  $("language-note").hidden = !task.languages.some((value) =>
    COMPILED_LANGUAGES.includes(value),
  );
  chooseLanguage(task.languages[0]);
  const rules = rulesInWords(task.rules);
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
  language = value;
  $("language").value = value;
  $("code").value = task.starterCode[value];
  $("code-label").textContent = `${languageLabel(value)} implementation`;
  $("compiled-note").hidden = !COMPILED_LANGUAGES.includes(value);
  $("target").replaceChildren(
    new Option("Whole task", ""),
    ...task.completionTargets
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
  if (hasStarted() || room || !task) return;
  // A request key names one task and language; reused for another, the
  // server would refuse it as an override.
  admissionKey = id();
  chooseLanguage($("language").value);
};

$("task-picker").onchange = () => {
  // The admission names one task: once a room is joined for it, the page
  // cannot show another.
  if (hasStarted() || room) return;
  admissionKey = id();
  taskId = $("task-picker").value;
  history.replaceState(
    null,
    "",
    `/t/${encodeURIComponent(setId)}/${encodeURIComponent(taskId)}${location.search}`,
  );
  revisionId = "initial";
  return loadTask().catch(error);
};

// --- Preparation -----------------------------------------------------------

function showPrepare() {
  show("prepare");
  acknowledgedAt = null;
  $("acknowledge").checked = false;
  $("devices").disabled = true;
}

$("acknowledge").onchange = () => {
  acknowledgedAt = $("acknowledge").checked ? new Date().toISOString() : null;
  if (!acknowledgedAt) $("start").disabled = true;
  $("devices").disabled = !acknowledgedAt || Boolean(media);
  $("calibrate").disabled =
    !acknowledgedAt || !media || !task || Boolean(room) || calibrating;
};

// Permission is granted now, before anything is timed, so no prompt can take
// the page out of fullscreen during the attempt.
$("devices").onclick = async () => {
  try {
    // The interviewer interrupts itself on the learner's voice here too, so
    // the audio asks for what the interview page asks for.
    media = await navigator.mediaDevices.getUserMedia({
      audio: CONSTRAINTS.audio,
      video: { width: 640, height: 480 },
    });
    $("preview").srcObject = media;
    $("preview").hidden = false;
    await $("preview")
      .play()
      .catch(() => {});
    // The calibration needs keypoints; a browser's native detector gives
    // only a count.
    detector?.close?.();
    detector = await createFacePresenceDetector({ native: false });
    if (!detector?.available)
      throw {
        code: "calibration_required",
        message:
          "The face detector could not start. Reload the page and try again.",
      };
    $("devices").disabled = true;
    $("calibrate").disabled = !task || !acknowledgedAt;
  } catch (value) {
    media?.getTracks().forEach((track) => track.stop());
    media = null;
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
  if (!detector || video.readyState < 2) return FACE_SAMPLE_UNAVAILABLE;
  let timer;
  try {
    // The detector takes its own frame from the video; copying one here
    // first would make two per sample.
    return await Promise.race([
      detector.detect(video),
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

async function samplesFor(ms) {
  const samples = [];
  const until = Date.now() + ms;
  while (Date.now() < until) {
    samples.push(await sample());
    await new Promise((resolve) => setTimeout(resolve, SAMPLE_INTERVAL_MS));
  }
  return samples;
}

$("calibrate").onclick = async () => {
  if (!task || task.taskId !== taskId || !acknowledgedAt || calibrating) return;
  calibrating = true;
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
    const typing = await samplesFor(CALIBRATION_PHASE_MS);
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
    calibration = result;
    $("calibration-result").textContent = "Calibrated. Connecting…";
    await connect();
    clearError();
    $("calibration-result").textContent =
      "Calibrated and connected. Press Start when you are ready.";
    $("start").disabled = false;
  } catch (value) {
    error(value);
  } finally {
    calibrating = false;
    $("calibrate").disabled =
      !task || !acknowledgedAt || Boolean(calibration && room);
    lockChoices(Boolean(room));
  }
};

// --- Connection ------------------------------------------------------------

async function send(topic, message) {
  if (!room) throw new Error("Task is not connected.");
  await room.localParticipant.publishData(
    new TextEncoder().encode(JSON.stringify(message)),
    { reliable: true, topic },
  );
}

async function connect() {
  if (!task || task.taskId !== taskId)
    throw new Error("Wait for the selected assignment to load.");
  const supported =
    /Firefox|Chrome|Chromium/.test(navigator.userAgent) &&
    !/Mobile|Android/.test(navigator.userAgent) &&
    Boolean(document.fullscreenEnabled);
  const token = await api(
    `/api/task-sets/${encodeURIComponent(setId)}/tasks/${encodeURIComponent(taskId)}/attempts`,
    {
      site,
      version,
      requestKey: admissionKey,
      language,
      supportedPlatform: supported,
      rulesAcknowledgedAt: acknowledgedAt,
      calibration,
    },
  );
  lockChoices(true);
  // The room never stops the devices' tracks: before Start a dropped room is
  // only left, and the same microphone is published to the next one.
  room = new Room({ stopLocalTrackOnUnpublish: false });
  const thisRoom = room;
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
  room.on(RoomEvent.ParticipantDisconnected, (participant) => {
    if (room === thisRoom && participant?.identity === agentIdentity) lost();
  });
  room.on(RoomEvent.Disconnected, () => {
    if (room === thisRoom) lost();
  });
  room.on(RoomEvent.TrackSubscribed, (track) => {
    if (room !== thisRoom) return;
    if (track.kind === "audio") {
      const audio = track.attach();
      audio.dataset.taskAudio = "1";
      document.body.append(audio);
    }
  });
  room.on(RoomEvent.DataReceived, (payload, participant, kind, topic) => {
    if (room !== thisRoom) return;
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
      if (!message.requestId && state?.phase === "feedback") return;
      // An error naming no request answers no capture: with none pending
      // either, undefined would match undefined.
      if (
        message.requestId &&
        pendingRevision?.requestId === message.requestId
      ) {
        pendingRevision.reject(message);
        pendingRevision = null;
      }
      error(message);
    }
  });
  try {
    await room.connect(token.serverUrl, token.token);
    await room.localParticipant.publishTrack(media.getAudioTracks()[0], {
      source: Track.Source.Microphone,
    });
    const setupDeadline = Date.now() + task.setupTimeoutSeconds * 1000;
    // Sent at least once even if a snapshot already arrived: the runtime
    // accepts Start only after this handshake, and a snapshot it published
    // on its own does not count as one.
    state = null;
    do {
      if (setupFailure) throw setupFailure;
      if (Date.now() >= setupDeadline)
        throw {
          code: "setup_failed",
          message: "The task connection timed out.",
        };
      await send(TASK_TOPICS.connected, taskConnectedPayload());
      if (!state) await new Promise((resolve) => setTimeout(resolve, 1000));
    } while (!state);
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
  activeRoomName = token.roomName;
}

/// Once Start has been sent, losing the room or the interviewer leaves only
/// the result to collect: the server finalizes the attempt without the page.
/// A Start never acknowledged may not have arrived, which the server's answer
/// settles.
function collectResult() {
  if (attemptFinished()) return;
  startUnconfirmed = attempt === "starting";
  startViolation = null;
  attempt = "lost";
  show("task");
  lock();
  stopMonitoring();
  leaveFullscreen();
  reviewPollUntil = Date.now() + task.reviewDeliverySeconds * 1000;
  rememberAttempt();
  $("message-text").textContent = startUnconfirmed
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
  activeRoomName = null;
  startUnconfirmed = false;
  forgetAttempt();
  stopAttemptTimers();
  dropConnection();
  attempt = "preparing";
  if (!task || restored) {
    restored = false;
    showUnlock();
    return error({
      code: "unlock_required",
      message: "The task did not start. Enter the PIN to prepare again.",
    });
  }
  show("prepare");
  lock();
  $("devices").disabled = !acknowledgedAt || Boolean(media);
  error({
    code: "devices_required",
    message:
      "The task did not start. Allow the microphone and camera again, then calibrate again.",
  });
}

/// Lets go of a room that can no longer start the attempt.
function dropConnection() {
  const oldRoom = room;
  room = null;
  // The interviewer's audio element belongs to the room just left.
  for (const audio of document.querySelectorAll("[data-task-audio]"))
    audio.remove();
  oldRoom?.disconnect(false)?.catch?.(() => {});
  // The old key would replay the admission of the room just left.
  admissionKey = id();
  state = null;
  calibration = null;
  $("start").disabled = true;
  lockChoices(false);
  $("calibrate").disabled = !media || !acknowledgedAt;
  $("calibration-result").textContent = "";
}

// --- Start and the rules -----------------------------------------------------

const FULLSCREEN_REQUIRED = {
  code: "fullscreen_required",
  message: "Fullscreen is required to start this task.",
};

// The Start in progress; a newer one makes an older one's continuation moot.
let currentStart = null;

$("start").onclick = () => {
  if (!room || !calibration || hasStarted()) return;
  // A camera that stopped since calibration would only interrupt the
  // attempt the moment it began.
  if (!videoTrackReady(media?.getVideoTracks()[0])) {
    media?.getTracks().forEach((track) => track.stop());
    media = null;
    dropConnection();
    $("devices").disabled = !acknowledgedAt;
    $("calibrate").disabled = true;
    return error({
      code: "devices_required",
      message:
        "The camera stopped. Allow the microphone and camera again, then calibrate.",
    });
  }
  const thisStart = {};
  currentStart = thisStart;
  // A newer Start, or a loss that moved the page on to collecting the
  // result, owns the page from then on.
  const superseded = () => currentStart !== thisStart || attempt !== "starting";
  const startingRoom = room;
  const startingCalibration = calibration;
  const startingRoomName = activeRoomName;
  const startRecoveryKey = recoveryKey();
  $("start").disabled = true;
  // Fullscreen must be requested synchronously inside this click.
  const fullscreen = $("workspace").requestFullscreen?.();
  Promise.resolve(fullscreen)
    .then(async () => {
      if (room !== startingRoom || calibration !== startingCalibration)
        throw {
          code: "setup_failed",
          message:
            "The connection changed before Start. Calibrate again to reconnect.",
        };
      if (!document.fullscreenElement) throw FULLSCREEN_REQUIRED;
      reviewPollUntil =
        Date.now() +
        (task.rules.durationMin * 60 + task.reviewDeliverySeconds) * 1000;
      const until = reviewPollUntil;
      // A reload while Start is on its way cannot tell whether it arrived,
      // so `pagehide` keeps a way back to the result in case it did.
      startInFlight = {
        room: startingRoomName,
        until,
        key: startRecoveryKey,
      };
      startViolation = null;
      attempt = "starting";
      try {
        await send(TASK_TOPICS.start, taskStartPayload());
      } catch {
        startInFlight = null;
        if (superseded()) return;
        attempt = "preparing";
        if (room === startingRoom) dropConnection();
        throw {
          code: "setup_failed",
          message: "Start could not be sent. Calibrate again to reconnect.",
        };
      }
      startInFlight = null;
      // Lost while it was sent, the page is already collecting the result;
      // a newer Start owns the page and its record.
      if (superseded()) return;
      rememberAttempt(startingRoomName, until, startRecoveryKey, true);
      const deadline = Date.now() + 10000;
      while (!superseded() && state?.phase === "ready" && Date.now() < deadline)
        await new Promise((resolve) => setTimeout(resolve, 200));
      if (superseded()) return;
      if (!["work", "wrap-up"].includes(state?.phase)) {
        // Unacknowledged is not refused: the server may be running the
        // clock, so the page leaves the room and asks for the result.
        collectResult();
        dropConnection();
        return;
      }
      attempt = "running";
      rememberAttempt(startingRoomName, until, startRecoveryKey, false);
      startAttemptTimers();
      show("task");
      clearError();
      armRules();
      // Leaving fullscreen or the page while Start was being acknowledged
      // fired before the rules were armed; it still counts, even if the
      // learner came back before the acknowledgement.
      const broken = startViolation;
      startViolation = null;
      if (broken) endAttempt("invalid", broken, 0);
      else if (!document.fullscreenElement)
        endAttempt("invalid", "fullscreen", 0);
      else if (document.hidden) endAttempt("invalid", "visibility", 0);
    })
    .catch((value) => {
      if (currentStart !== thisStart || (room && room !== startingRoom)) return;
      if (!hasStarted()) leaveFullscreen();
      $("start").disabled = !room || !calibration || state?.phase !== "ready";
      error(
        room !== startingRoom || calibration !== startingCalibration
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
      if (monitoring()) wakeLock = lock;
      else lock.release?.().catch(() => {});
    })
    .catch(() => {
      if (!monitoring()) return;
      $("results").textContent =
        "This browser would not keep the screen awake; keep it from locking yourself.";
    });
  const monitor = createLookAwayMonitor({
    limit: calibration.limit,
    metric: calibration.metric,
    thresholdMs: task.rules.lookAwaySeconds * 1000,
  });
  // A muted track delivers frozen or black frames, which would read as a
  // face held still or as nobody there; either way it is not evidence.
  for (const track of media.getVideoTracks()) {
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
    monitorTimer = setTimeout(tick, SAMPLE_INTERVAL_MS);
  };
  monitorTimer = setTimeout(tick, SAMPLE_INTERVAL_MS);
}

function monitoring() {
  return attempt === "running" && state?.phase !== "feedback";
}

/// Stops watching. Once an attempt has started, stopping is for good, so the
/// camera and microphone are let go too and their light goes out.
function stopMonitoring() {
  if (hasStarted()) {
    media?.getTracks().forEach((track) => track.stop());
    media = null;
    detector?.close?.();
    detector = null;
    beepContext?.close().catch(() => {});
    beepContext = null;
  }
  clearTimeout(monitorTimer);
  monitorTimer = null;
  $("countdown").hidden = true;
  wakeLock?.release?.().catch(() => {});
  wakeLock = null;
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
    Math.ceil((task.rules.lookAwaySeconds * 1000 - verdict.awayMs) / 1000),
  );
  $("countdown").textContent =
    `Face the screen: the attempt ends in ${left} seconds.`;
  $("countdown").hidden = false;
  try {
    beepContext ??= new AudioContext();
    const tone = beepContext.createOscillator();
    tone.frequency.value = 880;
    tone.connect(beepContext.destination);
    tone.start();
    tone.stop(beepContext.currentTime + 0.15);
  } catch {
    /* The visible countdown remains. */
  }
}

/// The attempt is over: from now the result is due within the delivery
/// window, however much of the task's time went unused.
function attemptOver() {
  attempt = "ending";
  reviewPollUntil = Date.now() + task.reviewDeliverySeconds * 1000;
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
    () => room?.disconnect()?.catch?.(() => {}),
  );
  leaveFullscreen();
}

function leaveFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
}

function ruleBroken(cause) {
  if (attempt === "starting") startViolation ??= cause;
  endAttempt("invalid", cause, 0);
}
document.addEventListener("fullscreenchange", () => {
  if (!document.fullscreenElement) ruleBroken("fullscreen");
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) ruleBroken("visibility");
});
window.addEventListener("pagehide", () => {
  if (startInFlight)
    rememberAttempt(
      startInFlight.room,
      startInFlight.until,
      startInFlight.key,
      true,
    );
  endAttempt("interrupted", "reload", 0);
});

// --- The work ----------------------------------------------------------------

function renderState(incoming) {
  const accepted = acceptTaskState(state, incoming);
  if (accepted === state) return;
  state = accepted;
  $("phase").textContent = `${state.phase} - Jim ${state.interviewer}`;
  $("checks").replaceChildren(
    ...state.checks.map((check) => {
      const li = document.createElement("li");
      const description =
        task.understandingChecks.find((row) => row.id === check.id)?.question ??
        check.id;
      li.textContent = `${description}: ${check.state}`;
      return li;
    }),
  );
  $("hint").textContent =
    `Hint (${Math.max(0, state.hintRungsMax - state.hintRungsUsed)} remaining)`;
  // The server stops honoring "thinking" once the work is frozen; the box,
  // locked from here on, says the same.
  if (state.finalRevisionId) $("thinking").checked = false;
  if (
    pendingRevision &&
    state.acknowledgedCaptureRequestId === pendingRevision.requestId
  ) {
    pendingRevision.resolve();
    pendingRevision = null;
  }
  // A capture the attempt's end overtook is refused without a word, the state
  // being the answer: it settles here rather than as a timeout over the review.
  if (pendingRevision && state.phase === "feedback") {
    pendingRevision.reject(ATTEMPT_ENDED);
    pendingRevision = null;
  }
  if (state.phase === "feedback" && attempt === "running") {
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
    ["ending", "lost", "done"].includes(attempt) ||
    actionPending ||
    taskLocked(state)
  );
}
function lock() {
  const locked = editorLocked();
  // A Finish confirmed after the attempt ended would act on nothing.
  if (
    $("confirm").open &&
    (["ending", "lost", "done"].includes(attempt) || taskLocked(state))
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
      (control === "run" && runBusy) ||
      (["hint", "discuss"].includes(control) &&
        state?.interviewer !== "available");
}

// What a capture settles with when the attempt ended before acknowledging it;
// nothing is left to report or retry.
const ATTEMPT_ENDED = Object.freeze({ attemptEnded: true });

/// Whether the attempt is over for good: its result is shown, or its room is
/// lost. A send still in flight then fails on the room that is gone, which is
/// no news to the learner looking at the result.
const attemptFinished = () => attempt === "done" || attempt === "lost";

const CODE_TOO_LARGE = Object.freeze({
  code: "action_rejected",
  message: `The code is longer than ${TASK_MAX_CODE_BYTES} bytes; shorten it to continue.`,
});

async function capture(trigger) {
  if (pendingRevision)
    throw new Error("A revision is still awaiting acknowledgement.");
  if (taskCodeTooLarge($("code").value)) throw CODE_TOO_LARGE;
  const captured = { id: revisionId, code: $("code").value };
  clearTimeout(editTimer);
  const requestId = id();
  const promise = new Promise((resolve, reject) => {
    pendingRevision = { requestId, resolve, reject };
  });
  const timer = setTimeout(() => {
    pendingRevision?.reject(new Error("Revision acknowledgement timed out."));
    pendingRevision = null;
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
    if (pendingRevision?.requestId === requestId) pendingRevision = null;
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
  revisionId = id();
  clearTimeout(editTimer);
  editTimer = setTimeout(sendCurrentEdit, 250);
};
function sendCurrentEdit() {
  if (!room || editorLocked()) return;
  // The server refuses it anyway; saying so here beats a silent drop.
  if (taskCodeTooLarge($("code").value)) return error(CODE_TOO_LARGE);
  send(
    topics.code,
    taskEditPayload(revisionId, $("code").value, language),
  ).catch(error);
}
// F2 lets the next Tab leave the editor, so the keyboard is not trapped in
// it. Not Escape, as in interviews: the attempt runs in fullscreen, which the
// browser leaves on Escape, and leaving it ends the attempt.
let tabLeavesEditor = false;
$("code").onkeydown = (event) => {
  if (event.key === "F2") {
    event.preventDefault();
    tabLeavesEditor = true;
    return;
  }
  if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
  const indents = event.key === "Tab" && !tabLeavesEditor;
  tabLeavesEditor = false;
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
      language,
    ),
  );
});
$("target").onchange = () => {
  const target = task.completionTargets.find(
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
  actionPending = true;
  lock();
  try {
    const captured = await capture(trigger);
    await action(trigger, captured.id);
  } catch (value) {
    if (value !== ATTEMPT_ENDED && !attemptFinished()) error(value);
  } finally {
    actionPending = false;
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
  runBusy = true;
  lock();
  try {
    const captured = await capture("run");
    unreported = captured.id;
    const results = await runBrowserTests(
      taskId,
      captured.code,
      language,
      (text) => {
        $("results").textContent = text;
      },
      [],
      task.publicCases,
    );
    $("results").textContent = results.runnerUnavailable
      ? `Practice runner unavailable: ${results.setupError}. No cases executed; this is a telemetry gap.`
      : `${results.passed}/${results.total} public cases passed (learner-reported practice evidence)\n${results.setupError ?? results.cases.map(caseLine).join("\n")}`;
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
    runBusy = false;
    lock();
  }
};
/// Public case `index` as the workspace names it, marked when the task
/// requires it to pass. By index, since a variant rename can make two cases'
/// shown labels equal.
function caseLabel(row, index) {
  const label = row.label ?? `Case ${index + 1}`;
  return (task?.requiredCases ?? []).includes(index)
    ? `${label} (required)`
    : label;
}
/// One case of a run: a failure says what went in, what was expected and what
/// came back, or the error, so the learner has something to debug from.
function caseLine(row, index) {
  const label = caseLabel(row, index);
  if (row.pass) return `OK: ${label}`;
  const detail = [
    row.input !== undefined && `input ${row.input}`,
    row.expected !== undefined && `expected ${row.expected}`,
    row.error
      ? `error ${row.error}`
      : row.got !== undefined && `got ${row.got}`,
  ].filter(Boolean);
  return `FAIL: ${label}${detail.length ? ` (${detail.join("; ")})` : ""}`;
}
$("thinking").onchange = () =>
  send(TASK_TOPICS.thinking, taskThinkingPayload($("thinking").checked)).catch(
    error,
  );

$("calculator").onsubmit = (event) => {
  event.preventDefault();
  try {
    $("calculator-result").textContent = String(
      evaluate($("calculator-input").value),
    );
  } catch (value) {
    $("calculator-result").textContent = value.message;
  }
};

// --- The result --------------------------------------------------------------

function renderReview(value, awaitingResult = false) {
  value = taskReviewForDisplay(value);
  clearError();
  review = value;
  attempt = "done";
  stopMonitoring();
  stopAttemptTimers();
  lock();
  // The result is the room's last message; nothing else comes from it.
  if (room) dropConnection();
  $("clock").textContent = "Task finished";
  $("review").hidden = false;
  $("outcome").textContent = outcomeSentence(value.taskAssessment?.outcome);
  const container = $("review-content");
  container.replaceChildren();
  $("new-attempt").hidden = awaitingResult;
  const finalCode = value.taskAssessment?.finalCode;
  // A restored tab has only what the server kept; without its code there is
  // nothing to download.
  const codeLost = restored && typeof finalCode !== "string";
  $("export-code").disabled = codeLost;
  if (value.feedbackUnavailable) {
    container.textContent = awaitingResult
      ? codeLost
        ? "The result is still being finalized. Your code cannot be retrieved yet; try reloading later or ask your instructor."
        : "The result is still being finalized. Download your code now and reload later to retrieve the result."
      : codeLost
        ? "No retained code is available. You can start a new attempt or ask your instructor."
        : "Feedback unavailable. Download your code and ask your instructor for a review.";
    return;
  }
  if (!value.taskAssessment?.dimensions) return;
  for (const [name, dimension] of Object.entries(
    value.taskAssessment.dimensions,
  )) {
    const p = document.createElement("p");
    p.textContent = `${name}: ${dimension.rating ?? "Insufficient evidence"}. ${dimension.reason}`;
    container.append(p);
  }
  for (const field of ["gaps", "nextActions"]) {
    const p = document.createElement("p");
    p.textContent = `${field}: ${value.taskAssessment[field].join(" ")}`;
    container.append(p);
  }
}
$("new-attempt").onclick = () => {
  forgetAttempt();
  location.reload();
};
$("export-code").onclick = () =>
  downloadText(
    review?.taskAssessment?.finalCode ?? $("code").value,
    taskCodeFileName(taskId, review?.taskAssessment?.language ?? language),
    "text/plain",
  );
/// What the exported file says about itself, so it is not passed along as
/// something it is not.
const EXPORT_NOTICE =
  "Generated by CodeTrial on the learner's own computer: learning feedback, not an authenticated submission or a secure grade.";
$("export-review").onclick = () =>
  downloadText(
    JSON.stringify({ notice: EXPORT_NOTICE, ...review }, null, 2),
    `${taskId}-result.json`,
    "application/json",
  );
// No warning while the rules are armed: a browser leaves fullscreen to show
// the dialog, so even "Stay" would end the attempt as invalid, where leaving
// outright is only an interruption. After the attempt ends, it guards the
// result still on its way.
window.addEventListener("beforeunload", (event) => {
  if (
    hasStarted() &&
    !restored &&
    !["starting", "done"].includes(attempt) &&
    !monitoring()
  ) {
    event.preventDefault();
    event.returnValue = "";
  }
});
// The clock and the result poll run from Start, or a restored attempt, until
// the result is shown; before and after there is nothing for them to do.
let attemptTimers = null;
function startAttemptTimers() {
  attemptTimers ??= [
    setInterval(() => {
      if (!review && state?.deadlineAt)
        $("clock").textContent =
          `${formatTime(timeLeft(Date.parse(state.deadlineAt), Date.now()).remaining)} remaining`;
    }, 1000),
    setInterval(pollReview, 5000),
  ];
}
function stopAttemptTimers() {
  for (const timer of attemptTimers ?? []) clearInterval(timer);
  attemptTimers = null;
}
async function pollReview() {
  if (!activeRoomName || attempt === "done" || !hasStarted() || reviewPolling)
    return;
  // A live room delivers the result itself; the server only has to be asked
  // once the attempt is over or the room is gone.
  if (monitoring() || attempt === "starting") return;
  reviewPolling = true;
  let value;
  let refused = false;
  try {
    value = await api(
      `/api/task-reviews/${encodeURIComponent(activeRoomName)}`,
    );
  } catch (refusal) {
    value = refusal;
    refused = true;
  } finally {
    reviewPolling = false;
  }
  // The room may have delivered the result while this was on its way.
  if (attempt === "done") return;
  if (refused && value.code === "result_unavailable") {
    if (startUnconfirmed) return startNeverArrived();
    activeRoomName = null;
    forgetAttempt();
    renderReview({ assessmentMode: "task", feedbackUnavailable: true });
  } else if (!refused && !value.pending) renderReview(value);
  // A server that keeps failing is waited on no longer than one that keeps
  // saying pending, and the attempt stays recoverable from a later reload.
  // Signing in again is the one refusal that still has its own way back: it
  // fetches the result the server kept.
  else if (
    Date.now() >= reviewPollUntil &&
    !(refused && value.code === "authentication_required")
  )
    renderReview({ assessmentMode: "task", feedbackUnavailable: true }, true);
  else if (refused) error(value);
}

// --- Boot --------------------------------------------------------------------

if (!site || !Number.isSafeInteger(version) || version < 1)
  error({
    code: "site_invalid",
    message:
      "Open this task from the assignment link on your instructor's page; it names the site and version.",
  });
else
  api("/api/session")
    // A result names a GitHub account, so one signed in under a name of its
    // own choosing signs in again here.
    .then((session) =>
      session.signedIn && session.identityVerified
        ? signedInFlow()
        : showSignin(),
    )
    .catch(error);
