// The app shows this page while the server starts, again with `?error=` when
// it cannot, and with `?browser=` when the interview runs in the browser. The
// reason is the server's own output, so it is written with textContent only:
// shown, never parsed.
const params = new URLSearchParams(location.search);
const error = params.get("error");
const browser = params.get("browser");
if (error !== null) {
  document.getElementById("starting").hidden = true;
  document.getElementById("failure").hidden = false;
  document.getElementById("reason").textContent = error;
  const logs = params.get("logs");
  if (logs !== null) {
    document.getElementById("logs-dir").textContent = logs;
    document.getElementById("logs").hidden = false;
  }
} else if (browser !== null) {
  document.getElementById("starting").hidden = true;
  document.getElementById("browser").hidden = false;
} else {
  // Tells the app whether this web view can make the interview's call. A
  // navigation the app intercepts and cancels, so the page stays and calls
  // nothing native.
  const webrtc = typeof RTCPeerConnection === "function" ? "yes" : "no";
  location.replace(`index.html?webrtc=${webrtc}`);
}
