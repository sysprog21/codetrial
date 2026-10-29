import { test } from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import sched from "../../scripts/long-interview-schedule.cjs";
import plan_ from "../../scripts/long-interview-plan.cjs";
import timing_ from "../../scripts/long-interview-timing.cjs";

const { archiveBounds } = timing_;

const {
  AUTHORED_SHA256,
  CYCLE_S,
  MIN_RECALL_GAP_S,
  answerFor,
  budgetFields,
  checkAuthoredPin,
  ffmpegDecodes,
  ffmpegPreflight,
  generateSchedule,
  hashOf,
  judgeBarge,
  judgeCheckpoint,
  judgeThinkingHold,
  playbackPlan,
  summarizeCompression,
  syntheticClipSet,
  validateClipBytes,
  validateClipSet,
  validatePlaybackPlan,
  validateSchedule,
  verifyTransmitted,
  wavBytes,
} = sched;

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const BUDGET = path.join(ROOT, "scripts", "interview-run-budget.py");

const QUAL = { durationMin: 90, observationTargetS: 5400 };
const PILOT = { durationMin: 60, observationTargetS: 3600 };
const SMOKE = { durationMin: 10, observationTargetS: 600 };

const bundles = {
  coding: generateSchedule("coding", QUAL),
  whiteboard: generateSchedule("whiteboard", QUAL),
};

// Pinned on purpose: a change to the authored questions, the timing or the
// answer key moves these, and the change then has to be made on purpose.
const PINS = {
  coding: "1a926968c26adbf61aeea60a5d10dd5a461180fc12686d196d30e94f7b2313a1",
  whiteboard:
    "d5851de32e3f63803d0adaab9a93e72ab732aa65a9d05f213f444bb6540c9b93",
};

test("the authored catalogue is hash pinned", () => {
  assert.equal(checkAuthoredPin(), AUTHORED_SHA256);
  assert.throws(() => checkAuthoredPin("0".repeat(64)), /catalogue changed/);
});

test("schedules are deterministic and pinned by hash", () => {
  for (const mode of ["coding", "whiteboard"]) {
    const again = generateSchedule(mode, QUAL);
    assert.deepEqual(again, bundles[mode]);
    assert.equal(bundles[mode].schedule.schedule_sha256, PINS[mode], mode);
    assert.deepEqual(
      validateSchedule(bundles[mode].schedule, bundles[mode].answer_key),
      [],
    );
  }
  assert.notEqual(
    bundles.coding.schedule.schedule_sha256,
    bundles.whiteboard.schedule.schedule_sha256,
  );
});

test("a tampered schedule or answer key is refused", () => {
  const { schedule, answer_key: key } = bundles.coding;
  const moved = structuredClone(schedule);
  moved.steps[3].at_s += 1;
  assert.match(validateSchedule(moved, key).join("\n"), /schedule_sha256/);
  const rekeyed = structuredClone(key);
  rekeyed.facts[0].text = "changed";
  assert.match(
    validateSchedule(schedule, rekeyed).join("\n"),
    /answer_key_sha256/,
  );
});

test("times derive from the validated observation target", () => {
  const smoke = generateSchedule("coding", SMOKE).schedule;
  assert.equal(smoke.observation_window_s, 480);
  assert.equal(smoke.cycles, 1);
  assert.equal(smoke.final_at_s, 450);
  assert.equal(bundles.coding.schedule.observation_window_s, 5280);
  assert.equal(bundles.coding.schedule.cycles, 17);
  assert.equal(bundles.coding.schedule.final_at_s, 5250);
  // A deadline smoke observes the grace as well; no stimulus goes in it.
  const deadline = generateSchedule("coding", {
    durationMin: 10,
    observationTargetS: 720,
  }).schedule;
  assert.equal(deadline.observation_window_s, 480);
  assert.ok(deadline.steps.every((s) => s.at_s <= 480));
  assert.throws(
    () =>
      generateSchedule("coding", { durationMin: 10, observationTargetS: 300 }),
    /no room/,
  );
  assert.throws(() => generateSchedule("chess", QUAL), /mode must be/);
});

test("a plan the long-interview validator accepts drives the generator", () => {
  const manifest = {
    run_id: "r1",
    run_kind: "pilot",
    end_mode: "manual",
    duration_min: 60,
    observation_target_s: 3600,
    selected_duration_s: 3600,
  };
  assert.deepEqual(plan_.validateManifest(manifest, 60), []);
  const { schedule } = generateSchedule("whiteboard", {
    durationMin: 60,
    observationTargetS: manifest.observation_target_s,
  });
  assert.equal(schedule.cycles, 11);
  assert.deepEqual(
    schedule.steps.filter((s) => s.kind === "join").map((s) => s.action_id),
    ["candidate_join"],
  );
});

