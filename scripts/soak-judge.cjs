// What the soak makes of the socket replacements the agent logs.
//
// Kept apart from scripts/browser-check.cjs so the verdict can be tested
// without a browser: it is a function of the `replacement:` lines and two
// flags, and every rule in it was learned from a run that went wrong.

const TRIGGERS = ["watchdog", "close", "goaway"];
const HANDOVERS = ["resumed", "cold_handoff", "degraded"];

// A provider that stalls on an utterance costs the reply watchdog's ten
// seconds and a replacement, and the replacement may have to be rebuilt locally
// when the socket it replaced was itself young. One such recovery is a
// property of the provider, not of the rotation under test; a second one in the
// same soak means the interview is not holding together.
const MAX_WATCHDOG_REBUILDS = 1;

// Why a rebuild after a reply timeout can be the provider's doing and not a
// defect: the replaced socket had offered no handle yet, or was itself a
// resumed one replaced young. A handle that was offered and refused is a
// resumption defect whatever made the replacement necessary.
const STALL_FALLBACKS = new Set(["no_handle", "resume_suppressed"]);

/// The complete lines of `log`: a trailing fragment has no newline yet, and
/// reading it now would print it once cut short and again when finished.
function completeLines(log) {
  const lines = log.split("\n");
  if (!log.endsWith("\n")) lines.pop();
  return lines;
}

/// The `key=value` fields of a line whose last field is `room=`: a room name
/// may hold spaces, so it is the rest of the line and the other fields are read
/// from what comes before. `null` for another room's line.
function lineFields(line, room) {
  const at = line.lastIndexOf(" room=");
  if (at < 0 || line.slice(at + " room=".length) !== room) return null;
  return Object.fromEntries(
    [...line.slice(0, at).matchAll(/(?:^|\s)([a-z_]+)=(\S+)/g)].map((m) => [
      m[1],
      m[2],
    ]),
  );
}

/// A field that is a whole number, as a number; anything else, `none` and a
/// field a legacy line never had included, is `null`.
function whole(value) {
  return /^-?\d+$/.test(value ?? "") ? Number(value) : null;
}

/// The `replacement:` lines of one room, in the order logged, each once.
///
/// A line that carries the ids appended with the lifecycle identity
/// (`pid`, `session`, `episode`, `replacement`) is one replacement per id, so
/// two swaps logged at the same displayed clock stay two and the same swap read
/// from the agent's output and from the server's log is counted once. A legacy
/// line has no ids and is told apart by the clock it was logged at, however the
/// two logs spell the rest of it. A line that does not carry a known trigger
/// and handover is left out, so a renamed value turns into a missing handover,
/// which fails, and not into a silent pass.
function parseReplacements(log, room) {
  const seen = new Set();
  const replacements = [];
  for (const line of completeLines(log)) {
    if (!line.startsWith("replacement: ")) continue;
    const fields = lineFields(line, room);
    if (!fields) continue;
    if (!TRIGGERS.includes(fields.trigger)) continue;
    if (!HANDOVERS.includes(fields.handover)) continue;
    if (!fields.at) continue;
    const ids = {
      pid: whole(fields.pid),
      session: whole(fields.session),
      episode: whole(fields.episode),
      replacement: whole(fields.replacement),
    };
    const identified = Object.values(ids).every((id) => id !== null);
    const replacement = {
      line,
      at: fields.at,
      trigger: fields.trigger,
      handover: fields.handover,
      fallback: fields.fallback ?? "none",
      legacy: !identified,
      ...ids,
      oldSocket: whole(fields.old_socket),
      newSocket: whole(fields.new_socket),
      obligation: whole(fields.obligation),
      interviewMs: whole(fields.interview_ms),
    };
    const key = replacementKey(replacement);
    if (seen.has(key)) continue;
    seen.add(key);
    replacements.push(replacement);
  }
  return replacements;
}

/// What tells one parsed replacement from another: its ids when the line
/// carries them, else the clock it was displayed at. Anything that merges
/// replacements across reads must use this and not `at`, or two swaps at one
/// displayed clock collapse.
function replacementKey(r) {
  return r.legacy
    ? `at:${r.at}`
    : `id:${r.pid}:${r.session}:${r.episode}:${r.replacement}`;
}

