// The deterministic stimulus schedule of BROWSER_CHECK_FLOW=long-interview,
// the answer key that judges it, and the finite audio clips it plays.
//
// A long interview that stays connected can still have lost the interview: the
// constraints, the stage, the code on the screen. So every five minutes the
// schedule asks a question whose right answer exists only in the interview as
// it has run, and an offline judge decides from recorded evidence whether the
// reply used it. A reply that repeats a code word proves the model heard the
// question, not that it knows the interview, so the nonce is one required
// element among several, and a claim about the editor, the board or an earlier
// remark also needs the matching tool call, by ID, and a result that carries
// the current revision.
//
// Two halves, deliberately separate. The schedule is what the worker plays:
// action IDs, times, clip text, response windows. The answer key is what only
// the judge reads: the facts, the tokens or reviewed rubrics that count as
// right, the revision markers. The worker never needs the answer key and the
// schedule never injects it as a reply. Both are generated from the validated
// observation target of scripts/long-interview-plan.cjs, canonicalised and
// hashed, and the authored catalogue below is pinned by hash so that editing a
// question without bumping the version fails at load.
//
// Pure functions and no browser, so the schedule, the judge and the clip
// checks all run without credentials. Nothing here claims the schedule has run.
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const SCHEDULE_VERSION = 1;
const CYCLE_S = 300;
/// The end and report reservation; no stimulus is scheduled inside it.
const STOP_BEFORE_END_S = 120;
const FINAL_BEFORE_WINDOW_END_S = 30;
const REPLY_WINDOW_S = 45;
const MIN_RECALL_GAP_S = 1800;
const THINKING_HOLD_S = 10;
const MAX_CLIP_SPEECH_S = 20;
const TRAILING_QUIET_S = 5;
const SPEECH_WORDS_PER_S = 2.5;
const JOIN_ACTION_ID = "candidate_join";

const CLIP_FORMAT = Object.freeze({
  container: "wav",
  codec: "pcm_s16le",
  sample_rate: 16000,
  channels: 1,
});

const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

const TOOL_BY_MODE = { coding: "read_editor", whiteboard: "read_board" };
const TOOL_CONVERSATION = "read_conversation";

/// Everything a person wrote, as data. Changing any of it changes
/// AUTHORED_SHA256, which a test and the load-time check pin.
const AUTHORED = Object.freeze({
  version: SCHEDULE_VERSION,
  phases: {
    coding: ["algorithm", "coding", "test", "optimizations"],
    whiteboard: ["repeat", "example", "algorithm", "optimizations"],
  },
  nonce_words: {
    first: [
      "amber",
      "birch",
      "cobalt",
      "dune",
      "ember",
      "fjord",
      "garnet",
      "harbor",
      "indigo",
      "juniper",
      "kestrel",
      "lantern",
      "meadow",
      "nickel",
      "onyx",
      "pewter",
    ],
    second: [
      "falcon",
      "otter",
      "badger",
      "heron",
      "lynx",
      "marten",
      "osprey",
      "plover",
      "quail",
      "raven",
      "stoat",
      "tern",
      "vole",
      "weasel",
      "yak",
      "zebra",
    ],
  },
  seeds: [
    {
      id: "seed_codename",
      cycle: 1,
      sentence:
        "For the record, my codename for this attempt is saffron heron.",
      recall_question: "What codename did I give for this attempt?",
      match: { mode: "folded", tokens: ["saffron heron", "saffron-heron"] },
    },
    {
      id: "seed_edge_case",
      cycle: 2,
      sentence: "The edge case I promised to test is a pair of zeros.",
      recall_question: "Which edge case did I promise to test?",
      match: {
        mode: "folded",
        tokens: ["pair of zeros", "two zeros", "zeros pair"],
      },
    },
  ],
  constraints: {
    sentence:
      "My constraints are at most ten thousand numbers, exactly one valid pair, and never reuse the same element.",
    review: {
      id: "constraints-v1",
      reviewer: "schedule author",
      status: "reviewed",
    },
    concepts: [
      ["ten thousand", "10000", "10,000", "10^4"],
      ["exactly one", "one valid pair", "single valid pair", "one pair"],
      [
        "same element",
        "reuse",
        "twice",
        "distinct indices",
        "different indices",
      ],
    ],
  },
  duplicate_case: {
    review: {
      id: "duplicate-v1",
      reviewer: "schedule author",
      status: "reviewed",
    },
    concepts: [
      [
        "different indices",
        "distinct indices",
        "two different",
        "two distinct",
        "separate indices",
        "not the same index",
      ],
    ],
  },
  pending: {
    id: "pending_question",
    sentence:
      "One question is still open for you: should I return the two positions in ascending order?",
    match: { mode: "folded", tokens: ["ascending order", "ascending"] },
  },
  say_loop: [
    "For each value I check the complement",
    "before adding the current index to the map.",
  ],
  inspect: {
    coding:
      "Please read the current source and tell me whether the duplicate value case uses two different indices.",
    whiteboard:
      "Please read the current diagram and tell me whether the duplicate value case uses two different indices.",
  },
  prime: "Could you compare the map approach with checking every pair?",
  barge:
    "Let me clarify the example first. I am still explaining my reasoning.",
  state_question:
    "What are the original constraints, our current stage, and the latest {thing}?",
  final_question:
    "Before we wrap up, what is the question I still have open, and what is the latest {thing}?",
  things: { coding: "source revision", whiteboard: "diagram revision" },
});

const canonicalize = (value) => {
  if (Array.isArray(value)) return `[${value.map(canonicalize).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalize(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
};
const sha256 = (data) => crypto.createHash("sha256").update(data).digest("hex");
const hashOf = (value) => sha256(canonicalize(value));

/// Pinned. Edit the catalogue above, bump SCHEDULE_VERSION, and replace this.
const AUTHORED_SHA256 =
  "323220654cdf1c5ce18e4bd3a739c9b45557f891e94536a92720035c1deceb1f";

function checkAuthoredPin(expected = AUTHORED_SHA256) {
  const actual = hashOf(AUTHORED);
  if (actual !== expected) {
    throw new Error(
      `the authored schedule catalogue changed (sha256 ${actual}); bump SCHEDULE_VERSION and its pin`,
    );
  }
  return actual;
}

const wordCount = (text) => text.trim().split(/\s+/).length;
const speechSeconds = (text) => Math.ceil(wordCount(text) / SPEECH_WORDS_PER_S);

function pad2(n) {
  return String(n).padStart(2, "0");
}

function nonceFor(mode, checkpointId) {
  const digest = crypto
    .createHash("sha256")
    .update(`${SCHEDULE_VERSION}|${mode}|${checkpointId}`)
    .digest();
  const { first, second } = AUTHORED.nonce_words;
  return `${first[digest[0] % first.length]} ${second[digest[1] % second.length]}`;
}

function markerFor(mode, index) {
  return `rev${pad2(index)}-${sha256(`${SCHEDULE_VERSION}|${mode}|rev|${index}`).slice(0, 6)}`;
}

/// The declared content of revision `index`. The marker is the only thing a
/// reply or a tool result must carry; the rest is what a candidate would type
/// or draw.
function revisionFixture(mode, index) {
  const marker = markerFor(mode, index);
  if (mode === "coding") {
    const source = [
      `// ${marker}`,
      "function matchDisputedCharge(nums, target) {",
      "  const seen = new Map();",
      "  for (let i = 0; i < nums.length; i++) {",
      "    const need = target - nums[i];",
      "    if (seen.has(need)) return [seen.get(need), i];",
      "    seen.set(nums[i], i);",
      "  }",
      "  return [];",
      "}",
      "",
    ].join("\n");
    return { marker, kind: "editor", language: "javascript", source };
  }
  return {
    kind: "board",
    marker,
    label: `complement map example ${marker}`,
    strokes: ["box nums", "arrow need", "box seen", `label ${marker}`],
  };
}

