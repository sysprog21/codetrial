import { peerApi } from "./peer-lib.js";

export function mountPeerPresence({
  nodes,
  sdk,
  onMatch,
  onAvailabilityChanged,
  onError,
}) {
  let waitingRoom = null;
  let connectionRevision = 0;
  // The member's latest choice, which a poll must not overwrite.
  let optedOut = false;
  let availabilityId = null;
  let stopped = false;
  let pending = false;
  let loading = false;
  let selectedConsent = new Set();
  let timer;
  let heartbeat;
  let lastMatch = null;
  let rosterVersion;
  let requestVersion;
  let lastSettings;
  let reconnectWaiting = false;
  const fail = (error) => {
    nodes.status.textContent = error.message;
    onError?.(error);
  };
  function preferenceText(settings, host = "Host") {
    const first = settings.interviewerFirst === 1 ? "Joining partner" : host;
    return `${settings.topic || "Any topic"} · ${settings.difficulty ?? ""} · ${settings.language ?? ""} · ${settings.roundMinutes ?? 25} minutes per round. ${first} interviews first. Save transcript: ${settings.saveTranscript ? "yes" : "no"}. Save audio: ${settings.saveAudio ? "yes" : "no"}.`;
  }

  async function disconnect() {
    connectionRevision++;
    const previous = waitingRoom;
    waitingRoom = null;
    await previous?.disconnect();
  }
  async function matched(sessionId) {
    if (!sessionId || sessionId === lastMatch) return;
    availabilityId = null;
    onAvailabilityChanged(false);
    await disconnect();
    // Recorded only once the session loaded, so a failed load is retried.
    await onMatch(sessionId);
    lastMatch = sessionId;
  }
  async function action(task) {
    if (pending) return;
    pending = true;
    try {
      await task();
      await refresh();
    } catch (error) {
      fail(error);
    } finally {
      pending = false;
    }
  }
  function renderRoster(answer) {
    nodes.roster.replaceChildren();
    for (const person of answer.available ?? []) {
      if (person.availabilityId === answer.you?.availabilityId) continue;
      const item = document.createElement("li");
      const name = document.createElement("strong");
      name.textContent = person.nickname || person.login;
      const login = document.createElement("span");
      login.className = "muted small";
      login.textContent = ` · GitHub @${person.login}`;
      const settings = document.createElement("p");
      settings.textContent = preferenceText(
        person.settings ?? {},
        person.nickname || person.login,
      );
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = `Request to join ${person.login}`;
      button.addEventListener(
        "click",
        () =>
          void action(async () => {
            await peerApi("/api/peer/pairing-requests", {
              availabilityId: person.availabilityId,
            });
          }),
      );
      item.append(name, login, settings, button);
      nodes.roster.append(item);
    }
    if (!nodes.roster.children.length)
      nodes.roster.textContent =
        "No partners are available right now. Become available and leave this page open.";
  }
  function renderRequests(answer) {
    nodes.requests.replaceChildren();
    for (const request of answer.requests ?? []) {
      const other =
        request.fromLogin === nodes.login ? request.toLogin : request.fromLogin;
      const item = document.createElement("li");
      const title = document.createElement("h3");
      const otherNickname =
        request.fromLogin === nodes.login
          ? request.toNickname
          : request.fromNickname;
      title.textContent = `Practice with ${otherNickname || other}${otherNickname ? ` (@${other})` : ""}`;
      const settings = document.createElement("p");
      settings.textContent = preferenceText(
        request.settings ?? {},
        request.toNickname || request.toLogin,
      );
      const progress = document.createElement("p");
      progress.textContent = request.confirmedByYou
        ? request.confirmedByPartner
          ? "Both participants confirmed."
          : "Waiting for partner confirmation."
        : request.confirmedByPartner
          ? "Your partner confirmed. Review the choices and confirm to join."
          : "Both participants must confirm before the interview starts.";
      item.append(title, settings, progress);
      if (request.status === "pending" && !request.confirmedByYou) {
        const label = document.createElement("label");
        label.className = "peer-consent";
        const consent = document.createElement("input");
        consent.type = "checkbox";
        const key = request.requestId;
        consent.checked = selectedConsent.has(key);
        const description = document.createElement("span");
        description.textContent =
          "I agree to a silent AI observer and post-session suggestions";
        label.append(consent, description);
        const notice = document.createElement("p");
        notice.className = "muted small";
        notice.textContent =
          "Confirm the transcript and audio saving choices above. The AI observes silently and gives advice after practice. Saved transcript, audio and AI suggestions expire 24 hours after practice ends. Workspace snapshots, your feedback and session details are kept for up to 30 days after the session is created. When transcript saving is off, temporary transcript content is deleted after advice is produced.";
        const confirm = document.createElement("button");
        confirm.type = "button";
        confirm.textContent = `Confirm practice with ${other}`;
        confirm.disabled = !consent.checked;
        consent.addEventListener("change", () => {
          if (consent.checked) selectedConsent.add(key);
          else selectedConsent.delete(key);
          confirm.disabled = !consent.checked;
        });
        confirm.addEventListener(
          "click",
          () =>
            void action(async () => {
              await peerApi(
                `/api/peer/pairing-requests/${encodeURIComponent(request.requestId)}/confirm`,
                { observerConsent: true },
              );
            }),
        );
        item.append(label, notice, confirm);
      }
      if (request.status === "pending") {
        const decline = document.createElement("button");
        decline.type = "button";
        decline.textContent = `Decline request with ${other}`;
        decline.addEventListener(
          "click",
          () =>
            void action(async () => {
              await peerApi(
                `/api/peer/pairing-requests/${encodeURIComponent(request.requestId)}/decline`,
                {},
              );
              selectedConsent = new Set(
                [...selectedConsent].filter((key) => key !== request.requestId),
              );
            }),
        );
        item.append(decline);
      }
      if (request.status !== "pending" && !request.sessionId)
        progress.textContent = `Request ${request.status}.`;
      nodes.requests.append(item);
    }
    if (!nodes.requests.children.length)
      nodes.requests.textContent = "No join requests yet.";
  }
  async function refresh() {
    if (stopped || loading) return;
    loading = true;
    try {
      const answer = await peerApi("/api/peer/lobby");
      if (stopped) return;
      const wasAvailable = Boolean(availabilityId);
      availabilityId = answer.you?.availabilityId ?? null;
      if (availabilityId && !optedOut) nodes.optIn.checked = true;
      else if (wasAvailable) nodes.optIn.checked = false;
      if (wasAvailable && !availabilityId && !answer.you?.sessionId)
        await disconnect();
      onAvailabilityChanged(Boolean(availabilityId));
      const nextRoster = JSON.stringify([
        answer.you?.availabilityId,
        answer.available,
      ]);
      if (nextRoster !== rosterVersion) {
        rosterVersion = nextRoster;
        renderRoster(answer);
      }
      const nextRequests = JSON.stringify(answer.requests);
      if (nextRequests !== requestVersion) {
        requestVersion = nextRequests;
        renderRequests(answer);
      }
      nodes.status.textContent = availabilityId
        ? "You are visible to signed-in partners. Keep this page open to remain available."
        : answer.you?.sessionId
          ? "You are matched. Your practice session is below."
          : "Choose reciprocal mode to make yourself available, or request to join a partner.";
      if (answer.you?.sessionId) await matched(answer.you.sessionId);
      if (reconnectWaiting && !stopped) {
        reconnectWaiting = false;
        if (availabilityId && lastSettings) await enter(lastSettings);
      }
    } finally {
      loading = false;
    }
  }
  async function optOut() {
    await peerApi("/api/peer/availability", {}, "DELETE");
    availabilityId = null;
    await disconnect();
    onAvailabilityChanged(false);
    await refresh();
  }
  nodes.optIn.addEventListener("change", () => {
    optedOut = !nodes.optIn.checked;
    if (!nodes.optIn.checked && availabilityId) void action(optOut);
    else onAvailabilityChanged(false);
  });
  async function enter(settings) {
    if (!nodes.optIn.checked || stopped) return;
    lastSettings = settings;
    let revision = connectionRevision;
    const answer = await peerApi("/api/peer/availability", settings);
    if (stopped || revision !== connectionRevision) return;
    if (!nodes.optIn.checked || optedOut) {
      await peerApi("/api/peer/availability", {}, "DELETE");
      return;
    }
    availabilityId = answer.availabilityId;
    onAvailabilityChanged(true);
    try {
      if (!sdk?.Room)
        throw new Error("The audio room could not be loaded. Try again.");
      await disconnect();
      revision = connectionRevision;
      if (stopped) return;
      const nextRoom = new sdk.Room({ disconnectOnPageLeave: false });
      waitingRoom = nextRoom;
      const disconnectedEvent = sdk.RoomEvent?.Disconnected;
      if (disconnectedEvent)
        nextRoom.on(disconnectedEvent, () => {
          if (waitingRoom !== nextRoom || stopped || !availabilityId) return;
          void (async () => {
            await refresh();
            if (availabilityId && !stopped) await enter(settings);
          })().catch(fail);
        });
      await nextRoom.connect(answer.url, answer.token);
      if (stopped || revision !== connectionRevision) {
        await nextRoom.disconnect();
        return;
      }
      await refresh();
    } catch (error) {
      if (stopped || revision !== connectionRevision) return;
      await optOut();
      throw error;
    }
  }
  return {
    enter,
    start() {
      stopped = false;
      reconnectWaiting = true;
      clearInterval(timer);
      clearInterval(heartbeat);
      void refresh().catch(fail);
      timer = setInterval(() => void refresh().catch(fail), 5000);
      heartbeat = setInterval(() => {
        if (availabilityId && !stopped)
          void peerApi("/api/peer/availability/heartbeat", {}).catch(fail);
      }, 15000);
    },
    async stop({ persisted = false } = {}) {
      stopped = true;
      clearInterval(timer);
      clearInterval(heartbeat);
      if (availabilityId && !persisted) {
        void fetch("/api/peer/availability", {
          method: "DELETE",
          credentials: "same-origin",
          keepalive: true,
          headers: { "content-type": "application/json" },
          body: "{}",
        }).catch(() => {});
      }
      await disconnect();
    },
  };
}
