// Named logs for the per-episode reply judge in scripts/soak-judge.cjs, each
// with the verdict declared before it is judged. The list is closed on
// purpose: tests/browser/soak-judge.test.js asserts how many there are, so a
// fixture that stops being discovered fails the run instead of shrinking it.

export const ROOM = "interview-browser-1";

const PID = 7;

/// A `replacement start:` line.
export function start(episode, f = {}) {
  const o = {
    session: 1,
    trigger: "goaway",
    oldSocket: episode,
    obligation: "none",
    origin: "none",
    advisories: 1,
    room: ROOM,
    ...f,
  };
  return `replacement start: pid=${PID} session=${o.session} episode=${episode} trigger=${o.trigger} old_socket=${o.oldSocket} obligation=${o.obligation} obligation_origin=${o.origin} advisories=${o.advisories} interview_ms=${episode * 1000} room=${o.room}\n`;
}

/// A `replacement end:` line. An installed episode takes the socket after the
/// one it replaced unless `newSocket` says otherwise.
export function end(episode, result, f = {}) {
  const o = {
    session: 1,
    trigger: "goaway",
    oldSocket: episode,
    obligation: "none",
    room: ROOM,
    ...f,
  };
  const installed = result === "installed";
  const newSocket = installed ? (f.newSocket ?? o.oldSocket + 1) : "none";
  const replacement = installed ? episode : "none";
  return `replacement end: pid=${PID} session=${o.session} episode=${episode} result=${result} trigger=${o.trigger} old_socket=${o.oldSocket} new_socket=${newSocket} replacement=${replacement} attempt=1 resume_tried=true obligation=${o.obligation} notice_ms=900 setup_ms=200 connected_ms=1150 opener_ms=1400 install_ms=1410 interview_ms=${episode * 1000 + 1410} room=${o.room}\n`;
}

/// A `reply_latency` record of the owed reply `obligation`. `outcome` is
/// `open` before the terminal record; `episodes` are the replacement episodes
/// that began while it was owed.
export function reply(obligation, outcome, f = {}) {
  const o = {
    session: 1,
    episodes: [],
    acceptSocket: null,
    room: ROOM,
    stage: outcome === "open" ? "socket_replaced" : "turn_complete",
    ...f,
  };
  const record = {
    event: "reply_latency",
    version: 1,
    pid: PID,
    room: o.room,
    session: o.session,
    socket: o.acceptSocket ?? 1,
    turn: obligation,
    stage: o.stage,
    origin: "prompt",
    obligation,
    obligation_outcome: outcome,
    interview_ms: 5000,
    obligation_episodes: o.episodes,
    obligation_episode_count: o.episodes.length,
    first_accept_ms: o.acceptSocket === null ? null : 120,
    first_accept_socket: o.acceptSocket,
  };
  return `codetrial latency ${JSON.stringify(record)}\n`;
}

/// One episode that swapped sockets while `obligation` was owed, then the
/// owed reply's terminal record.
export function swapped(episode, obligation, outcome, acceptSocket, f = {}) {
  return (
    start(episode, { obligation, origin: "prompt", ...f }) +
    reply(obligation, "open", { episodes: [episode], ...f }) +
    end(episode, "installed", { obligation, ...f }) +
    reply(obligation, outcome, {
      episodes: [episode],
      acceptSocket,
      ...f,
    })
  );
}

const status = (rows) => rows.map(([episode, s, cls]) => [episode, s, cls]);

