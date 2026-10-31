import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import judge from "../../scripts/soak-judge.cjs";
import * as fixtures from "./soak-judge-fixtures.js";

const ROOM = "interview-browser-1";

function line(fields, room = ROOM) {
  const { trigger, handover, fallback = "none", at = "1:00.000" } = fields;
  return `replacement: at=${at} resumed=${handover === "resumed"} trigger=${trigger} handover=${handover} fallback=${fallback} rotation_gap_ms=500 checkpoint_age=none owed=true room=${room}\n`;
}

const parse = (log, room = ROOM) => judge.parseReplacements(log, room);
const verdict = (log, flags = {}) =>
  judge.judgeSoak({
    replacements: parse(log),
    goAway: true,
    expired: false,
    ...flags,
  }).verdict;

test("a replacement line is read by its key=value fields, once", () => {
  const log = line({ trigger: "goaway", handover: "resumed" });
  const found = parse(log + log);
  assert.equal(found.length, 1);
  assert.equal(found[0].trigger, "goaway");
  assert.equal(found[0].handover, "resumed");
  assert.equal(found[0].fallback, "none");
});

test("the same replacement read from two logs is one replacement", () => {
  const fromAgent = line({ trigger: "watchdog", handover: "degraded" });
  const fromServer = fromAgent.replace("owed=true", "owed=true extra=1");
  assert.equal(parse(fromAgent + fromServer).length, 1);
});

test("another room's lines and unknown spellings are left out", () => {
  const other = line({ trigger: "goaway", handover: "resumed" }, "other-room");
  const renamed = line({ trigger: "goaway", handover: "continued" });
  assert.deepEqual(parse(other + renamed), []);
});

test("the first audio line and a cut-short trailing line are not replacements", () => {
  const firstAudio = `replacement first audio: at=1 resumed=true since_start_ms=9 room=${ROOM}\n`;
  const whole = line({ trigger: "goaway", handover: "resumed" });
  assert.deepEqual(parse(firstAudio), []);
  // Without its newline the line may be cut; it is read once it is finished.
  assert.deepEqual(parse(whole.trimEnd()), []);
  assert.equal(parse(whole).length, 1);
});

test("a GoAway followed by a resumed or designed cold handover passes", () => {
  assert.equal(
    verdict(line({ trigger: "goaway", handover: "resumed" })),
    "pass",
  );
  assert.equal(
    verdict(
      line({
        trigger: "goaway",
        handover: "cold_handoff",
        fallback: "checkpoint_before_handoff",
      }),
    ),
    "pass",
  );
});

test("a GoAway rebuilt from local state without the designed reason fails", () => {
  const result = judge.judgeSoak({
    replacements: parse(
      line({ trigger: "goaway", handover: "degraded", fallback: "no_handle" }),
    ),
    goAway: true,
    expired: false,
  });
  assert.equal(result.verdict, "fail");
  assert.match(result.reasons[0], /trigger=goaway fallback=no_handle/);
});

test("one recovered watchdog rebuild is tolerated, a second is not", () => {
  const rotation = line({
    trigger: "goaway",
    handover: "resumed",
    at: "3:00.000",
  });
  const stall = (at) =>
    line({
      trigger: "watchdog",
      handover: "degraded",
      fallback: "resume_suppressed",
      at,
    });
  assert.equal(verdict(stall("1:00.000") + rotation), "pass");
  assert.equal(
    verdict(stall("1:00.000") + stall("2:00.000") + rotation),
    "fail",
  );
});

test("a watchdog resume earns no rotation credit", () => {
  assert.equal(
    verdict(line({ trigger: "watchdog", handover: "resumed" })),
    "fail",
  );
});

test("a GoAway nobody replaced fails, and no GoAway at all is inconclusive", () => {
  assert.equal(verdict("", { goAway: true }), "fail");
  assert.equal(verdict("", { goAway: false }), "inconclusive");
  assert.equal(
    verdict(line({ trigger: "watchdog", handover: "resumed" }), {
      goAway: false,
    }),
    "inconclusive",
  );
});

test("a failure outranks an absent GoAway", () => {
  const log = line({
    trigger: "close",
    handover: "degraded",
    fallback: "no_handle",
  });
  assert.equal(verdict(log, { goAway: false }), "fail");
});

test("an expired GoAway fails", () => {
  assert.equal(
    verdict(line({ trigger: "goaway", handover: "resumed" }), {
      expired: true,
    }),
    "fail",
  );
});

test("before the end nothing is decided unless something has failed", () => {
  const early = judge.judgeSoak({
    replacements: [],
    goAway: true,
    expired: false,
    final: false,
  });
  assert.equal(early.verdict, "pending");
  const failed = judge.judgeSoak({
    replacements: parse(
      line({ trigger: "close", handover: "degraded", fallback: "no_handle" }),
    ),
    goAway: false,
    expired: false,
    final: false,
  });
  assert.equal(failed.verdict, "fail");
});

test("a room name with spaces is read whole", () => {
  const room = "interview browser 1";
  const log = line({ trigger: "goaway", handover: "resumed" }, room);
  assert.equal(parse(log, room).length, 1);
  assert.equal(parse(log, "interview").length, 0);
});

test("a refused handle after a reply timeout is a defect, not a stall", () => {
  const result = judge.judgeSoak({
    replacements: parse(
      line({
        trigger: "watchdog",
        handover: "degraded",
        fallback: "resume_refused",
      }) + line({ trigger: "goaway", handover: "resumed", at: "3:00.000" }),
    ),
    goAway: true,
    expired: false,
  });
  assert.equal(result.verdict, "fail");
  assert.match(result.reasons[0], /fallback=resume_refused/);
});

