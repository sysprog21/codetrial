import { readLocalHistory } from "./history.js";
import { pickProblem, suggestDifficulty } from "./problem-picker.js";
import { buildProgressModel, progressPhases } from "./progress.js";

let problem;
let duration;
let mode = "scored";
let reports = [];
let manualProblem = false;
let manualDuration = false;
let manualDifficulty = false;
let historyReady = false;
/// The longest interview this deployment can record, which only the server
/// knows. `Infinity` until `/api/session` answers: the duration row is live
/// before that lands, and `/api/token` clamps anything that gets out in the
/// meantime, so the gap costs a shortened interview rather than a lost
/// recording.
///
/// Declared with the other module state rather than beside the functions that
/// read it. The setup below calls `setDuration` while a `let` further down the
/// file is still in its temporal dead zone, and reading one there throws
/// before the lobby has rendered anything at all.
let durationCeiling = Infinity;

// The roll a recommendation is drawn with. It changes when the candidate asks
// for something different and at no other time, so recommending twice for one
// set of choices lands on the same problem. That is what lets the back/forward
// restore re-run the recommendation without the card moving under whoever is
// reading it.
let roll = Math.random();

const nodes = {
  accountStatus: document.querySelector("#account-status"),
  githubLogin: document.querySelector("#github-login"),
  loginLink: document.querySelector("#login-link"),
  logout: document.querySelector("#logout"),
  history: document.querySelector("#history"),
  recommendation: document.querySelector("#recommendation"),
  durationNote: document.querySelector("#duration-note"),
  progressSummary: document.querySelector("#progress-summary"),
  progressTrends: document.querySelector("#progress-trends"),
  progressWeaknesses: document.querySelector("#progress-weaknesses"),
  progressDifficulty: document.querySelector("#progress-difficulty"),
  progressLanguage: document.querySelector("#progress-language"),
  progressDuration: document.querySelector("#progress-duration"),
  progressMode: document.querySelector("#progress-mode"),
  profileRole: document.querySelector("#profile-role"),
  profileSeniority: document.querySelector("#profile-seniority"),
  profileCompany: document.querySelector("#profile-company"),
};

// Every card carries the pressed state from the start, not only the one that
// is on: one pressed button among 149 plain ones does not read as a choice.
const cards = [...document.querySelectorAll("[data-problem]")].map((button) => ({
  id: button.dataset.problem,
  difficulty: button.dataset.difficulty,
  button,
}));
for (const card of cards) mark(card.button, false);
const levels = [...document.querySelectorAll('[name="difficulty"]')];

// A picked card is a choice about this one interview, not about the filter the
// checkboxes carry, so it leaves them alone. It suggests a length to go with
// the level it belongs to, which `setDuration` applies only if the candidate
// has not already chosen one.
for (const card of cards) {
  card.button.addEventListener("click", () => {
    manualProblem = true;
    setProblem(card);
    setDuration(suggestedDuration(new Set([card.difficulty])));
    nodes.recommendation.textContent = `Selected: ${title(card)}.`;
  });
}

for (const input of levels) {
  input.addEventListener("change", () => {
    // Unchecking the last level is not a change: the box goes straight back,
    // so nothing downstream may move either, the recommendation included.
    if (!selectedDifficulties().size) {
      input.checked = true;
      return;
    }
    manualDifficulty = true;
    applyDifficulties();
    // A pick made by hand survives a filter that still includes it. Only one
    // that now hides it hands the choice back to the lobby.
    if (manualProblem && selectedDifficulties().has(problem?.difficulty)) return;
    manualProblem = false;
    roll = Math.random();
    // Not before the reports are in. Recommending from an empty history here
    // would offer a problem the candidate has already passed and then swap it
    // when the fetch lands. `settle` makes the pick for this level instead, and
    // until it does there is nothing to start: whatever was picked belongs to
    // the levels this change just replaced.
    if (historyReady) {
      recommend();
    } else {
      setProblem(null);
      nodes.recommendation.textContent = "";
    }
  });
}