const EPISODE_RESULTS = ["installed", "failed", "no_setup", "cancelled"];
const MILESTONES = [
  "notice_ms",
  "setup_ms",
  "connected_ms",
  "opener_ms",
  "install_ms",
];

// The milestones counted from the same start, so they can only grow; `setup_ms`
// is a duration inside that span and must not exceed `connected_ms`.
const ORDERED_MILESTONES = MILESTONES.filter((name) => name !== "setup_ms");

/// The replacement episodes of one room, from `replacement start:` and
/// `replacement end:` lines, in the order each started, one per
/// `(pid, session, episode)` however many copies of its lines the logs hold.
///
/// An episode with no end line is `incomplete`: the log was cut, or the wait
/// never reached a terminal, and either is a fact to report and not a pass. An
/// end line with an unknown result, or one with no start, is kept as
/// `unmatched` so it cannot vanish. Episodes are told apart by their ids, never
/// by the clock they were displayed at.
function parseEpisodes(log, room) {
  const episodes = new Map();
  const unmatched = [];
  const keyOf = (f) => `${f.pid}:${f.session}:${f.episode}`;
  // Starts are read before ends, whatever order the lines arrived in: a poller
  // that joins a rolling window and a second log can see an end first.
  const lines = completeLines(log);
  const ordered = [
    ...lines.filter((line) => line.startsWith("replacement start: ")),
    ...lines.filter((line) => !line.startsWith("replacement start: ")),
  ];
  for (const line of ordered) {
    const kind = line.startsWith("replacement start: ")
      ? "start"
      : line.startsWith("replacement end: ")
        ? "end"
        : null;
    if (!kind) continue;
    const f = lineFields(line, room);
    if (!f) continue;
    const ids = {
      pid: whole(f.pid),
      session: whole(f.session),
      episode: whole(f.episode),
    };
    if (Object.values(ids).some((id) => id === null)) continue;
    const key = keyOf(ids);
    if (kind === "start") {
      if (episodes.has(key)) continue;
      episodes.set(key, {
        ...ids,
        trigger: f.trigger,
        oldSocket: whole(f.old_socket),
        obligation: whole(f.obligation),
        obligationOrigin:
          f.obligation_origin && f.obligation_origin !== "none"
            ? f.obligation_origin
            : null,
        advisories: whole(f.advisories) ?? 0,
        startInterviewMs: whole(f.interview_ms),
        result: "incomplete",
      });
      continue;
    }
    const episode = episodes.get(key);
    if (!episode || !EPISODE_RESULTS.includes(f.result)) {
      unmatched.push({ ...ids, result: f.result });
      continue;
    }
    // The first end wins: a copy of the same line from another log is the same
    // result, and a differing one is a defect the caller can see in `ends`.
    episode.ends = (episode.ends ?? 0) + 1;
    if (episode.ends > 1) continue;
    episode.result = f.result;
    episode.replacement = whole(f.replacement);
    episode.newSocket = whole(f.new_socket);
    episode.attempt = whole(f.attempt);
    episode.resumeTried = f.resume_tried === "true";
    episode.endInterviewMs = whole(f.interview_ms);
    for (const name of MILESTONES) episode[name] = whole(f[name]);
  }
  return { episodes: [...episodes.values()], unmatched };
}

/// What the episodes of one room say about whether each start resolved once:
/// the ones still `incomplete`, the unmatched end lines, ids that do not grow
/// in the order logged within a `(pid, session)`, and milestones out of order.
/// A milestone that was not reached is `null` and is skipped.
function judgeEpisodes({ episodes, unmatched }) {
  const reasons = [];
  const incomplete = episodes.filter((e) => e.result === "incomplete");
  if (incomplete.length > 0) {
    reasons.push(
      `${incomplete.length} replacement start(s) never reached a result: ${incomplete.map((e) => e.episode).join(",")}`,
    );
  }
  if (unmatched.length > 0) {
    reasons.push(`${unmatched.length} replacement end line(s) with no start`);
  }
  const last = new Map();
  for (const e of episodes) {
    const scope = `${e.pid}:${e.session}`;
    if (last.has(scope) && e.episode <= last.get(scope)) {
      reasons.push(`episode ${e.episode} does not follow ${last.get(scope)}`);
    }
    last.set(scope, e.episode);
    const reached = ORDERED_MILESTONES.map((name) => e[name]).filter(
      (ms) => ms !== null && ms !== undefined,
    );
    for (let i = 1; i < reached.length; i += 1) {
      if (reached[i] < reached[i - 1]) {
        reasons.push(`episode ${e.episode} milestones run backwards`);
        break;
      }
    }
    if (
      e.setup_ms !== null &&
      e.setup_ms !== undefined &&
      e.connected_ms !== null &&
      e.connected_ms !== undefined &&
      e.setup_ms > e.connected_ms
    ) {
      reasons.push(`episode ${e.episode} setup outlasted its own connection`);
    }
    if (e.result === "installed" && e.replacement === null) {
      reasons.push(
        `episode ${e.episode} was installed without a replacement id`,
      );
    }
  }
  const structural = reasons.slice(incomplete.length > 0 ? 1 : 0);
  return {
    ok: reasons.length === 0,
    reasons,
    structural,
    incomplete: incomplete.length,
  };
}