test("a stall before the first checkpoint is tolerated like a young socket", () => {
  const log =
    line({ trigger: "watchdog", handover: "degraded", fallback: "no_handle" }) +
    line({ trigger: "goaway", handover: "resumed", at: "3:00.000" });
  assert.equal(verdict(log), "pass");
});

test("the advisory counts only in its own room", () => {
  const mine = `Gemini requested a transport restart in 50s; room=${ROOM}\n`;
  const other = "Gemini requested a transport restart in 50s; room=other\n";
  const needle = "Gemini requested a transport restart in";
  assert.equal(judge.roomLogged(other, needle, ROOM), false);
  assert.equal(judge.roomLogged(other + mine, needle, ROOM), true);
  assert.equal(judge.roomLogged(mine.trimEnd(), needle, ROOM), false);
});

// Lifecycle identity. The legacy `replacement:` line has no ids and is told
// apart by its displayed clock; a line with ids is told apart by them.

const ids = (n, extra = "") =>
  `pid=7 session=1 episode=${n} replacement=${n} old_socket=${n} new_socket=${n + 1} obligation=none interview_ms=${n * 1000}${extra}`;

function identified(fields, n, room = ROOM) {
  const { trigger, handover, fallback = "none", at = "1:00.000" } = fields;
  return `replacement: at=${at} resumed=${handover === "resumed"} trigger=${trigger} handover=${handover} fallback=${fallback} rotation_gap_ms=500 checkpoint_age=none owed=true (candidate=false) owed_chars=0 editor_chars=0 ${ids(n)} room=${room}\n`;
}

const start = (n, extra = {}, room = ROOM) => {
  const f = {
    trigger: "goaway",
    obligation: "none",
    obligation_origin: "none",
    advisories: 0,
    ...extra,
  };
  return `replacement start: pid=7 session=1 episode=${n} trigger=${f.trigger} old_socket=${n} obligation=${f.obligation} obligation_origin=${f.obligation_origin} advisories=${f.advisories} interview_ms=${n * 1000} room=${room}\n`;
};

const end = (n, result, extra = {}, room = ROOM) => {
  const f = {
    trigger: "goaway",
    new_socket: result === "installed" ? n + 1 : "none",
    replacement: result === "installed" ? n : "none",
    resume_tried: "true",
    notice_ms: 900,
    setup_ms: 200,
    connected_ms: 1150,
    opener_ms: 1400,
    install_ms: 1410,
    ...extra,
  };
  return `replacement end: pid=7 session=1 episode=${n} result=${result} trigger=${f.trigger} old_socket=${n} new_socket=${f.new_socket} replacement=${f.replacement} attempt=1 resume_tried=${f.resume_tried} obligation=none notice_ms=${f.notice_ms} setup_ms=${f.setup_ms} connected_ms=${f.connected_ms} opener_ms=${f.opener_ms} install_ms=${f.install_ms} interview_ms=${n * 1000 + 1410} room=${room}\n`;
};

test("two swaps at the same displayed clock are two replacements by id", () => {
  const log =
    identified({ trigger: "goaway", handover: "resumed" }, 1) +
    identified({ trigger: "goaway", handover: "resumed" }, 2);
  const found = parse(log);
  assert.equal(found.length, 2);
  assert.deepEqual(
    found.map((r) => [r.episode, r.replacement, r.legacy]),
    [
      [1, 1, false],
      [2, 2, false],
    ],
  );
  assert.equal(found[0].at, found[1].at);
});

test("the same identified swap read from two logs is one replacement", () => {
  const line = identified({ trigger: "goaway", handover: "resumed" }, 1);
  const again = line.replace("owed=true", "owed=true extra=1");
  assert.equal(parse(line + again).length, 1);
});

test("legacy lines without ids still parse and still dedupe by clock", () => {
  const legacy = line({ trigger: "goaway", handover: "resumed" });
  const found = parse(legacy + legacy);
  assert.equal(found.length, 1);
  assert.equal(found[0].legacy, true);
  assert.equal(found[0].episode, null);
  // A legacy and an identified line at one clock are distinct replacements.
  const both = parse(
    legacy + identified({ trigger: "goaway", handover: "resumed" }, 1),
  );
  assert.equal(both.length, 2);
  // The verdict still reads the legacy fields exactly as before.
  assert.equal(verdict(legacy), "pass");
});

test("a line with only some ids is read as legacy, not as an identity", () => {
  const partial = line({ trigger: "goaway", handover: "resumed" }).replace(
    " room=",
    " episode=1 room=",
  );
  const found = parse(partial + partial);
  assert.equal(found.length, 1);
  assert.equal(found[0].legacy, true);
});

test("every start resolves once: installed, failed, cancelled and no_setup", () => {
  const log =
    start(1) +
    end(1, "installed") +
    start(2, { trigger: "close" }) +
    end(2, "failed", { trigger: "close", setup_ms: "none" }) +
    start(3) +
    end(3, "cancelled") +
    start(4, { trigger: "watchdog" }) +
    end(4, "no_setup", {
      trigger: "watchdog",
      resume_tried: "false",
      notice_ms: "none",
      setup_ms: "none",
      connected_ms: "none",
      opener_ms: "none",
      install_ms: "none",
    });
  const parsed = judge.parseEpisodes(log, ROOM);
  assert.deepEqual(
    parsed.episodes.map((e) => [e.episode, e.result]),
    [
      [1, "installed"],
      [2, "failed"],
      [3, "cancelled"],
      [4, "no_setup"],
    ],
  );
  const installed = parsed.episodes[0];
  assert.equal(installed.replacement, 1);
  assert.equal(installed.newSocket, 2);
  assert.equal(installed.notice_ms, 900);
  assert.equal(installed.setup_ms, 200);
  assert.equal(parsed.episodes[3].setup_ms, null);
  const judged = judge.judgeEpisodes(parsed);
  assert.equal(judged.ok, true, judged.reasons.join("; "));
});