export const FIXTURES = [
  {
    name: "repeated_notices_and_one_valid_swap_pass",
    log:
      start(1, { obligation: 3, origin: "prompt", advisories: 9 }) +
      reply(3, "open", { episodes: [1] }) +
      end(1, "installed", { obligation: 3 }) +
      reply(3, "answered", { episodes: [1], acceptSocket: 2 }),
    expect: {
      verdict: "pass",
      episodes: status([[1, "pass", "answered"]]),
    },
  },
  {
    name: "swap_with_no_reply_owed_is_classified_idle",
    log: start(1) + end(1, "installed"),
    expect: { verdict: "pass", episodes: status([[1, "pass", "idle"]]) },
  },
  {
    name: "reply_ended_by_a_hold_is_classified_not_forced",
    log: swapped(1, 4, "superseded", null),
    expect: { verdict: "pass", episodes: status([[1, "pass", "held"]]) },
  },
  {
    name: "two_socket_episodes_with_only_the_first_swapped_fail",
    log:
      swapped(1, 3, "answered", 2) +
      start(2, { obligation: 4, origin: "prompt" }) +
      reply(4, "open", { episodes: [2] }) +
      end(2, "failed", { obligation: 4 }),
    expect: {
      verdict: "fail",
      episodes: status([
        [1, "pass", "answered"],
        [2, "fail", "not_replaced"],
      ]),
    },
  },
  {
    name: "second_episode_with_a_truncated_log_is_uncovered",
    log:
      swapped(1, 3, "answered", 2) +
      start(2, { obligation: 4, origin: "prompt" }) +
      "replacement end: pid=7",
    expect: {
      verdict: "fail",
      episodes: status([
        [1, "pass", "answered"],
        [2, "uncovered", "truncated"],
      ]),
    },
  },
  {
    name: "latest_swap_whose_reply_never_finished_fails",
    log:
      swapped(1, 3, "answered", 2) +
      start(2, { obligation: 4, origin: "prompt" }) +
      reply(4, "open", { episodes: [2] }) +
      end(2, "installed", { obligation: 4 }),
    expect: {
      verdict: "fail",
      episodes: status([
        [1, "pass", "answered"],
        [2, "fail", "no_terminal"],
      ]),
    },
  },
  {
    name: "reply_record_cut_mid_line_is_a_missing_reply",
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      end(1, "installed", { obligation: 3 }) +
      reply(3, "answered", { episodes: [1], acceptSocket: 2 }).trimEnd(),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "no_reply_record"]]),
    },
  },
  {
    name: "audio_accepted_before_the_rotation_does_not_satisfy_the_swap",
    log: swapped(1, 5, "answered", 1),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "pre_rotation_audio"]]),
    },
  },
  {
    name: "earlier_watchdog_recovery_does_not_satisfy_a_later_swap",
    log:
      swapped(1, 5, "answered", 2, { trigger: "watchdog" }) +
      start(2, { obligation: 6, origin: "prompt" }) +
      reply(6, "open", { episodes: [2] }) +
      end(2, "installed", { obligation: 6, oldSocket: 2 }),
    expect: {
      verdict: "fail",
      episodes: status([
        [1, "pass", "answered"],
        [2, "fail", "no_terminal"],
      ]),
    },
  },
  {
    name: "another_rooms_answer_to_the_same_obligation_is_ignored",
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      end(1, "installed", { obligation: 3 }) +
      reply(3, "answered", {
        room: "other-room",
        episodes: [1],
        acceptSocket: 2,
      }),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "no_reply_record"]]),
    },
  },
  {
    name: "another_sessions_answer_to_the_same_obligation_is_ignored",
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      end(1, "installed", { obligation: 3 }) +
      reply(3, "answered", { session: 2, episodes: [1], acceptSocket: 2 }),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "no_reply_record"]]),
    },
  },
  {
    name: "reply_that_does_not_list_the_episode_was_not_owed_across_it",
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      end(1, "installed", { obligation: 3 }) +
      reply(3, "answered", { episodes: [9], acceptSocket: 2 }),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "wrong_reply"]]),
    },
  },
  {
    name: "turn_completed_without_audio_after_the_swap_fails",
    log: swapped(1, 3, "ended", null),
    expect: { verdict: "fail", episodes: status([[1, "fail", "no_audio"]]) },
  },
  {
    name: "failed_setup_retried_on_the_same_socket_passes",
    log:
      start(1) +
      end(1, "failed", { oldSocket: 1 }) +
      start(2, { oldSocket: 1 }) +
      end(2, "installed", { oldSocket: 1 }),
    expect: {
      verdict: "pass",
      episodes: status([
        [1, "pass", "retried"],
        [2, "pass", "idle"],
      ]),
    },
  },
  {
    name: "reply_carried_across_two_episodes_is_answered_on_the_last_socket",
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      reply(3, "open", { episodes: [1] }) +
      end(1, "installed", { obligation: 3 }) +
      start(2, { obligation: 3, origin: "prompt" }) +
      reply(3, "open", { episodes: [1, 2] }) +
      end(2, "installed", { obligation: 3 }) +
      reply(3, "answered", { episodes: [1, 2], acceptSocket: 3 }),
    expect: {
      verdict: "pass",
      episodes: status([
        [1, "pass", "answered"],
        [2, "pass", "answered"],
      ]),
    },
  },
  {
    name: "cancelled_episode_is_uncovered_not_passed",
    log: start(1) + end(1, "cancelled"),
    expect: {
      verdict: "fail",
      episodes: status([[1, "uncovered", "cancelled"]]),
    },
  },
  {
    name: "audio_accepted_on_the_new_socket_with_the_turn_still_running_passes",
    log: swapped(1, 3, "open", 2),
    expect: { verdict: "pass", episodes: status([[1, "pass", "answered"]]) },
  },
  {
    name: "audio_accepted_on_the_old_socket_with_the_turn_still_running_fails",
    log: swapped(1, 3, "open", 1),
    expect: {
      verdict: "fail",
      episodes: status([[1, "fail", "pre_rotation_audio"]]),
    },
  },
  {
    name: "episode_end_with_no_start_fails",
    log: end(4, "installed"),
    expect: { verdict: "fail", episodes: [] },
  },
  {
    name: "open_reply_before_the_end_is_pending_not_failed",
    final: false,
    log:
      start(1, { obligation: 3, origin: "prompt" }) +
      reply(3, "open", { episodes: [1] }) +
      end(1, "installed", { obligation: 3 }),
    expect: {
      verdict: "pending",
      episodes: status([[1, "pending", "awaiting_reply"]]),
    },
  },
];