function factOf(id, seededAtS, text, match) {
  const fact = { id, seeded_at_s: seededAtS, text, match };
  return { ...fact, sha256: hashOf(fact) };
}

function buildTimeline(durationMin, observationTargetS) {
  if (!Number.isSafeInteger(durationMin) || durationMin <= 0) {
    throw new Error("durationMin must be a positive integer");
  }
  if (!Number.isSafeInteger(observationTargetS) || observationTargetS <= 0) {
    throw new Error("observationTargetS must be a positive integer");
  }
  const observed = Math.min(observationTargetS, durationMin * 60);
  const window = observed - STOP_BEFORE_END_S;
  const cycles = Math.floor(window / CYCLE_S);
  const finalAt = window - FINAL_BEFORE_WINDOW_END_S;
  if (cycles < 1 || finalAt < cycles * CYCLE_S) {
    throw new Error(
      `an observation target of ${observationTargetS}s leaves no room for a cycle and a final checkpoint`,
    );
  }
  return { window, cycles, finalAt };
}

function pickPhase(mode, cycle, cycles) {
  const phases = AUTHORED.phases[mode];
  return phases[
    Math.min(
      phases.length - 1,
      Math.floor(((cycle - 1) * phases.length) / cycles),
    )
  ];
}

/// Which cycle's checkpoint recalls which seeded fact: the first cycles whose
/// checkpoint falls at least MIN_RECALL_GAP_S after the seed, one fact per
/// cycle, in two rounds so a long run recalls each seed twice.
function recallPlan(cycles) {
  const plan = new Map();
  const checkpointAt = (cycle) => (cycle - 1) * CYCLE_S + 240;
  const seedAt = (seed) => (seed.cycle - 1) * CYCLE_S;
  for (let round = 0; round < 2; round++) {
    let floorCycle = round === 0 ? 1 : Math.ceil(cycles * 0.75);
    for (const seed of AUTHORED.seeds) {
      for (let cycle = floorCycle; cycle <= cycles; cycle++) {
        if (plan.has(cycle)) continue;
        if (checkpointAt(cycle) - seedAt(seed) < MIN_RECALL_GAP_S) continue;
        plan.set(cycle, seed.id);
        floorCycle = cycle + 1;
        break;
      }
    }
  }
  return plan;
}

const rubricMatch = (spec) => ({
  mode: "rubric",
  concepts: spec.concepts,
  review: spec.review,
});