test("a start with no end is incomplete and a truncated line is not read", () => {
  const log =
    start(1) + end(1, "installed") + start(2) + "replacement end: pid=7";
  const parsed = judge.parseEpisodes(log, ROOM);
  assert.equal(parsed.episodes.length, 2);
  assert.equal(parsed.episodes[1].result, "incomplete");
  const judged = judge.judgeEpisodes(parsed);
  assert.equal(judged.ok, false);
  assert.equal(judged.incomplete, 1);
  assert.match(judged.reasons[0], /never reached a result: 2/);
});

test("duplicate episode lines count once and an end with no start is kept visible", () => {
  const log =
    start(1) +
    start(1) +
    end(1, "installed") +
    end(1, "installed") +
    end(9, "failed");
  const parsed = judge.parseEpisodes(log, ROOM);
  assert.equal(parsed.episodes.length, 1);
  assert.equal(parsed.episodes[0].ends, 2);
  assert.equal(parsed.unmatched.length, 1);
  assert.equal(judge.judgeEpisodes(parsed).ok, false);
  // Another room's lines are not this room's.
  assert.deepEqual(
    judge.parseEpisodes(start(1, {}, "other"), ROOM).episodes,
    [],
  );
});

test("episode ids and milestones that run backwards are reported", () => {
  const backwards = judge.judgeEpisodes(
    judge.parseEpisodes(
      start(2) + end(2, "installed") + start(1) + end(1, "installed"),
      ROOM,
    ),
  );
  assert.match(backwards.reasons.join(";"), /episode 1 does not follow 2/);
  const milestones = judge.judgeEpisodes(
    judge.parseEpisodes(
      start(1) + end(1, "installed", { opener_ms: 100, install_ms: 50 }),
      ROOM,
    ),
  );
  assert.match(milestones.reasons.join(";"), /milestones run backwards/);
});

test("the physical setup is kept apart from the notice and the install", () => {
  const parsed = judge.parseEpisodes(
    start(1) + end(1, "installed", { notice_ms: 9000, setup_ms: 150 }),
    ROOM,
  );
  const e = parsed.episodes[0];
  assert.equal(e.notice_ms, 9000);
  assert.equal(e.setup_ms, 150);
  assert.ok(e.notice_ms > e.setup_ms, "notice delay is not setup time");
});

test("repeated advisories keep their raw count on the episode", () => {
  const parsed = judge.parseEpisodes(
    start(1, { advisories: 9, obligation: 3, obligation_origin: "prompt" }) +
      end(1, "installed"),
    ROOM,
  );
  assert.equal(parsed.episodes[0].advisories, 9);
  assert.equal(parsed.episodes[0].obligation, 3);
  assert.equal(parsed.episodes[0].obligationOrigin, "prompt");
});

test("clock samples anchor the run and cover its duration across reconnects", () => {
  const sample = (n, ms, extra = "") =>
    `interview clock: pid=7 session=1 start_unix_ms=1700000000000 elapsed_ms=${ms} sample=${n}${extra} room=${ROOM}\n`;
  const log =
    sample(0, 0) +
    sample(1, 60000) +
    sample(1, 60000) +
    `interview clock: pid=7 session=1 start_unix_ms=none elapsed_ms=x sample=2 room=${ROOM}\n` +
    sample(11, 660000);
  const samples = judge.parseClock(log, ROOM);
  assert.deepEqual(
    samples.map((s) => s.sample),
    [0, 1, 11],
  );
  const covered = judge.clockCoverage(samples, 660);
  assert.deepEqual(
    [covered.anchored, covered.monotonic, covered.covers],
    [true, true, true],
  );
  assert.equal(judge.clockCoverage(samples, 661).covers, false);
  const backwards = judge.clockCoverage(
    judge.parseClock(sample(1, 60000) + sample(2, 1000), ROOM),
    1,
  );
  assert.equal(backwards.monotonic, false);
  assert.equal(backwards.covers, false);
  assert.deepEqual(judge.parseClock(sample(0, 0), "other"), []);
});

test("a setup longer than its own connection is reported", () => {
  const judged = judge.judgeEpisodes(
    judge.parseEpisodes(
      start(1) + end(1, "installed", { setup_ms: 5000, connected_ms: 1150 }),
      ROOM,
    ),
  );
  assert.equal(judged.ok, false);
  assert.match(judged.reasons.join(";"), /setup outlasted its own connection/);
});

test("merging across reads keys replacements by id, not by displayed clock", () => {
  const a = parse(identified({ trigger: "goaway", handover: "resumed" }, 1))[0];
  const b = parse(identified({ trigger: "goaway", handover: "resumed" }, 2))[0];
  assert.equal(a.at, b.at);
  assert.notEqual(judge.replacementKey(a), judge.replacementKey(b));
  const legacy = parse(line({ trigger: "goaway", handover: "resumed" }))[0];
  assert.equal(judge.replacementKey(legacy), `at:${legacy.at}`);
});

test("a copy of a start read after its end does not reopen the episode", () => {
  const parsed = judge.parseEpisodes(
    start(1) + end(1, "installed") + start(1),
    ROOM,
  );
  assert.equal(parsed.episodes.length, 1);
  assert.equal(parsed.episodes[0].result, "installed");
  assert.equal(judge.judgeEpisodes(parsed).ok, true);
});

// Per-episode reply judging. Each fixture declares its verdict and the
// status and class of every episode before it is judged; the count is fixed
// here so a fixture that stops being discovered fails and cannot shrink the
// suite.

const EXPECTED_FIXTURES = 20;

