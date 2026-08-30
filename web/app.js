import { readLocalHistory } from "./history.js";

let problem = "two-sum";
let duration = 45;

const nodes = {
  accountStatus: document.querySelector("#account-status"),
  githubLogin: document.querySelector("#github-login"),
  loginLink: document.querySelector("#login-link"),
  logout: document.querySelector("#logout"),
  history: document.querySelector("#history"),
};

for (const button of document.querySelectorAll("[data-problem]")) {
  button.addEventListener("click", () => {
    problem = button.dataset.problem;
    select("[data-problem]", button);
  });
}

for (const button of document.querySelectorAll("[data-duration]")) {
  button.addEventListener("click", () => {
    duration = Number(button.dataset.duration);
    select("[data-duration]", button);
  });
}

const start = document.querySelector("#start");

let signInFirst = false;

// The button by name, not by asking the event which element it was dispatched
// to: the browser clears that property when dispatch ends, so every line after
// the first await was writing to null. A gated candidate signed in, the handler
// threw on the next line, and the button stayed disabled on "Recording
// GitHub..." with the interview never starting.
start.addEventListener("click", async () => {
  start.disabled = true;
  if (signInFirst) {
    start.textContent = "Recording GitHub...";
    if (!(await recordGitHubLogin(false))) {
      start.disabled = false;
      setStartGate(true);
      return;
    }
  }
  start.textContent = "Starting...";
  window.location.href = `/interview?problem=${encodeURIComponent(problem)}&duration=${duration}`;
});

// Returning from the media preflight can restore this page from the browser's
// back/forward cache after the start button was deliberately disabled.
window.addEventListener("pageshow", (event) => {
  // Only the restore. A normal load fires this too, after `load`, which is
  // late enough to re-enable a button the candidate already pressed and hand
  // them a second navigation; and it would run before `loadAccount` has
  // answered, so the gate it restores is the placeholder rather than the
  // session's.
  if (!event.persisted) return;
  start.disabled = false;
  setStartGate(signInFirst);
});

nodes.loginLink.addEventListener("click", async () => {
  nodes.loginLink.disabled = true;
  await recordGitHubLogin(true);
  nodes.loginLink.disabled = false;
});

/// The server refuses to mint a token without a session, so this only saves the
/// candidate a round trip into a room they cannot join. The token endpoint is
/// the real gate; this only points the button at GitHub sooner.
function setStartGate(blocked) {
  signInFirst = blocked;
  start.textContent = blocked ? "Use GitHub to start" : "Start interview";
}

nodes.logout.addEventListener("click", async () => {
  nodes.logout.disabled = true;
  try {
    await fetch("/api/logout", { method: "POST" });
  } finally {
    window.location.reload();
  }
});

loadAccount();

async function loadAccount() {
  try {
    const session = await fetchJson("/api/session");
    if (session.signedIn) {
      nodes.accountStatus.textContent = `Signed in as ${session.user.login}`;
      nodes.githubLogin.hidden = true;
      nodes.loginLink.hidden = true;
      nodes.logout.hidden = false;
      setStartGate(false);
      await renderServerHistory();
      return;
    }
    if (session.loginRequired) {
      nodes.accountStatus.textContent = "Enter your GitHub username to start.";
      nodes.githubLogin.hidden = false;
      nodes.loginLink.hidden = false;
      nodes.logout.hidden = true;
      setStartGate(true);
      renderLocalHistory();
      return;
    }
  } catch {
    // A server that cannot answer about accounts is not one that will mint a
    // token either, but the editor still works offline, so do not lock the page.
  }
  nodes.accountStatus.textContent = "Signed out";
  nodes.githubLogin.hidden = false;
  nodes.loginLink.hidden = false;
  nodes.logout.hidden = true;
  setStartGate(false);
  renderLocalHistory();
}

async function recordGitHubLogin(reload) {
  const login = nodes.githubLogin.value.trim().replace(/^@+/, "");
  if (!/^[a-z\d](?:[a-z\d-]{0,37}[a-z\d])?$/i.test(login)) {
    nodes.accountStatus.textContent = "Enter a valid GitHub username.";
    nodes.githubLogin.focus();
    return false;
  }
  try {
    const response = await fetch("/api/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ login }),
    });
    if (!response.ok) {
      // The server already said what is wrong (bad handle, rate limited, not
      // configured); replacing that with a generic line throws it away.
      const body = await response.json().catch(() => null);
      nodes.accountStatus.textContent = body?.error || "Could not record GitHub username.";
      return false;
    }
    if (reload) window.location.reload();
    return true;
  } catch {
    nodes.accountStatus.textContent = "Could not record GitHub username.";
    return false;
  }
}

async function renderServerHistory() {
  try {
    const data = await fetchJson("/api/reports");
    renderHistoryCount(data.reports.length, "saved to your account");
  } catch {
    nodes.history.hidden = true;
  }
}

function renderLocalHistory() {
  try {
    renderHistoryCount(readLocalHistory().length, "saved on this device");
  } catch {
    nodes.history.hidden = true;
  }
}

function renderHistoryCount(count, suffix) {
  if (count <= 0) {
    nodes.history.hidden = true;
    return;
  }
  nodes.history.hidden = false;
  nodes.history.textContent = `${count} past report${count === 1 ? "" : "s"} ${suffix}.`;
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} returned ${response.status}`);
  return response.json();
}

function select(selector, selected) {
  for (const button of document.querySelectorAll(selector)) {
    button.classList.toggle("selected", button === selected);
  }
}