let progressEntries = [];
let progressSuffix = "saved";

for (const filter of [nodes.progressDifficulty, nodes.progressLanguage, nodes.progressDuration, nodes.progressMode]) {
  filter.addEventListener("change", renderProgress);
}

const durations = [...document.querySelectorAll("[data-duration]")];
for (const button of durations) {
  button.addEventListener("click", () => setDuration(Number(button.dataset.duration), true));
}

for (const button of document.querySelectorAll("[data-mode]")) {
  button.addEventListener("click", () => {
    mode = button.dataset.mode === "practice" ? "practice" : "scored";
    select("[data-mode]", button);
  });
}

const start = document.querySelector("#start");

let signInFirst = false;

/// True for as long as a start is on its way out. The sign-in round trip is the
/// one window where this handler is suspended with the page still live under
/// it, and everything the candidate can touch during it writes the button's
/// disabled state. Without this a card click re-armed a button that was already
/// leaving and bought a second navigation out of one start.
let starting = false;

// The button by name, not by asking the event which element it was dispatched
// to: the browser clears that property when dispatch ends, so every line after
// the first `await` was writing to null. A gated candidate signed in, the
// handler threw on the next line, and the button stayed disabled on "Recording
// GitHub..." with the interview never starting.
start.addEventListener("click", async () => {
  // Both of them read before the round trip below, because everything the
  // candidate can still touch during it writes one of the two. Taking the
  // problem and leaving the length behind shipped an interview that was 45
  // minutes when the button was pressed and 60 by the time it was answered.
  // The mode is on the same round trip and reads the same way.
  const chosen = problem;
  const minutes = duration;
  const chosenMode = mode;
  if (!chosen) return;
  starting = true;
  start.disabled = true;
  if (signInFirst) {
    start.textContent = "Recording GitHub...";
    if (!(await recordGitHubLogin(false))) {
      starting = false;
      // Not `false`: the same round trip can leave nothing selected, and a
      // button armed over nothing is one whose handler returns above.
      start.disabled = !problem;
      setStartGate(true);
      return;
    }
  }
  start.textContent = "Starting...";
  const destination = new URL("/interview", window.location.origin);
  destination.searchParams.set("problem", chosen.id);
  destination.searchParams.set("duration", String(minutes));
  destination.searchParams.set("mode", chosenMode);
  const profile = {
    role: nodes.profileRole.value.trim(),
    seniority: nodes.profileSeniority.value,
    targetCompany: nodes.profileCompany.value.trim(),
  };
  if (profile.role) destination.searchParams.set("role", profile.role);
  if (profile.seniority) destination.searchParams.set("seniority", profile.seniority);
  if (profile.targetCompany) destination.searchParams.set("company", profile.targetCompany);
  window.location.href = destination.toString();
});