function judgeFixture(fixture) {
  const parsed = judge.parseEpisodes(fixture.log, fixtures.ROOM);
  const obligations = judge.parseObligations(fixture.log, fixtures.ROOM);
  return judge.judgeEpisodeReplies(parsed, obligations, fixture.final ?? true);
}

test("the reply fixtures are all discovered", () => {
  assert.equal(fixtures.FIXTURES.length, EXPECTED_FIXTURES);
  assert.equal(fixtures.FIXTURE_COUNT, EXPECTED_FIXTURES);
  const names = fixtures.FIXTURES.map((f) => f.name);
  assert.equal(new Set(names).size, EXPECTED_FIXTURES, "names are unique");
});

const outcomes = [];
for (const fixture of fixtures.FIXTURES) {
  test(`reply fixture: ${fixture.name}`, () => {
    const judged = judgeFixture(fixture);
    outcomes.push({
      name: fixture.name,
      verdict: judged.verdict,
      episodes: judged.episodes.map((e) => ({
        episode: e.episode,
        status: e.status,
        class: e.class,
        reasons: e.reasons,
      })),
    });
    assert.deepEqual(
      judged.episodes.map((e) => [e.episode, e.status, e.class]),
      fixture.expect.episodes,
      judged.reasons.join("; "),
    );
    assert.equal(judged.verdict, fixture.expect.verdict);
    if (fixture.expect.verdict !== "pass") {
      assert.ok(judged.reasons.length > 0, "a non-pass says why");
    }
  });
}

test("the fixture count is recorded in result.json with no discovery failure", () => {
  const dir = process.env.SOAK_JUDGE_RESULT_DIR;
  const discovered = outcomes.length;
  const result = {
    fixtures_expected: EXPECTED_FIXTURES,
    fixtures_discovered: discovered,
    discovery_failures: Math.abs(EXPECTED_FIXTURES - discovered),
    fixtures: outcomes,
  };
  assert.equal(result.discovery_failures, 0);
  if (dir) {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(
      path.join(dir, "result.json"),
      `${JSON.stringify(result, null, 2)}\n`,
    );
  }
});

test("a reply read from two logs is one reply and a cut record is none", () => {
  const log =
    fixtures.reply(3, "open", { episodes: [1] }) +
    fixtures.reply(3, "open", { episodes: [1] }) +
    fixtures.reply(3, "answered", { episodes: [1], acceptSocket: 2 }) +
    fixtures.reply(4, "answered", { episodes: [], acceptSocket: 2 }).trimEnd();
  const found = judge.parseObligations(log, fixtures.ROOM);
  assert.equal(found.size, 1);
  const o = found.get("7:1:3");
  assert.equal(o.records, 2);
  assert.equal(o.outcome, "answered");
  assert.equal(o.firstAcceptSocket, 2);
  assert.equal(judge.parseObligations(log, "other-room").size, 0);
});

test("a swap judged before its reply is final is pending, then fails if missing", () => {
  const log =
    fixtures.start(1, { obligation: 3, origin: "prompt" }) +
    fixtures.end(1, "installed", { obligation: 3 });
  const parsed = judge.parseEpisodes(log, fixtures.ROOM);
  const none = judge.parseObligations(log, fixtures.ROOM);
  assert.equal(
    judge.judgeEpisodeReplies(parsed, none, false).verdict,
    "pending",
  );
  assert.equal(judge.judgeEpisodeReplies(parsed, none, true).verdict, "fail");
});

test("a logged swap with no installed episode is not covered by the others", () => {
  const swap = identified({ trigger: "goaway", handover: "resumed" }, 2);
  const covered = judge.parseEpisodes(
    fixtures.start(2) + fixtures.end(2, "installed"),
    fixtures.ROOM,
  ).episodes;
  const other = judge.parseEpisodes(
    fixtures.start(1) + fixtures.end(1, "installed"),
    fixtures.ROOM,
  ).episodes;
  const replacements = parse(swap);
  assert.equal(
    judge.replacementsWithoutEpisode(replacements, covered).length,
    0,
  );
  assert.equal(judge.replacementsWithoutEpisode(replacements, other).length, 1);
  // A legacy line has no ids to match and is left to the zero-episode check.
  const legacy = parse(line({ trigger: "goaway", handover: "resumed" }));
  assert.equal(judge.replacementsWithoutEpisode(legacy, other).length, 0);
});

test("an end line ahead of its start is pending until the verdict is final", () => {
  const parsed = judge.parseEpisodes(
    fixtures.end(4, "installed"),
    fixtures.ROOM,
  );
  const none = judge.parseObligations("", fixtures.ROOM);
  assert.equal(
    judge.judgeEpisodeReplies(parsed, none, false).verdict,
    "pending",
  );
  assert.equal(judge.judgeEpisodeReplies(parsed, none, true).verdict, "fail");
});

test("an end line that arrives before its start still closes the episode", () => {
  const log =
    fixtures.end(1, "installed") + fixtures.start(1) + fixtures.start(2);
  const parsed = judge.parseEpisodes(log, fixtures.ROOM);
  assert.deepEqual(parsed.unmatched, []);
  assert.deepEqual(
    parsed.episodes.map((e) => [e.episode, e.result]),
    [
      [1, "installed"],
      [2, "incomplete"],
    ],
  );
});

// Verdict modes, watchdog counting, exposure and the audio decoded after each
// swap. Every row declares its expectation in tests/browser/soak-judge-fixtures.js
// or here before it is judged.

const EXPECTED_VERDICT_FIXTURES = 12;

const judgeMode = (fixture, mode) =>
  judge.judgeSoak({
    replacements: parse(fixture.log),
    goAway: fixture.goAway ?? true,
    expired: fixture.expired ?? false,
    mode,
  });

