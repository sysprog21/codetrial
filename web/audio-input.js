// The preflight's microphone picker, which works on the pool before its stream
// is published. Moving the published input once the interview is under way is
// web/mic-switch.js.
export function createAudioInput({
  mediaDevices,
  pool,
  select,
  note,
  isFinished,
}) {
  let listing = 0;
  const showNote = (text) => {
    note.textContent = text;
    note.hidden = !text;
  };
  const supported =
    pool.supported &&
    Boolean(mediaDevices?.enumerateDevices) &&
    Boolean(mediaDevices?.getSupportedConstraints?.().deviceId);
  select.disabled = !supported;
  if (!supported) {
    showNote("Microphone selection is unavailable. Use the system default.");
  }
  const refresh = async () => {
    if (!supported || isFinished()) return;
    const request = ++listing;
    try {
      const devices = await mediaDevices.enumerateDevices();
      if (isFinished() || request !== listing) return;
      const selected = select.value;
      const options = [new Option("System default", "")];
      for (const device of devices) {
        if (
          device.kind !== "audioinput" ||
          !device.deviceId ||
          device.deviceId === "default"
        )
          continue;
        options.push(
          new Option(
            device.label || `Microphone ${options.length}`,
            device.deviceId,
          ),
        );
      }
      select.replaceChildren(...options);
      select.value = options.some((option) => option.value === selected)
        ? selected
        : "";
      showNote("");
      if (selected && !select.value) pool.selectMicrophone("");
    } catch {
      if (!isFinished() && request === listing)
        showNote(
          "Microphones could not be listed. Check your browser permissions and reload the page.",
        );
    }
  };
  const change = () => {
    if (!isFinished()) pool.selectMicrophone(select.value);
  };
  select.addEventListener("change", change);
  mediaDevices?.addEventListener?.("devicechange", refresh);
  return {
    refresh,
    close() {
      listing += 1;
      select.removeEventListener("change", change);
      mediaDevices?.removeEventListener?.("devicechange", refresh);
    },
  };
}