/// The `interview clock:` lines of one room: the numeric start anchor and the
/// bounded elapsed samples, one per `(pid, session, sample)`. All of them read
/// the assessment clock, which a reconnect does not reset, so the long harness
/// can check the observed duration from the last sample alone.
function parseClock(log, room) {
  const seen = new Set();
  const samples = [];
  for (const line of completeLines(log)) {
    if (!line.startsWith("interview clock: ")) continue;
    const f = lineFields(line, room);
    if (!f) continue;
    const row = {
      pid: whole(f.pid),
      session: whole(f.session),
      startUnixMs: whole(f.start_unix_ms),
      elapsedMs: whole(f.elapsed_ms),
      sample: whole(f.sample),
    };
    if (row.pid === null || row.session === null) continue;
    if (row.elapsedMs === null || row.sample === null) continue;
    const key = `${row.pid}:${row.session}:${row.sample}`;
    if (seen.has(key)) continue;
    seen.add(key);
    samples.push(row);
  }
  return samples;
}

/// Whether the clock samples of one session cover `seconds` of the assessment
/// clock without going backwards. Anchored means the start was logged as a
/// number.
function clockCoverage(samples, seconds) {
  const elapsed = samples.map((s) => s.elapsedMs);
  const monotonic = elapsed.every((ms, i) => i === 0 || ms >= elapsed[i - 1]);
  const max = elapsed.length ? Math.max(...elapsed) : 0;
  return {
    anchored: samples.some((s) => s.startUnixMs !== null),
    monotonic,
    maxElapsedMs: max,
    covers: monotonic && max >= seconds * 1000,
  };
}

// The server keeps this many episode numbers on an owed reply; past it the
// oldest are dropped and `obligation_episode_count` is the true number.
const EPISODES_KEPT = 8;
const LATENCY_PREFIX = "codetrial latency ";
const TERMINAL_OUTCOMES = ["answered", "superseded", "ended"];

/// The owed replies of one room from its `reply_latency` records, keyed by
/// `(pid, session, obligation)`; one entry per reply however many records and
/// copies of them the logs hold. A record is read by the room field it
/// carries, so another room's reply with the same number is never this one's,
/// and only complete lines are read, so a cut record is a missing one.
///
/// Records are ordered by the ids they carry (socket, episode) and the
/// assessment clock, never by when a poller happened to read them: a rolling
/// window and a second copy of the log make the order of reading meaningless.
function parseObligations(log, room) {
  const seen = new Set();
  const obligations = new Map();
  for (const line of completeLines(log)) {
    const at = line.indexOf(LATENCY_PREFIX);
    if (at < 0) continue;
    const json = line.slice(at + LATENCY_PREFIX.length);
    if (seen.has(json)) continue;
    let record;
    try {
      record = JSON.parse(json);
    } catch {
      continue;
    }
    if (record?.event !== "reply_latency" || record.room !== room) continue;
    seen.add(json);
    const pid = whole(String(record.pid));
    const session = whole(String(record.session));
    const obligation = whole(String(record.obligation));
    if (pid === null || session === null || obligation === null) continue;
    const key = `${pid}:${session}:${obligation}`;
    if (!obligations.has(key)) {
      obligations.set(key, {
        pid,
        session,
        obligation,
        origin: record.origin ?? null,
        episodes: new Set(),
        episodeCount: 0,
        outcome: "missing",
        conflict: false,
        firstAcceptSocket: null,
        records: 0,
      });
    }
    const o = obligations.get(key);
    o.records += 1;
    const listed = Array.isArray(record.obligation_episodes)
      ? record.obligation_episodes
      : [];
    for (const e of listed) {
      if (Number.isSafeInteger(e)) o.episodes.add(e);
    }
    o.episodeCount = Math.max(
      o.episodeCount,
      record.obligation_episode_count ?? 0,
    );
    if (
      o.firstAcceptSocket === null &&
      Number.isSafeInteger(record.first_accept_socket)
    ) {
      o.firstAcceptSocket = record.first_accept_socket;
    }
    if (TERMINAL_OUTCOMES.includes(record.obligation_outcome)) {
      if (o.outcome !== "missing" && o.outcome !== record.obligation_outcome) {
        o.conflict = true;
      }
      if (o.outcome === "missing") o.outcome = record.obligation_outcome;
    }
  }
  return obligations;
}

