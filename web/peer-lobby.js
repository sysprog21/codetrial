import { mountPeerPresence } from "./peer-presence.js";
import {
  INVITE_STORAGE_KEY,
  peerApi,
  normalizePeerNickname,
} from "./peer-lib.js";
import { readStored, storageArea, writeStored } from "./audio-output.js";
const byId = (id) => document.getElementById(id);
const status = byId("peer-status");
let state = null;
let polling = false;
let busy = false;
let timer;
let presence;
let audioAvailable = false;
let signedInAccount;
let nickname;
function updateAvailability(active) {
  for (const control of byId("peer-create").elements) control.disabled = active;
  byId("peer-available").disabled = active || !byId("peer-opt-in").checked;
  byId("peer-save-audio").disabled = active || !audioAvailable;
}
// Read once, through `storageArea`: at module scope a blocked `sessionStorage`
// would otherwise stop the whole page from loading.
const tabStorage = storageArea("sessionStorage");
const tabStorageBlocked = !tabStorage;
// A full site-data block refuses the session cookie too, so there is no join
// to protect by keeping an invite token in the address bar or across sign-in.
const SITE_DATA_NEEDED =
  "Peer practice needs site data, which this browser is blocking for this site. Allow site data for this site, then reload this page.";
const SITE_DATA_NEEDED_INVITE =
  "Peer practice needs site data, which this browser is blocking for this site. Allow site data for this site, then open the invite link again.";