/// The schedule and its answer key for `mode`, derived from `plan`
/// ({durationMin, observationTargetS}, as long-interview-plan.cjs validated).
function generateSchedule(mode, plan) {
  checkAuthoredPin();
  if (!TOOL_BY_MODE[mode]) {
    throw new Error(`mode must be coding or whiteboard, not ${mode}`);
  }
  const { durationMin, observationTargetS } = plan;
  const { window, cycles, finalAt } = buildTimeline(
    durationMin,
    observationTargetS,
  );
  const tool = TOOL_BY_MODE[mode];
  const thing = AUTHORED.things[mode];
  const recalls = recallPlan(cycles);

  const steps = [{ action_id: JOIN_ACTION_ID, at_s: 0, kind: "join" }];
  const clips = [];
  const checkpoints = [];
  const keyCheckpoints = [];
  const facts = [];
  const seedFacts = new Map();

  const addClip = (clipId, role, parts, atS) => {
    const text = parts.join(" ");
    const speechS = speechSeconds(text);
    if (speechS > MAX_CLIP_SPEECH_S) {
      throw new Error(
        `clip ${clipId} would speak for ${speechS}s, past ${MAX_CLIP_SPEECH_S}s`,
      );
    }
    clips.push({
      clip_id: clipId,
      role,
      parts,
      source_text: text,
      speech_s: speechS,
      internal_pause_s: parts.length > 1 ? 2 : 0,
      trailing_quiet_s: TRAILING_QUIET_S,
    });
    return speechS;
  };

  const constraints = factOf(
    "constraints",
    0,
    AUTHORED.constraints.sentence,
    rubricMatch(AUTHORED.constraints),
  );
  const duplicate = factOf(
    "duplicate_case",
    0,
    "The duplicate value case must use two different indices.",
    rubricMatch(AUTHORED.duplicate_case),
  );
  facts.push(constraints, duplicate);
  for (const seed of AUTHORED.seeds) {
    const fact = factOf(
      seed.id,
      (seed.cycle - 1) * CYCLE_S,
      seed.sentence,
      seed.match,
    );
    seedFacts.set(seed.id, fact);
    facts.push(fact);
  }

  const revisionsByCycle = [];
  const phaseByCycle = [];
  for (let cycle = 1; cycle <= cycles; cycle++) {
    const c = pad2(cycle);
    const base = (cycle - 1) * CYCLE_S;
    const phase = pickPhase(mode, cycle, cycles);
    phaseByCycle.push(phase);
    const revision = revisionFixture(mode, cycle);
    revisionsByCycle.push(revision);
    const phaseFact = factOf(
      `phase_${c}`,
      base,
      `We are in the ${phase} stage.`,
      { mode: "folded", tokens: [phase] },
    );
    const revisionFact = factOf(
      `revision_${c}`,
      base + 110,
      `The latest ${thing} is ${revision.marker}.`,
      { mode: "exact", tokens: [revision.marker] },
    );
    facts.push(phaseFact, revisionFact);

    const planParts = [`Stage ${cycle}. We are now in the ${phase} stage.`];
    if (cycle === 1) planParts.push(AUTHORED.constraints.sentence);
    for (const seed of AUTHORED.seeds) {
      if (seed.cycle === cycle) planParts.push(seed.sentence);
    }
    if (planParts.length === 1) {
      planParts.push("I continue from the approach I described.");
    }
    const loopParts = [...AUTHORED.say_loop];
    if (cycle === cycles) loopParts.push(AUTHORED.pending.sentence);

    const id = (name) => `c${c}_${name}`;
    steps.push(
      {
        action_id: id("say_plan"),
        at_s: base,
        kind: "speech",
        clip_id: id("say_plan"),
      },
      {
        action_id: id("say_loop"),
        at_s: base + 45,
        kind: "speech",
        clip_id: id("say_loop"),
      },
      {
        action_id: id("think_start"),
        at_s: base + 80,
        kind: "thinking_start",
        hold_s: THINKING_HOLD_S,
      },
      {
        action_id: id("think_end"),
        at_s: base + 80 + THINKING_HOLD_S,
        kind: "thinking_end",
      },
      {
        action_id: id("revise"),
        at_s: base + 110,
        kind: "revision",
        revision: {
          index: cycle,
          marker: revision.marker,
          fixture_sha256: hashOf(revision),
        },
      },
      {
        action_id: id("ask_inspect"),
        at_s: base + 125,
        kind: "checkpoint_speech",
        clip_id: id("ask_inspect"),
        checkpoint_id: id("cp_inspect"),
      },
      {
        action_id: id("prime"),
        at_s: base + 175,
        kind: "barge_prime",
        clip_id: id("prime"),
      },
      {
        action_id: id("barge"),
        at_s: base + 175,
        kind: "barge",
        conditional: true,
        wait_for_agent_speaking_max_s: 10,
        // The prime clip (12s), its quiet (5s) and the 10s wait for Speaking.
        latest_s: base + 202,
        clip_id: id("barge"),
        if_not_speaking: "uncovered",
      },
      {
        action_id: id("checkpoint"),
        at_s: base + 240,
        kind: "checkpoint_speech",
        clip_id: id("checkpoint"),
        checkpoint_id: id("cp_state"),
      },
    );
    addClip(id("say_plan"), "seed", planParts, base);
    addClip(id("say_loop"), "seed", loopParts, base + 45);

    const inspectNonce = nonceFor(mode, id("cp_inspect"));
    const stateNonce = nonceFor(mode, id("cp_state"));
    const withNonce = (text, nonce) =>
      `${text} Please also say the code words ${nonce}.`;
    addClip(
      id("ask_inspect"),
      "checkpoint",
      [withNonce(AUTHORED.inspect[mode], inspectNonce)],
      base + 125,
    );
    addClip(id("prime"), "barge_prime", [AUTHORED.prime], base + 175);
    addClip(id("barge"), "barge", [AUTHORED.barge], base + 175);

    const recallId = recalls.get(cycle);
    const stateQuestion = AUTHORED.state_question.replace("{thing}", thing);
    const recallSeed = recallId
      ? AUTHORED.seeds.find((s) => s.id === recallId)
      : null;
    const stateText = recallSeed
      ? `${stateQuestion} Also, ${recallSeed.recall_question.charAt(0).toLowerCase()}${recallSeed.recall_question.slice(1)}`
      : stateQuestion;
    addClip(
      id("checkpoint"),
      "checkpoint",
      [withNonce(stateText, stateNonce)],
      base + 240,
    );

    const durationOf = (clipId) => {
      const clip = clips.find((c2) => c2.clip_id === clipId);
      return clip.speech_s + clip.internal_pause_s;
    };
    const nextInput = (atS, nextS) => Math.min(REPLY_WINDOW_S, nextS - atS);

    const inspectAt = base + 125;
    const inspectEnd = inspectAt + durationOf(id("ask_inspect"));
    checkpoints.push({
      checkpoint_id: id("cp_inspect"),
      kind: "tool_continuation",
      at_s: inspectAt,
      stimulus_end_s: inspectEnd,
      response_window_s: nextInput(inspectEnd, base + 175),
      clip_id: id("ask_inspect"),
      cycle,
      source_fact_ids: ["duplicate_case", revisionFact.id],
      required_tool: { name: tool, call_id_required: true },
      required_tools: [tool],
    });
    keyCheckpoints.push({
      checkpoint_id: id("cp_inspect"),
      nonce: inspectNonce,
      element_fact_ids: ["duplicate_case", revisionFact.id],
      tool: {
        name: tool,
        result_fact_ids: [revisionFact.id],
        not_before_s: base + 110,
      },
    });

    const stateAt = base + 240;
    const stateEnd = stateAt + durationOf(id("checkpoint"));
    const stateFacts = ["constraints", phaseFact.id, revisionFact.id];
    const tools = [
      {
        name: tool,
        result_fact_ids: [revisionFact.id],
        not_before_s: base + 110,
      },
    ];
    if (recallId) {
      stateFacts.push(recallId);
      tools.push({
        name: TOOL_CONVERSATION,
        result_fact_ids: [recallId],
        not_before_s: 0,
      });
    }
    checkpoints.push({
      checkpoint_id: id("cp_state"),
      kind: recallId ? "recall" : "state",
      at_s: stateAt,
      stimulus_end_s: stateEnd,
      response_window_s: nextInput(
        stateEnd,
        cycle < cycles ? base + CYCLE_S : finalAt,
      ),
      clip_id: id("checkpoint"),
      cycle,
      source_fact_ids: stateFacts,
      required_tool: { name: tool, call_id_required: true },
      ...(recallId
        ? {
            recall: {
              fact_id: recallId,
              seeded_at_s: seedFacts.get(recallId).seeded_at_s,
              gap_s: stateAt - seedFacts.get(recallId).seeded_at_s,
            },
            required_tools: [tool, TOOL_CONVERSATION],
          }
        : { required_tools: [tool] }),
    });
    keyCheckpoints.push({
      checkpoint_id: id("cp_state"),
      nonce: stateNonce,
      element_fact_ids: stateFacts,
      tools,
    });
  }

  // The checkpoint reply window must not run into the next stimulus.
  const pendingFact = factOf(
    AUTHORED.pending.id,
    (cycles - 1) * CYCLE_S + 45,
    AUTHORED.pending.sentence,
    AUTHORED.pending.match,
  );
  facts.push(pendingFact);
  const finalNonce = nonceFor(mode, "cp_final");
  const finalText = `${AUTHORED.final_question.replace("{thing}", thing)} Please also say the code words ${finalNonce}.`;
  const finalSpeech = addClip(
    "final_checkpoint",
    "checkpoint",
    [finalText],
    finalAt,
  );
  const lastRevisionFact = `revision_${pad2(cycles)}`;
  const finalPhaseFact = `phase_${pad2(cycles)}`;
  steps.push({
    action_id: "final_checkpoint",
    at_s: finalAt,
    kind: "checkpoint_speech",
    clip_id: "final_checkpoint",
    checkpoint_id: "cp_final",
  });
  const finalFacts = [pendingFact.id, finalPhaseFact, lastRevisionFact];
  checkpoints.push({
    checkpoint_id: "cp_final",
    kind: "final",
    at_s: finalAt,
    stimulus_end_s: finalAt + finalSpeech,
    response_window_s: REPLY_WINDOW_S,
    clip_id: "final_checkpoint",
    cycle: cycles,
    source_fact_ids: finalFacts,
    required_tool: { name: tool, call_id_required: true },
    required_tools: [tool],
  });
  keyCheckpoints.push({
    checkpoint_id: "cp_final",
    nonce: finalNonce,
    element_fact_ids: finalFacts,
    tools: [
      {
        name: tool,
        result_fact_ids: [lastRevisionFact],
        not_before_s: (cycles - 1) * CYCLE_S + 110,
      },
    ],
  });

  steps.sort((a, b) => a.at_s - b.at_s);
  const actions = steps.map((s) => s.action_id);

  const answerKey = {
    version: SCHEDULE_VERSION,
    mode,
    facts,
    checkpoints: keyCheckpoints,
  };
  const schedule = {
    version: SCHEDULE_VERSION,
    authored_sha256: AUTHORED_SHA256,
    mode,
    problem: "two-sum",
    duration_min: durationMin,
    observation_target_s: observationTargetS,
    observation_window_s: window,
    cycles,
    final_at_s: finalAt,
    clock: "controller monotonic seconds since the candidate joined",
    steps,
    clips,
    checkpoints,
    revisions: revisionsByCycle.map((r, i) => ({
      index: i + 1,
      marker: r.marker,
      fixture_sha256: hashOf(r),
    })),
    phases: phaseByCycle,
    actions,
    answer_key_sha256: hashOf(answerKey),
  };
  schedule.schedule_sha256 = hashOf(schedule);
  return { schedule, answer_key: answerKey };
}

