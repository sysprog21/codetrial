// The interview owns the microphone; only display capture and the audio mix
// belong to this recorder. No recording bytes enter the interview data path.
export function initLocalRecording({
  state,
  nodes,
  recordingEnabled,
  onRecordingChange,
}) {
  const panel = nodes.localRecording;
  const start = panel.querySelector("[data-local-start]");
  const stop = panel.querySelector("[data-local-stop]");
  const status = panel.querySelector("[data-local-status]");
  const download = panel.querySelector("[data-local-download]");
  let phase = "idle";
  let ended = false;
  let disposed = false;
  let capture, context, destination, recorder, timer, url;
  let chunks = [];
  let startedAt = 0;
  let failure = "";

  function paint(message) {
    start.disabled =
      ended ||
      phase === "requesting" ||
      phase === "recording" ||
      phase === "stopping";
    stop.disabled = phase !== "recording";
    start.hidden = ended || ["recording", "stopping"].includes(phase);
    stop.hidden = !["recording", "stopping"].includes(phase);
    status.hidden =
      (["idle", "recording", "stopping"].includes(phase) && !failure) ||
      (phase === "ready" && Boolean(url) && !failure);
    if (message) status.textContent = message;
  }

  function release() {
    clearInterval(timer);
    onRecordingChange?.(false);
    capture?.getTracks().forEach((track) => track.stop());
    destination?.stream.getTracks().forEach((track) => track.stop());
    void context?.close().catch(() => {});
    capture = context = destination = null;
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
      if (!disposed && chunks.length) {
        url = URL.createObjectURL(
          new Blob(chunks, { type: recorder.mimeType }),
        );
        download.href = url;
        download.download = `codetrial-${new Date().toISOString().replace(/[:.]/g, "-")}.webm`;
        download.hidden = false;
        if (!failure) download.click();
      }
    } catch {
      failure =
        "Local recording could not be saved. Your interview can continue.";
    }
    chunks = [];
    recorder = null;
    phase = "ready";
    paint(
      failure ||
        (url
          ? "Local recording ready. Download again if needed."
          : "No local recording was saved."),
    );
  }

  function stopRecording() {
    if (phase !== "recording") return;
    phase = "stopping";
    stop.textContent = "Saving...";
    clearInterval(timer);
    paint("Finalizing local recording...");
    // An automatic stop still owes its final dataavailable and stop events.
    if (recorder.state === "inactive") return;
    try {
      recorder.stop();
    } catch {
      failure =
        "Local recording could not finish. Your interview can continue.";
      finalize();
    }
  }

  async function startRecording() {
    if (ended || !["idle", "ready", "error"].includes(phase)) return;
    phase = "requesting";
    failure = "";
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
      const mic = state.localUserStream
        ?.getAudioTracks()
        .filter((track) => track.readyState === "live");
      if (!mic?.length)
        throw new Error(
          "Local recording needs the interview microphone. Complete the interview setup first.",
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
        return;
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
      context = new AudioContext();
      destination = context.createMediaStreamDestination();
      context
        .createMediaStreamSource(new MediaStream(capture.getAudioTracks()))
        .connect(destination);
      context
        .createMediaStreamSource(new MediaStream(mic))
        .connect(destination);
      await context.resume();
      if (ended) {
        release();
        phase = "idle";
        paint("Local recording was cancelled because the interview ended.");
        return;
      }
      recorder = new MediaRecorder(
        new MediaStream([video, ...destination.stream.getAudioTracks()]),
        { mimeType: "video/webm" },
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
      video.addEventListener("ended", stopRecording, { once: true });
      recorder.start(1000);
      revoke();
      phase = "recording";
      onRecordingChange?.(true);
      startedAt = Date.now();
      const tick = () => {
        const seconds = Math.floor((Date.now() - startedAt) / 1000);
        const elapsed = `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
        stop.textContent = `End recording ${elapsed}`;
        paint(`Recording locally ${elapsed}`);
      };
      tick();
      timer = setInterval(tick, 1000);
    } catch (error) {
      release();
      recorder = null;
      chunks = [];
      phase = "error";
      paint(
        error.name === "NotAllowedError"
          ? "Local recording permission was denied. Start again to allow tab capture; your interview can continue."
          : `Local recording: ${error.message}`,
      );
    }
  }
  start.addEventListener("click", startRecording);
  stop.addEventListener("click", stopRecording);
  panel.hidden = recordingEnabled;
  paint();

  return {
    start: startRecording,
    finish(target) {
      ended = true;
      stopRecording();
      paint();
      if (target && !recordingEnabled) target.append(panel);
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