const requested = new URLSearchParams(location.search).get("session");
const fragment = new URLSearchParams(location.hash.slice(1));
// Always clear the invite hash. The write is made and read back here rather
// than through writeStored, which swallows a refusal, so a refused write is
// known and the page asks for the invite link again instead of running init.
let inviteUnstored = false;
if (fragment.has("invite")) {
  const token = fragment.get("invite");
  let stored = false;
  if (tabStorage) {
    try {
      tabStorage.setItem(INVITE_STORAGE_KEY, token);
      stored = tabStorage.getItem(INVITE_STORAGE_KEY) === token;
    } catch {
      // Getter answered; the write did not.
    }
  }
  if (!stored) inviteUnstored = true;
  history.replaceState(null, "", location.pathname + location.search);
}
const base = () => `/api/peer/sessions/${encodeURIComponent(state.sessionId)}`;
const STATUS_TEXT = {
  pending: "Waiting for you both to enter and get ready.",
  lobby: "Waiting for you both to enter and get ready.",
  active: "Practice in progress.",
  finalizing: "Finishing up.",
  complete: "Practice complete.",
  cancelled: "This practice was cancelled.",
};
function render(next) {
  state = next;
  const appearing = byId("peer-session").hidden;
  byId("peer-session").hidden = false;
  byId("peer-members").textContent =
    next.members
      .map(
        (member) =>
          `${member.nickname ? `${member.nickname} (@${member.login})` : member.login}${member.entered ? " · entered" : ""}${member.ready ? " · ready" : ""}`,
      )
      .join(" and ") || "Waiting for your partner";
  byId("peer-session-status").textContent =
    STATUS_TEXT[next.status] ?? next.status;
  const mine = next.members.find((member) => member.slot === next.you.slot);
  byId("peer-enter").disabled = Boolean(mine?.entered);
  byId("peer-leave").disabled = !mine?.entered;
  byId("peer-ready").hidden = true;
  byId("peer-start").hidden = true;
  byId("peer-start").disabled = true;
  byId("peer-open-interview").hidden = next.members.length !== 2;
  byId("peer-open-interview").href =
    `/peer-interview?session=${encodeURIComponent(next.sessionId)}`;
  byId("peer-open-interview").textContent = next.currentRound
    ? "Open interview"
    : "Connect audio and get ready";
  byId("peer-invite").hidden =
    next.you.slot !== 0 ||
    next.members.length !== 1 ||
    next.status !== "pending";
  byId("peer-copy").disabled = !byId("peer-invite-link").value;
  if (next.inviteToken) showInvite(next.inviteToken);
  // A pairing that completes while the person is reading the list is announced
  // and brought into view, rather than appearing silently at the bottom.
  if (appearing) {
    const title = byId("session-title");
    title.tabIndex = -1;
    title.focus();
  }
}
function showInvite(token) {
  byId("peer-invite").hidden = false;
  byId("peer-copy").disabled = false;
  byId("peer-invite-link").value =
    `${location.origin}/peer-lobby#invite=${encodeURIComponent(token)}`;
}
async function refresh() {
  if (!state || polling || busy) return;
  polling = true;
  try {
    render(await peerApi(base()));
  } catch (error) {
    status.textContent = error.message;
  } finally {
    polling = false;
  }
}
async function act(action) {
  if (busy) return;
  busy = true;
  try {
    await action();
    status.textContent = "Session updated.";
  } catch (error) {
    status.textContent = error.message;
  } finally {
    busy = false;
  }
}
for (const action of ["enter", "leave", "cancel"]) {
  byId(`peer-${action}`).addEventListener("click", () =>
    act(async () => {
      const answer = await peerApi(`${base()}/${action}`, {
        expectedVersion: state.version,
        ...(action === "ready" ? { ready: true } : {}),
      });
      render(answer.sessionId ? answer : await peerApi(base()));
    }),
  );
}
byId("peer-create").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const available = event.submitter?.value === "available";
  void act(async () => {
    const data = Object.fromEntries(new FormData(form));
    data.saveTranscript = byId("peer-save-transcript").checked;
    data.saveAudio = audioAvailable && byId("peer-save-audio").checked;
    data.roundMinutes = Number(data.roundMinutes);
    data.interviewerFirst = Number(data.interviewerFirst);
    if (available) {
      await presence.enter(data);
      return;
    }
    render(await peerApi("/api/peer/sessions", data));
    history.replaceState(
      null,
      "",
      `/peer-lobby?session=${encodeURIComponent(state.sessionId)}`,
    );
    await listSessions();
  });
});
byId("peer-copy").addEventListener("click", () =>
  act(async () => {
    await navigator.clipboard.writeText(byId("peer-invite-link").value);
  }),
);
byId("peer-regenerate").addEventListener("click", () =>
  act(async () => {
    const answer = await peerApi(`${base()}/invite`, {
      expectedVersion: state.version,
    });
    render(answer);
  }),
);
byId("peer-revoke").addEventListener("click", () =>
  act(async () => {
    const answer = await peerApi(
      `${base()}/invite`,
      { expectedVersion: state.version },
      "DELETE",
    );
    byId("peer-invite-link").value = "";
    render(answer);
  }),
);
async function listSessions() {
  const answer = await peerApi("/api/peer/sessions");
  const list = byId("peer-session-list");
  list.replaceChildren();
  for (const session of answer.sessions ?? answer) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = `/peer-lobby?session=${encodeURIComponent(session.sessionId)}`;
    link.textContent = `${session.status} · ${session.members?.map((member) => (member.nickname ? `${member.nickname} (@${member.login})` : member.login)).join(" and ") ?? session.sessionId}`;
    item.append(link);
    list.append(item);
  }
  if (!list.children.length)
    list.textContent = "No sessions yet. Become available or join a partner.";
}
async function init() {
  const account = await peerApi("/api/session");
  if (!account.signedIn) {
    byId("peer-signin").hidden = false;
    if (readStored(INVITE_STORAGE_KEY, tabStorage))
      location.assign("/api/login?next=peer-join");
    status.textContent = "Sign in with GitHub to practice with a partner.";
    return;
  }
  if (!account.peerEnabled) {
    status.textContent = "Peer practice is not enabled on this server.";
    return;
  }
  if (!(account.identityVerified ?? account.user?.identityVerified)) {
    byId("peer-signin").hidden = false;
    status.textContent =
      "Verify your identity by signing in with GitHub OAuth.";
    return;
  }
  signedInAccount = account;
  if (account.peerMaxRoundMinutes)
    document.querySelector('[name="roundMinutes"]').max =
      account.peerMaxRoundMinutes;
  const profile = await peerApi("/api/peer/profile");
  if (!normalizePeerNickname(profile.nickname)) {
    byId("peer-profile").hidden = false;
    byId("peer-nickname").focus();
    status.textContent = `Signed in as ${account.user.login}`;
    return;
  }
  nickname = profile.nickname;
  await enterReciprocalMode(account);
}
async function enterReciprocalMode(account) {
  byId("peer-profile").hidden = true;
  audioAvailable = account.peerAudioAvailable === true;
  byId("peer-save-audio").disabled = !audioAvailable;
  byId("peer-audio-availability").textContent = audioAvailable
    ? "Audio is saved only after both partners explicitly agree."
    : "Audio saving is unavailable on this server.";
  byId("peer-content").hidden = false;
  presence = mountPeerPresence({
    nodes: {
      optIn: byId("peer-opt-in"),
      roster: byId("peer-available-list"),
      requests: byId("peer-requests"),
      status: byId("peer-presence-status"),
      login: account.user.login,
    },
    sdk: window.LivekitClient,
    onAvailabilityChanged: updateAvailability,
    async onMatch(sessionId) {
      render(
        await peerApi(`/api/peer/sessions/${encodeURIComponent(sessionId)}`),
      );
      history.replaceState(
        null,
        "",
        `/peer-lobby?session=${encodeURIComponent(sessionId)}`,
      );
      await listSessions();
    },
  });
  presence.start();
  status.textContent = `${nickname} · GitHub @${account.user.login}`;
  // Polling starts first, so one stale link or failed list read does not leave
  // the page without updates.
  clearInterval(timer);
  timer = setInterval(() => void refresh(), 5000);
  const invite = readStored(INVITE_STORAGE_KEY, tabStorage);
  try {
    if (invite) {
      try {
        render(await peerApi("/api/peer/invites/accept", { token: invite }));
      } catch (error) {
        // A rejected invitation never becomes valid, so a reload must not retry it.
        if (error.code === "invite_unavailable")
          writeStored(INVITE_STORAGE_KEY, "", tabStorage);
        throw error;
      }
      writeStored(INVITE_STORAGE_KEY, "", tabStorage);
      history.replaceState(
        null,
        "",
        `/peer-lobby?session=${encodeURIComponent(state.sessionId)}`,
      );
    } else if (requested)
      render(
        await peerApi(`/api/peer/sessions/${encodeURIComponent(requested)}`),
      );
  } catch (error) {
    status.textContent = error.message;
  }
  await listSessions().catch((error) => {
    status.textContent = error.message;
  });
}
byId("peer-profile-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const chosen = normalizePeerNickname(byId("peer-nickname").value);
  if (!chosen) {
    byId("peer-nickname-error").textContent =
      "Use 1 to 24 English letters or digits, including at least one letter.";
    byId("peer-nickname").focus();
    return;
  }
  byId("peer-profile-save").disabled = true;
  void (async () => {
    const profile = await peerApi(
      "/api/peer/profile",
      { nickname: chosen },
      "PUT",
    );
    nickname = profile.nickname;
    await enterReciprocalMode(signedInAccount);
  })()
    .catch((error) => {
      byId("peer-nickname-error").textContent = error.message;
      status.textContent = error.message;
    })
    .finally(() => {
      byId("peer-profile-save").disabled = false;
    });
});
window.addEventListener("pagehide", (event) => {
  clearInterval(timer);
  void presence?.stop({ persisted: event.persisted });
});
window.addEventListener("pageshow", (event) => {
  if (!event.persisted || !presence) return;
  presence.start();
  void refresh();
  void listSessions().catch((error) => {
    status.textContent = error.message;
  });
  clearInterval(timer);
  timer = setInterval(() => void refresh(), 5000);
});
if (tabStorageBlocked || inviteUnstored) {
  status.textContent = inviteUnstored
    ? SITE_DATA_NEEDED_INVITE
    : SITE_DATA_NEEDED;
} else {
  void init().catch((error) => {
    status.textContent = error.message;
  });
}
