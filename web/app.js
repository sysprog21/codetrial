import {
  consumeRandomDraw,
  createRandomDraw,
  randomDrawCompleted,
  storeRandomDraw,
} from "./random-draw.js";
import { FRAMEWORKS, codingLoop, interviewMode } from "./lib.js";
import {
  clearReportHistory,
  deleteReport,
  readDeviceHistory,
  renameLocalHistory,
} from "./history.js";
import {
  pickProblem,
  practiceFocus,
  storeSharedFocus,
  suggestDifficulty,
} from "./problem-picker.js";
import {
  normalizeProgressEntries,
  pickerEntry,
  progressModelFrom,
} from "./progress.js";
import { reportMarkdown, reportMarkup } from "./render.js";
import { downloadMarkdown, reportFilename } from "./download.js";
import { loadPageMap, loadTopicMap } from "./problem-data.js";
import {
  availableTopics,
  filterProblems,
  recentPerformance,
} from "./practice-insights.js";
import {
  parseGroundingFile,
  retainedSelection,
  selectedGroundingPacket,
  storeGroundingPacket,
} from "./document-grounding.js";

const requestTimeoutMs = 10_000;

let problem;
let duration;
let interviewLoop = "coding_behavioral";
/// Which surface the interview runs on; see the format row in index.html.
let mode = "coding";
let reports = [];
/// The practice focus the share box currently refers to.
let sharedFocus = null;
let manualProblem = false;
/// The reports a drawn problem kept across a difficulty change was drawn from,
/// serialized, or null when nothing is kept. Not `manualProblem`: a hand pick
/// stands for good, while a kept draw stands only until the history it was
/// drawn from changes.
let keptDraw = null;
let manualDuration = false;
let manualDifficulty = false;
let historyReady = false;
let historyLoad = 0;
// Initial history may arrive after a manual pick. Later account refreshes must
// finish before that pick can start, even if an older load completes first.
let historyRefreshing = false;
let loginPending = false;
// Successful and uncertain POSTs need an account check before another login.
// Null means no check is owed; otherwise this is "recorded" or "unknown".
let loginResult = null;
let deletingReports = false;
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
// Keep the exclusion with the roll so restoring the page repeats the same draw.
let avoidedProblem;
// Keep the draw policy until its filters change or its interview completes.
let pickerMode = "recommend";
// Only a new graded attempt at the drawn problem ends the random choice.
let randomDraw = null;

const nodes = {
  accountStatus: document.querySelector("#account-status"),
  githubLogin: document.querySelector("#github-login"),
  loginLink: document.querySelector("#login-link"),
  logout: document.querySelector("#logout"),
  historyHeader: document.querySelector("#history-header"),
  history: document.querySelector("#history"),
  deleteReports: document.querySelector("#delete-reports"),
  reportDeleteStatus: document.querySelector("#report-delete-status"),
  recommendation: document.querySelector("#recommendation"),
  randomProblem: document.querySelector("#random-problem"),
  practiceFocus: document.querySelector("#practice-focus"),
  practiceFocusShare: document.querySelector("#practice-focus-share"),
  practiceFocusShareInput: document.querySelector(
    "#practice-focus-share-input",
  ),
  progressSummary: document.querySelector("#progress-summary"),
  attemptHistory: document.querySelector("#attempt-history"),
  progressTrends: document.querySelector("#progress-trends"),
  progressWeaknesses: document.querySelector("#progress-weaknesses"),
  progressTopics: document.querySelector("#progress-topics"),
  progressDifficulty: document.querySelector("#progress-difficulty"),
  progressLanguage: document.querySelector("#progress-language"),
  progressDuration: document.querySelector("#progress-duration"),
  profileRole: document.querySelector("#profile-role"),
  profileSeniority: document.querySelector("#profile-seniority"),
  profileCompany: document.querySelector("#profile-company"),
  groundingJd: document.querySelector("#grounding-jd"),
  groundingResume: document.querySelector("#grounding-resume"),
  groundingJdStatus: document.querySelector("#grounding-jd-status"),
  groundingResumeStatus: document.querySelector("#grounding-resume-status"),
  groundingChoices: document.querySelector("#grounding-choices"),
  groundingConsent: document.querySelector("#grounding-consent"),
  groundingClear: document.querySelector("#grounding-clear"),
  groundingError: document.querySelector("#grounding-error"),
  durationNote: document.querySelector("#duration-note"),
  problemTopic: document.querySelector("#problem-topic"),
  problemPicker: document.querySelector("details.problem-picker"),
  problemFiltersReset: document.querySelector("#problem-filters-reset"),
  problemFilterSummary: document.querySelector("#problem-filter-summary"),
  recentPerformance: document.querySelector("#recent-performance"),
  recentPerformanceSummary: document.querySelector(
    "#recent-performance-summary",
  ),
  recentWeakTopics: document.querySelector("#recent-weak-topics"),
};

// Every card carries the pressed state from the start, not only the one that
// is on: one pressed button among 149 plain ones does not read as a choice.
// A card's id is its page name, the only name the browser has for a problem.
const cards = [...document.querySelectorAll("[data-problem]")].map(
  (button) => ({
    id: button.dataset.problem,
    difficulty: button.dataset.difficulty,
    topics: [],
    button,
  }),
);
const cardIds = new Set(cards.map((card) => card.id));
for (const card of cards) mark(card.button, false);

// The published names are off unless the candidate turns them on, and the
// choice is theirs to keep between visits. A convenience, so storage that
// throws (a private window, blocked site data) just means off.
const SHOW_SOURCES_KEY = "codetrial.showProblemSources";
const showSources = document.querySelector("#show-sources");
try {
  showSources.checked = localStorage.getItem(SHOW_SOURCES_KEY) === "1";
} catch {
  /* off */
}
// The published names are not in the page: they arrive with the map, fetched
// the first time the candidate turns them on.
const applySources = async () => {
  if (showSources.checked) {
    const pages = await loadPageMap().catch(() => null);
    const sourceOf = new Map(
      Object.values(pages ?? {})
        .filter((entry) => entry.source)
        .map((entry) => [entry.page, entry.source]),
    );
    for (const card of cards) {
      const source = card.button.querySelector(".problem-source");
      if (sourceOf.has(card.id))
        source.textContent = `LeetCode: ${sourceOf.get(card.id)}`;
    }
  }
  for (const source of document.querySelectorAll(".problem-source")) {
    source.hidden = !showSources.checked || source.textContent === "";
  }
};
showSources.addEventListener("change", () => {
  void applySources();
  try {
    localStorage.setItem(SHOW_SOURCES_KEY, showSources.checked ? "1" : "0");
  } catch {
    /* the page still shows what was chosen */
  }
});
void applySources();
const levels = [...document.querySelectorAll('[name="difficulty"]')];
let topicLoad;
nodes.problemTopic.disabled = true;