/// The complete lines of `log` that the per-episode judge reads for `room`:
/// its `replacement start:` and `replacement end:` lines and its `reply_latency`
/// records. A caller that polls a rolling window keeps these as they go by.
function episodeEvidenceLines(log, room) {
  const marker = `"room":${JSON.stringify(room)}`;
  return completeLines(log).filter(
    (line) =>
      ((line.startsWith("replacement start: ") ||
        line.startsWith("replacement end: ")) &&
        line.endsWith(` room=${room}`)) ||
      (line.includes(LATENCY_PREFIX) && line.includes(marker)),
  );
}

/// What happened to the reply each replacement episode owed, one verdict per
/// episode: `pass`, `fail`, `uncovered` (the log cannot say) or, before
/// `final`, `pending` while a reply may still be on its way.
///
/// An installed episode that began with a reply owed passes only when that
/// same reply (same pid, session and obligation) was answered with audio first
/// accepted on a socket at or after the one the episode installed. Audio taken
/// on the replaced socket, a reply owed by another episode, another session
/// or another room, and an earlier recovery's answer are different replies or
/// sockets and satisfy nothing. An episode that began with nothing owed was
/// idle (the interviewer was waiting on the candidate or thinking) and is
/// classified, not required to speak; an owed reply that a hold, cut or
/// barge-in ended before any audio is classified the same way.
function judgeEpisodeReplies(
  { episodes, unmatched },
  obligations,
  final = true,
) {
  const results = [];
  const retriedBy = (e) =>
    episodes.find(
      (o) =>
        o.pid === e.pid &&
        o.session === e.session &&
        o.episode > e.episode &&
        o.oldSocket === e.oldSocket &&
        o.result === "installed",
    );
  for (const e of episodes) {
    const row = {
      pid: e.pid,
      session: e.session,
      episode: e.episode,
      trigger: e.trigger,
      result: e.result,
      status: "pass",
      class: "",
      reasons: [],
    };
    const mark = (status, cls, reason) => {
      row.status = status;
      row.class = cls;
      row.reasons.push(reason);
    };
    const owed = e.obligation;
    const o = obligations.get(`${e.pid}:${e.session}:${owed}`);
    if (e.result === "incomplete") {
      if (final) {
        mark(
          "uncovered",
          "truncated",
          "replacement start has no end line: the log was cut or the wait never resolved",
        );
      } else {
        mark("pending", "unresolved", "replacement has not ended yet");
      }
    } else if (e.result === "cancelled") {
      mark(
        "uncovered",
        "cancelled",
        "a terminal intent cancelled the replacement before a socket was installed",
      );
    } else if (e.result !== "installed") {
      const retry = retriedBy(e);
      if (retry) {
        mark(
          "pass",
          "retried",
          `no socket installed (${e.result}); episode ${retry.episode} replaced the same socket`,
        );
      } else {
        mark(
          "fail",
          "not_replaced",
          `no socket was installed (${e.result}) and no later episode replaced socket ${e.oldSocket}`,
        );
      }
    } else if (owed === null) {
      mark(
        "pass",
        "idle",
        "no reply was owed at the swap; an idle or thinking wait is not forced to speak",
      );
    } else if (!o) {
      if (final) {
        mark(
          "fail",
          "no_reply_record",
          `no reply_latency record for obligation ${owed} in this session: the latest reply is missing or the log was truncated`,
        );
      } else {
        mark(
          "pending",
          "awaiting_reply",
          `obligation ${owed} has no record yet`,
        );
      }
    } else if (o.conflict) {
      mark(
        "fail",
        "conflict",
        `obligation ${owed} logged more than one terminal outcome`,
      );
    } else if (o.episodeCount <= EPISODES_KEPT && !o.episodes.has(e.episode)) {
      mark(
        "fail",
        "wrong_reply",
        `obligation ${owed} does not list episode ${e.episode}, so it was not owed across this swap`,
      );
    } else if (
      o.outcome === "answered" ||
      (o.outcome === "missing" && o.firstAcceptSocket !== null)
    ) {
      // Audio accepted is the debt paid, whether or not the turn has ended.
      if (o.firstAcceptSocket === null || e.newSocket === null) {
        mark(
          "fail",
          "no_accept_socket",
          `obligation ${owed} was answered but no accepted-audio socket or installed socket was logged`,
        );
      } else if (o.firstAcceptSocket < e.newSocket) {
        mark(
          "fail",
          "pre_rotation_audio",
          `obligation ${owed} first accepted audio on socket ${o.firstAcceptSocket}, before the swap to socket ${e.newSocket}`,
        );
      } else {
        mark(
          "pass",
          "answered",
          `obligation ${owed} answered with audio first accepted on socket ${o.firstAcceptSocket}, at or after the swap to socket ${e.newSocket}`,
        );
      }
    } else if (o.outcome === "superseded") {
      mark(
        "pass",
        "held",
        `obligation ${owed} was ended locally (hold, cut or barge-in) before any audio, not forced to speak`,
      );
    } else if (o.outcome === "ended") {
      mark(
        "fail",
        "no_audio",
        `obligation ${owed} completed its turn with no audio after the swap`,
      );
    } else if (final) {
      mark(
        "fail",
        "no_terminal",
        `obligation ${owed} never reached a terminal record: the latest reply is missing`,
      );
    } else {
      mark("pending", "awaiting_reply", `obligation ${owed} is still open`);
    }
    results.push(row);
  }
  const reasons = [];
  if (unmatched.length > 0) {
    reasons.push(`${unmatched.length} replacement end line(s) with no start`);
  }
  for (const r of results) {
    if (r.status === "pass") continue;
    reasons.push(`episode ${r.episode} ${r.status}: ${r.reasons.join("; ")}`);
  }
  const counts = { pass: 0, fail: 0, uncovered: 0, pending: 0 };
  for (const r of results) counts[r.status] += 1;
  // An end line read before its start may only be ahead of it in the poll.
  const orphans = unmatched.length > 0;
  const bad = counts.fail + counts.uncovered > 0 || (orphans && final);
  const verdict = bad
    ? "fail"
    : counts.pending > 0 || orphans
      ? "pending"
      : "pass";
  return {
    verdict,
    ok: verdict === "pass",
    episodes: results,
    counts,
    reasons,
  };
}