test("a checkpoint every five minutes plus the required kinds", () => {
  for (const mode of ["coding", "whiteboard"]) {
    const { schedule } = bundles[mode];
    const at = schedule.checkpoints
      .filter((c) => c.kind !== "tool_continuation" && c.kind !== "final")
      .map((c) => c.at_s);
    assert.equal(at.length, 17);
    at.forEach((t, i) => assert.equal(t, i * CYCLE_S + 240));
    const kinds = new Set(schedule.checkpoints.map((c) => c.kind));
    for (const kind of ["state", "recall", "tool_continuation", "final"]) {
      assert.ok(kinds.has(kind), `${mode} has a ${kind} checkpoint`);
    }
    for (const kind of [
      "thinking_start",
      "thinking_end",
      "barge",
      "barge_prime",
      "revision",
    ]) {
      assert.equal(
        schedule.steps.filter((s) => s.kind === kind).length,
        17,
        `${mode} ${kind} per cycle`,
      );
    }
    assert.equal(schedule.checkpoints.at(-1).checkpoint_id, "cp_final");
    const tool = mode === "coding" ? "read_editor" : "read_board";
    for (const c of schedule.checkpoints) {
      assert.equal(c.required_tool.name, tool);
      assert.equal(c.required_tool.call_id_required, true);
      assert.ok(c.source_fact_ids.length >= 2);
      assert.ok(c.response_window_s > 0 && c.response_window_s <= 45);
    }
  }
});

test("every 90 minute schedule recalls facts seeded 30 minutes earlier", () => {
  for (const mode of ["coding", "whiteboard"]) {
    const { schedule, answer_key: key } = bundles[mode];
    const recalls = schedule.checkpoints.filter((c) => c.recall);
    assert.equal(recalls.length, 4);
    for (const c of recalls) {
      assert.ok(c.recall.gap_s >= MIN_RECALL_GAP_S);
      // The seed is really spoken by an earlier clip, at the stated time.
      const fact = key.facts.find((f) => f.id === c.recall.fact_id);
      const step = schedule.steps.find(
        (s) =>
          s.clip_id &&
          s.at_s <= fact.seeded_at_s + 45 &&
          s.at_s >= fact.seeded_at_s,
      );
      const spoken = schedule.clips.filter((clip) =>
        clip.source_text.includes(fact.text),
      );
      assert.ok(spoken.length >= 1, `${fact.id} is spoken`);
      assert.ok(step);
      assert.equal(c.at_s - fact.seeded_at_s, c.recall.gap_s);
      assert.ok(c.required_tools.includes("read_conversation"));
    }
    const ids = new Set(recalls.map((c) => c.recall.fact_id));
    assert.equal(ids.size, 2);
  }
  // 60 minutes also reaches two.
  const pilot = generateSchedule("coding", PILOT).schedule;
  assert.ok(pilot.checkpoints.filter((c) => c.recall).length >= 2);
  // A ten minute smoke has no 30 minute recall to make and does not pretend to.
  const smoke = generateSchedule("coding", SMOKE).schedule;
  assert.equal(smoke.checkpoints.filter((c) => c.recall).length, 0);
  const noRecalls = structuredClone(pilot);
  noRecalls.checkpoints.forEach((c) => delete c.recall);
  noRecalls.schedule_sha256 = "";
  assert.match(
    validateSchedule(noRecalls).join("\n"),
    /at least two long-gap recalls/,
  );
});

test("the nonce is never the whole answer in the answer key", () => {
  for (const mode of ["coding", "whiteboard"]) {
    for (const k of bundles[mode].answer_key.checkpoints) {
      assert.ok(k.element_fact_ids.length >= 2);
      assert.ok((k.tools ?? [k.tool]).length >= 1);
    }
  }
});

test("action IDs are unique and satisfy the run budget manifest invariant", () => {
  for (const mode of ["coding", "whiteboard"]) {
    const { schedule } = bundles[mode];
    const set = new Set(schedule.actions);
    assert.equal(set.size, schedule.actions.length);
    assert.equal(schedule.actions[0], "candidate_join");
    assert.ok(!set.has("end"));
    const { actions, action_ceiling } = budgetFields(
      schedule,
      syntheticClipSet(schedule).manifest,
    );
    assert.equal(action_ceiling, actions.length + 1);
    assert.equal(actions.length, 155);
    // The conditional barge keeps its slot: unused is allowed, unlisted is not.
    assert.ok(set.has("c05_barge") && set.has("c05_prime"));
  }
});