/// Every reason a schedule is not the one the generator would write, empty
/// when it is. Checks the structure the run depends on, not just the hash.
function validateSchedule(schedule, answerKey) {
  const errors = [];
  const withoutHash = { ...schedule };
  delete withoutHash.schedule_sha256;
  if (hashOf(withoutHash) !== schedule.schedule_sha256) {
    errors.push("schedule_sha256 does not match the schedule");
  }
  if (answerKey && hashOf(answerKey) !== schedule.answer_key_sha256) {
    errors.push("answer_key_sha256 does not match the answer key");
  }
  if (schedule.authored_sha256 !== AUTHORED_SHA256) {
    errors.push("the schedule was written from a different authored catalogue");
  }
  try {
    const again = generateSchedule(schedule.mode, {
      durationMin: schedule.duration_min,
      observationTargetS: schedule.observation_target_s,
    });
    if (hashOf(again.schedule) !== hashOf(schedule)) {
      errors.push(
        "the schedule is not the one the generator writes for its mode and target",
      );
    }
    if (answerKey && hashOf(again.answer_key) !== hashOf(answerKey)) {
      errors.push("the answer key is not the one the generator writes");
    }
  } catch (error) {
    errors.push(`the schedule cannot be regenerated: ${error.message}`);
  }
  const seen = new Set();
  for (const id of schedule.actions) {
    if (!ID_PATTERN.test(id)) errors.push(`action ID ${id} is not a plain ID`);
    if (id === "end")
      errors.push("the reserved end permit is not a schedule action");
    if (seen.has(id)) errors.push(`duplicate action ID ${id}`);
    seen.add(id);
  }
  if (schedule.actions[0] !== JOIN_ACTION_ID) {
    errors.push(`the first action must be ${JOIN_ACTION_ID}`);
  }
  const times = schedule.steps.map((s) => s.at_s);
  if (times.some((t, i) => i > 0 && t < times[i - 1])) {
    errors.push("steps are not in time order");
  }
  if (schedule.steps.some((s) => s.at_s > schedule.observation_window_s)) {
    errors.push("a step falls inside the end and report reservation");
  }
  const cps = schedule.checkpoints.filter(
    (c) => c.kind !== "tool_continuation",
  );
  for (let cycle = 1; cycle <= schedule.cycles; cycle++) {
    const at = (cycle - 1) * CYCLE_S + 240;
    if (!cps.some((c) => c.at_s === at)) {
      errors.push(`no checkpoint at ${at}s`);
    }
  }
  const required = ["tool_continuation", "final"];
  for (const kind of required) {
    if (!schedule.checkpoints.some((c) => c.kind === kind))
      errors.push(`no ${kind} checkpoint`);
  }
  for (const kind of ["thinking_start", "barge", "revision"]) {
    if (!schedule.steps.some((s) => s.kind === kind))
      errors.push(`no ${kind} step`);
  }
  const recalls = schedule.checkpoints.filter((c) => c.recall);
  if (recalls.some((c) => c.recall.gap_s < MIN_RECALL_GAP_S)) {
    errors.push(`a recall is closer than ${MIN_RECALL_GAP_S}s to its seed`);
  }
  if (schedule.duration_min >= 60 && recalls.length < 2) {
    errors.push(
      `a ${schedule.duration_min} minute schedule needs at least two long-gap recalls, has ${recalls.length}`,
    );
  }
  for (const checkpoint of schedule.checkpoints) {
    if (!checkpoint.source_fact_ids?.length)
      errors.push(`${checkpoint.checkpoint_id} has no source fact`);
    if (!(checkpoint.response_window_s > 0))
      errors.push(`${checkpoint.checkpoint_id} has no response window`);
    if (!checkpoint.required_tool?.call_id_required)
      errors.push(`${checkpoint.checkpoint_id} requires no tool call ID`);
  }
  return errors;
}

/// The budget manifest fields a run built on `schedule` must carry.
function budgetFields(schedule, clipManifest) {
  if (!clipManifest) {
    throw new Error(
      "the run manifest pins the clip manifest: pass the one the run will use",
    );
  }
  const { manifest_sha256: pinned, ...rest } = clipManifest;
  if (hashOf(rest) !== pinned) {
    throw new Error("the clip manifest does not match its own hash");
  }
  if (clipManifest.schedule_sha256 !== schedule.schedule_sha256) {
    throw new Error("the clip manifest belongs to a different schedule");
  }
  return {
    actions: [...schedule.actions],
    action_ceiling: schedule.actions.length + 1,
    fixture_sha256: hashOf({
      schedule_sha256: schedule.schedule_sha256,
      clip_manifest_sha256: clipManifest.manifest_sha256,
    }),
  };
}