test("the verdict-mode fixtures are all discovered", () => {
  assert.equal(fixtures.VERDICT_FIXTURES.length, EXPECTED_VERDICT_FIXTURES);
  assert.equal(fixtures.VERDICT_FIXTURE_COUNT, EXPECTED_VERDICT_FIXTURES);
  const names = fixtures.VERDICT_FIXTURES.map((f) => f.name);
  assert.equal(new Set(names).size, EXPECTED_VERDICT_FIXTURES);
});

for (const fixture of fixtures.VERDICT_FIXTURES) {
  test(`verdict fixture: ${fixture.name}`, () => {
    const strict = judgeMode(fixture, "strict");
    const pilot = judgeMode(fixture, "pilot");
    const { expect } = fixture;
    assert.equal(strict.verdict, expect.strict, strict.reasons.join("; "));
    assert.equal(strict.mode, "strict");
    assert.equal(strict.qualification, expect.strict);
    // Pilot's verdict is the structural one, and it still reports the strict
    // outcome beside it; it never claims to have qualified.
    assert.equal(pilot.verdict, expect.structural, pilot.reasons.join("; "));
    assert.equal(pilot.structuralVerdict, expect.structural);
    assert.equal(pilot.strictVerdict, expect.strict);
    assert.equal(pilot.qualification, "not_evaluated");
    assert.equal(strict.structuralVerdict, expect.structural);
    const result = judge.soakResult({
      soak: pilot,
      replies: null,
      audio: null,
      exposure: { exposure_s: null, basis: "no_clock_samples" },
    });
    for (const [name, value] of Object.entries(expect.counts)) {
      assert.equal(result[name], value, name);
    }
    assert.equal(result.mode, "pilot");
    assert.equal(result.qualification, "not_evaluated");
    assert.equal(result.structural_verdict, expect.structural);
    assert.equal(result.strict_verdict, expect.strict);
    assert.equal(
      pilot.diagnostics.length,
      expect.diagnostics ?? 0,
      "resumed watchdog swaps are diagnosed separately",
    );
  });
}

test("the verdict mode is strict unless pilot is named, and nothing else is accepted", () => {
  assert.equal(judge.parseVerdictMode(undefined), "strict");
  assert.equal(judge.parseVerdictMode(""), "strict");
  assert.equal(judge.parseVerdictMode("strict"), "strict");
  assert.equal(judge.parseVerdictMode("pilot"), "pilot");
  for (const bad of ["Pilot", "loose", "none", " pilot"]) {
    assert.throws(() => judge.parseVerdictMode(bad), /strict or pilot/);
  }
  assert.throws(
    () =>
      judge.judgeSoak({
        replacements: [],
        goAway: true,
        expired: false,
        mode: "loose",
      }),
    /strict or pilot/,
  );
});

test("strict keeps the one-watchdog cap and says it qualified or not", () => {
  const f = fixtures.VERDICT_FIXTURES.find((x) =>
    x.name.startsWith("two_permitted"),
  );
  const strict = judgeMode(f, "strict");
  assert.equal(strict.verdict, "fail");
  assert.equal(strict.qualification, "fail");
  assert.match(strict.reasons[0], /2 rebuilds .* more than the 1/);
  assert.equal(strict.capReasons.length, 1);
  const pilot = judgeMode(f, "pilot");
  assert.deepEqual(pilot.reasons, []);
  assert.equal(pilot.capReasons.length, 1, "the cap outcome is still reported");
});

test("a pilot cannot hide a structural failure behind its relaxed cap", () => {
  const f = fixtures.VERDICT_FIXTURES.find((x) =>
    x.name.startsWith("refused_handle_among"),
  );
  for (const mode of ["strict", "pilot"]) {
    const result = judgeMode(f, mode);
    assert.equal(result.verdict, "fail", mode);
    assert.match(result.reasons.join(";"), /fallback=resume_refused/);
  }
});

test("a pilot mid-run does not abort on the cap alone", () => {
  const f = fixtures.VERDICT_FIXTURES.find((x) =>
    x.name.startsWith("many_watchdog"),
  );
  const run = (mode) =>
    judge.judgeSoak({
      replacements: parse(f.log),
      goAway: true,
      expired: false,
      final: false,
      mode,
    }).verdict;
  assert.equal(run("strict"), "fail");
  assert.equal(run("pilot"), "pending");
});

test("interview clock lines are kept per room and complete lines only", () => {
  const sample = (room, tail = "\n") =>
    `interview clock: pid=7 session=1 start_unix_ms=1700000000000 elapsed_ms=60000 sample=1 room=${room}${tail}`;
  const log = sample(ROOM) + sample("other") + sample(ROOM, "");
  assert.deepEqual(judge.clockEvidenceLines(log, ROOM), [
    sample(ROOM, "").trimEnd(),
  ]);
});

function clockSamples(rows) {
  return judge.parseClock(
    rows
      .map(
        ([session, sample, ms, start = "1700000000000"]) =>
          `interview clock: pid=7 session=${session} start_unix_ms=${start} elapsed_ms=${ms} sample=${sample} room=${ROOM}\n`,
      )
      .join(""),
    ROOM,
  );
}

test("exposure is the verified assessment clock, and unknown is never zero", () => {
  assert.deepEqual(judge.soakExposure(clockSamples([[1, 11, 660000]])), {
    exposure_s: 660,
    basis: "interview_clock",
  });
  assert.equal(
    judge.soakExposure(
      clockSamples([
        [1, 1, 60000],
        [1, 12, 720500],
      ]),
    ).exposure_s,
    720.5,
  );
  const unknown = [
    [[], "no_clock_samples"],
    [clockSamples([[1, 1, 60000, "none"]]), "clock_not_anchored"],
    [
      clockSamples([
        [1, 1, 60000],
        [1, 2, 1000],
      ]),
      "clock_not_monotonic",
    ],
    [clockSamples([[1, 0, 0]]), "no_elapsed_time"],
  ];
  for (const [samples, basis] of unknown) {
    const exposure = judge.soakExposure(samples);
    assert.equal(exposure.exposure_s, null, basis);
    assert.equal(exposure.basis, basis);
  }
});

