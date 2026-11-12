export const PEER_PREVIEW_TOPIC = "peer.workspace.preview.v1";
export const INVITE_STORAGE_KEY = "codetrial:peerInvite";

export function peerRole(state) {
  const role = state?.currentRound?.role;
  return role === "candidate" || role === "interviewer" ? role : null;
}

// `seq` orders previews that share a revision: the transport is unreliable and
// unordered, so a receiver drops any packet not newer than the last it applied.
export function previewPayload({ round, revision, seq, language, code }) {
  const packet = { kind: "preview", round, revision, seq, language, code };
  return new TextEncoder().encode(JSON.stringify(packet)).length > 12 * 1024
    ? { kind: "revision", round, revision, seq }
    : packet;
}

export function acceptsPreview(state, participant, packet) {
  if (
    !state?.currentRound ||
    state.currentRound.status !== "active" ||
    packet?.round !== state.currentRound.index
  )
    return false;
  if (
    !["preview", "revision"].includes(packet.kind) ||
    !Number.isSafeInteger(packet.revision) ||
    !Number.isSafeInteger(packet.seq)
  )
    return false;
  const candidateSlot =
    peerRole(state) === "candidate" ? state.you.slot : 1 - state.you.slot;
  const candidate = state.members.find(
    (member) => member.slot === candidateSlot,
  );
  return Boolean(candidate && participant?.identity === candidate.identity);
}

// What a person should read for the codes the server sends. A code missing here
// is shown as it arrives, which is still better than nothing.
const PEER_ERRORS = {
  invite_unavailable:
    "This invitation is unavailable. Ask your partner for a new link.",
  version_conflict: "Your partner just changed something. Showing the latest.",
  invalid_nickname:
    "A nickname is letters, with digits allowed after them, for example Lucky7.",
  nickname_required: "Choose a nickname before you start.",
  waiting_for_partner: "Waiting for your partner to join and be ready.",
  observer_consent_required:
    "Both of you must agree to the silent observer before starting.",
  observer_unavailable: "The silent observer is not available right now.",
  observer_at_capacity: "The silent observer is busy. Try again in a moment.",
  at_capacity: "All practice rooms are in use. Try again in a few minutes.",
  too_many_invitations:
    "You have too many invitations waiting. Cancel one to create another.",
  rate_limited: "Too many requests. Wait a moment and try again.",
  request_limit: "Too many requests. Wait a moment and try again.",
  feedback_required: "Submit your feedback before moving on.",
  lease_lost:
    "Edit access moved to another tab or expired. Use Recover access to continue.",
  round_sealed: "This round has ended.",
  round_unavailable: "This round is not available.",
  not_writer: "Only the candidate can edit the code.",
  session_closed: "This practice has ended.",
  not_found: "That is no longer available.",
  provider_unavailable: "Voice is unavailable right now. Try again shortly.",
  invalid_snapshot: "The code is too large to save. Shorten it and try again.",
  invalid_feedback: "A feedback answer is too long. Shorten it and try again.",
  body_too_large: "That is too large to send.",
  database_unavailable: "The service is busy. Try again in a moment.",
};

export async function peerApi(path, body, method = "POST") {
  const response = await fetch(path, {
    credentials: "same-origin",
    method: body === undefined ? "GET" : method,
    headers: {
      accept: "application/json",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(
      PEER_ERRORS[payload.code] ??
        payload.error ??
        payload.code ??
        `Request failed (${response.status})`,
    );
    error.code = payload.code;
    error.status = response.status;
    throw error;
  }
  return payload;
}

export function normalizePeerNickname(value) {
  const nickname = typeof value === "string" ? value.trim() : "";
  return /^(?=.*[A-Za-z])[A-Za-z0-9]{1,24}$/.test(nickname) ? nickname : null;
}