// A topic names the technique behind a scenario, so the page does not ship the
// full lookup. Opening the specific-problem picker opts into fetching it; no
// later visit inherits that choice.
async function loadTopics() {
  topicLoad ??= loadTopicMap()
    .then((mapping) => {
      for (const card of cards) card.topics = mapping?.[card.id] ?? [];
      for (const topic of availableTopics(cards)) {
        const option = document.createElement("option");
        option.value = topic;
        option.textContent = topic;
        nodes.problemTopic.append(option);
      }
      nodes.problemTopic.disabled = false;
      nodes.problemFilterSummary.textContent = "";
      return true;
    })
    .catch(() => {
      topicLoad = undefined;
      nodes.problemFilterSummary.textContent =
        "Topic filters could not be loaded.";
      return false;
    });
  return topicLoad;
}

nodes.problemPicker.addEventListener("toggle", () => {
  if (nodes.problemPicker.open) void loadTopics();
});

// A picked card is a choice about this one interview, not about the filter the
// checkboxes carry, so it leaves them alone. It suggests a length to go with
// the level it belongs to, which `setDuration` applies only if the candidate
// has not already chosen one.
for (const card of cards) {
  card.button.addEventListener("click", () => {
    pickerMode = "recommend";
    randomDraw = null;
    storeRandomDraw(null);
    manualProblem = true;
    setProblem(card);
    setDuration(suggestedDuration(new Set([card.difficulty])));
    nodes.recommendation.textContent = `Selected problem: ${title(card)}.`;
  });
}

nodes.randomProblem.addEventListener("click", () => {
  if (!historyReady || accountUpdatePending() || deletingReports) return;
  manualProblem = false;
  keptDraw = null;
  roll = Math.random();
  avoidedProblem = problem?.id;
  pickerMode = "random";
  randomDraw = null;
  storeRandomDraw(null);
  applyDifficulties();
  recommend();
});

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
    // A problem survives a filter that still includes it, whether it was
    // picked by hand or drawn: the checkbox answered which levels to offer,
    // not whether to throw away the problem on screen. Only a filter that now
    // hides it hands the choice back to the lobby.
    if (problem && !problem.button.hidden) {
      // Kept, a drawn problem is no longer what `roll` draws under this
      // filter, so the next `settle` would swap it. Held against the reports
      // in hand, which while the history is still loading are the ones about
      // to be replaced, so `settle` redraws then.
      if (!manualProblem) keptDraw = JSON.stringify(reports);
      return;
    }
    manualProblem = false;
    keptDraw = null;
    roll = Math.random();
    avoidedProblem = undefined;
    pickerMode = "recommend";
    randomDraw = null;
    storeRandomDraw(null);
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

nodes.problemTopic.addEventListener("change", () => {
  filterSelectionChanged();
});

nodes.problemFiltersReset.addEventListener("click", () => {
  nodes.problemTopic.value = "";
  filterSelectionChanged();
});

let grounding = { requirements: [], skills: [], anchors: [] };
const groundingReads = { jd: 0, resume: 0 };

nodes.groundingJd.addEventListener("change", () => loadGroundingFile("jd"));
nodes.groundingResume.addEventListener("change", () =>
  loadGroundingFile("resume"),
);
nodes.groundingClear.addEventListener("click", clearGrounding);

let progressNormalized = [];
let progressSuffix = "saved";
/// Whether the history on screen came from an account: null until /api/session
/// answers, and never assumed false, because a browser that cannot ask is not a
/// browser that knows there is nothing on the server.
let accountHistory = null;

for (const filter of [
  nodes.progressDifficulty,
  nodes.progressLanguage,
  nodes.progressDuration,
]) {
  filter.addEventListener("change", renderProgress);
}
nodes.deleteReports.addEventListener("click", deleteSavedReports);

const durations = [...document.querySelectorAll("[data-duration]")];
for (const button of durations) {
  button.addEventListener("click", () =>
    setDuration(Number(button.dataset.duration), true),
  );
}

for (const button of document.querySelectorAll("[data-loop]")) {
  button.addEventListener("click", () => {
    interviewLoop = codingLoop(button.dataset.loop);
    select("[data-loop]", button);
  });
}