/// The swaps of `room` that installed a socket, as "pid:session:episode", from
/// the complete `replacement end:` lines of `log`, each once. Cheap enough to
/// run on every few hundred milliseconds of new log, which is what lets the
/// page mark a swap soon after it happens.
function installedSwapKeys(log, room) {
  const keys = new Set();
  for (const line of completeLines(log)) {
    if (!line.startsWith("replacement end: ")) continue;
    const f = lineFields(line, room);
    if (!f || f.result !== "installed") continue;
    const ids = [whole(f.pid), whole(f.session), whole(f.episode)];
    if (ids.some((id) => id === null)) continue;
    keys.add(ids.join(":"));
  }
  return [...keys];
}

/// The identified `replacement:` lines that no installed episode accounts
/// for. A swap whose start and end lines were never captured would otherwise be
/// judged by the other episodes alone.
function replacementsWithoutEpisode(replacements, episodes) {
  return replacements.filter(
    (r) =>
      !r.legacy &&
      !episodes.some(
        (e) =>
          e.pid === r.pid &&
          e.session === r.session &&
          e.episode === r.episode &&
          e.result === "installed",
      ),
  );
}

/// Whether `replacement` is the proactive handover a GoAway asked for: the
/// conversation continued, from a handle or by the designed cold fallback.
function isRotation(replacement) {
  return (
    replacement.trigger === "goaway" && replacement.handover !== "degraded"
  );
}