function validateWithBudgetTool(
  mode,
  durationBundle = bundles[mode],
  fields = {},
) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lis-"));
  try {
    const clips = syntheticClipSet(durationBundle.schedule);
    const hex = (s) => crypto.createHash("sha256").update(s).digest("hex");
    const exe = fs.readFileSync(process.execPath);
    const budget = budgetFields(durationBundle.schedule, clips.manifest);
    const manifest = {
      schema_version: 1,
      run_id: `sched-${mode}`,
      binaries: {
        candidate: { path: process.execPath, sha256: hex(exe) },
      },
      settings_sha256: hex("settings"),
      selected_duration_s: durationBundle.schedule.observation_target_s,
      outer_wall_cap_s: durationBundle.schedule.observation_target_s + 660,
      cleanup_reserve_s: 360,
      max_archive_bytes: archiveBounds(
        durationBundle.schedule.observation_target_s + 660,
      ).floor,
      end_mode: "manual",
      run_kind: "qualification",
      duration_min: durationBundle.schedule.duration_min,
      observation_target_s: durationBundle.schedule.observation_target_s,
      limits: {},
      scopes: { scripted_actions: "hard", wall_time: "hard" },
      ...budget,
      ...fields,
    };
    const file = path.join(dir, "manifest.json");
    fs.writeFileSync(file, JSON.stringify(manifest));
    const result = spawnSync(
      "python3",
      ["-I", BUDGET, "validate", "--manifest", file],
      { encoding: "utf8" },
    );
    return { result, manifest };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("interview-run-budget.py validate consumes the generated manifest fields", () => {
  for (const mode of ["coding", "whiteboard"]) {
    const { result, manifest } = validateWithBudgetTool(mode);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^ok sched-/);
    assert.deepEqual(plan_.validateManifest(manifest, 90), []);
  }
  // The invariant is real: a ceiling that forgets the End permit is refused.
  const { result } = validateWithBudgetTool("coding", bundles.coding, {
    action_ceiling: 155,
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /nonfinal action count plus one/);
  // And a schedule that dropped the join action is refused.
  const noJoin = validateWithBudgetTool("coding", bundles.coding, {
    actions: bundles.coding.schedule.actions.slice(1),
    action_ceiling: 155,
  });
  assert.notEqual(noJoin.result.status, 0);
  assert.match(noJoin.result.stderr, /join action/);
});

// ---------------------------------------------------------------- the judge

const CP_STATE = "c07_cp_state"; // a recall: two tool reads, five facts
const CP_INSPECT = "c05_cp_inspect";

function evidenceFor(mode, id) {
  return structuredClone(answerFor(bundles[mode], id));
}

test("a faithful answer passes every checkpoint of both schedules", () => {
  for (const mode of ["coding", "whiteboard"]) {
    for (const c of bundles[mode].schedule.checkpoints) {
      const verdict = judgeCheckpoint(
        bundles[mode],
        answerFor(bundles[mode], c.checkpoint_id),
      );
      assert.deepEqual(
        verdict,
        { verdict: "pass", reasons: [] },
        c.checkpoint_id,
      );
    }
  }
});

const key = (mode, id) =>
  bundles[mode].answer_key.checkpoints.find((k) => k.checkpoint_id === id);
const factsOf = (mode) => bundles[mode].answer_key.facts;
const marker = (mode, n) =>
  factsOf(mode).find((f) => f.id === `revision_${String(n).padStart(2, "0")}`)
    .match.tokens[0];

// Each fixture names the reason it must fail. Every one of them keeps the
// nonce, and all but the first few keep the expected tokens, so a judge that
// stops at the nonce or at the words passes them.
const NEGATIVES = [
  {
    name: "nonce only, tool evidence valid",
    cp: CP_STATE,
    reason: /reply lacks/,
    edit: (e, b) => {
      e.reply.text = key("coding", CP_STATE).nonce;
    },
  },
  {
    name: "nonce only, no tool evidence",
    cp: CP_INSPECT,
    reason: /lacks|no read_editor call/,
    edit: (e) => {
      e.reply.text = key("coding", CP_INSPECT).nonce;
      e.tool_calls = [];
      e.reply.tool_call_ids = [];
    },
  },
  {
    name: "wrong revision in the tool result",
    cp: CP_INSPECT,
    reason: /result lacks revision_05/,
    edit: (e) => {
      e.tool_calls[0].result_text = marker("coding", 4);
    },
  },
  {
    name: "wrong revision stated in the reply",
    cp: CP_INSPECT,
    reason: /lacks revision_05|stale revision/,
    edit: (e) => {
      e.reply.text = e.reply.text.replace(
        marker("coding", 5),
        marker("coding", 4),
      );
    },
  },
  {
    name: "current and stale revision both stated",
    cp: CP_INSPECT,
    reason: /stale revision revision_04/,
    edit: (e) => {
      e.reply.text += ` ${marker("coding", 4)}`;
    },
  },
  {
    name: "expected token present but no tool call recorded",
    cp: CP_STATE,
    reason: /no read_editor call/,
    edit: (e) => {
      e.tool_calls = e.tool_calls.filter((c) => c.name !== "read_editor");
    },
  },
  {
    name: "paraphrase of the source without any tool read",
    cp: CP_INSPECT,
    reason: /no read_editor call/,
    edit: (e) => {
      e.tool_calls = [];
      e.reply.tool_call_ids = [];
    },
  },
  {
    name: "tool call exists but the reply cites another call ID",
    cp: CP_INSPECT,
    reason: /does not cite call/,
    edit: (e) => {
      e.reply.tool_call_ids = ["call-somewhere-else"];
    },
  },
  {
    name: "tool call has no ID",
    cp: CP_INSPECT,
    reason: /a call has no ID/,
    edit: (e) => {
      delete e.tool_calls[0].call_id;
      e.reply.tool_call_ids = [undefined];
    },
  },
  {
    name: "tool read before the question was asked",
    cp: "c03_cp_state",
    reason: /before the question/,
    edit: (e) => {
      e.tool_calls[0].at_s = 740;
    },
  },
  {
    name: "tool call has no timestamp",
    cp: CP_INSPECT,
    reason: /has no timestamp/,
    edit: (e) => {
      delete e.tool_calls[0].at_s;
    },
  },
  {
    name: "a stale stage is stated",
    cp: "c12_cp_state",
    reason: /reply lacks phase_12/,
    edit: (e) => {
      e.reply.text = e.reply.text.replace(
        "phase 12: test",
        "phase 12: algorithm",
      );
    },
  },
  {
    name: "the wrong tool was read",
    cp: CP_INSPECT,
    reason: /no read_editor call/,
    edit: (e) => {
      e.tool_calls[0].name = "read_conversation";
    },
  },
  {
    name: "tool result is empty",
    cp: CP_INSPECT,
    reason: /result lacks/,
    edit: (e) => {
      e.tool_calls[0].result_text = "";
    },
  },
  {
    name: "tool call failed",
    cp: CP_INSPECT,
    reason: /did not succeed/,
    edit: (e) => {
      e.tool_calls[0].ok = false;
    },
  },
  {
    name: "tool read before the revision existed",
    cp: CP_INSPECT,
    reason: /before the revision/,
    edit: (e) => {
      e.tool_calls[0].at_s = 100;
    },
  },
  {
    name: "nonce missing",
    cp: CP_STATE,
    reason: /nonce is missing/,
    edit: (e) => {
      e.reply.text = e.reply.text.replace(key("coding", CP_STATE).nonce, "");
    },
  },
  {
    name: "reply after the response window",
    cp: CP_STATE,
    reason: /outside/,
    edit: (e) => {
      e.reply.started_at_s += 300;
    },
  },
  {
    name: "reply interrupted before completing",
    cp: CP_STATE,
    reason: /interrupted/,
    edit: (e) => {
      e.reply.interrupted = true;
    },
  },
  {
    name: "constraints incomplete",
    cp: CP_STATE,
    reason: /rubric concept/,
    edit: (e) => {
      e.reply.text = e.reply.text.replace(/ten thousand/g, "");
    },
  },
  {
    name: "recall of the seeded fact is wrong",
    cp: CP_STATE,
    reason: /reply lacks seed_codename/,
    edit: (e) => {
      e.reply.text = e.reply.text.replace(/saffron heron/g, "violet crane");
    },
  },
  {
    name: "recall without reading the conversation",
    cp: CP_STATE,
    reason: /no read_conversation call/,
    edit: (e) => {
      e.tool_calls = e.tool_calls.filter((c) => c.name !== "read_conversation");
    },
  },
];

test("the negative fixtures are a fixed set and every one fails", () => {
  assert.equal(NEGATIVES.length, 22);
  for (const fixture of NEGATIVES) {
    const evidence = evidenceFor("coding", fixture.cp);
    // Each starts from a passing answer, so the edit alone is what fails it.
    assert.equal(
      judgeCheckpoint(bundles.coding, evidence).verdict,
      "pass",
      fixture.name,
    );
    fixture.edit(evidence, bundles.coding);
    const verdict = judgeCheckpoint(bundles.coding, evidence);
    assert.equal(verdict.verdict, "fail", fixture.name);
    assert.match(verdict.reasons.join("\n"), fixture.reason, fixture.name);
  }
});

test("a judge key that drops the schedule's tool requirement is refused", () => {
  const forged = structuredClone(bundles.coding);
  const k = forged.answer_key.checkpoints.find(
    (c) => c.checkpoint_id === CP_INSPECT,
  );
  delete k.tool;
  k.tools = [];
  const evidence = evidenceFor("coding", CP_INSPECT);
  evidence.tool_calls = [];
  evidence.reply.tool_call_ids = [];
  // Unhashed, the key is not the one the schedule pins.
  assert.match(
    judgeCheckpoint(forged, evidence).reasons.join("\n"),
    /not the one the schedule pins/,
  );
  // Re-pinned, it still has to ask for what the schedule says it asks for.
  forged.schedule.answer_key_sha256 = hashOf(forged.answer_key);
  const repinned = judgeCheckpoint(forged, evidence);
  assert.equal(repinned.verdict, "fail");
  assert.match(repinned.reasons.join("\n"), /does not require the tool reads/);
  // And a schedule re-hashed after an edit is not the generator's.
  const edited = structuredClone(bundles.coding.schedule);
  edited.steps[3].at_s += 1;
  const rest = { ...edited };
  delete rest.schedule_sha256;
  edited.schedule_sha256 = hashOf(rest);
  assert.match(
    validateSchedule(edited, bundles.coding.answer_key).join("\n"),
    /not the one the generator writes/,
  );
  assert.throws(
    () =>
      budgetFields(bundles.coding.schedule, {
        ...synthetic.manifest,
        manifest_sha256: "a".repeat(64),
      }),
    /does not match its own hash/,
  );
  assert.throws(
    () => budgetFields(bundles.whiteboard.schedule, synthetic.manifest),
    /different schedule/,
  );
  assert.throws(
    () => budgetFields(bundles.coding.schedule),
    /pins the clip manifest/,
  );
});

test("the same fixtures fail on the whiteboard schedule", () => {
  // Board revisions carry the same markers through read_board.
  const evidence = evidenceFor("whiteboard", CP_INSPECT);
  assert.equal(evidence.tool_calls[0].name, "read_board");
  evidence.tool_calls[0].result_text = marker("whiteboard", 4);
  assert.match(
    judgeCheckpoint(bundles.whiteboard, evidence).reasons.join("\n"),
    /result lacks revision_05/,
  );
  const coding = evidenceFor("whiteboard", CP_INSPECT);
  coding.tool_calls[0].name = "read_editor";
  assert.match(
    judgeCheckpoint(bundles.whiteboard, coding).reasons.join("\n"),
    /no read_board call/,
  );
});

test("a withheld or corrupted expected fact fails the checkpoint", () => {
  const evidence = evidenceFor("coding", CP_STATE);
  const facts = bundles.coding.answer_key.facts;
  const withheld = judgeCheckpoint(bundles.coding, evidence, {
    facts: facts.filter((f) => f.id !== "seed_codename"),
  });
  assert.equal(withheld.verdict, "fail");
  assert.match(withheld.reasons.join("\n"), /seed_codename is withheld/);

  const corrupted = judgeCheckpoint(bundles.coding, evidence, {
    facts: facts.map((f) =>
      f.id === "constraints"
        ? { ...f, match: { ...f.match, concepts: [["anything"]] } }
        : f,
    ),
  });
  assert.equal(corrupted.verdict, "fail");
  assert.match(corrupted.reasons.join("\n"), /constraints is corrupted/);

  // Corrupting the text of a fact is as visible as corrupting its tokens.
  const reworded = judgeCheckpoint(bundles.coding, evidence, {
    facts: facts.map((f) =>
      f.id === "seed_codename" ? { ...f, text: `${f.text} (edited)` } : f,
    ),
  });
  assert.match(reworded.reasons.join("\n"), /seed_codename is corrupted/);
});

test("an unreviewed rubric cannot certify a reply", () => {
  const evidence = evidenceFor("coding", CP_STATE);
  const facts = bundles.coding.answer_key.facts.map((f) => {
    if (f.id !== "constraints") return f;
    const loosened = {
      ...f,
      match: { ...f.match, review: { ...f.match.review, status: "draft" } },
    };
    const rest = { ...loosened };
    delete rest.sha256;
    return { ...rest, sha256: hashOf(rest) };
  });
  const verdict = judgeCheckpoint(bundles.coding, evidence, { facts });
  assert.equal(verdict.verdict, "fail");
  assert.match(verdict.reasons.join("\n"), /rubric is not reviewed/);
});

test("exact tokens are case sensitive and folded tokens are not", () => {
  const evidence = evidenceFor("coding", CP_INSPECT);
  evidence.reply.text = evidence.reply.text.replace(
    marker("coding", 5),
    marker("coding", 5).toUpperCase(),
  );
  assert.match(
    judgeCheckpoint(bundles.coding, evidence).reasons.join("\n"),
    /reply lacks revision_05/,
  );
  const folded = evidenceFor("coding", CP_STATE);
  folded.reply.text = folded.reply.text.replace(
    /saffron heron/g,
    "SAFFRON   Heron",
  );
  assert.equal(judgeCheckpoint(bundles.coding, folded).verdict, "pass");
});

test("the final pending question is judged", () => {
  const evidence = evidenceFor("coding", "cp_final");
  assert.equal(judgeCheckpoint(bundles.coding, evidence).verdict, "pass");
  evidence.reply.text = evidence.reply.text.replace(
    /ascending( order)?/g,
    "descending",
  );
  assert.match(
    judgeCheckpoint(bundles.coding, evidence).reasons.join("\n"),
    /reply lacks pending_question/,
  );
});

test("thinking, barge and compression have their own verdicts", () => {
  const think = bundles.coding.schedule.steps.find(
    (s) => s.kind === "thinking_start",
  );
  assert.equal(
    judgeThinkingHold(think, { agent_speech_spans: [[0, 5]] }).verdict,
    "pass",
  );
  assert.equal(
    judgeThinkingHold(think, {
      agent_speech_spans: [[think.at_s + 2, think.at_s + 4]],
    }).verdict,
    "fail",
  );
  assert.equal(
    judgeBarge({ agent_speaking_at_dispatch: false }).verdict,
    "uncovered",
  );
  assert.equal(
    judgeBarge({
      agent_speaking_at_dispatch: true,
      dispatched: true,
      agent_yielded: false,
    }).verdict,
    "fail",
  );
  assert.equal(
    judgeBarge({
      agent_speaking_at_dispatch: true,
      dispatched: true,
      agent_yielded: true,
    }).verdict,
    "pass",
  );
  assert.deepEqual(summarizeCompression([]), {
    usage_high_water_tokens: 0,
    compression_events: 0,
    compression_claim: "open",
  });
  assert.deepEqual(
    summarizeCompression([
      { usage_tokens: 900 },
      { usage_tokens: 4200, compression: true },
      { usage_tokens: 3000 },
    ]),
    {
      usage_high_water_tokens: 4200,
      compression_events: 1,
      compression_claim: "observed",
    },
  );
  assert.equal(
    summarizeCompression([{ usage_tokens: 900 }]).compression_claim,
    "open",
  );
});

// ------------------------------------------------------------------- audio

const synthetic = syntheticClipSet(bundles.coding.schedule);

test("the synthetic clips validate and are labelled as containing no speech", () => {
  const { manifest, audio } = synthetic;
  assert.equal(manifest.origin, "synthetic-tone");
  assert.equal(manifest.contains_speech, false);
  assert.deepEqual(
    validateClipSet(bundles.coding.schedule, manifest, audio),
    [],
  );
  assert.equal(manifest.clips.length, bundles.coding.schedule.clips.length);
  for (const clip of manifest.clips) {
    assert.deepEqual(clip.format, {
      container: "wav",
      codec: "pcm_s16le",
      sample_rate: 16000,
      channels: 1,
    });
    assert.match(clip.sha256, /^[0-9a-f]{64}$/);
    assert.ok(clip.source_text.length > 0);
    assert.ok(clip.speech_s <= 20);
  }
  // Deterministic bytes.
  assert.equal(
    syntheticClipSet(bundles.coding.schedule).manifest.manifest_sha256,
    manifest.manifest_sha256,
  );
});

test("tones are refused wherever real speech is required", () => {
  const errors = validateClipSet(
    bundles.coding.schedule,
    synthetic.manifest,
    synthetic.audio,
    {
      requireSpeech: true,
    },
  );
  assert.match(
    errors.join("\n"),
    /real spoken clips authored outside the checkout/,
  );
  // A manifest cannot dress tones up as speech: the hash covers the label.
  const lie = { ...synthetic.manifest, contains_speech: true };
  const lieErrors = validateClipSet(
    bundles.coding.schedule,
    lie,
    synthetic.audio,
    { requireSpeech: true },
  );
  assert.match(
    lieErrors.join("\n"),
    /manifest_sha256|only for recorded-speech/,
  );
});

test("clip duration, PCM format and hash are checked", () => {
  const entry = synthetic.manifest.clips[0];
  const bytes = synthetic.audio.get(entry.clip_id);
  assert.deepEqual(validateClipBytes(entry, bytes), []);
  const flipped = Buffer.from(bytes);
  flipped[100] ^= 0xff;
  assert.match(validateClipBytes(entry, flipped).join("\n"), /sha256/);
  const longer = Buffer.concat([bytes, Buffer.alloc(32000)]);
  const longEntry = {
    ...entry,
    sha256: crypto.createHash("sha256").update(longer).digest("hex"),
  };
  assert.match(validateClipBytes(longEntry, longer).join("\n"), /RIFF|ms/);
  const stereo = Buffer.from(bytes);
  stereo.writeUInt16LE(2, 22);
  const stereoEntry = {
    ...entry,
    sha256: crypto.createHash("sha256").update(stereo).digest("hex"),
  };
  assert.match(
    validateClipBytes(stereoEntry, stereo).join("\n"),
    /is not the declared/,
  );
  const silent = wavBytes(new Array(bytes.length / 2 - 22).fill(0));
  const silentEntry = {
    ...entry,
    sha256: crypto.createHash("sha256").update(silent).digest("hex"),
    duration_ms: Math.round(((bytes.length / 2 - 22) / 16000) * 1000),
  };
  assert.match(validateClipBytes(silentEntry, silent).join("\n"), /silent/);
  assert.match(
    validateClipBytes(entry, Buffer.from("not a wav")).join("\n"),
    /RIFF/,
  );
  const text = structuredClone(synthetic.manifest);
  text.clips[1].source_text = "different words";
  assert.match(
    validateClipSet(bundles.coding.schedule, text, synthetic.audio).join("\n"),
    /manifest_sha256|source text differs/,
  );
  const missing = new Map(synthetic.audio);
  missing.delete(entry.clip_id);
  assert.match(
    validateClipSet(bundles.coding.schedule, synthetic.manifest, missing).join(
      "\n",
    ),
    /audio missing/,
  );
});

test("ffmpeg is a required preflight", () => {
  const absent = ffmpegPreflight({
    run: () => ({ error: { code: "ENOENT" }, status: null }),
  });
  assert.equal(absent.ok, false);
  assert.match(absent.error, /required before launch/);
  const impostor = ffmpegPreflight({
    run: () => ({ status: 0, stdout: "something else\n" }),
  });
  assert.equal(impostor.ok, false);
  const present = ffmpegPreflight({
    run: () => ({ status: 0, stdout: "ffmpeg version 6.0\n" }),
  });
  assert.deepEqual(present, { ok: true, version: "ffmpeg version 6.0" });
  assert.equal(
    ffmpegDecodes("x", { run: () => ({ status: 0, stderr: "" }) }),
    true,
  );
  assert.equal(
    ffmpegDecodes("x", { run: () => ({ status: 0, stderr: "Invalid data" }) }),
    false,
  );
  assert.equal(
    ffmpegDecodes("x", { run: () => ({ status: 1, stderr: "" }) }),
    false,
  );
});

test("real ffmpeg decodes the synthetic clips when the host has it", (t) => {
  const bin = process.env.FFMPEG || "ffmpeg";
  const preflight = ffmpegPreflight({ bin });
  if (!preflight.ok) {
    t.skip(`ffmpeg is not on this host: ${preflight.error}`);
    return;
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lis-ff-"));
  try {
    for (const entry of synthetic.manifest.clips.slice(0, 3)) {
      const file = path.join(dir, entry.file);
      fs.writeFileSync(file, synthetic.audio.get(entry.clip_id));
      assert.equal(ffmpegDecodes(file, { bin }), true, entry.clip_id);
    }
    const bad = path.join(dir, "bad.wav");
    fs.writeFileSync(bad, "not audio at all");
    assert.equal(ffmpegDecodes(bad, { bin }), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("playback is controller clocked, fits its slots and never loops", () => {
  const plan = playbackPlan(bundles.coding.schedule, synthetic.manifest);
  assert.deepEqual(validatePlaybackPlan(plan, bundles.coding.schedule), []);
  assert.ok(plan.every((e) => e.loop === false && e.clock === "controller"));
  assert.equal(new Set(plan.map((e) => e.clip_id)).size, plan.length);
  assert.equal(plan.filter((e) => e.conditional).length, 17);
  // Mutations of the plan are caught.
  const looped = plan.map((e, i) => (i === 3 ? { ...e, loop: true } : e));
  assert.match(
    validatePlaybackPlan(looped, bundles.coding.schedule).join("\n"),
    /looping/,
  );
  const clocked = plan.map((e, i) =>
    i === 3 ? { ...e, clock: "browser" } : e,
  );
  assert.match(
    validatePlaybackPlan(clocked, bundles.coding.schedule).join("\n"),
    /controller-clocked/,
  );
  const overrun = plan.map((e, i) =>
    i === 0 ? { ...e, end_s: e.start_s + 60 } : e,
  );
  assert.match(
    validatePlaybackPlan(overrun, bundles.coding.schedule).join("\n"),
    /overruns/,
  );
});

test("transmitted clip IDs are checked against the plan", () => {
  const plan = playbackPlan(bundles.coding.schedule, synthetic.manifest);
  const sent = plan
    .filter((e) => !e.conditional)
    .map((e) => ({
      clip_id: e.clip_id,
      action_id: e.action_id,
      started_at_s: e.start_s + 0.4,
    }));
  const clean = verifyTransmitted(plan, sent);
  assert.deepEqual(clean.errors, []);
  // Every barge slot is uncovered, never silently passed.
  assert.equal(clean.uncovered.length, 17);
  const looped = verifyTransmitted(plan, [...sent, sent[2]]);
  assert.match(
    looped.errors.join("\n"),
    /transmitted 2 times; clips never loop/,
  );
  const dropped = verifyTransmitted(plan, sent.slice(1));
  assert.match(dropped.errors.join("\n"), /planned but never transmitted/);
  const early = verifyTransmitted(
    plan,
    sent.map((s, i) =>
      i === 4 ? { ...s, started_at_s: s.started_at_s - 40 } : s,
    ),
  );
  assert.match(early.errors.join("\n"), /started at/);
  const stranger = verifyTransmitted(plan, [
    ...sent,
    { clip_id: "nope", action_id: "x", started_at_s: 1 },
  ]);
  assert.match(stranger.errors.join("\n"), /transmitted but not planned/);
  const barge = plan.find((e) => e.conditional);
  const covered = verifyTransmitted(plan, [
    ...sent,
    {
      clip_id: barge.clip_id,
      action_id: barge.action_id,
      started_at_s: barge.start_s + 12,
    },
  ]);
  assert.equal(covered.uncovered.length, 16);
  assert.deepEqual(covered.errors, []);
});

test("the schedule never injects expected answers or transcript text as speech", () => {
  const { schedule, answer_key: answerKey } = bundles.coding;
  // The worker applies a revision, so the revision steps carry its marker; no
  // spoken clip or checkpoint does, because the marker is what a reply must
  // read from the editor or board.
  const spokenAndJudged = JSON.stringify([
    schedule.clips,
    schedule.checkpoints,
  ]);
  for (const f of answerKey.facts.filter((f) => f.id.startsWith("revision_"))) {
    assert.ok(!spokenAndJudged.includes(f.match.tokens[0]), f.id);
  }
  for (const clip of schedule.clips) {
    assert.ok(!/rev\d\d-[0-9a-f]{6}/.test(clip.source_text), clip.clip_id);
  }
  assert.ok(!("facts" in schedule) && !("elements" in schedule));
});

test("the CLI writes a schedule, an answer key and labelled synthetic clips", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "lis-cli-"));
  try {
    const run = spawnSync(
      process.execPath,
      [
        path.join(ROOT, "scripts", "long-interview-schedule.cjs"),
        "generate",
        "--mode",
        "coding",
        "--duration-min",
        "10",
        "--target-s",
        "600",
        "--out",
        dir,
        "--synthetic-clips",
        "1",
      ],
      { encoding: "utf8" },
    );
    assert.equal(run.status, 0, run.stderr);
    assert.match(run.stderr, /contain no speech/);
    const schedule = JSON.parse(
      fs.readFileSync(path.join(dir, "schedule.json"), "utf8"),
    );
    assert.equal(run.stdout.trim(), schedule.schedule_sha256);
    const manifest = JSON.parse(
      fs.readFileSync(path.join(dir, "clips", "manifest.json"), "utf8"),
    );
    assert.equal(manifest.contains_speech, false);
    assert.equal(
      fs.readdirSync(path.join(dir, "clips")).length,
      schedule.clips.length + 1,
    );
    const bad = spawnSync(
      process.execPath,
      [path.join(ROOT, "scripts", "long-interview-schedule.cjs"), "bogus"],
      {
        encoding: "utf8",
      },
    );
    assert.equal(bad.status, 2);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