/// The number of fixtures the suite must find. Changing the list means
/// changing this number in tests/browser/soak-judge.test.js too.
export const FIXTURE_COUNT = FIXTURES.length;

// Verdict-mode fixtures. Each names a soak by the replacements it logged, with
// the verdict of both modes and the four counters declared before it is judged.
// The list is closed like the one above: tests/browser/soak-judge.test.js
// asserts how many there are.

/// An identified `replacement:` line of swap `n`.
export function swap(n, trigger, handover, fallback = "none") {
  return `replacement: at=${n}:00.000 resumed=${handover === "resumed"} trigger=${trigger} handover=${handover} fallback=${fallback} rotation_gap_ms=500 checkpoint_age=none owed=true (candidate=false) owed_chars=0 editor_chars=0 pid=${PID} session=1 episode=${n} replacement=${n} old_socket=${n} new_socket=${n + 1} obligation=none interview_ms=${n * 1000} room=${ROOM}\n`;
}

const stall = (n, fallback = "no_handle") =>
  swap(n, "watchdog", "degraded", fallback);
const rotation = (n) => swap(n, "goaway", "resumed");
const counts = (watchdog, eligible, resumed, goaway) => ({
  watchdog_rebuilds_total: watchdog,
  strict_cap_eligible_total: eligible,
  watchdog_resumed_total: resumed,
  goaway_swaps_total: goaway,
});

export const VERDICT_FIXTURES = [
  {
    name: "zero_watchdog_rebuilds_with_one_rotation",
    log: rotation(1),
    expect: { strict: "pass", structural: "pass", counts: counts(0, 0, 0, 1) },
  },
  {
    name: "one_permitted_watchdog_rebuild_stays_under_the_cap",
    log: stall(1) + rotation(2),
    expect: { strict: "pass", structural: "pass", counts: counts(1, 1, 0, 1) },
  },
  {
    name: "two_permitted_watchdog_rebuilds_exceed_only_the_cap",
    log: stall(1) + stall(2, "resume_suppressed") + rotation(3),
    expect: { strict: "fail", structural: "pass", counts: counts(2, 2, 0, 1) },
  },
  {
    name: "many_watchdog_rebuilds_are_all_counted",
    log: stall(1) + stall(2) + stall(3, "resume_suppressed") + rotation(4),
    expect: { strict: "fail", structural: "pass", counts: counts(3, 3, 0, 1) },
  },
  {
    name: "natural_goaway_swaps_never_count_as_watchdog_rebuilds",
    log:
      rotation(1) +
      swap(2, "goaway", "cold_handoff", "checkpoint_before_handoff") +
      rotation(3) +
      stall(4),
    expect: { strict: "pass", structural: "pass", counts: counts(1, 1, 0, 3) },
  },
  {
    name: "degraded_goaway_is_structural_and_not_a_watchdog_rebuild",
    log: swap(1, "goaway", "degraded", "no_handle") + rotation(2),
    expect: { strict: "fail", structural: "fail", counts: counts(0, 0, 0, 2) },
  },
  {
    name: "watchdog_rebuild_after_a_refused_handle_is_total_but_not_cap_eligible",
    log: stall(1, "resume_refused") + rotation(2),
    expect: { strict: "fail", structural: "fail", counts: counts(1, 0, 0, 1) },
  },
  {
    name: "resumed_watchdog_swaps_are_diagnosed_apart",
    log:
      swap(1, "watchdog", "resumed") +
      swap(2, "watchdog", "resumed") +
      rotation(3),
    expect: {
      strict: "pass",
      structural: "pass",
      counts: counts(0, 0, 2, 1),
      diagnostics: 1,
    },
  },
  {
    name: "refused_handle_among_permitted_rebuilds_fails_both_modes",
    log: stall(1) + stall(2) + stall(3, "resume_refused") + rotation(4),
    expect: { strict: "fail", structural: "fail", counts: counts(3, 2, 0, 1) },
  },
  {
    name: "cap_exceeded_without_a_goaway_is_inconclusive_in_pilot",
    goAway: false,
    log: stall(1) + stall(2),
    expect: {
      strict: "fail",
      structural: "inconclusive",
      counts: counts(2, 2, 0, 0),
    },
  },
  {
    name: "expired_goaway_fails_both_modes",
    expired: true,
    log: rotation(1),
    expect: { strict: "fail", structural: "fail", counts: counts(0, 0, 0, 1) },
  },
  {
    name: "goaway_without_a_handover_fails_both_modes",
    log: stall(1),
    expect: { strict: "fail", structural: "fail", counts: counts(1, 1, 0, 0) },
  },
];

export const VERDICT_FIXTURE_COUNT = VERDICT_FIXTURES.length;