test("a log with no clock lines leaves exposure_s null in the result", () => {
  const soak = judge.judgeSoak({
    replacements: parse(fixtures.swap(1, "goaway", "resumed")),
    goAway: true,
    expired: false,
  });
  const result = judge.soakResult({
    soak,
    replies: null,
    audio: null,
    exposure: judge.soakExposure(judge.parseClock("", ROOM)),
  });
  assert.equal(result.exposure_s, null);
  assert.equal(result.exposure_basis, "no_clock_samples");
  assert.notEqual(result.exposure_s, 0);
});

// The audio decoded after each swap.

const owedSwap = (n) => fixtures.swapped(n, n + 2, "answered", n + 1);
const replyTo = (log) =>
  judge.judgeEpisodeReplies(
    judge.parseEpisodes(log, fixtures.ROOM),
    judge.parseObligations(log, fixtures.ROOM),
  );
const rearm = (n, f = {}) => ({
  swap: `7:1:${n}`,
  seq: n,
  status: "ok",
  reason: null,
  edgesAtMark: 4,
  windowEdge: 5,
  counterPairs: 3,
  bytesReceived: 900,
  packetsReceived: 9,
  peakRms: 0.2,
  ...f,
});
const audioOf = (log, rearms) => judge.judgeSwapAudio(replyTo(log), rearms);
const statuses = (audio) => audio.episodes.map((e) => [e.episode, e.status]);

test("a fresh decoded sample after the swap verifies an answered swap", () => {
  const audio = audioOf(owedSwap(1), [rearm(1)]);
  assert.equal(audio.ok, true);
  assert.deepEqual(statuses(audio), [[1, "verified"]]);
});

test("a sample whose window did not open after the mark is stale and fails", () => {
  for (const stale of [
    { windowEdge: 4 },
    { windowEdge: 3 },
    { windowEdge: null },
    { edgesAtMark: null },
    { windowEdge: undefined },
  ]) {
    const audio = audioOf(owedSwap(1), [rearm(1, stale)]);
    assert.equal(audio.ok, false, JSON.stringify(stale));
    assert.deepEqual(statuses(audio), [[1, "stale_sample"]]);
    assert.match(audio.reasons[0], /did not open on a Speaking transition/);
  }
});

test("a sample with no counter pair or decoded signal is not fresh", () => {
  for (const weak of [
    { counterPairs: 0 },
    { bytesReceived: 0 },
    { packetsReceived: null },
    { peakRms: 0 },
    { peakRms: null },
  ]) {
    const audio = audioOf(owedSwap(1), [rearm(1, weak)]);
    assert.equal(audio.ok, false, JSON.stringify(weak));
  }
});

test("silent or undecodable audio after the swap fails with its reason", () => {
  const cases = [
    ["transport-silent", "audio-packets-did-not-increase", "silent"],
    ["decoded-silent", "packets-arrived-without-decoded-signal", "silent"],
    ["unavailable", "decoded-signal-unavailable", "unavailable"],
    ["unavailable", "receiver-stats-unavailable", "unavailable"],
    ["unavailable", "sampling-cancelled", "unavailable"],
  ];
  for (const [status, reason, expected] of cases) {
    const audio = audioOf(owedSwap(1), [
      rearm(1, { status, reason, peakRms: null }),
    ]);
    assert.deepEqual(statuses(audio), [[1, expected]], reason);
    assert.equal(audio.ok, false);
    assert.match(audio.reasons[0], new RegExp(reason));
  }
});

test("a healthy sample from before the swap on a reused track covers nothing", () => {
  // The track is the same across sockets: its generation record is ok, and no
  // sample was taken for this swap.
  const audio = audioOf(owedSwap(1), []);
  assert.deepEqual(statuses(audio), [[1, "missing_sample"]]);
  const other = audioOf(owedSwap(1), [rearm(2)]);
  assert.deepEqual(statuses(other), [[1, "missing_sample"]]);
  assert.equal(other.ok, false);
});

test("a swap that owed no audio is not made to speak, whatever its sample says", () => {
  const idle = audioOf(fixtures.start(1) + fixtures.end(1, "installed"), []);
  assert.equal(idle.ok, true);
  assert.deepEqual(statuses(idle), [[1, "not_required"]]);
  const held = audioOf(fixtures.swapped(1, 4, "superseded", null), [
    rearm(1, { status: "unavailable", reason: "sampling-cancelled" }),
  ]);
  assert.equal(held.ok, true);
  assert.deepEqual(statuses(held), [[1, "not_required"]]);
});

test("a swap superseded by a later swap is covered only by that swap's fresh sample", () => {
  const log = owedSwap(1) + owedSwap(2);
  const superseded = rearm(1, {
    status: "superseded",
    reason: "superseded-by-later-swap",
    windowEdge: null,
    counterPairs: 0,
    bytesReceived: null,
    packetsReceived: null,
    peakRms: null,
  });
  const covered = audioOf(log, [superseded, rearm(2)]);
  assert.equal(covered.ok, true);
  assert.deepEqual(statuses(covered), [
    [1, "covered_by_later_swap"],
    [2, "verified"],
  ]);
  const uncovered = audioOf(log, [
    superseded,
    rearm(2, { status: "unavailable", reason: "sampling-cancelled" }),
  ]);
  assert.deepEqual(statuses(uncovered), [
    [1, "superseded_uncovered"],
    [2, "unavailable"],
  ]);
  // An earlier sample never covers a later swap.
  const backwards = audioOf(log, [
    rearm(1),
    rearm(2, { status: "superseded", reason: "superseded-by-later-swap" }),
  ]);
  assert.deepEqual(statuses(backwards), [
    [1, "verified"],
    [2, "superseded_uncovered"],
  ]);
  // A stale later sample does not cover either.
  const stale = audioOf(log, [superseded, rearm(2, { windowEdge: 4 })]);
  assert.equal(stale.ok, false);
});

