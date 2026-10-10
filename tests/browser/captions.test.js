import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";

let captionCopies = 0;

async function setupCaptions(t) {
  // Caption state is module-level, so each case imports a fresh copy.
  const captions = await import(
    `../../web/captions.js?copy=${++captionCopies}`
  );
  const nodes = {
    captionsText: { textContent: "" },
    captionsBar: { hidden: true },
  };
  captions.initCaptions({ nodes });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  return { captions, nodes };
}

// Execute page logic against test dependencies without its browser startup.
function loadInterviewFunction(name, scope) {
  const body = `${functionBody(read("web/interview.js"), name)}\n}`;
  return new Function("scope", `with (scope) { ${body}\nreturn ${name}; }`)(
    scope,
  );
}

test("the listening placeholder hides after twelve seconds", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  nodes.captionsText.textContent = "Listening...";
  captions.showCaptions();

  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(11999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("candidate captions keep their window and renew the idle timeout", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const text =
    `${"I am checking the input constraints ".repeat(6)}` +
    "before choosing a data structure. I will use a hash map.";

  captions.updateCaptions("you", text, "you-1");
  assert.equal(nodes.captionsText.textContent, "[You]: I will use a hash map.");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(11000);
  assert.equal(nodes.captionsBar.hidden, false);

  captions.updateCaptions("you", "I will scan the array once.", "you-1");
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will scan the array once.",
  );

  // The previous update's deadline must not hide the renewed caption.
  t.mock.timers.tick(1000);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(10999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("interim interviewer captions pace cumulative updates", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a ha");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  captions.updateCaptions("interviewer", "Use a", "jim-1");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sort");
  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
});

test("a finished interviewer turn is shown in full", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const text =
    "Start by checking how often each value appears in the input. " +
    "Store counts in a hash map so repeated values are handled consistently. " +
    "Then scan the array once and compare each value with its required complement.";

  assert.ok(text.length > 160);

  captions.updateCaptions("interviewer", text, "jim-1");
  captions.updateCaptions("interviewer", text, "jim-1", { final: true });

  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);

  // A pending pacing tick must not trim the completed turn.
  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);
});

test("a finished interviewer turn outlives idle timers", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.showCaptions();
  t.mock.timers.tick(11000);

  captions.updateCaptions("interviewer", "Think.", "jim-1", {
    final: true,
  });

  // The placeholder's old deadline must not hide the completed turn.
  t.mock.timers.tick(1000);
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Think.");
});

test("candidate speech replaces Jim without late revival", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  captions.updateCaptions("you", "I will count the values.", "you-1");

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  t.mock.timers.tick(1000);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      "Use a hash map and count each value.",
      "jim-1",
      { final },
    );
    assert.equal(
      nodes.captionsText.textContent,
      "[You]: I will count the values.",
    );
    assert.equal(nodes.captionsBar.hidden, false);
  }

  t.mock.timers.tick(10999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

for (const final of [false, true]) {
  test(`a dismissed ${final ? "completed" : "interim"} Jim turn stays hidden`, async (t) => {
    const { captions, nodes } = await setupCaptions(t);

    captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
      final,
    });
    captions.dismissCaptions();
    assert.equal(nodes.captionsBar.hidden, true);

    t.mock.timers.tick(300);
    assert.equal(nodes.captionsBar.hidden, true);

    for (const lateFinal of [false, true]) {
      captions.updateCaptions(
        "interviewer",
        "Use a hash map and count each value.",
        "jim-1",
        { final: lateFinal },
      );
      assert.equal(nodes.captionsBar.hidden, true);
    }

    captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
      final: true,
    });
    assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
    assert.equal(nodes.captionsBar.hidden, false);

    t.mock.timers.tick(60000);
    assert.equal(nodes.captionsBar.hidden, false);
  });
}

test("a new Jim turn replaces the previous turn despite late updates", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sort");

  t.mock.timers.tick(300);
  captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
    final: true,
  });

  captions.updateCaptions(
    "interviewer",
    "Use a hash map and count each value.",
    "jim-1",
    { final: true },
  );
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
});

