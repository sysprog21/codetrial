const roomName = new URLSearchParams(window.location.search).get("room");
const join = document.querySelector("#join-observer");
const status = document.querySelector("#observer-status");

join.addEventListener("click", async () => {
  join.disabled = true;
  status.textContent = "Connecting…";
  try {
    if (!roomName) throw new Error("This observer link has no room name.");
    const response = await fetch("/api/observer-token", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ roomName }),
    });
    const connection = await response.json();
    if (!response.ok) throw new Error(connection.error || "Could not join this room.");
    const room = new window.LivekitClient.Room({ adaptiveStream: true, dynacast: true });
    room.on(window.LivekitClient.RoomEvent.TrackSubscribed, (track) => {
      if (track.kind !== "audio") return;
      const audio = track.attach();
      audio.autoplay = true;
      document.body.append(audio);
      void audio.play().catch(() => {});
    });
    await room.connect(connection.serverUrl, connection.token);
    status.textContent = "Connected. Listening to the interview.";
  } catch (error) {
    status.textContent = error.message || "Could not join this room.";
    join.disabled = false;
  }
});