test("a swap whose track ended before a sample is not covered by anything earlier", () => {
  const ended = rearm(1, {
    status: "superseded",
    reason: "track-ended-before-counter-pair",
    windowEdge: 5,
    counterPairs: 0,
    peakRms: null,
    bytesReceived: null,
    packetsReceived: null,
  });
  const audio = audioOf(owedSwap(1), [ended]);
  assert.deepEqual(statuses(audio), [[1, "superseded_uncovered"]]);
});

test("installed swaps are read from complete end lines of the room, once", () => {
  const log =
    fixtures.end(1, "installed") +
    fixtures.end(1, "installed") +
    fixtures.end(2, "failed") +
    fixtures.end(3, "cancelled") +
    fixtures.end(4, "installed", { room: "other" }) +
    fixtures.start(5) +
    fixtures.end(6, "installed").trimEnd();
  assert.deepEqual(judge.installedSwapKeys(log, fixtures.ROOM), ["7:1:1"]);
  assert.deepEqual(judge.installedSwapKeys(`${log}\n`, fixtures.ROOM), [
    "7:1:1",
    "7:1:6",
  ]);
  assert.deepEqual(judge.installedSwapKeys("", fixtures.ROOM), []);
});

// What a soak writes about itself.

test("a capture of several watchdog rebuilds reports both outcomes in either mode", () => {
  const f = fixtures.VERDICT_FIXTURES.find((x) =>
    x.name.startsWith("many_watchdog"),
  );
  const exposure = judge.soakExposure(clockSamples([[1, 11, 660000]]));
  const result = (mode) =>
    judge.soakResult({
      soak: judgeMode(f, mode),
      replies: { ok: true, verdict: "pass", counts: {}, episodes: [] },
      audio: { ok: true, episodes: [], reasons: [] },
      exposure,
    });
  const strict = result("strict");
  assert.equal(strict.verdict, "fail");
  assert.equal(strict.qualification, "fail");
  assert.equal(strict.watchdog_rebuilds_total, 3);
  assert.equal(strict.strict_cap_eligible_total, 3);
  assert.equal(strict.goaway_swaps_total, 1);
  assert.equal(strict.exposure_s, 660);
  const pilot = result("pilot");
  assert.equal(pilot.verdict, "pass");
  assert.equal(pilot.strict_verdict, "fail");
  assert.equal(pilot.structural_verdict, "pass");
  assert.equal(pilot.qualification, "not_evaluated");
  assert.equal(pilot.watchdog_rebuilds_total, 3);
  assert.equal(pilot.cap_reasons.length, 1);
  assert.equal(pilot.exposure_s, 660);
});

test("a reply or audio failure is structural in both modes", () => {
  const f = fixtures.VERDICT_FIXTURES[0];
  const exposure = { exposure_s: 660, basis: "interview_clock" };
  const bad = { ok: false, verdict: "fail", counts: {}, episodes: [] };
  const good = { ok: true, verdict: "pass", counts: {}, episodes: [] };
  const badAudio = { ok: false, episodes: [], reasons: ["x"] };
  const goodAudio = { ok: true, episodes: [], reasons: [] };
  for (const mode of ["strict", "pilot"]) {
    for (const [replies, audio] of [
      [bad, goodAudio],
      [good, badAudio],
    ]) {
      const result = judge.soakResult({
        soak: judgeMode(f, mode),
        replies,
        audio,
        exposure,
      });
      assert.equal(result.verdict, "fail", mode);
      assert.equal(result.structural_verdict, "fail", mode);
      assert.equal(result.strict_verdict, "fail", mode);
    }
  }
});

test("no GoAway stays inconclusive and is never turned into a pass by pilot", () => {
  const soak = judge.judgeSoak({
    replacements: [],
    goAway: false,
    expired: false,
    mode: "pilot",
  });
  assert.equal(soak.verdict, "inconclusive");
  const result = judge.soakResult({
    soak,
    replies: null,
    audio: null,
    exposure: { exposure_s: null, basis: "no_clock_samples" },
  });
  assert.equal(result.verdict, "inconclusive");
  assert.equal(result.qualification, "not_evaluated");
});

// The launcher and the driver.

test("browser-check.sh validates the verdict mode and only accepts it for the soak", async () => {
  const { spawnSync } = await import("node:child_process");
  const run = (env) =>
    spawnSync("sh", ["scripts/browser-check.sh"], {
      encoding: "utf8",
      env: {
        PATH: process.env.PATH,
        BROWSER_CHECK_VALIDATE_ENV_ONLY: "1",
        LIVEKIT_URL: "wss://example.livekit.cloud",
        LIVEKIT_API_KEY: "key",
        LIVEKIT_API_SECRET: "secret",
        GOOGLE_API_KEY: "google",
        BROWSER_CHECK_AGENT: "rust",
        ...env,
      },
    });
  const soak = { BROWSER_CHECK_FLOW: "soak" };
  assert.equal(run(soak).status, 0);
  assert.equal(
    run({ ...soak, BROWSER_CHECK_SOAK_VERDICT_MODE: "strict" }).status,
    0,
  );
  assert.equal(
    run({ ...soak, BROWSER_CHECK_SOAK_VERDICT_MODE: "pilot" }).status,
    0,
  );
  const bad = run({ ...soak, BROWSER_CHECK_SOAK_VERDICT_MODE: "bogus" });
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /must be strict or pilot/);
  const other = run({
    BROWSER_CHECK_FLOW: "voice",
    BROWSER_CHECK_SOAK_VERDICT_MODE: "pilot",
  });
  assert.equal(other.status, 2);
  assert.match(other.stderr, /applies to BROWSER_CHECK_FLOW=soak/);
  // Exported empty by a runner, it names nothing.
  assert.equal(
    run({
      BROWSER_CHECK_FLOW: "voice",
      BROWSER_CHECK_AGENT: "home",
      BROWSER_CHECK_SOAK_VERDICT_MODE: "",
    }).status,
    0,
  );
});