const VERDICT_MODES = ["strict", "pilot"];

/// The verdict mode named by `value`: `strict` when unset or empty, else one of
/// VERDICT_MODES. Anything else throws, so a mistyped mode cannot quietly turn
/// a qualification run into a pilot.
function parseVerdictMode(value) {
  const mode = value === undefined || value === "" ? "strict" : value;
  if (!VERDICT_MODES.includes(mode)) {
    throw new Error(
      `BROWSER_CHECK_SOAK_VERDICT_MODE must be strict or pilot, not ${JSON.stringify(mode)}`,
    );
  }
  return mode;
}

/// What the replacements of one soak were, counted by what caused them.
///
/// Only a degraded watchdog rebuild is a rebuild from local state after a reply
/// timeout (`watchdogRebuildsTotal`), and only one whose fallback a stalled
/// provider can account for counts against the strict cap
/// (`strictCapEligibleTotal`). A natural GoAway swap is a rotation, not a
/// stall, so it is counted apart and never enters either number. A watchdog
/// swap that resumed the session kept the conversation and is reported on its
/// own.
function tallyReplacements(replacements) {
  const tally = {
    watchdogRebuildsTotal: 0,
    strictCapEligibleTotal: 0,
    watchdogResumedTotal: 0,
    watchdogSwapsTotal: 0,
    goAwaySwapsTotal: 0,
  };
  for (const r of replacements) {
    if (r.trigger === "goaway") tally.goAwaySwapsTotal += 1;
    if (r.trigger !== "watchdog") continue;
    tally.watchdogSwapsTotal += 1;
    if (r.handover === "resumed") tally.watchdogResumedTotal += 1;
    if (r.handover !== "degraded") continue;
    tally.watchdogRebuildsTotal += 1;
    if (STALL_FALLBACKS.has(r.fallback)) tally.strictCapEligibleTotal += 1;
  }
  return tally;
}

/// pass, fail or inconclusive, with the reasons behind a failure; `pending`
/// before `final`, when nothing has failed yet.
///
/// A run in which Gemini sent no GoAway tested nothing about rotation, so it is
/// neither a pass nor a failure of it. A GoAway that was not followed by a
/// handover is a failure, and so is any rebuild from local state that the
/// provider's stalls do not account for.
///
/// `mode` decides what the one-watchdog cap is worth. `strict` is the
/// qualification: more than MAX_WATCHDOG_REBUILDS permitted rebuilds fails.
/// `pilot` leaves the cap out of `verdict`, which is then the structural
/// verdict, and reports both outcomes with `qualification: "not_evaluated"`, so
/// a pilot that completed cannot be read as having qualified. `strictVerdict`
/// and `structuralVerdict` are always both present.
function judgeSoak({
  replacements,
  goAway,
  expired,
  final = true,
  mode = "strict",
}) {
  parseVerdictMode(mode);
  const structural = [];
  const cap = [];
  const tally = tallyReplacements(replacements);
  for (const r of replacements) {
    if (r.handover !== "degraded") continue;
    if (r.trigger === "watchdog" && STALL_FALLBACKS.has(r.fallback)) continue;
    structural.push(
      `rebuilt from local state on trigger=${r.trigger} fallback=${r.fallback} at=${r.at}`,
    );
  }
  if (tally.strictCapEligibleTotal > MAX_WATCHDOG_REBUILDS) {
    cap.push(
      `${tally.strictCapEligibleTotal} rebuilds from local state after reply timeouts, more than the ${MAX_WATCHDOG_REBUILDS} a stalled provider can account for`,
    );
  }
  if (expired) {
    structural.push(
      "the server closed the socket before a held GoAway was spent",
    );
  }
  const decide = (reasons) => {
    if (reasons.length > 0) return { verdict: "fail", reasons };
    // Mid-run, no failure is not yet a result: the GoAway's replacement may be
    // up to its notice away, and a run that has seen none may still see one.
    if (!final) return { verdict: "pending", reasons: [] };
    if (!goAway) {
      return {
        verdict: "inconclusive",
        reasons: ["no transport restart was requested during the soak"],
      };
    }
    if (!replacements.some(isRotation)) {
      return {
        verdict: "fail",
        reasons: ["a GoAway was seen but no handover followed it"],
      };
    }
    return { verdict: "pass", reasons: [] };
  };
  const strict = decide([...structural, ...cap]);
  const structuralOnly = decide(structural);
  const chosen = mode === "pilot" ? structuralOnly : strict;
  return {
    verdict: chosen.verdict,
    reasons: chosen.reasons,
    mode,
    strictVerdict: strict.verdict,
    structuralVerdict: structuralOnly.verdict,
    capReasons: cap,
    qualification: mode === "pilot" ? "not_evaluated" : strict.verdict,
    ...tally,
    diagnostics:
      tally.watchdogResumedTotal > 0
        ? [
            `${tally.watchdogResumedTotal} watchdog swap(s) resumed the session; reported apart from the rebuild count and earning no rotation credit`,
          ]
        : [],
  };
}

