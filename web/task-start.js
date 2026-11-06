// Split out of task.js: what pressing Start does, from the fullscreen request to the
// first acknowledged state. It reaches what task.js still owns through `host`
// (see task-state.js), so it can be imported without the page.

import { videoTrackReady } from "./audio-check.js";
import { TASK_TOPICS, taskStartPayload } from "./lib.js";
import { $ } from "./task-dom.js";
import { collectResult, dropConnection, send } from "./task-room.js";
import { hasStarted, host, st } from "./task-state.js";

export const startAttempt = () => {
  if (
    !st.room ||
    !st.calibration ||
    hasStarted() ||
    host.localRecording.settingUp
  )
    return;
  // A camera that stopped since calibration would only interrupt the
  // attempt the moment it began.
  if (!videoTrackReady(st.media?.getVideoTracks()[0])) {
    host.releaseMedia();
    dropConnection();
    $("devices").disabled = !st.acknowledgedAt;
    $("calibrate").disabled = true;
    return host.error({
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
  const startRecoveryKey = host.recoveryKey();
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
      if (!document.fullscreenElement) throw host.FULLSCREEN_REQUIRED;
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
      host.rememberAttempt(startingRoomName, until, startRecoveryKey, true);
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
      host.rememberAttempt(startingRoomName, until, startRecoveryKey, false);
      host.startAttemptTimers();
      $("task").prepend($("local-recording"));
      host.show("task");
      host.clearError();
      host.armRules();
      // Leaving fullscreen or the page while Start was being acknowledged
      // fired before the rules were armed; it still counts, even if the
      // learner came back before the acknowledgement.
      const broken = st.startViolation;
      st.startViolation = null;
      if (broken) host.endAttempt("invalid", broken, 0);
      else if (!document.fullscreenElement)
        host.endAttempt("invalid", "fullscreen", 0);
      else if (document.hidden) host.endAttempt("invalid", "visibility", 0);
    })
    .catch((value) => {
      if (st.currentStart !== thisStart) return;
      st.enteringFullscreen = false;
      if (st.room && st.room !== startingRoom) return;
      if (!hasStarted()) host.leaveFullscreen();
      $("start").disabled = !host.startReady();
      host.error(
        st.room !== startingRoom || st.calibration !== startingCalibration
          ? {
              code: "setup_failed",
              message:
                "The connection ended before Start. Calibrate again to reconnect.",
            }
          : value?.code
            ? value
            : host.FULLSCREEN_REQUIRED,
      );
    });
};
