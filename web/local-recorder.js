// The interview owns the microphone; only display capture and the audio mix
// belong to this recorder. No recording bytes enter the interview data path.
export function initLocalRecording({
  state,
  nodes,
  recordingEnabled,
  isRoomLive,
  prepareMicrophone,
  onRecordingChange,
  stopFocusTarget,
  canStart,
  onSetupChange,
}) {
  const panel = nodes.localRecording;
  const start = panel.querySelector("[data-local-start]");
  const stop = panel.querySelector("[data-local-stop]");
  const status = panel.querySelector("[data-local-status]");
  const download = panel.querySelector("[data-local-download]");
  const microphoneLostMessage =
    "The interview microphone stopped, so recording stopped. Download any available recording, then start again.";
  let phase = "idle";
  let ended = false;
  let disposed = false;
  let capture, context, destination, microphoneSource, recorder, timer, url;
  let chunks = [];
  let startedAt = 0;
  let failure = "";
  let discarded = false;
  let displacedFocus = null;
  // Setup cannot retry until the previous recording's final chunks are saved.
  let stopped = Promise.resolve();
  let resolveStopped;
  let cancelStartup;
  let stopMicrophoneWatch = () => {};
  let captureHandle;
  if (!recordingEnabled) {
    try {
      const handle = crypto.randomUUID();
      navigator.mediaDevices.setCaptureHandleConfig({
        handle,
        exposeOrigin: false,
        permittedOrigins: [location.origin],
      });
      captureHandle = handle;
    } catch {
      // An unavailable capture API must not prevent the interview starting.
    }
  }

  function requireInterviewTab(video) {
    if (
      !captureHandle ||
      video.getSettings().displaySurface !== "browser" ||
      video.getCaptureHandle?.()?.handle !== captureHandle
    )
      throw new Error(
        "Wrong capture source. Start again, choose this CodeTrial tab, and enable Share tab audio.",
      );
  }

  function paint(message) {
    const hideStart = ended || ["recording", "stopping"].includes(phase);
    const hideStop = !["recording", "stopping"].includes(phase);
    let focused = document.activeElement;
    if (focused === document.body && displacedFocus) focused = displacedFocus;
    else displacedFocus = null;
    let nextFocus;
    if (focused === start && hideStart)
      nextFocus = !hideStop ? stop : !download.hidden ? download : null;
    else if (focused === stop && hideStop)
      nextFocus = !download.hidden
        ? download
        : (stopFocusTarget?.() ?? (!hideStart ? start : null));
    if (nextFocus) {
      nextFocus.hidden = false;
      nextFocus.removeAttribute("disabled");
      nextFocus.removeAttribute("aria-disabled");
      nextFocus.focus();
      displacedFocus = null;
    }
    // Pending controls keep focus until their replacement can receive it.
    start.disabled = ended || phase === "recording" || phase === "stopping";
    start.setAttribute(
      "aria-disabled",
      String(start.disabled || phase === "requesting"),
    );
    stop.disabled = hideStop;
    stop.setAttribute("aria-disabled", String(phase !== "recording"));
    start.hidden = hideStart;
    stop.hidden = hideStop;
    status.classList.toggle(
      "visually-hidden",
      (["idle", "recording", "stopping"].includes(phase) && !failure) ||
        (phase === "ready" && Boolean(url) && !failure),
    );
    if (message) status.textContent = message;
  }

  function release() {
    clearInterval(timer);
    cancelStartup = null;
    stopMicrophoneWatch();
    onRecordingChange?.(false);
    capture?.getTracks().forEach((track) => track.stop());
    destination?.stream.getTracks().forEach((track) => track.stop());
    microphoneSource?.disconnect();
    void context?.close().catch(() => {});
    capture = context = destination = microphoneSource = null;
  }

  function bindMicrophone() {
    const mic = state.localUserStream
      ?.getAudioTracks()
      .filter((track) => track.readyState === "live");
    if (!mic?.length)
      throw new Error(
        phase === "recording"
          ? microphoneLostMessage
          : "Local recording needs the interview microphone. Complete the interview setup first.",
      );
    const source = context.createMediaStreamSource(new MediaStream(mic));
    source.connect(destination);
    stopMicrophoneWatch();
    microphoneSource?.disconnect();
    microphoneSource = source;
    const microphoneEnded = () => {
      // A live room owns recovery; its switch rebinds the input or stops us.
      if (
        microphoneSource !== source ||
        phase !== "recording" ||
        isRoomLive?.()
      )
        return;
      failure = microphoneLostMessage;
      stopRecording();
    };
    for (const track of mic)
      track.addEventListener("ended", microphoneEnded, { once: true });
    stopMicrophoneWatch = () => {
      for (const track of mic)
        track.removeEventListener("ended", microphoneEnded);
      stopMicrophoneWatch = () => {};
    };
  }

  function revoke() {
    if (url) URL.revokeObjectURL(url);
    url = null;
    download.hidden = true;
    download.removeAttribute("href");
  }

  function finalize() {
    if (!recorder) return;
    release();
    try {
      if (!disposed && !discarded && chunks.length) {
        url = URL.createObjectURL(
          new Blob(chunks, { type: recorder.mimeType }),
        );
        download.href = url;
        download.download = `codetrial-${new Date().toISOString().replace(/[:.]/g, "-")}.webm`;
        download.hidden = false;
        // A take that stopped on an error is saved too: leaving the page
        // revokes it in `dispose`, possibly before anyone used the link.
        download.click();
      }
    } catch {
      failure =
        "Local recording could not be saved. Your interview can continue.";
    }
    chunks = [];
    recorder = null;
    phase = "ready";
    paint(
      (discarded && "Recording discarded. Start again when you are ready.") ||
        failure ||
        (url
          ? "Local recording ready. If the download did not start, use Download."
          : "No local recording was saved."),
    );
    resolveStopped?.();
    resolveStopped = null;
  }

  function stopRecording({ discard = false } = {}) {
    if (discard && ["recording", "stopping"].includes(phase)) discarded = true;
    if (phase === "requesting") cancelStartup?.();
    if (phase !== "recording") return stopped;
    phase = "stopping";
    stop.textContent = "Saving...";
    clearInterval(timer);
    paint("Finalizing local recording...");
    // An automatic stop still owes its final dataavailable and stop events.
    if (recorder.state === "inactive") return stopped;
    try {
      recorder.stop();
    } catch {
      failure =
        "Local recording could not finish. Your interview can continue; download any available recording.";
      finalize();
    }
    return stopped;
  }

  // `onSetupChange` brackets the whole request, picker and audio startup
  // included, so a host can hold its own actions off while it is open.
  async function startRecording() {
    if (ended || !["idle", "ready", "error"].includes(phase))
      return !ended && phase === "recording";
    if (canStart && !canStart()) return false;
    onSetupChange?.(true);
    try {
      return await requestRecording();
    } finally {
      onSetupChange?.(false);
    }
  }

  async function requestRecording() {
    phase = "requesting";
    failure = "";
    discarded = false;
    paint("Choose the CodeTrial tab and enable Share tab audio.");
    try {
      if (
        !navigator.mediaDevices?.getDisplayMedia ||
        !globalThis.MediaRecorder ||
        !globalThis.AudioContext ||
        !MediaRecorder.isTypeSupported("video/webm")
      )
        throw new Error(
          "Local recording needs Chrome or Chromium with tab capture and WebM support.",
        );
      if (!captureHandle)
        throw new Error(
          "Local recording needs Chrome or Chromium with Capture Handle support. Open this CodeTrial page in its own tab.",
        );
      capture = await navigator.mediaDevices.getDisplayMedia({
        video: true,
        // Capture already-rendered Jim audio without treating it as mic echo.
        audio: {
          suppressLocalAudioPlayback: false,
          restrictOwnAudio: false,
          echoCancellation: false,
          noiseSuppression: false,
          autoGainControl: false,
        },
        preferCurrentTab: true,
        selfBrowserSurface: "include",
      });
      // Ending cannot dismiss Chrome's picker. Release a late grant without
      // starting a recording after the interview has already ended.
      if (ended) {
        release();
        phase = "idle";
        paint("Local recording was cancelled because the interview ended.");
        return false;
      }
      if (!capture.getAudioTracks().length)
        throw new Error(
          "No tab audio. Start again, choose the CodeTrial tab, and enable Share tab audio to include Jim's voice.",
        );
      const video = capture.getVideoTracks()[0];
      if (!video || video.readyState !== "live")
        throw new Error(
          "No tab video. Start again and select the CodeTrial tab.",
        );
      requireInterviewTab(video);
      const selectedCapture = capture;
      const captureStoppedMessage =
        "Tab sharing stopped before recording started. Start again to record.";
      const interrupted = new Promise((_, reject) => {
        cancelStartup = (error = new Error(captureStoppedMessage)) =>
          reject(error);
      });
      video.addEventListener(
        "ended",
        () => {
          if (capture === selectedCapture) stopRecording();
        },
        { once: true },
      );
      video.addEventListener("capturehandlechange", () => {
        if (
          capture !== selectedCapture ||
          !["requesting", "recording"].includes(phase)
        )
          return;
        try {
          requireInterviewTab(video);
        } catch (error) {
          if (phase === "requesting") cancelStartup(error);
          else {
            failure =
              "Local recording stopped: wrong capture source. Download any available recording, then start again, choose this CodeTrial tab, and enable Share tab audio.";
            stopRecording();
          }
        }
      });
      context = new AudioContext();
      destination = context.createMediaStreamDestination();
      context
        .createMediaStreamSource(new MediaStream(capture.getAudioTracks()))
        .connect(destination);
      await Promise.race([
        (async () => {
          await context.resume();
          await prepareMicrophone?.(
            () =>
              !ended && capture === selectedCapture && phase === "requesting",
          );
        })(),
        interrupted,
      ]);
      cancelStartup = null;
      if (ended) {
        release();
        phase = "idle";
        paint("Local recording was cancelled because the interview ended.");
        return false;
      }
      if (video.readyState !== "live") throw new Error(captureStoppedMessage);
      requireInterviewTab(video);
      // Setup may have replaced the microphone while audio startup waited.
      bindMicrophone();
      recorder = new MediaRecorder(
        new MediaStream([video, ...destination.stream.getAudioTracks()]),
        {
          mimeType: "video/webm",
          videoBitsPerSecond: 1_000_000,
          audioBitsPerSecond: 128_000,
        },
      );
      recorder.ondataavailable = ({ data }) => {
        if (data.size) chunks.push(data);
      };
      recorder.onstop = finalize;
      recorder.onerror = () => {
        failure =
          "Local recording failed. Your interview can continue; download any available recording.";
        phase = "stopping";
        clearInterval(timer);
        paint(failure);
      };
      recorder.start(1000);
      stopped = new Promise((resolve) => (resolveStopped = resolve));
      revoke();
      phase = "recording";
      onRecordingChange?.(true);
      startedAt = Date.now();
      const tick = () => {
        const seconds = Math.floor((Date.now() - startedAt) / 1000);
        const elapsed = `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
        stop.textContent = `End recording ${elapsed}`;
      };
      tick();
      paint("Recording locally.");
      timer = setInterval(tick, 1000);
      return true;
    } catch (error) {
      release();
      recorder = null;
      chunks = [];
      resolveStopped?.();
      resolveStopped = null;
      phase = "error";
      paint(
        error.name === "NotAllowedError"
          ? "Local recording permission was denied. Start again to allow tab capture; your interview can continue."
          : `Local recording: ${error.message}`,
      );
      return false;
    }
  }
  start.addEventListener("click", startRecording);
  stop.addEventListener("click", stopRecording);
  panel.hidden = recordingEnabled;
  paint();

  return {
    start: startRecording,
    stop: stopRecording,
    // True while the capture picker or audio startup is open.
    get settingUp() {
      return phase === "requesting";
    },
    get needsMicrophone() {
      return !ended && ["requesting", "recording", "stopping"].includes(phase);
    },
    refreshMicrophone() {
      if (phase !== "recording") return;
      try {
        bindMicrophone();
      } catch (error) {
        failure =
          error.message === microphoneLostMessage
            ? microphoneLostMessage
            : `Local recording: ${error.message} Download any available recording.`;
        stopRecording();
      }
    },
    // Whether closing the tab now would lose media the learner has not saved.
    get unsaved() {
      return ["recording", "stopping"].includes(phase);
    },
    finish(target) {
      ended = true;
      stopRecording();
      paint();
      if (target && !recordingEnabled && panel.parentElement !== target) {
        const focused = document.activeElement;
        target.append(panel);
        if (panel.contains(focused)) {
          focused.focus();
          // The ending overlay may not be visible until this call returns.
          if (document.activeElement !== focused) displacedFocus = focused;
        }
      }
    },
    dispose() {
      disposed = true;
      ended = true;
      stopRecording();
      release();
      revoke();
    },
  };
}