/// The `interview clock:` lines of one room, kept as they go by for the same
/// reason as the episode evidence.
function clockEvidenceLines(log, room) {
  return completeLines(log).filter(
    (line) =>
      line.startsWith("interview clock: ") && line.endsWith(` room=${room}`),
  );
}

/// The exposure a soak can vouch for, in seconds of the assessment clock, from
/// the clock samples the runtime logged. Anything the samples cannot establish
/// (none logged, no numeric anchor, a clock that goes backwards, no elapsed
/// time) is `null` with the reason: unknown is not zero, and a rate computed
/// from an unknown exposure is not a rate.
function soakExposure(samples) {
  if (samples.length === 0) {
    return { exposure_s: null, basis: "no_clock_samples" };
  }
  const scopes = new Map();
  for (const s of samples) {
    const scope = `${s.pid}:${s.session}`;
    if (!scopes.has(scope)) scopes.set(scope, []);
    scopes.get(scope).push(s);
  }
  let max = 0;
  for (const rows of scopes.values()) {
    const coverage = clockCoverage(rows, 0);
    if (!coverage.anchored) {
      return { exposure_s: null, basis: "clock_not_anchored" };
    }
    if (!coverage.monotonic) {
      return { exposure_s: null, basis: "clock_not_monotonic" };
    }
    max = Math.max(max, coverage.maxElapsedMs);
  }
  if (!(max > 0)) return { exposure_s: null, basis: "no_elapsed_time" };
  return { exposure_s: max / 1000, basis: "interview_clock" };
}

/// Whether one re-armed sample is evidence of audio after its swap: decoded,
/// with a counter pair of its own, and its window opened on a Speaking
/// transition that came after the swap was marked. A sample that carries no
/// such transition cannot be told from one taken before the swap, so it is not
/// evidence.
function isFreshDecodedSample(r) {
  return (
    r.status === "ok" &&
    Number.isSafeInteger(r.edgesAtMark) &&
    Number.isSafeInteger(r.windowEdge) &&
    r.windowEdge > r.edgesAtMark &&
    r.counterPairs > 0 &&
    r.bytesReceived > 0 &&
    r.packetsReceived > 0 &&
    Number.isFinite(r.peakRms) &&
    r.peakRms > 0
  );
}

const SWAP_AUDIO_OK = ["not_required", "verified", "covered_by_later_swap"];

