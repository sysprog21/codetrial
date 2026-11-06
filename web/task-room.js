// Split out of task.js: the learner's connection to the room and what follows when it
// ends or never comes. It reaches what task.js still owns through `host` (see
// task-state.js), so it can be imported without the page.

import { TASK_TOPICS, taskConnectedPayload } from "./lib.js";
import { $, id } from "./task-dom.js";
import { attemptFinished, hasStarted, host, st } from "./task-state.js";

export async function send(topic, message) {
  if (!st.room) throw new Error("Task is not connected.");
  await st.room.localParticipant.publishData(
    new TextEncoder().encode(JSON.stringify(message)),
    { reliable: true, topic },
  );
}

export async function connect() {
  if (!st.task || st.task.taskId !== st.taskId)
    throw new Error("Wait for the selected assignment to load.");
  const supported =
    /Firefox|Chrome|Chromium/.test(navigator.userAgent) &&
    !/Mobile|Android/.test(navigator.userAgent) &&
    Boolean(document.fullscreenEnabled);
  const token = await host.api(
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
  host.lockChoices(true);
  // The room never stops the devices' tracks: before Start a dropped room is
  // only left, and the same microphone is published to the next one.
  st.room = new host.Room({ stopLocalTrackOnUnpublish: false });
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
    host.error(setupFailure);
  };
  const lost = () => (hasStarted() ? collectResult() : lostBeforeStart());
  st.room.on(host.RoomEvent.ParticipantDisconnected, (participant) => {
    if (st.room === thisRoom && participant?.identity === agentIdentity) lost();
  });
  st.room.on(host.RoomEvent.Disconnected, () => {
    if (st.room === thisRoom) lost();
  });
  st.room.on(host.RoomEvent.TrackSubscribed, (track) => {
    if (st.room !== thisRoom) return;
    if (track.kind === "audio") {
      const audio = track.attach();
      audio.dataset.taskAudio = "1";
      document.body.append(audio);
    }
  });
  st.room.on(
    host.RoomEvent.DataReceived,
    (payload, participant, kind, topic) => {
      if (st.room !== thisRoom) return;
      if (participant?.identity !== agentIdentity) return;
      let message;
      try {
        message = JSON.parse(new TextDecoder().decode(payload));
      } catch {
        return;
      }
      if (topic === TASK_TOPICS.state) host.renderState(message);
      else if (topic === TASK_TOPICS.review) host.renderReview(message);
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
        host.error(message);
      }
    },
  );
  try {
    await st.room.connect(token.serverUrl, token.token);
    await st.room.localParticipant.publishTrack(st.media.getAudioTracks()[0], {
      source: host.Track.Source.Microphone,
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
export function collectResult() {
  if (attemptFinished()) return;
  st.startUnconfirmed = st.attempt === "starting";
  st.startViolation = null;
  st.attempt = "lost";
  host.show("task");
  host.lock();
  host.stopMonitoring();
  host.leaveFullscreen();
  st.reviewPollUntil = Date.now() + st.task.reviewDeliverySeconds * 1000;
  host.rememberAttempt();
  $("message-text").textContent = st.startUnconfirmed
    ? "The connection ended while Start was being confirmed. Checking whether the attempt began."
    : "The room disconnected. Waiting for your result.";
  $("message").hidden = false;
  $("recovery").hidden = true;
  host.startAttemptTimers();
  host.pollReview();
}

/// The server holds nothing for a Start it never acknowledged, so it never
/// began: the learner prepares again, with the devices the page let go of.
export function startNeverArrived() {
  st.activeRoomName = null;
  st.startUnconfirmed = false;
  host.forgetAttempt();
  host.stopAttemptTimers();
  dropConnection();
  st.attempt = "preparing";
  host.localRecordingHome.append($("local-recording"));
  if (!st.task || st.restored) {
    st.restored = false;
    host.showUnlock();
    return host.error({
      code: "unlock_required",
      message: "The task did not start. Enter the PIN to prepare again.",
    });
  }
  host.show("prepare");
  host.lock();
  $("devices").disabled = !st.acknowledgedAt || Boolean(st.media);
  host.error({
    code: "devices_required",
    message:
      "The task did not start. Allow the microphone and camera again, then calibrate again.",
  });
}

/// Lets go of a room that can no longer start the attempt.
export function dropConnection() {
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
  host.lockChoices(false);
  $("calibrate").disabled = !st.media || !st.acknowledgedAt;
  $("calibration-result").textContent = "";
}