// --------------------------------------------------------------------------
// The judge
// --------------------------------------------------------------------------

const escapeRegExp = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const fold = (s) =>
  String(s).normalize("NFKC").toLowerCase().replace(/\s+/g, " ");

function containsToken(text, token, caseFolded) {
  const haystack = caseFolded ? fold(text) : String(text);
  const needle = caseFolded ? fold(token) : token;
  const pattern = new RegExp(
    `(?<![A-Za-z0-9])${escapeRegExp(needle).replace(/ /g, "\\s+")}(?![A-Za-z0-9])`,
  );
  return pattern.test(haystack);
}

/// Whether `text` satisfies a fact's match, and why not. A rubric needs every
/// concept; a token list needs any one alternative.
function matchFact(match, text) {
  if (match.mode === "exact" || match.mode === "folded") {
    const hit = match.tokens.some((t) =>
      containsToken(text, t, match.mode === "folded"),
    );
    return hit ? [] : [`none of the allowed ${match.mode} tokens appear`];
  }
  if (match.mode === "rubric") {
    if (match.review?.status !== "reviewed")
      return ["the rubric is not reviewed"];
    const misses = match.concepts
      .map((concept, i) =>
        concept.some((p) => containsToken(text, p, true)) ? null : i + 1,
      )
      .filter((i) => i !== null);
    return misses.length
      ? [`rubric concept ${misses.join(", ")} is missing`]
      : [];
  }
  return [`unknown match mode ${match.mode}`];
}

function factIntegrity(fact) {
  const { sha256: pinned, ...rest } = fact;
  return hashOf(rest) === pinned;
}

/// Judges one checkpoint from recorded evidence.
///
/// evidence: {
///   checkpoint_id,
///   reply: {text, started_at_s, completed, interrupted, tool_call_ids},
///   tool_calls: [{call_id, name, at_s, ok, result_text}],
/// }
/// Returns {verdict: "pass" | "fail", reasons}. `facts` defaults to the answer
/// key's and is a parameter so a fixture can withhold or corrupt one.
function judgeCheckpoint(bundle, evidence, options = {}) {
  const { schedule, answer_key: answerKey } = bundle;
  const facts = new Map(
    (options.facts ?? answerKey.facts).map((f) => [f.id, f]),
  );
  const reasons = [];
  const fail = (reason) => reasons.push(reason);
  const cp = schedule.checkpoints.find(
    (c) => c.checkpoint_id === evidence.checkpoint_id,
  );
  const key = answerKey.checkpoints.find(
    (c) => c.checkpoint_id === evidence.checkpoint_id,
  );
  if (!cp || !key) return { verdict: "fail", reasons: ["unknown checkpoint"] };
  if (hashOf(answerKey) !== schedule.answer_key_sha256) {
    fail("the answer key is not the one the schedule pins");
  }
  const keyTools = (key.tools ?? [key.tool]).map((t) => t.name).sort();
  if (canonicalize(keyTools) !== canonicalize([...cp.required_tools].sort())) {
    fail("the answer key does not require the tool reads the schedule does");
  }

  for (const id of new Set([
    ...key.element_fact_ids,
    ...(key.tools ?? [key.tool]).flatMap((t) => t.result_fact_ids),
  ])) {
    const fact = facts.get(id);
    if (!fact) fail(`expected fact ${id} is withheld`);
    else if (!factIntegrity(fact)) fail(`expected fact ${id} is corrupted`);
  }
  const factOfId = (id) => facts.get(id);

  const reply = evidence.reply;
  if (!reply || typeof reply.text !== "string" || reply.text === "") {
    fail("no reply was recorded");
    return { verdict: "fail", reasons };
  }
  if (reply.completed !== true) fail("the reply did not complete");
  if (reply.interrupted === true) fail("the reply was interrupted");
  const from = cp.stimulus_end_s;
  const to = cp.stimulus_end_s + cp.response_window_s;
  if (!(reply.started_at_s >= from && reply.started_at_s <= to)) {
    fail(
      `the reply started at ${reply.started_at_s}s, outside ${from}..${to}s`,
    );
  }
  if (!Number.isFinite(reply.started_at_s)) fail("the reply has no start time");
  if (!containsToken(reply.text, key.nonce, true)) fail("the nonce is missing");

  for (const id of key.element_fact_ids) {
    const fact = factOfId(id);
    if (!fact || !factIntegrity(fact)) continue;
    const misses = matchFact(fact.match, reply.text);
    if (misses.length) fail(`reply lacks ${id}: ${misses[0]}`);
  }
  // A reply that names an earlier revision is not about the current one.
  for (const fact of answerKey.facts) {
    if (
      !/^revision_\d+$/.test(fact.id) ||
      key.element_fact_ids.includes(fact.id)
    )
      continue;
    if (factIntegrity(fact) && matchFact(fact.match, reply.text).length === 0) {
      fail(`reply states the stale revision ${fact.id}`);
    }
  }

  const tools = key.tools ?? [key.tool];
  const calls = evidence.tool_calls ?? [];
  const cited = new Set(reply.tool_call_ids ?? []);
  for (const need of tools) {
    const named = calls.filter((c) => c.name === need.name);
    if (named.length === 0) {
      fail(`no ${need.name} call was recorded`);
      continue;
    }
    let satisfied = false;
    const notes = [];
    for (const call of named) {
      if (typeof call.call_id !== "string" || call.call_id === "") {
        notes.push("a call has no ID");
      } else if (!cited.has(call.call_id)) {
        notes.push(`reply does not cite call ${call.call_id}`);
      } else if (call.ok !== true) {
        notes.push(`call ${call.call_id} did not succeed`);
      } else if (!Number.isFinite(call.at_s)) {
        notes.push(`call ${call.call_id} has no timestamp`);
      } else if (call.at_s < Math.max(need.not_before_s, cp.at_s)) {
        notes.push(
          `call ${call.call_id} was read before the revision it must show or before the question`,
        );
      } else if (call.at_s > to) {
        notes.push(`call ${call.call_id} came after the reply window`);
      } else {
        const bad = need.result_fact_ids.flatMap((id) => {
          const fact = factOfId(id);
          if (!fact || !factIntegrity(fact)) return [`${id} is unavailable`];
          return matchFact(fact.match, call.result_text ?? "").length
            ? [id]
            : [];
        });
        if (bad.length)
          notes.push(`call ${call.call_id} result lacks ${bad.join(", ")}`);
        else satisfied = true;
      }
    }
    if (!satisfied)
      fail(`no matching ${need.name} call and result: ${notes[0]}`);
  }
  return { verdict: reasons.length ? "fail" : "pass", reasons };
}