/// Whether the audio the browser decoded after each swap that answered a reply
/// is on record. `replies` is the result of judgeEpisodeReplies and `rearms`
/// the per-swap samples the page took after marking each installed swap
/// (scripts/media-probe.cjs), each keyed by `swap` = "pid:session:episode".
///
/// A swap that was answered with audio needs a fresh decoded sample of its own,
/// or one that a later swap's fresh sample covers when this one's was
/// superseded by it. The sample taken before the swap, however healthy, covers
/// nothing: the track is reused across sockets, so its proof is older than the
/// socket under test. A swap that owed nothing, or whose reply was held, is not
/// made to speak. This is a decoded signal on the receiving track and says
/// nothing about physical audibility or the delay before it began.
function judgeSwapAudio(replies, rearms = []) {
  const rows = [];
  for (const e of replies.episodes) {
    const swap = `${e.pid}:${e.session}:${e.episode}`;
    const row = {
      swap,
      episode: e.episode,
      status: "not_required",
      reasons: [],
    };
    rows.push(row);
    if (e.status !== "pass" || e.class !== "answered") continue;
    const fail = (status, reason) => {
      row.status = status;
      row.reasons.push(reason);
    };
    const own = rearms.filter((r) => r.swap === swap);
    if (own.length === 0) {
      fail(
        "missing_sample",
        `no post-swap audio sample was taken for episode ${e.episode}`,
      );
    } else if (own.some(isFreshDecodedSample)) {
      row.status = "verified";
    } else if (own.some((r) => r.status === "ok")) {
      fail(
        "stale_sample",
        `episode ${e.episode} has a sample whose window did not open on a Speaking transition after the swap`,
      );
    } else {
      const seq = Math.min(...own.map((r) => r.seq ?? Infinity));
      const superseded = own.every((r) => r.status === "superseded");
      const covering = rearms.find(
        (r) => (r.seq ?? -Infinity) > seq && isFreshDecodedSample(r),
      );
      const silent = own.find((r) => /silent/.test(r.status));
      if (superseded && covering) {
        row.status = "covered_by_later_swap";
      } else if (silent) {
        fail(
          "silent",
          `episode ${e.episode} reached the track but not the decoder: ${silent.status} (${silent.reason})`,
        );
      } else {
        fail(
          superseded ? "superseded_uncovered" : "unavailable",
          `episode ${e.episode} audio sample ${own[0].status} (${own[0].reason ?? "no reason"}) and no later swap's sample covers it`,
        );
      }
    }
  }
  const failed = rows.filter((r) => !SWAP_AUDIO_OK.includes(r.status));
  return {
    ok: failed.length === 0,
    episodes: rows,
    reasons: failed.flatMap((r) => r.reasons),
  };
}

/// What a soak writes about itself. Every field is the verdict of a part, so a
/// reader cannot mistake a pilot that completed for a qualification: in pilot
/// mode `qualification` is `not_evaluated` whatever `verdict` says.
///
/// `replies` and `audio` are the per-episode judgements, or `null` when the
/// soak stopped before they were made. Both gate the structural verdict, in
/// either mode; only the watchdog cap differs.
function soakResult({ soak, replies, audio, exposure }) {
  const partsFailed =
    (replies !== null && !replies.ok) || (audio !== null && !audio.ok);
  const structural =
    partsFailed || soak.structuralVerdict === "fail"
      ? "fail"
      : soak.structuralVerdict;
  const strict =
    partsFailed || soak.strictVerdict === "fail" ? "fail" : soak.strictVerdict;
  const verdict = soak.mode === "pilot" ? structural : strict;
  return {
    mode: soak.mode,
    verdict,
    structural_verdict: structural,
    strict_verdict: strict,
    qualification: soak.mode === "pilot" ? "not_evaluated" : strict,
    watchdog_rebuilds_total: soak.watchdogRebuildsTotal,
    strict_cap_eligible_total: soak.strictCapEligibleTotal,
    watchdog_resumed_total: soak.watchdogResumedTotal,
    goaway_swaps_total: soak.goAwaySwapsTotal,
    exposure_s: exposure.exposure_s,
    exposure_basis: exposure.basis,
    reasons: soak.reasons,
    cap_reasons: soak.capReasons,
    diagnostics: soak.diagnostics,
    replies: replies && {
      verdict: replies.verdict,
      counts: replies.counts,
      episodes: replies.episodes,
    },
    swap_audio: audio,
  };
}

/// Whether `log` has a complete line of this room that holds `needle`. The
/// advisory and its expiry are logged with the room, and a log that carries
/// other rooms' must not read as this one's.
function roomLogged(log, needle, room) {
  return completeLines(log).some(
    (line) => line.includes(needle) && line.includes(`room=${room}`),
  );
}

module.exports = {
  EPISODES_KEPT,
  MAX_WATCHDOG_REBUILDS,
  VERDICT_MODES,
  clockCoverage,
  clockEvidenceLines,
  completeLines,
  episodeEvidenceLines,
  installedSwapKeys,
  isRotation,
  judgeEpisodes,
  judgeSoak,
  judgeSwapAudio,
  judgeEpisodeReplies,
  parseClock,
  parseVerdictMode,
  parseEpisodes,
  parseObligations,
  parseReplacements,
  replacementKey,
  replacementsWithoutEpisode,
  roomLogged,
  soakExposure,
  soakResult,
  tallyReplacements,
};