for (const button of document.querySelectorAll("[data-mode]")) {
  button.addEventListener("click", () => {
    // A start already on its way out has read the mode it will open, so a
    // click now would light a surface the interview is not going to use.
    if (starting) return;
    mode = interviewMode(button.dataset.mode);
    select("[data-mode]", button);
    // Said once, here, because every other difference the candidate will meet
    // follows from it: no editor, no test runner, and a board the interviewer
    // is sent as they draw.
    const note = document.querySelector("#mode-note");
    note.textContent =
      mode === "whiteboard"
        ? "Whiteboard: no editor and no test runner. You explain by drawing, and Jim sees the board as you go."
        : "";
    note.hidden = mode !== "whiteboard";
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
  if (!problem || starting || accountUpdatePending() || deletingReports) return;
  // The whole form is read here, before the sign-in round trip below, because
  // every control the candidate can still touch during it feeds this URL.
  // Reading them afterwards shipped an interview that was 45 minutes when the
  // button was pressed and 60 by the time it was answered, and the same for
  // the round plan and the profile beside them. One read, then go.
  const destination = new URL("/interview", window.location.origin);
  // The scenario's page name, not the id: the address bar is on screen for the
  // whole interview, and the id is the published problem's slug.
  destination.searchParams.set("problem", problem.id);
  destination.searchParams.set("duration", String(duration));
  destination.searchParams.set("loop", interviewLoop);
  destination.searchParams.set("mode", mode);
  if (!manualProblem && randomDraw !== null)
    destination.searchParams.set("draw", randomDraw.id);
  const profile = {
    role: nodes.profileRole.value.trim(),
    seniority: nodes.profileSeniority.value,
    targetCompany: nodes.profileCompany.value.trim(),
  };
  if (profile.role) destination.searchParams.set("role", profile.role);
  if (profile.seniority)
    destination.searchParams.set("seniority", profile.seniority);
  if (profile.targetCompany)
    destination.searchParams.set("company", profile.targetCompany);
  const focus = nodes.practiceFocusShareInput.checked
    ? practiceFocus(reports)
    : null;
  let packet;
  try {
    packet = selectedGroundingPacket(
      grounding,
      checkedGrounding(),
      nodes.groundingConsent.checked,
    );
  } catch (error) {
    nodes.groundingError.textContent = error.message;
    return;
  }

  starting = true;
  syncLobbyControls();
  nodes.groundingError.textContent = "";
  if (signInFirst) {
    start.textContent = "Recording GitHub...";
    if (!(await recordGitHubLogin())) {
      starting = false;
      // The same round trip can leave nothing selected, so readiness must be
      // recomputed instead of unconditionally enabling Start.
      syncLobbyControls();
      setStartGate(true);
      return;
    }
    // The login is recorded, so the gate is answered. Leaving it set told a
    // candidate who had just signed in to sign in again on the next bail-out
    // below, and spent a second /api/login doing it.
    signInFirst = false;
  }
  start.textContent = "Starting...";
  if (!nodes.groundingConsent.checked) packet = null;
  try {
    storeGroundingPacket(sessionStorage, packet);
    storeSharedFocus(sessionStorage, focus?.weakness ?? null);
  } catch (error) {
    nodes.groundingError.textContent = error.message;
    starting = false;
    syncLobbyControls();
    setStartGate(signInFirst);
    return;
  }
  storeRandomDraw(manualProblem ? null : randomDraw);
  window.location.href = destination.toString();
});

async function loadGroundingFile(kind) {
  const input = kind === "jd" ? nodes.groundingJd : nodes.groundingResume;
  const status =
    kind === "jd" ? nodes.groundingJdStatus : nodes.groundingResumeStatus;
  const read = ++groundingReads[kind];
  status.textContent = "Reading locally...";
  let parsed = null;
  let message = "Parsed locally. Select only snippets you want to send.";
  try {
    parsed = await parseGroundingFile(input.files[0], kind);
  } catch (error) {
    message = error.message;
  }
  if (read !== groundingReads[kind]) return;
  if (kind === "jd") grounding.requirements = parsed?.requirements ?? [];
  else {
    grounding.skills = parsed?.skills ?? [];
    grounding.anchors = parsed?.anchors ?? [];
  }
  status.textContent = message;
  renderGroundingChoices(retainedSelection(checkedGrounding(), kind));
}

function checkedGrounding() {
  const selected = { requirements: [], skills: [], anchors: [] };
  for (const input of nodes.groundingChoices.querySelectorAll("input:checked"))
    selected[input.dataset.group].push(Number(input.value));
  return selected;
}

function renderGroundingChoices(selected) {
  nodes.groundingChoices.replaceChildren();
  for (const [group, label] of [
    ["requirements", "JD requirements"],
    ["skills", "Resume skills"],
    ["anchors", "Resume experience/project anchors"],
  ]) {
    if (!grounding[group].length) continue;
    const fieldset = document.createElement("fieldset");
    const legend = document.createElement("legend");
    legend.textContent = label;
    fieldset.append(legend);
    grounding[group].forEach((snippet, index) => {
      const row = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.dataset.group = group;
      checkbox.value = String(index);
      checkbox.checked = selected[group].includes(index);
      row.append(checkbox, document.createTextNode(` ${snippet}`));
      fieldset.append(row);
    });
    nodes.groundingChoices.append(fieldset);
  }
}

function clearGrounding() {
  groundingReads.jd++;
  groundingReads.resume++;
  grounding = { requirements: [], skills: [], anchors: [] };
  nodes.groundingJd.value = "";
  nodes.groundingResume.value = "";
  nodes.groundingJdStatus.textContent = "";
  nodes.groundingResumeStatus.textContent = "";
  nodes.groundingConsent.checked = false;
  nodes.groundingError.textContent = "";
  nodes.groundingChoices.replaceChildren();
  try {
    storeGroundingPacket(sessionStorage, null);
  } catch {
    /* clearing is best effort */
  }
}

// Returning from the media preflight can restore this page from the browser's
// back/forward cache after the start button was deliberately disabled.
window.addEventListener("pageshow", (event) => {
  // Only the restore. A normal load fires this too, after `load`, which is
  // late enough to re-enable a button the candidate already pressed and hand
  // them a second navigation.
  if (!event.persisted) return;
  const resetDraw = nodes.problemTopic.value !== "";
  nodes.problemTopic.value = "";
  applyProblemFilters();
  // The cache holds the page as it was before the candidate left, and a login
  // recorded on the way out is in none of it, so restoring what it holds told
  // somebody who had just signed in to sign in. Ask the server instead, and
  // treat the reports in hand as in flight again while it answers: the button
  // stays down and a difficulty change waits, exactly as on a first load.
  // `starting` with it: the page came back, so whatever start was on its way
  // out did not happen, and a latch left set here disables the button for good.
  starting = false;
  refreshHistory();
  if (resetDraw) filterSelectionChanged();
});

nodes.githubLogin.addEventListener("keydown", (event) => {
  // Some input methods end composition before its confirming keydown. The
  // legacy IME code remains 229 even when isComposing is already false.
  if (event.key !== "Enter" || event.isComposing || event.keyCode === 229)
    return;
  event.preventDefault();
  nodes.loginLink.click();
});

nodes.loginLink.addEventListener("click", async () => {
  if (starting || nodes.loginLink.hidden || nodes.loginLink.disabled) return;
  loginPending = true;
  syncLobbyControls();
  try {
    // Retrying an account check must not create another recorded session.
    if (!loginResult) {
      if (!(await recordGitHubLogin())) return;
      loginResult = "recorded";
    }
    await refreshHistory();
  } finally {
    loginPending = false;
    syncLobbyControls();
  }
});

// Card selection and history completion can both re-arm Start. Keep their
// view of the account transition in one place, including the GET-only retry.
function syncLobbyControls() {
  const accountBusy = accountUpdatePending();
  start.disabled = !problem || starting || accountBusy || deletingReports;
  nodes.loginLink.disabled =
    starting || loginPending || historyRefreshing || deletingReports;
  nodes.randomProblem.disabled =
    !historyReady || accountBusy || deletingReports;
  const deleteBlocked = reportDeletesBlocked();
  nodes.deleteReports.disabled = deleteBlocked;
  for (const remove of nodes.attemptHistory.querySelectorAll(
    "[data-delete-report]",
  ))
    remove.disabled = deleteBlocked;
}

function accountUpdatePending() {
  return loginPending || loginResult !== null || historyRefreshing;
}

/// One rule for the bulk delete and every row's: until the lobby knows whose
/// list is on screen, a delete cannot say which copy it is removing.
function reportDeletesBlocked() {
  return !historyReady || starting || accountUpdatePending() || deletingReports;
}

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

// The browser may reconstruct the lobby instead of restoring its JS heap.
window.addEventListener("pagehide", () => {
  const state = { ...window.history.state };
  if (pickerMode === "random" && randomDraw !== null)
    state.codetrialRandomDraw = {
      draw: randomDraw,
      difficulties: [...selectedDifficulties()],
      topic: nodes.problemTopic.value,
      roll,
      avoidedProblem,
      manualDifficulty,
      duration,
      manualDuration,
      note: nodes.recommendation.textContent,
    };
  else delete state.codetrialRandomDraw;
  window.history.replaceState(state, "");
});

function restoreLobbyDraw() {
  const state = { ...window.history.state };
  const saved = state.codetrialRandomDraw;
  if (!saved) return;
  delete state.codetrialRandomDraw;
  window.history.replaceState(state, "");
  if (
    !Array.isArray(saved.difficulties) ||
    !saved.difficulties.length ||
    !saved.difficulties.every((value) =>
      levels.some((input) => input.value === value),
    )
  )
    return;
  for (const input of levels)
    input.checked = saved.difficulties.includes(input.value);
  manualDifficulty = saved.manualDifficulty === true;
  if (
    durations.some(
      (button) => Number(button.dataset.duration) === saved.duration,
    )
  )
    setDuration(saved.duration, saved.manualDuration === true);
  applyDifficulties();
  // Topic filters reset on every return, including one without a cached page.
  if (saved.topic) {
    storeRandomDraw(null);
    return;
  }
  const card = cards.find(
    (candidate) => candidate.id === saved.draw?.problemId,
  );
  if (
    !card ||
    typeof saved.draw.id !== "string" ||
    !saved.draw.id ||
    !saved.difficulties.includes(card.difficulty) ||
    !Number.isFinite(saved.roll) ||
    saved.roll < 0 ||
    saved.roll >= 1
  )
    return;
  pickerMode = "random";
  randomDraw = saved.draw;
  roll = saved.roll;
  avoidedProblem = saved.avoidedProblem;
  setProblem(card);
  nodes.recommendation.textContent =
    typeof saved.note === "string" ? saved.note : "";
}

applyDifficulties();
restoreLobbyDraw();
// Nothing is recommended before the reports arrive, because they choose the
// level as well as the problem. The button ships disabled and `setProblem` is
// what enables it, so the gap is a button that cannot be pressed rather than
// one that starts an interview on nothing. `finally` rather than `then` for the
// same reason: a rejection in `loadAccount` would otherwise leave it disabled
// for good.
refreshHistory();

function refreshHistory() {
  const generation = ++historyLoad;
  historyRefreshing = generation > 1;
  historyReady = false;
  nodes.reportDeleteStatus.textContent = "";
  syncLobbyControls();
  // A cached page may retain its selection while its account is unknown.
  start.disabled = true;
  // A restored page can start a second load before the first one finishes.
  return loadAccount(generation).finally(() => {
    if (generation === historyLoad) {
      historyRefreshing = false;
      settle();
    }
  });
}

/// What the lobby settles into once it knows the candidate's history: the level
/// that history points at, a problem at that level, and a start button.
///
/// Shared by sign-in and back/forward restores. Both refetch the reports, so
/// only re-enabling Start would leave the checked level and its explanation
/// describing history the page had already replaced.
function settle() {
  historyReady = true;
  if (
    pickerMode === "random" &&
    randomDraw !== null &&
    (consumeRandomDraw(randomDraw) || randomDrawCompleted(randomDraw, reports))
  ) {
    pickerMode = "recommend";
    avoidedProblem = undefined;
    randomDraw = null;
    storeRandomDraw(null);
    keptDraw = null;
  } else if (pickerMode === "random" && randomDraw !== null) {
    // Deletion and account-history merges do not revoke a displayed draw.
    keptDraw = JSON.stringify(reports);
  }
  // Refreshed reports that differ from the ones a kept draw came from may have
  // just recorded it as passed, so the lobby draws again.
  if (keptDraw !== null && keptDraw !== JSON.stringify(reports))
    keptDraw = null;
  // The level suggestion only applies when the candidate has not already said
  // what they want. Moving their checkboxes would also hide the card they just
  // picked.
  const note =
    manualDifficulty || manualProblem || randomDraw !== null
      ? ""
      : applySuggestedLevel();
  recommend(note);
  // `recommend` returns without touching anything when the candidate's own pick
  // still stands, so the button the account refresh held down needs releasing
  // here rather than there.
  syncLobbyControls();
}

async function loadAccount(generation) {
  accountHistory = null;
  let signedOutMessage = "Signed out";
  try {
    const session = await fetchJson("/api/session");
    if (generation !== historyLoad) return;
    if (
      typeof session?.signedIn !== "boolean" ||
      (session.signedIn && typeof session.user?.login !== "string")
    ) {
      throw new Error("Invalid account response");
    }
    accountHistory = false;
    applyDurationCeiling(session.maxDurationMin);
    if (session.signedIn) {
      loginResult = null;
      accountHistory = true;
      nodes.loginLink.textContent = "Use GitHub";
      nodes.accountStatus.textContent = `Signed in as ${session.user.login}`;
      nodes.githubLogin.hidden = true;
      nodes.loginLink.hidden = true;
      nodes.logout.hidden = false;
      setStartGate(false);
      await renderServerHistory(generation);
      return;
    }
    const unconfirmedLogin = loginResult !== null;
    if (unconfirmedLogin) {
      signedOutMessage =
        loginResult === "recorded"
          ? "GitHub username recorded, but no signed-in session was found. Try signing in again."
          : "No signed-in session was found. Try signing in again.";
    }
    loginResult = null;
    nodes.loginLink.textContent = "Use GitHub";
    if (session.loginRequired) {
      nodes.accountStatus.textContent = unconfirmedLogin
        ? signedOutMessage
        : "Enter your GitHub username to start.";
      nodes.githubLogin.hidden = false;
      nodes.loginLink.hidden = false;
      nodes.logout.hidden = true;
      setStartGate(true);
      await renderLocalHistory(generation);
      return;
    }
  } catch {
    // A server that cannot answer about accounts is not one that will mint a
    // token either, but the editor still works offline, so do not lock the page.
  }
  if (generation !== historyLoad) return;
  if (loginResult) {
    showAccountCheckError();
    return;
  }
  nodes.accountStatus.textContent = signedOutMessage;
  nodes.githubLogin.hidden = false;
  nodes.loginLink.hidden = false;
  nodes.logout.hidden = true;
  setStartGate(false);
  await renderLocalHistory(generation);
}

function showAccountCheckError() {
  nodes.accountStatus.textContent =
    loginResult === "recorded"
      ? "GitHub username recorded, but the account could not be refreshed."
      : "Could not confirm the sign-in result. Retry the account check.";
  nodes.githubLogin.hidden = true;
  nodes.loginLink.hidden = false;
  nodes.loginLink.textContent = "Retry account check";
  nodes.logout.hidden = false;
  showProgressError(
    "Could not load account progress. Retry the account check.",
  );
}

async function recordGitHubLogin() {
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
      signal: AbortSignal.timeout(requestTimeoutMs),
    });
    if (!response.ok) {
      // The server already said what is wrong (bad handle, rate limited, not
      // configured); replacing that with a generic line throws it away.
      const body = await response.json().catch(() => null);
      nodes.accountStatus.textContent =
        body?.error || "Could not record GitHub username.";
      return false;
    }
    return true;
  } catch {
    // A timeout or dropped connection does not undo a server-side write.
    // Invalidate reads sent before this outcome, then offer a GET-only retry.
    loginResult = "unknown";
    historyLoad += 1;
    historyRefreshing = false;
    accountHistory = null;
    showAccountCheckError();
    settle();
    return false;
  }
}