for (const { name, final, broken } of [
  {
    name: "a complete final stream retains the caption",
    final: true,
    broken: false,
  },
  {
    name: "an interrupted final stream keeps partial captions temporary",
    final: true,
    broken: true,
  },
  {
    name: "an interim stream keeps captions temporary",
    final: false,
    broken: false,
  },
]) {
  test(name, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const updates = [];
    const consume = loadInterviewFunction("consumeTranscript", {
      isCurrentAgent: () => true,
      responseWindowIndex: () => 0,
      updateTranscriptSegment: () => {},
      recordReplay: () => {},
      updateCaptions(speaker, text, id, options = {}) {
        updates.push(options.final === true);
        captions.updateCaptions(speaker, text, id, options);
      },
    });

    async function* chunks() {
      yield "Think";
      if (broken) throw new Error("stream interrupted");
      yield ".";
    }

    const reader = chunks();
    reader.info = {
      attributes: {
        "lk.segment_id": "jim-1",
        "lk.transcription_final": String(final),
      },
    };

    await consume({ localParticipant: { identity: "candidate" } }, reader, {
      identity: "jim",
    });

    assert.deepEqual(updates, broken ? [false, false] : [false, false, final]);
    assert.equal(
      nodes.captionsText.textContent,
      broken ? "[Jim]: Think" : "[Jim]: Think.",
    );

    t.mock.timers.tick(12000);
    assert.equal(nodes.captionsBar.hidden, broken || !final);
  });
}

test("only changed editor text dismisses Jim", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const events = [];
  let input;
  nodes.editor = {
    value: "",
    addEventListener(type, handler) {
      assert.equal(type, "input");
      input = handler;
    },
  };
  const state = {
    language: "javascript",
    codeByLanguage: { javascript: "" },
  };

  const binding = functionBody(read("web/interview.js"), "bindEvents");
  const start = binding.indexOf('nodes.editor.addEventListener("input",');
  const end = binding.indexOf(
    '\n  nodes.report.addEventListener("click"',
    start,
  );
  assert.ok(start >= 0, "the editor input listener must exist");
  assert.ok(end > start, "the listener boundary must exist");

  new Function("scope", `with (scope) { ${binding.slice(start, end)} }`)({
    nodes,
    state,
    dismissCaptions: captions.dismissCaptions,
    flushPendingLanguagePublish: () => events.push("language"),
    scheduleEditorPaint: () => events.push("paint"),
    flushPendingEditorPublish: () => events.push("publish"),
    codePublishTimer: null,
    CODE_PUBLISH_DEBOUNCE_MS: 300,
    setTimeout,
    clearTimeout,
  });

  captions.updateCaptions("interviewer", "Think.", "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(typeof input, "function");

  input();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(state.codeByLanguage.javascript, "");
  assert.deepEqual(events, ["language", "paint"]);
  t.mock.timers.tick(300);
  assert.deepEqual(events, ["language", "paint", "publish"]);
  events.length = 0;

  nodes.editor.value = "const answer = 42;";
  input();
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(state.codeByLanguage.javascript, "const answer = 42;");
  assert.deepEqual(events, ["language", "paint"]);

  t.mock.timers.tick(299);
  assert.deepEqual(events, ["language", "paint"]);
  t.mock.timers.tick(1);
  assert.deepEqual(events, ["language", "paint", "publish"]);

  captions.updateCaptions("interviewer", "Think about the input.", "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, true);
  captions.updateCaptions("interviewer", "Try another approach.", "jim-2", {
    final: true,
  });
  input();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try another approach.");
  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
});

test("a candidate final replay does not dismiss Jim", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  const interimJimText = nodes.captionsText.textContent;

  const consume = loadInterviewFunction("consumeTranscript", {
    isCurrentAgent: () => true,
    responseWindowIndex: () => 0,
    updateTranscriptSegment: () => {},
    recordReplay: () => {},
    updateCaptions: captions.updateCaptions,
  });

  async function* chunks() {
    yield "I will count";
    yield " the values.";
  }

  const reader = chunks();
  reader.info = {
    attributes: {
      "lk.segment_id": "you-1",
      "lk.transcription_final": "true",
    },
  };

  await consume({ localParticipant: { identity: "candidate" } }, reader, {
    identity: "candidate",
  });

  assert.equal(nodes.captionsText.textContent, interimJimText);

  captions.updateCaptions("interviewer", jimText, "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${jimText}`);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "you",
      "I will count the values and check their complements.",
      "you-1",
      { final },
    );
    assert.equal(nodes.captionsText.textContent, `[Jim]: ${jimText}`);
  }

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);

  captions.updateCaptions("you", "I will compare complements.", "you-2");
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will compare complements.",
  );
  t.mock.timers.tick(11999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("new candidate text can interrupt Jim before completion", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1");
  captions.updateCaptions("you", "I will count the values.", "you-1");

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );
  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, true);
});