// Returning from the media preflight can restore this page from the browser's
// back/forward cache after the start button was deliberately disabled.
window.addEventListener("pageshow", (event) => {
  // Only the restore. A normal load fires this too, after `load`, which is
  // late enough to re-enable a button the candidate already pressed and hand
  // them a second navigation.
  if (!event.persisted) return;
  // The cache holds the page as it was before the candidate left, and a login
  // recorded on the way out is in none of it, so restoring what it holds told
  // somebody who had just signed in to sign in. Ask the server instead, and
  // treat the reports in hand as in flight again while it answers: the button
  // stays down and a difficulty change waits, exactly as on a first load.
  // `starting` with it: the page came back, so whatever start was on its way
  // out did not happen, and a latch left set here disables the button for good.
  historyReady = false;
  starting = false;
  start.disabled = true;
  loadAccount().finally(settle);
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

applyDifficulties();
// Nothing is recommended before the reports arrive, because they choose the
// level as well as the problem. The button ships disabled and `setProblem` is
// what enables it, so the gap is a button that cannot be pressed rather than
// one that starts an interview on nothing. `finally` rather than `then` for the
// same reason: a rejection in `loadAccount` would otherwise leave it disabled
// for good.
loadAccount().finally(settle);

/// What the lobby settles into once it knows the candidate's history: the level
/// that history points at, a problem at that level, and a start button.
///
/// Shared with the back/forward restore above rather than written twice. That
/// path refetches the reports, so leaving it to only re-enable the button meant
/// the checked level and the sentence explaining it went on describing history
/// the page had already replaced.
function settle() {
  historyReady = true;
  // The level suggestion only applies when the candidate has not already said
  // what they want. Moving their checkboxes would also hide the card they just
  // picked.
  const note = manualDifficulty || manualProblem ? "" : applySuggestedLevel();
  recommend(note);
  // `recommend` returns without touching anything when the candidate's own pick
  // still stands, so the button the restore below held down needs releasing
  // here rather than there.
  start.disabled = !problem || starting;
}

async function loadAccount() {
  try {
    const session = await fetchJson("/api/session");
    applyDurationCeiling(session.maxDurationMin);
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
    // `/api/reports` wraps each entry in `payload` (accounts.rs list_reports);
    // history.js stores the same entry flat. Flattened here so the picker knows
    // one shape instead of guessing between two. The progress panel takes the
    // wire shape as it comes: progress.js unwraps `payload` itself.
    reports = data.reports.map((entry) => ({ problemId: entry.problemId, report: entry.payload?.report }));
    showProgress(data.reports, "saved to your account");
  } catch {
    showProgressError("Could not load saved account progress.");
  }
}

function renderLocalHistory() {
  try {
    reports = readLocalHistory();
    showProgress(reports, "saved on this device");
  } catch {
    showProgressError("Could not load progress saved on this device.");
  }
}

function selectedDifficulties() {
  return new Set(levels.filter((input) => input.checked).map((input) => input.value));
}

/// The checkboxes moved, so everything read off them moves too: which cards the
/// picker offers, and the length the level implies. One place, because three
/// copies of these two lines is three places to forget one.
function applyDifficulties() {
  const difficulties = selectedDifficulties();
  // The picker is a shortcut to one problem, not a second copy of the wall the
  // checkboxes just hid, so it shows what the checkboxes selected.
  for (const card of cards) card.button.hidden = !difficulties.has(card.difficulty);
  setDuration(suggestedDuration(difficulties));
}

/// `note` is the sentence explaining a level the reports chose, empty when the
/// candidate chose it themselves and so already knows.
function recommend(note = "") {
  // A candidate who picked a card has answered the question this line asks, so
  // it stays answered. Naming a different problem here contradicted the card
  // they had just selected.
  if (manualProblem) return;
  const choice = pickProblem(cards, selectedDifficulties(), reports, () => roll);
  // Nothing to offer is still an answer, and it has to go through `setProblem`
  // like every other one. Returning here left whatever was picked for the
  // levels this call just replaced sitting selected behind a live button, on a
  // card `applyDifficulties` had already hidden.
  if (!choice) {
    setProblem(null);
    nodes.recommendation.textContent = "";
    return;
  }
  setProblem(choice.picked);
  nodes.recommendation.textContent = choice.repeat
    ? `${note}You have passed every problem at this level. Recommended again: ${title(choice.picked)}.`
    : `${note}Recommended: ${title(choice.picked)}.`;
}

/// Check the level the candidate's own results point at and return the sentence
/// saying why. Empty when the reports have no opinion yet, which leaves the
/// markup's default standing.
function applySuggestedLevel() {
  const suggestion = suggestDifficulty(cards, reports);
  if (!suggestion) return "";
  for (const input of levels) input.checked = input.value === suggestion.difficulty;
  applyDifficulties();
  // Named after the level just finished, not the one being suggested: those
  // differ whenever the streak actually moves the candidate.
  return suggestion.reason === "passed"
    ? `You passed your last two ${suggestion.from} problems. `
    : `Your last two ${suggestion.from} problems did not land. `;
}

/// The only writer of `problem` and of the start button's disabled state, so
/// the two cannot drift apart. `null` is the case where what was on screen
/// stopped being something the candidate could have meant: picking a card by
/// hand and then dropping its difficulty left it selected but hidden, with the
/// button still carrying it into an interview on a level they had just cleared.
///
/// The button is enabled here and nowhere earlier because the recommendation
/// waits on the report history, and a click during that wait used to build
/// `?problem=undefined`. Callers own the sentence beside it: this only knows
/// which problem, never why.
function setProblem(card) {
  // Two buttons, not a sweep of all 150. `problem` still holds the outgoing
  // card here, which is what makes that possible.
  mark(problem?.button, false);
  mark(card?.button, true);
  problem = card;
  start.disabled = !card || starting;
}

/// Says whether a button is the chosen one, in both the ways that answer has to
/// be given. Without `aria-pressed` the choice is a border colour, which a
/// screen reader does not read out, and the two had already drifted once: the
/// class was set in three places and the attribute in two.
///
/// A button rather than a card, because the duration row is the same question
/// about buttons that carry no card, and wrapping each of those in a throwaway
/// `{ button }` to get in here was the object existing for the parameter.
function mark(button, on) {
  if (!button) return;
  button.classList.toggle("selected", on);
  button.setAttribute("aria-pressed", String(on));
}

/// Capped at the server's own default length, which src/config.rs holds at or
/// under the recording cap, so nothing the lobby picks on its own outlives its
/// recording. Only the suggestion is capped: the sixty minute button is one
/// click away and a deployment can set the two environment variables apart, so
/// this narrows the default rather than guaranteeing anything.
function suggestedDuration(difficulties) {
  return difficulties.has("Medium") || difficulties.has("Hard") ? 45 : 30;
}

/// A suggestion unless `chosen` says the candidate picked it out loud, and once
/// they have, nothing suggests over it again. The latch never releases: someone
/// who asks for sixty minutes keeps it even if their history later moves them
/// down to Easy.
///
/// The ceiling is applied here rather than at each caller, so a length can no
/// more get past it by being suggested than by being clicked.
function setDuration(minutes, chosen = false) {
  if (manualDuration && !chosen) return;
  manualDuration ||= chosen;
  duration = underCeiling(minutes);
  select("[data-duration]", document.querySelector(`[data-duration="${duration}"]`));
}

/// Snapped to a length the row actually offers, not to the cap itself: a cap
/// of forty would otherwise leave `duration` at forty with no button to show
/// for it, and the candidate reading a row where nothing is selected.
function underCeiling(minutes) {
  if (minutes <= durationCeiling) return minutes;
  const offered = durations
    .map((button) => Number(button.dataset.duration))
    .filter((value) => value <= durationCeiling);
  return offered.length ? Math.max(...offered) : minutes;
}

/// What the server said it can record, turned into a row that says so. The
/// button is disabled rather than removed: a length that quietly stops being
/// on offer is the same silence as one that quietly gets shortened, and this
/// one has a reason worth reading.
function applyDurationCeiling(minutes) {
  if (!Number.isFinite(minutes)) return;
  durationCeiling = minutes;
  const over = durations.filter((button) => Number(button.dataset.duration) > minutes);
  // A cap under every length on offer is a misconfigured deployment, not a
  // lobby with nothing to press. Leave the row alone and let the server's own
  // floor decide, rather than handing back a page that cannot start anything.
  const capped = over.length < durations.length ? over : [];
  for (const button of durations) button.disabled = capped.includes(button);
  nodes.durationNote.textContent = capped.length
    ? `Interviews here are recorded for at most ${minutes} minutes, so longer ones are not offered.`
    : "";
  nodes.durationNote.hidden = !capped.length;
  // The cap outranks a length the candidate chose out loud, which nothing else
  // here does: it is not a second opinion about what suits them, it is what
  // this server can record.
  if (duration > durationCeiling) {
    duration = underCeiling(duration);
    select("[data-duration]", document.querySelector(`[data-duration="${duration}"]`));
  }
}

function title(card) {
  return card.button.querySelector(".problem-title").textContent;
}

function showProgressError(message) {
  nodes.history.hidden = false;
  nodes.progressSummary.textContent = message;
  nodes.progressTrends.replaceChildren();
  nodes.progressWeaknesses.replaceChildren();
}

function showProgress(entries, suffix) {
  progressEntries = entries;
  progressSuffix = suffix;
  nodes.history.hidden = false;
  const model = buildProgressModel(progressEntries);
  syncFilter(nodes.progressDifficulty, model.options.difficulty, (value) => value);
  syncFilter(nodes.progressLanguage, model.options.language, languageLabel);
  syncFilter(nodes.progressDuration, model.options.durationMin, (value) => `${value} min`);
  syncFilter(nodes.progressMode, model.options.mode, titleCase);
  renderProgress();
}

function renderProgress() {
  const filters = {
    difficulty: nodes.progressDifficulty.value,
    language: nodes.progressLanguage.value,
    durationMin: nodes.progressDuration.value,
    mode: nodes.progressMode.value,
  };
  const model = buildProgressModel(progressEntries, filters);
  nodes.progressTrends.replaceChildren();
  nodes.progressWeaknesses.replaceChildren();
  if (model.total === 0) {
    nodes.progressSummary.textContent = `No past reports are ${progressSuffix}. Complete an interview to start a trend.`;
    return;
  }
  if (model.attempts.length === 0) {
    nodes.progressSummary.textContent = `No saved attempts match these filters. ${model.total} remain ${progressSuffix}.`;
    return;
  }
  const assessed = model.attempts.filter((attempt) => attempt.report.frameworkAssessment).length;
  nodes.progressSummary.textContent = assessed === 0
    ? `${model.attempts.length} of ${model.total} attempts shown · these legacy or unassessed reports have no versioned phase scores, so no zeroes are plotted · ${progressSuffix}.`
    : `${model.attempts.length} of ${model.total} attempts shown · ${assessed} have comparable phase scores · ${progressSuffix}.`;
  const table = document.createElement("table");
  const caption = document.createElement("caption");
  caption.textContent = "REACTO and STAR phase trends; rubric versions are separate series";
  table.append(caption);
  const head = table.createTHead().insertRow();
  for (const label of ["Phase", "Assessed scores by rubric version"]) {
    const cell = document.createElement("th");
    cell.scope = "col";
    cell.textContent = label;
    head.append(cell);
  }
  const body = table.createTBody();
  for (const phase of progressPhases) {
    const row = body.insertRow();
    const heading = document.createElement("th");
    heading.scope = "row";
    heading.textContent = phase;
    row.append(heading);
    const trend = row.insertCell();
    const segments = model.series[phase];
    trend.textContent = segments.length
      ? segments.map((segment) => `Rubric v${segment.rubricVersion}: ${segment.points.map((point) => `attempt ${point.attemptIndex + 1}: ${point.score}`).join(" → ")}`).join(" | ")
      : "Not assessed in these attempts";
  }
  nodes.progressTrends.append(table);
  if (model.weaknesses.length === 0) {
    const item = document.createElement("li");
    item.textContent = "No grounded weakness tags in these attempts.";
    nodes.progressWeaknesses.append(item);
  } else {
    for (const weakness of model.weaknesses) {
      const item = document.createElement("li");
      item.textContent = `${weakness.tag} · ${weakness.count} attempt${weakness.count === 1 ? "" : "s"}`;
      nodes.progressWeaknesses.append(item);
    }
  }
}

function syncFilter(select, values, label) {
  const selected = select.value;
  select.replaceChildren(new Option("All", "all"));
  for (const value of values) select.add(new Option(label(value), String(value)));
  select.value = [...select.options].some((option) => option.value === selected) ? selected : "all";
}

function languageLabel(value) {
  return ({ cpp: "C++", c: "C", java: "Java", javascript: "JavaScript", python: "Python" })[value] || value;
}

function titleCase(value) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} returned ${response.status}`);
  return response.json();
}

function select(selector, chosen) {
  for (const button of document.querySelectorAll(selector)) {
    mark(button, button === chosen);
  }
}