/// A faithful reply and tool trace for `checkpointId`, built from the answer
/// key. Test and smoke use only: the worker never sees this.
function answerFor(bundle, checkpointId) {
  const { schedule, answer_key: answerKey } = bundle;
  const cp = schedule.checkpoints.find((c) => c.checkpoint_id === checkpointId);
  const key = answerKey.checkpoints.find(
    (c) => c.checkpoint_id === checkpointId,
  );
  const byId = new Map(answerKey.facts.map((f) => [f.id, f]));
  const phrase = (fact) => {
    if (fact.match.mode === "rubric")
      return fact.match.concepts.map((c) => c[0]).join(", ");
    return fact.match.tokens[0];
  };
  const text = [
    key.nonce,
    ...key.element_fact_ids.map(
      (id) => `${id.replace(/_/g, " ")}: ${phrase(byId.get(id))}`,
    ),
  ].join(". ");
  const tools = key.tools ?? [key.tool];
  const calls = tools.map((need, i) => ({
    call_id: `call-${checkpointId}-${i + 1}`,
    name: need.name,
    at_s: Math.max(need.not_before_s, cp.stimulus_end_s) + 1,
    ok: true,
    result_text: need.result_fact_ids
      .map((id) => phrase(byId.get(id)))
      .join("\n"),
  }));
  return {
    checkpoint_id: checkpointId,
    reply: {
      text,
      started_at_s: cp.stimulus_end_s + 5,
      completed: true,
      interrupted: false,
      tool_call_ids: calls.map((c) => c.call_id),
    },
    tool_calls: calls,
  };
}

/// The Thinking hold: nothing the agent says may start inside it.
function judgeThinkingHold(step, evidence) {
  const end = step.at_s + step.hold_s;
  const over = (evidence.agent_speech_spans ?? []).filter(
    ([a, b]) => a < end && b > step.at_s,
  );
  return over.length
    ? {
        verdict: "fail",
        reasons: [`the agent spoke during the ${step.hold_s}s Thinking hold`],
      }
    : { verdict: "pass", reasons: [] };
}

/// The conditional barge-in. If the agent was not speaking when it came due,
/// nothing is claimed.
function judgeBarge(evidence) {
  if (
    evidence.agent_speaking_at_dispatch !== true ||
    evidence.dispatched !== true
  ) {
    return {
      verdict: "uncovered",
      reasons: ["no interruption occurred; none is claimed"],
    };
  }
  return evidence.agent_yielded === true
    ? { verdict: "pass", reasons: [] }
    : { verdict: "fail", reasons: ["the agent did not yield to the barge-in"] };
}

/// Compression claims need compression evidence; absence leaves them open.
function summarizeCompression(observations) {
  const rows = observations ?? [];
  const highWater = rows.reduce((m, r) => Math.max(m, r.usage_tokens ?? 0), 0);
  const events = rows.filter((r) => r.compression === true).length;
  return {
    usage_high_water_tokens: highWater,
    compression_events: events,
    compression_claim: events > 0 ? "observed" : "open",
  };
}

// --------------------------------------------------------------------------
// Audio clips
// --------------------------------------------------------------------------

const SAMPLE_RATE = CLIP_FORMAT.sample_rate;