async function renderServerHistory(generation) {
  try {
    const data = await fetchJson("/api/reports");
    if (generation !== historyLoad) return;
    // The picker needs the account row's timestamp for review scheduling and
    // the verdict as it was saved; the progress panel normalizes its own.
    // The server reads its rows back under page names, so no map is needed.
    reports = data.reports.map(pickerEntry);
    showProgress(data.reports, "saved to your account");
  } catch {
    if (generation !== historyLoad) return;
    showProgressError("Could not load saved account progress.");
  }
}

/// Local history saved before problems had page names carries published ids.
/// The map, fetched only when such an entry is there, renames them in place
/// once, so the picker still knows what the candidate has passed and the next
/// visit fetches nothing. Account history needs none of this: the server reads
/// it back under page names.
async function renderLocalHistory(generation = historyLoad) {
  try {
    // Both stores, merged by `readDeviceHistory`: the review list keeps
    // attempts long after the 20-row history has dropped them, so it can hold
    // a published id the history no longer shows.
    let entries = readDeviceHistory();
    if (
      entries.some(
        (entry) =>
          !cardIds.has(pickerEntry(entry).problemId) &&
          entry?.pageMapChecked !== true,
      )
    ) {
      const pages = await loadHistoryPageMap();
      if (generation !== historyLoad) return;
      if (pages) {
        renameLocalHistory(pages, undefined, true);
        entries = readDeviceHistory();
      }
    }
    reports = entries.map(pickerEntry);
    showProgress(entries, "saved on this device");
  } catch {
    if (generation !== historyLoad) return;
    showProgressError("Could not load progress saved on this device.");
  }
}