test("the driver judges with the mode, exits 77 only for no GoAway and 1 for the rest", () => {
  const driver = fs.readFileSync("scripts/browser-check.cjs", "utf8");
  const script = fs.readFileSync("scripts/browser-check.sh", "utf8");
  assert.match(script, /BROWSER_CHECK_SOAK_VERDICT_MODE="\$SOAK_VERDICT_MODE"/);
  assert.match(driver, /mode: soakVerdictMode/);
  assert.match(driver, /const SOAK_INCONCLUSIVE_EXIT = 77;/);
  assert.match(
    driver,
    /process\.exit\(error instanceof InconclusiveSoak \? SOAK_INCONCLUSIVE_EXIT : 1\)/,
  );
  assert.equal(
    (driver.match(/new InconclusiveSoak\(/g) ?? []).length,
    1,
    "only a soak that learned nothing is inconclusive",
  );
});

// A reduced synthetic series shared with the Python analyzer, not live evidence.
const recoverySeries = fs.readFileSync(
  new URL("../fixtures/gemini-recovery-series.txt", import.meta.url),
  "utf8",
);
const seriesRoom = "recovery-series";
const seriesReplies = (log = recoverySeries) =>
  judge.judgeEpisodeReplies(
    judge.parseEpisodes(log, seriesRoom),
    judge.parseObligations(log, seriesRoom),
  );

test("six resumed rotations preserve one owed crossing without inventing five replies", () => {
  const parsed = judge.parseEpisodes(recoverySeries, seriesRoom);
  assert.equal(parsed.episodes.length, 6);
  const swaps = judge.parseReplacements(recoverySeries, seriesRoom);
  assert.equal(swaps.length, 6);
  assert.deepEqual(
    swaps.map((s) => s.newSocket),
    [2, 3, 4, 5, 6, 7],
  );
  const tally = judge.tallyReplacements(swaps);
  assert.equal(tally.goAwaySwapsTotal, 6);
  assert.equal(tally.watchdogSwapsTotal, 0);
  const replies = seriesReplies();
  assert.equal(replies.verdict, "pass");
  assert.deepEqual(
    replies.episodes.map((e) => e.class),
    ["idle", "answered", "idle", "idle", "idle", "idle"],
  );
  assert.equal(judge.parseObligations(recoverySeries, seriesRoom).size, 6);
});

test("two copies of the series keep exactly six swaps and one answered crossing", () => {
  const twice = recoverySeries + recoverySeries;
  assert.equal(judge.parseReplacements(twice, seriesRoom).length, 6);
  assert.deepEqual(seriesReplies(twice), seriesReplies());
  assert.equal(judge.parseObligations(twice, seriesRoom).size, 6);
  assert.deepEqual(judge.parseEpisodes(twice, "foreign-room").episodes, []);
});

for (let episode = 1; episode <= 6; episode++) {
  test(`the series cannot hide missing handover ${episode} behind its other swaps`, () => {
    const log = recoverySeries
      .split("\n")
      .filter(
        (line) =>
          !(
            line.startsWith("replacement end:") &&
            line.includes(` episode=${episode} `)
          ),
      )
      .join("\n");
    const replies = seriesReplies(log);
    assert.equal(replies.episodes.length, 6);
    assert.equal(replies.verdict, "fail");
    const missing = replies.episodes.find((e) => e.episode === episode);
    assert.equal(missing.status, "uncovered");
    assert.equal(missing.class, "truncated");
  });
}

test("the carried reply in the series rejects audio accepted on the old socket", () => {
  const log = recoverySeries
    .split("\n")
    .map((line) => {
      if (!line.startsWith("codetrial latency ")) return line;
      const record = JSON.parse(line.slice("codetrial latency ".length));
      if (record.obligation === 2) {
        record.socket = 2;
        record.first_accept_socket = 2;
      }
      return `codetrial latency ${JSON.stringify(record)}`;
    })
    .join("\n");
  const replies = seriesReplies(log);
  assert.equal(replies.verdict, "fail");
  assert.equal(
    replies.episodes.find((e) => e.episode === 2).class,
    "pre_rotation_audio",
  );
});

test("native capture records in the series do not prove fresh decoded audio", () => {
  const replies = seriesReplies();
  assert.equal(replies.ok, true);
  const absent = judge.judgeSwapAudio(replies, []);
  assert.equal(absent.ok, false);
  assert.equal(
    absent.episodes.find((e) => e.episode === 2).status,
    "missing_sample",
  );
  const unrelated = judge.judgeSwapAudio(replies, [rearm(6)]);
  assert.equal(unrelated.ok, false);
  const stale = judge.judgeSwapAudio(replies, [rearm(2, { windowEdge: 4 })]);
  assert.equal(stale.ok, false);
  const fresh = judge.judgeSwapAudio(replies, [rearm(2)]);
  assert.equal(fresh.ok, true);
  assert.equal(fresh.episodes.filter((e) => e.status === "verified").length, 1);
  assert.equal(
    fresh.episodes.filter((e) => e.status === "not_required").length,
    5,
  );
});