function wavBytes(samples) {
  const data = Buffer.alloc(samples.length * 2);
  samples.forEach((s, i) => data.writeInt16LE(s, i * 2));
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(CLIP_FORMAT.channels, 22);
  header.writeUInt32LE(SAMPLE_RATE, 24);
  header.writeUInt32LE(SAMPLE_RATE * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

/// SYNTHETIC. A deterministic tone burst of the clip's speech length, then
/// silence for the internal pause and the trailing quiet. It contains no
/// speech: it lets the harness exercise clip hashing, timing and playback. A
/// run that asks a model to answer needs real spoken clips authored outside
/// the checkout.
function syntheticClipBytes(clip) {
  const hue = crypto.createHash("sha256").update(clip.clip_id).digest()[0];
  const freq = 300 + (hue % 16) * 40;
  const total = Math.round(
    (clip.speech_s + clip.internal_pause_s + clip.trailing_quiet_s) *
      SAMPLE_RATE,
  );
  const speech = clip.speech_s * SAMPLE_RATE;
  const samples = new Array(total).fill(0);
  const burst = (from, n) => {
    for (let i = 0; i < n; i++) {
      const fade = Math.min(1, i / 160, (n - i) / 160);
      samples[from + i] = Math.round(
        8000 * fade * Math.sin((2 * Math.PI * freq * i) / SAMPLE_RATE),
      );
    }
  };
  if (clip.parts.length > 1) {
    const first = Math.round(speech / 2);
    burst(0, first);
    burst(first + clip.internal_pause_s * SAMPLE_RATE, speech - first);
  } else {
    burst(0, speech);
  }
  return wavBytes(samples);
}

/// {sampleRate, channels, bits, format, samples: Int16Array} or {error}.
function parseWav(buf) {
  if (
    buf.length < 44 ||
    buf.toString("latin1", 0, 4) !== "RIFF" ||
    buf.toString("latin1", 8, 12) !== "WAVE"
  ) {
    return { error: "not a RIFF/WAVE file" };
  }
  let offset = 12;
  let fmt = null;
  let data = null;
  while (offset + 8 <= buf.length) {
    const id = buf.toString("latin1", offset, offset + 4);
    const size = buf.readUInt32LE(offset + 4);
    const body = offset + 8;
    if (body + size > buf.length)
      return { error: `chunk ${id} runs past the end of the file` };
    if (id === "fmt ") fmt = buf.subarray(body, body + size);
    if (id === "data") data = buf.subarray(body, body + size);
    offset = body + size + (size % 2);
  }
  if (offset !== buf.length || buf.readUInt32LE(4) + 8 !== buf.length) {
    return { error: "the file size disagrees with its RIFF header" };
  }
  if (!fmt || fmt.length < 16) return { error: "no fmt chunk" };
  if (!data) return { error: "no data chunk" };
  const audioFormat = fmt.readUInt16LE(0);
  const channels = fmt.readUInt16LE(2);
  const sampleRate = fmt.readUInt32LE(4);
  const bits = fmt.readUInt16LE(14);
  if (audioFormat !== 1)
    return { error: `format tag ${audioFormat} is not PCM` };
  if (data.length % (2 * channels) !== 0)
    return { error: "data is not whole frames" };
  const samples = new Int16Array(data.length / 2);
  for (let i = 0; i < samples.length; i++) samples[i] = data.readInt16LE(i * 2);
  return { sampleRate, channels, bits, samples };
}

const peak = (samples, from, to) => {
  let m = 0;
  for (let i = Math.max(0, from); i < Math.min(samples.length, to); i++) {
    m = Math.max(m, Math.abs(samples[i]));
  }
  return m;
};

/// Every reason `bytes` is not the clip `entry` describes.
function validateClipBytes(entry, bytes) {
  const errors = [];
  if (sha256(bytes) !== entry.sha256)
    errors.push(`${entry.clip_id}: sha256 does not match the manifest`);
  const wav = parseWav(bytes);
  if (wav.error) return [...errors, `${entry.clip_id}: ${wav.error}`];
  if (
    wav.sampleRate !== entry.format.sample_rate ||
    wav.channels !== entry.format.channels ||
    wav.bits !== 16
  ) {
    errors.push(
      `${entry.clip_id}: ${wav.sampleRate} Hz ${wav.channels} ch ${wav.bits} bit is not the declared ${entry.format.sample_rate} Hz ${entry.format.channels} ch 16 bit PCM`,
    );
    return errors;
  }
  const durationMs = Math.round((wav.samples.length / wav.sampleRate) * 1000);
  if (durationMs !== entry.duration_ms) {
    errors.push(
      `${entry.clip_id}: ${durationMs}ms is not the declared ${entry.duration_ms}ms`,
    );
  }
  const ceilingMs =
    (entry.speech_s + entry.internal_pause_s + entry.trailing_quiet_s) * 1000;
  if (durationMs > ceilingMs + 500 || durationMs <= 0) {
    errors.push(
      `${entry.clip_id}: ${durationMs}ms is outside the clip's ${ceilingMs}ms budget`,
    );
  }
  const speechEnd =
    (durationMs / 1000 - entry.trailing_quiet_s) * wav.sampleRate;
  if (peak(wav.samples, 0, Math.min(speechEnd, wav.sampleRate)) < 1000) {
    errors.push(`${entry.clip_id}: the opening of the clip is silent`);
  }
  if (
    peak(wav.samples, speechEnd + 0.2 * wav.sampleRate, wav.samples.length) >
    500
  ) {
    errors.push(`${entry.clip_id}: the trailing quiet is not quiet`);
  }
  return errors;
}

/// The clip manifest: for each clip its text, format, size, duration and hash,
/// and where the audio came from. `origin` is "synthetic-tone" for the
/// generator above and "recorded-speech" for clips authored outside the
/// checkout; only the second contains speech.
function buildClipManifest(schedule, audioByClipId, origin, source = {}) {
  const speech = origin === "recorded-speech";
  const entries = schedule.clips.map((clip) => {
    const bytes = audioByClipId.get(clip.clip_id);
    if (!bytes) throw new Error(`no audio for clip ${clip.clip_id}`);
    const wav = parseWav(bytes);
    if (wav.error) throw new Error(`${clip.clip_id}: ${wav.error}`);
    return {
      ...clip,
      source_text_sha256: sha256(clip.source_text),
      format: { ...CLIP_FORMAT },
      bytes: bytes.length,
      duration_ms: Math.round((wav.samples.length / wav.sampleRate) * 1000),
      sha256: sha256(bytes),
      file: `${clip.clip_id}.wav`,
    };
  });
  const manifest = {
    version: SCHEDULE_VERSION,
    schedule_sha256: schedule.schedule_sha256,
    origin,
    contains_speech: speech,
    tool:
      source.tool ??
      (speech ? null : "long-interview-schedule.cjs synthetic tones"),
    tool_version: source.tool_version ?? null,
    format: { ...CLIP_FORMAT },
    clips: entries,
  };
  manifest.manifest_sha256 = hashOf(manifest);
  return manifest;
}

function syntheticClipSet(schedule) {
  const audio = new Map(
    schedule.clips.map((c) => [c.clip_id, syntheticClipBytes(c)]),
  );
  return {
    audio,
    manifest: buildClipManifest(schedule, audio, "synthetic-tone"),
  };
}

/// Every reason the clip manifest and its audio cannot back `schedule`.
/// `requireSpeech` is the paid-run setting: tones are refused.
function validateClipSet(
  schedule,
  manifest,
  audioByClipId,
  { requireSpeech = false } = {},
) {
  const errors = [];
  const { manifest_sha256: pinned, ...rest } = manifest;
  if (hashOf(rest) !== pinned)
    errors.push("manifest_sha256 does not match the clip manifest");
  if (manifest.schedule_sha256 !== schedule.schedule_sha256) {
    errors.push("the clip manifest belongs to a different schedule");
  }
  if (
    requireSpeech &&
    !(
      manifest.contains_speech === true && manifest.origin === "recorded-speech"
    )
  ) {
    errors.push(
      "a run that asks the model to answer needs real spoken clips authored outside the checkout; these contain no speech",
    );
  }
  if (
    manifest.contains_speech === true &&
    manifest.origin !== "recorded-speech"
  ) {
    errors.push("a manifest may claim speech only for recorded-speech clips");
  }
  const byId = new Map(manifest.clips.map((c) => [c.clip_id, c]));
  for (const clip of schedule.clips) {
    const entry = byId.get(clip.clip_id);
    if (!entry) {
      errors.push(`${clip.clip_id}: no clip in the manifest`);
      continue;
    }
    if (
      entry.source_text !== clip.source_text ||
      entry.source_text_sha256 !== sha256(clip.source_text)
    ) {
      errors.push(`${clip.clip_id}: source text differs from the schedule`);
    }
    const bytes = audioByClipId.get(clip.clip_id);
    if (!bytes) errors.push(`${clip.clip_id}: audio missing`);
    else errors.push(...validateClipBytes(entry, bytes));
  }
  if (byId.size !== manifest.clips.length)
    errors.push("duplicate clip IDs in the manifest");
  for (const id of byId.keys()) {
    if (!schedule.clips.some((c) => c.clip_id === id))
      errors.push(`${id}: not a clip of this schedule`);
  }
  return errors;
}

/// ffmpeg is required before launch: it validates and, when a recorded clip is
/// not already 16 kHz mono PCM, converts it. `run` is injectable for tests.
function ffmpegPreflight({
  run = spawnSync,
  bin = process.env.FFMPEG || "ffmpeg",
} = {}) {
  const result = run(bin, ["-hide_banner", "-version"], { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    return {
      ok: false,
      error: `ffmpeg is required before launch and ${bin} did not run: ${result.error ? result.error.code : `exit ${result.status}`}`,
    };
  }
  const first = String(result.stdout).split("\n")[0];
  return /^ffmpeg version /.test(first)
    ? { ok: true, version: first }
    : { ok: false, error: `${bin} did not identify itself as ffmpeg` };
}

/// Decodes a file end to end with ffmpeg; a clip it cannot decode is refused.
function ffmpegDecodes(
  file,
  { run = spawnSync, bin = process.env.FFMPEG || "ffmpeg" } = {},
) {
  const result = run(bin, ["-v", "error", "-i", file, "-f", "null", "-"], {
    encoding: "utf8",
  });
  return result.status === 0 && !String(result.stderr).trim();
}

/// The controller-clocked playback plan. Times are seconds on the controller's
/// monotonic clock after the candidate joined, never the browser's fake-file
/// clock, and each clip plays exactly once.
function playbackPlan(schedule, clipManifest) {
  const byClip = new Map(clipManifest.clips.map((c) => [c.clip_id, c]));
  const entries = [];
  for (const step of schedule.steps) {
    if (!step.clip_id) continue;
    const clip = byClip.get(step.clip_id);
    if (!clip)
      throw new Error(
        `step ${step.action_id} names no manifest clip ${step.clip_id}`,
      );
    entries.push({
      action_id: step.action_id,
      clip_id: step.clip_id,
      start_s: step.at_s,
      duration_s: clip.duration_ms / 1000,
      end_s: step.at_s + clip.duration_ms / 1000,
      conditional: step.conditional === true,
      latest_s: step.latest_s ?? step.at_s,
      loop: false,
      clock: "controller",
    });
  }
  return entries;
}

function validatePlaybackPlan(plan, schedule) {
  const errors = [];
  const ids = new Set();
  for (const e of plan) {
    if (e.loop !== false) errors.push(`${e.clip_id}: looping is not allowed`);
    if (e.clock !== "controller")
      errors.push(`${e.clip_id}: playback must be controller-clocked`);
    if (ids.has(e.clip_id)) errors.push(`${e.clip_id}: scheduled twice`);
    ids.add(e.clip_id);
    if (e.end_s > schedule.observation_window_s + REPLY_WINDOW_S) {
      errors.push(`${e.clip_id}: ends after the observation window`);
    }
  }
  // A clip must finish, with its quiet, before the next unconditional input.
  const fixed = plan.filter((e) => !e.conditional);
  for (let i = 0; i + 1 < fixed.length; i++) {
    const a = fixed[i];
    const b = fixed[i + 1];
    const barge = plan.find(
      (e) => e.conditional && e.start_s >= a.start_s && e.latest_s < b.start_s,
    );
    const bargeEnd =
      barge && a.clip_id.endsWith("_prime")
        ? barge.latest_s + barge.duration_s
        : 0;
    if (Math.max(a.end_s, bargeEnd) > b.start_s) {
      errors.push(`${a.clip_id} overruns the next input ${b.clip_id}`);
    }
  }
  return errors;
}

/// Compares what the page actually sent with the plan: every clip exactly once
/// and in order; only conditional clips may be absent, and then they are
/// uncovered, not passed.
function verifyTransmitted(plan, transmitted, toleranceS = 3) {
  const errors = [];
  const uncovered = [];
  const counts = new Map();
  for (const t of transmitted)
    counts.set(t.clip_id, (counts.get(t.clip_id) ?? 0) + 1);
  for (const [id, n] of counts) {
    if (n > 1) errors.push(`${id}: transmitted ${n} times; clips never loop`);
    if (!plan.some((e) => e.clip_id === id))
      errors.push(`${id}: transmitted but not planned`);
  }
  for (const e of plan) {
    const sent = transmitted.find((t) => t.clip_id === e.clip_id);
    if (!sent) {
      if (e.conditional) uncovered.push(e.clip_id);
      else errors.push(`${e.clip_id}: planned but never transmitted`);
      continue;
    }
    if (sent.action_id !== e.action_id)
      errors.push(
        `${e.clip_id}: sent under ${sent.action_id}, planned as ${e.action_id}`,
      );
    const late = e.conditional ? e.latest_s : e.start_s;
    if (
      sent.started_at_s < e.start_s - toleranceS ||
      sent.started_at_s > late + toleranceS
    ) {
      errors.push(
        `${e.clip_id}: started at ${sent.started_at_s}s, planned ${e.start_s}..${late}s`,
      );
    }
  }
  return { errors, uncovered };
}

// --------------------------------------------------------------------------
// Command line
// --------------------------------------------------------------------------

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function main(argv) {
  const [command, ...rest] = argv;
  const flags = {};
  for (let i = 0; i < rest.length; i += 2)
    flags[rest[i].replace(/^--/, "")] = rest[i + 1];
  if (command === "preflight") {
    const result = ffmpegPreflight();
    console.log(result.ok ? result.version : result.error);
    return result.ok ? 0 : 2;
  }
  if (command !== "generate") {
    console.error(
      "usage: long-interview-schedule.cjs preflight | generate --mode coding|whiteboard --duration-min N --target-s S --out DIR [--synthetic-clips 1]",
    );
    return 2;
  }
  const out = flags.out;
  if (!out) {
    console.error("generate needs --out");
    return 2;
  }
  const bundle = generateSchedule(flags.mode, {
    durationMin: Number(flags["duration-min"]),
    observationTargetS: Number(flags["target-s"]),
  });
  fs.mkdirSync(out, { recursive: true });
  writeJson(path.join(out, "schedule.json"), bundle.schedule);
  writeJson(path.join(out, "answer-key.json"), bundle.answer_key);
  if (flags["synthetic-clips"] === "1") {
    const clips = syntheticClipSet(bundle.schedule);
    fs.mkdirSync(path.join(out, "clips"), { recursive: true });
    for (const [id, bytes] of clips.audio)
      fs.writeFileSync(path.join(out, "clips", `${id}.wav`), bytes);
    writeJson(path.join(out, "clips", "manifest.json"), clips.manifest);
    console.error(
      "wrote SYNTHETIC tone clips: they contain no speech and cannot drive a paid run",
    );
  }
  console.log(bundle.schedule.schedule_sha256);
  return 0;
}

checkAuthoredPin();
if (require.main === module) process.exit(main(process.argv.slice(2)));

module.exports = {
  sha256,
  AUTHORED,
  AUTHORED_SHA256,
  CLIP_FORMAT,
  CYCLE_S,
  FINAL_BEFORE_WINDOW_END_S,
  JOIN_ACTION_ID,
  MIN_RECALL_GAP_S,
  SCHEDULE_VERSION,
  STOP_BEFORE_END_S,
  TOOL_BY_MODE,
  TOOL_CONVERSATION,
  answerFor,
  budgetFields,
  buildClipManifest,
  buildTimeline,
  checkAuthoredPin,
  ffmpegDecodes,
  ffmpegPreflight,
  generateSchedule,
  hashOf,
  judgeBarge,
  judgeCheckpoint,
  judgeThinkingHold,
  parseWav,
  playbackPlan,
  recallPlan,
  revisionFixture,
  summarizeCompression,
  syntheticClipBytes,
  syntheticClipSet,
  validateClipBytes,
  validateClipSet,
  validatePlaybackPlan,
  validateSchedule,
  verifyTransmitted,
  wavBytes,
};