async function loadHistoryPageMap() {
  let timer;
  try {
    // The shared lookup also serves the title toggle. Bound this caller's
    // wait without cancelling the lookup for its other consumers.
    return await Promise.race([
      loadPageMap().catch(() => null),
      new Promise((resolve) => {
        timer = setTimeout(() => resolve(null), requestTimeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

const deleteFailedMessage =
  "Could not delete saved reports. Your reports may not have been removed.";

async function deleteSavedReports() {
  if (starting || accountUpdatePending() || deletingReports) return;
  const confirmed = window.confirm(
    "Delete saved reports and progress? Recording files follow their separate retention policy. This cannot be undone.",
  );
  if (!confirmed) return;
  deletingReports = true;
  syncLobbyControls();
  nodes.reportDeleteStatus.textContent = "";
  try {
    const result = await clearReportHistory({ account: accountHistory });
    if (result === "cleared") {
      showReportDeleteStatus(
        "Saved reports and progress were deleted.",
        "good small",
      );
      reports = [];
      showProgress([], "saved");
      settle();
      return;
    }
    if (result === "account-cleared-local-failed") {
      showReportDeleteStatus(
        "Account reports were deleted, but reports saved on this device could not be deleted.",
      );
      await renderLocalHistory();
      settle();
      return;
    }
    showReportDeleteStatus(deleteFailedMessage);
  } catch {
    if (!nodes.reportDeleteStatus.textContent)
      showReportDeleteStatus(deleteFailedMessage);
    nodes.reportDeleteStatus.focus();
  } finally {
    deletingReports = false;
    syncLobbyControls();
  }
}

/// Keyed on `attempt.id`, never on the row's position: the list is filtered, so
/// the third row shown is rarely the third report stored.
async function deleteSavedReport(attempt, when, button) {
  if (reportDeletesBlocked()) return;
  const confirmed = window.confirm(
    `Delete the ${when} report for ${attempt.problemTitle}? Recording files follow their separate retention policy. This cannot be undone.`,
  );
  if (!confirmed) return;
  const position = [...nodes.attemptHistory.children].indexOf(
    button.closest("li"),
  );
  deletingReports = true;
  syncLobbyControls();
  nodes.reportDeleteStatus.textContent = "";
  let message = "Could not delete the report. It may not have been removed.";
  let className = "critical small";
  try {
    // `accountHistory` is null when the session never answered, and the list
    // drawn then is this device's, so only `true` names an account list.
    const result = await deleteReport(attempt.id, {
      account: accountHistory === true,
    });
    if (result !== "failed") {
      if (result === "account-deleted-local-failed") {
        message =
          "The report was deleted from your account, but its copy on this device could not be deleted.";
      } else if (result === "missing") {
        message = "That report was not found, so nothing was deleted.";
      } else {
        message = "The report was deleted.";
        className = "good small";
      }
      await reloadAfterDelete(result === "account-deleted-local-failed");
    }
  } catch {
    // The failure message above stands.
  } finally {
    deletingReports = false;
    syncLobbyControls();
  }
  showReportDeleteStatus(message, className, false);
  focusAttemptRow(position, button);
}

/// Reloaded rather than spliced out of what is on screen, so the filters lose a
/// language or a length only the deleted report had.
///
/// A generation of its own, so this reload and any other in flight cannot both
/// land: a `GET /api/reports` sent before the `DELETE` answered would otherwise
/// repaint the report it removed. A back/forward restore still reading the
/// session is restarted instead of cut off halfway through `loadAccount`.
///
/// `local` draws this device's copy whatever the account holds, because that
/// is the one the status line says was left behind.
async function reloadAfterDelete(local) {
  if (!historyReady) {
    await refreshHistory();
    return;
  }
  const generation = ++historyLoad;
  if (accountHistory && !local) await renderServerHistory(generation);
  else await renderLocalHistory(generation);
  if (generation === historyLoad) settle();
}

/// Focus stays in the list, on the row that took the deleted one's place, so a
/// keyboard user deleting several is not thrown past the whole progress panel
/// after each. The status line is a live region and announces itself.
function focusAttemptRow(position, button) {
  if (button.isConnected) {
    button.focus();
    return;
  }
  const rows = nodes.attemptHistory.children;
  const row = rows[Math.min(position, rows.length - 1)];
  const target =
    row?.querySelector("[data-delete-report]") ?? row?.querySelector("button");
  if (target) target.focus();
  else nodes.reportDeleteStatus.focus();
}

function showReportDeleteStatus(
  message,
  className = "critical small",
  focus = true,
) {
  nodes.reportDeleteStatus.className = className;
  nodes.reportDeleteStatus.textContent = message;
  if (focus) nodes.reportDeleteStatus.focus();
}

function selectedDifficulties() {
  return new Set(
    levels.filter((input) => input.checked).map((input) => input.value),
  );
}

/// The checkboxes moved, so everything read off them moves too: which cards the
/// picker offers, and the length the level implies. One place, because three
/// copies of these two lines is three places to forget one.
function applyDifficulties() {
  const difficulties = selectedDifficulties();
  // The picker is a shortcut to one problem, not a second copy of the wall the
  // checkboxes just hid, so it shows what the checkboxes selected.
  applyProblemFilters(difficulties);
  setDuration(suggestedDuration(difficulties));
}

function topicEligibleCards() {
  return filterProblems(cards, { topic: nodes.problemTopic.value });
}

function applyProblemFilters(difficulties = selectedDifficulties()) {
  const visible = new Set(
    filterProblems(cards, {
      difficulties,
      topic: nodes.problemTopic.value,
    }),
  );
  for (const card of cards) card.button.hidden = !visible.has(card);
  const topic = nodes.problemTopic.value;
  nodes.problemFilterSummary.textContent = topic
    ? `${visible.size} problem${visible.size === 1 ? "" : "s"} tagged ${topic} shown.`
    : "";
}

function filterSelectionChanged() {
  applyProblemFilters();
  if (manualProblem && !problem?.button.hidden) return;
  manualProblem = false;
  keptDraw = null;
  roll = Math.random();
  avoidedProblem = undefined;
  pickerMode = "recommend";
  randomDraw = null;
  storeRandomDraw(null);
  if (historyReady) recommend();
  else {
    setProblem(null);
    nodes.recommendation.textContent = "";
  }
}

/// `note` is the sentence explaining a level the reports chose, empty when the
/// candidate chose it themselves and so already knows.
function recommend(note = "") {
  // A candidate who picked a card has answered the question this line asks, so
  // it stays answered. Naming a different problem here contradicted the card
  // they had just selected. A kept draw stands the same way until `settle`
  // drops it.
  if (manualProblem || keptDraw !== null) return;
  const choice = pickProblem(
    topicEligibleCards(),
    selectedDifficulties(),
    reports,
    () => roll,
    undefined,
    avoidedProblem,
    cards,
    pickerMode,
  );
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
  // A random draw is not explained by the reports, so its line names the
  // level instead of a review interval or a streak.
  if (pickerMode === "random") {
    randomDraw = createRandomDraw(choice.picked.id);
    storeRandomDraw(randomDraw);
    nodes.recommendation.textContent = `${note}Selected problem: ${title(choice.picked)} (${choice.picked.difficulty}).`;
    return;
  }
  if (choice.review) {
    // A review can fall outside the levels now selected, so the card is
    // unhidden and the level said out loud rather than silently ignored.
    choice.picked.button.hidden = false;
    const level = selectedDifficulties().has(choice.picked.difficulty)
      ? ""
      : ` (${choice.picked.difficulty})`;
    const days = choice.review.intervalDays;
    nodes.recommendation.textContent = `${note}Selected problem: ${title(choice.picked)}. Review due after ${days} day${days === 1 ? "" : "s"}${level}.`;
    return;
  }
  nodes.recommendation.textContent = choice.repeat
    ? `${note}Selected problem: ${title(choice.picked)}. ${nodes.problemTopic.value ? "You have passed every problem matching this topic and level." : "You have passed every problem at this level."}`
    : `${note}Selected problem: ${title(choice.picked)}.`;
}

/// Read off `reports` alone, so it is rendered wherever those change: the two
/// history paths below, and nowhere the selection moves.
function renderPracticeFocus() {
  const focus = practiceFocus(reports);
  nodes.practiceFocus.textContent = focus
    ? `Carry forward${focus.occurrences === 1 ? "" : ` (${focus.occurrences} reports)`}: ${focus.weakness}. Drill: ${focus.drill}. Success: ${focus.successCriterion}.`
    : "";
  nodes.practiceFocus.hidden = focus === null;
  nodes.practiceFocusShare.hidden = focus === null;
  // Consent is to share this text. Reloaded history can change it, and a box
  // left ticked would then send words the candidate never saw beside it.
  if (focus?.weakness !== sharedFocus)
    nodes.practiceFocusShareInput.checked = false;
  sharedFocus = focus?.weakness ?? null;
}

/// Check the level the candidate's own results point at and return the sentence
/// saying why. Empty when the reports have no opinion yet, which leaves the
/// markup's default standing.
function applySuggestedLevel() {
  const suggestion = suggestDifficulty(cards, reports);
  if (!suggestion) {
    for (const input of levels) input.checked = input.defaultChecked;
    applyDifficulties();
    return "";
  }
  for (const input of levels)
    input.checked = input.value === suggestion.difficulty;
  applyDifficulties();
  // Named after the level just finished, not the one being suggested: those
  // differ whenever the streak actually moves the candidate.
  return suggestion.reason === "passed"
    ? `You passed your last two ${suggestion.from} problems. `
    : `Your last two ${suggestion.from} problems did not land. `;
}

/// Clearing a filtered-out card must not leave Start pointing at it, and
/// picking another card must not bypass an account request already in flight.
/// Callers own the explanation beside the picker; this only knows which card.
function setProblem(card) {
  // Two buttons, not a sweep of all 150. `problem` still holds the outgoing
  // card here, which is what makes that possible.
  mark(problem?.button, false);
  mark(card?.button, true);
  problem = card;
  syncLobbyControls();
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
  select(
    "[data-duration]",
    document.querySelector(`[data-duration="${duration}"]`),
  );
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
  const over = durations.filter(
    (button) => Number(button.dataset.duration) > minutes,
  );
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
    select(
      "[data-duration]",
      document.querySelector(`[data-duration="${duration}"]`),
    );
  }
}

function title(card) {
  return card.button.querySelector(".problem-title").textContent;
}

function showProgressError(message) {
  // Emptied, not just unpainted. These outlive the panel: recommendations and
  // every filter change read them again afterwards, so a lobby that failed to
  // load one account's history went on answering from whichever account's
  // history it had last managed to load.
  reports = [];
  progressNormalized = [];
  renderPracticeFocus();
  hideRecentPerformance();
  nodes.historyHeader.hidden = false;
  nodes.history.hidden = false;
  nodes.progressSummary.textContent = message;
  nodes.progressTrends.replaceChildren();
  nodes.progressWeaknesses.replaceChildren();
  nodes.progressTopics.replaceChildren();
  nodes.attemptHistory.replaceChildren();
}

function showProgress(entries, suffix) {
  renderPracticeFocus();
  renderRecentPerformance();
  // Normalized once here, not per render: the filters below only select from
  // these rows, so a dropdown change has nothing to re-sanitize.
  progressNormalized = normalizeProgressEntries(entries);
  progressSuffix = suffix;
  const erasable = entries.length > 0 || readDeviceHistory().length > 0;
  nodes.historyHeader.hidden = !erasable;
  nodes.history.hidden = false;
  const model = progressModelFrom(progressNormalized);
  syncFilter(
    nodes.progressDifficulty,
    model.options.difficulty,
    (value) => value,
  );
  syncFilter(nodes.progressLanguage, model.options.language, languageLabel);
  syncFilter(
    nodes.progressDuration,
    model.options.durationMin,
    (value) => `${value} min`,
  );
  renderProgress();
}

function hideRecentPerformance() {
  nodes.recentPerformance.hidden = true;
  nodes.recentPerformanceSummary.textContent = "";
  nodes.recentWeakTopics.textContent = "";
}

function renderRecentPerformance() {
  const snapshot = recentPerformance(reports);
  if (!snapshot) {
    hideRecentPerformance();
    return;
  }
  const result = `${snapshot.passes} passed, ${snapshot.misses} missed`;
  const streak =
    snapshot.streak > 1
      ? ` Current ${snapshot.latestDecision === "HIRE" ? "pass" : "miss"} streak: ${snapshot.streak}.`
      : "";
  nodes.recentPerformanceSummary.textContent = `Last ${snapshot.attempts} assessed interview${snapshot.attempts === 1 ? "" : "s"}: ${result} (${snapshot.passRate}%).${streak}`;
  nodes.recentWeakTopics.textContent = snapshot.weakTopics.length
    ? `Topics to revisit: ${snapshot.weakTopics.slice(0, 5).join(", ")}.`
    : "No recurring weak topic in these interviews.";
  nodes.recentPerformance.hidden = false;
}

/// `attempts` is what `progressModelFrom` already filtered, oldest first.
///
/// Taken rather than re-derived: normalizing again here is what let the trends
/// panel and this list disagree about which stored rows count as an attempt,
/// and it cost one more sanitizing pass over every stored report.
function renderAttemptHistory(attempts) {
  nodes.attemptHistory.replaceChildren();
  for (const attempt of [...attempts].reverse()) {
    const item = document.createElement("li");
    const date = new Date(attempt.at).toLocaleDateString();
    const verdict = attempt.report.incomplete
      ? "INCOMPLETE"
      : (attempt.report.decision ?? "UNSCORED");
    const label = document.createElement("p");
    label.textContent = `${date} · ${attempt.problemTitle} · ${languageLabel(attempt.language ?? "not recorded")} · ${verdict}`;
    const open = document.createElement("button");
    open.type = "button";
    open.textContent = "Open report";
    const download = document.createElement("button");
    download.type = "button";
    download.textContent = "Download report (.md)";
    download.setAttribute(
      "aria-label",
      `Download report (.md) for ${attempt.problemTitle}, ${new Date(attempt.at).toLocaleString()}`,
    );
    download.addEventListener("click", () => downloadSavedReport(attempt));
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "Try again";
    const report = document.createElement("div");
    report.innerHTML = reportMarkup({
      report: attempt.report,
      problemTitle: attempt.problemTitle,
      language: languageLabel(attempt.language ?? "not recorded"),
      code: "(final code was not saved)",
    });
    report.querySelector(".report-actions")?.remove();
    report.hidden = true;
    open.addEventListener("click", () => {
      if (!report.hidden) {
        report.hidden = true;
        open.textContent = "Open report";
        return;
      }
      report.hidden = false;
      open.textContent = "Collapse report";
    });
    retry.addEventListener("click", () => {
      const card = cards.find(
        (candidate) => candidate.id === attempt.problemId,
      );
      if (!card) return;
      pickerMode = "recommend";
      randomDraw = null;
      storeRandomDraw(null);
      manualProblem = true;
      card.button.hidden = false;
      setProblem(card);
      setDuration(suggestedDuration(new Set([card.difficulty])));
      nodes.recommendation.textContent = `Selected problem: ${title(card)}.`;
    });
    item.append(label, open, download, retry);
    // A row with no id has nothing both stores and the account agree on, and
    // deleting by anything weaker could take a different report than this one.
    if (attempt.id) {
      // The time as well as the day: `Try again` makes two attempts at one
      // problem on one day, and the confirmation and the accessible name are
      // all that tells their two Delete buttons apart.
      const when = new Date(attempt.at).toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      });
      const remove = document.createElement("button");
      remove.type = "button";
      remove.dataset.deleteReport = "";
      remove.textContent = "Delete";
      remove.setAttribute(
        "aria-label",
        `Delete the ${when} report for ${attempt.problemTitle}`,
      );
      remove.disabled = reportDeletesBlocked();
      remove.addEventListener("click", () =>
        deleteSavedReport(attempt, when, remove),
      );
      item.append(remove);
    }
    item.append(report);
    nodes.attemptHistory.append(item);
  }
}

function downloadSavedReport(attempt) {
  const markdown = reportMarkdown({
    report: {
      ...attempt.report,
      incomplete:
        attempt.report.incomplete || attempt.recordedDecision === null,
    },
    problemTitle: attempt.problemTitle,
    language: attempt.language,
    code: null,
    transcript: null,
    at: new Date(attempt.at).toLocaleString(),
  });
  downloadMarkdown(
    markdown,
    reportFilename(attempt.problemId, new Date(attempt.at)),
  );
}

// How many weaknesses a phase row lists before folding the rest away. A
// report can file four under one phase, and a phase flagged across many
// attempts would otherwise grow a line for every one of them.
const MAX_WEAKNESSES_SHOWN = 3;

/// One weakness per item, as text: every wording is the model's.
function weaknessList(tags) {
  const list = document.createElement("ul");
  for (const tag of tags) {
    const item = document.createElement("li");
    item.textContent = tag;
    list.append(item);
  }
  return list;
}

function renderProgress() {
  const filters = {
    difficulty: nodes.progressDifficulty.value,
    language: nodes.progressLanguage.value,
    durationMin: nodes.progressDuration.value,
  };
  const model = progressModelFrom(progressNormalized, filters);
  nodes.progressTrends.replaceChildren();
  nodes.progressWeaknesses.replaceChildren();
  nodes.progressTopics.replaceChildren();
  renderAttemptHistory(model.attempts);
  if (model.total === 0) {
    nodes.history.hidden = true;
    return;
  }
  if (model.attempts.length === 0) {
    nodes.progressSummary.textContent = `No saved attempts match these filters. ${model.total} remain ${progressSuffix}.`;
    return;
  }
  const assessed = model.attempts.filter(
    (attempt) => attempt.report.frameworkAssessment,
  ).length;
  nodes.progressSummary.textContent =
    assessed === 0
      ? `${model.attempts.length} of ${model.total} attempts shown · these legacy or unassessed reports have no versioned phase scores, so no zeroes are plotted · ${progressSuffix}.`
      : `${model.attempts.length} of ${model.total} attempts shown · ${assessed} have comparable formative phase scores, not calibrated hiring evidence · ${progressSuffix}.`;
  // A table each, not one table with the two as groups inside it. They score
  // different exercises: REACTO is how a problem was worked, STAR is how past
  // work was recounted. A single Phase column ran them together as one
  // ten-step scale, and neither table refers to the other because a candidate
  // reading one has no use for the other's rows.
  for (const framework of Object.values(FRAMEWORKS)) {
    const table = document.createElement("table");
    const caption = document.createElement("caption");
    caption.textContent = `${framework.name} · ${framework.scenario}`;
    table.append(caption);
    const head = table.createTHead().insertRow();
    for (const label of ["Step", "Assessed scores by rubric version"]) {
      const cell = document.createElement("th");
      cell.scope = "col";
      cell.textContent = label;
      head.append(cell);
    }
    const body = table.createTBody();
    for (const step of framework.steps) {
      const row = body.insertRow();
      const heading = document.createElement("th");
      heading.scope = "row";
      heading.textContent = step.label;
      row.append(heading);
      const trend = row.insertCell();
      const segments = model.series[step.label];
      trend.textContent = segments.length
        ? segments
            .map(
              (segment) =>
                `Rubric v${segment.rubricVersion}: ${segment.points.map((point) => `attempt ${point.attemptIndex + 1}: ${point.score}`).join(" → ")}`,
            )
            .join(" | ")
        : "Not assessed in these attempts";
    }
    nodes.progressTrends.append(table);
  }
  if (model.weaknesses.length === 0) {
    const item = document.createElement("li");
    item.textContent = "No grounded weakness tags in these attempts.";
    nodes.progressWeaknesses.append(item);
  } else {
    for (const weakness of model.weaknesses) {
      const item = document.createElement("li");
      const heading = document.createElement("p");
      heading.textContent = `${weakness.framework} · ${weakness.phase} · flagged in ${weakness.count} of ${weakness.assessed} assessed attempt${weakness.assessed === 1 ? "" : "s"}`;
      item.append(
        heading,
        weaknessList(weakness.tags.slice(0, MAX_WEAKNESSES_SHOWN)),
      );
      // Folded rather than dropped, and outside the list so the fold is not
      // announced as one more weakness.
      const rest = weakness.tags.slice(MAX_WEAKNESSES_SHOWN);
      if (rest.length > 0) {
        const more = document.createElement("details");
        const summary = document.createElement("summary");
        summary.textContent = `${rest.length} more weakness${rest.length === 1 ? "" : "es"}`;
        more.append(summary, weaknessList(rest));
        item.append(more);
      }
      nodes.progressWeaknesses.append(item);
    }
  }
  if (model.topics.length === 0) {
    const item = document.createElement("li");
    item.textContent = "No topic labels are available for these attempts.";
    nodes.progressTopics.append(item);
  } else {
    for (const topic of model.topics) {
      const item = document.createElement("li");
      item.textContent = `${topic.topic}: ${topic.attempts} attempt${topic.attempts === 1 ? "" : "s"}, ${topic.passes} pass${topic.passes === 1 ? "" : "es"}; last attempt ${new Date(topic.lastAttempt).toLocaleDateString()}`;
      nodes.progressTopics.append(item);
    }
  }
}

function syncFilter(select, values, label) {
  const selected = select.value;
  select.replaceChildren(new Option("All", "all"));
  for (const value of values)
    select.add(new Option(label(value), String(value)));
  select.value = [...select.options].some((option) => option.value === selected)
    ? selected
    : "all";
}

function languageLabel(value) {
  return (
    {
      cpp: "C++",
      c: "C",
      java: "Java",
      javascript: "JavaScript",
      python: "Python",
    }[value] || value
  );
}

async function fetchJson(url) {
  // Keep the deadline attached while the response body is being read too.
  const response = await fetch(url, {
    signal: AbortSignal.timeout(requestTimeoutMs),
  });
  if (!response.ok) throw new Error(`${url} returned ${response.status}`);
  return response.json();
}

function select(selector, chosen) {
  for (const button of document.querySelectorAll(selector)) {
    mark(button, button === chosen);
  }
}
