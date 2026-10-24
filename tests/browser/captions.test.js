import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";
import { MAX_UNCLEAR_LINES } from "../../web/lib.js";

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

function finishJim(t, nodes, text) {
  const expected = `[Jim]: ${text}`;
  for (let ticks = 0; ticks < text.length; ticks++) {
    if (nodes.captionsText.textContent === expected) break;
    t.mock.timers.tick(300);
  }
  assert.equal(nodes.captionsText.textContent, expected);
}

function finishDismissal(t, nodes, text) {
  for (
    let ticks = 0;
    ticks < text.length && !nodes.captionsBar.hidden;
    ticks++
  ) {
    t.mock.timers.tick(300);
  }
  assert.equal(
    nodes.captionsBar.hidden,
    true,
    "a pending edit must dismiss Jim after the final reveal",
  );
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
  const beforeFinal = nodes.captionsText.textContent;
  captions.updateCaptions("interviewer", text, "jim-1", { final: true });

  assert.equal(nodes.captionsText.textContent, beforeFinal);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text.slice(0, 16)}`);

  finishJim(t, nodes, text);

  // Later ticks must neither trim nor hide the retained turn.
  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);
  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
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
  finishJim(t, nodes, "Use a hash map.");
  captions.updateCaptions("you", "I will count the values.", "you-1");

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, false);
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
  test(`a code edit during ${final ? "a final" : "an interim"} Jim reveal is remembered`, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const text = "Use a hash map and count each value.";

    captions.updateCaptions("interviewer", text, "jim-1", { final });
    const beforeEdit = nodes.captionsText.textContent;

    captions.dismissCaptions();
    assert.equal(nodes.captionsBar.hidden, false);
    assert.equal(nodes.captionsText.textContent, beforeEdit);

    t.mock.timers.tick(300);
    assert.equal(nodes.captionsBar.hidden, false);
    assert.notEqual(nodes.captionsText.textContent, beforeEdit);

    captions.updateCaptions("interviewer", text, "jim-1", {
      final: true,
    });
    assert.equal(nodes.captionsBar.hidden, false);

    finishDismissal(t, nodes, text);

    for (const lateFinal of [false, true]) {
      captions.updateCaptions(
        "interviewer",
        `${text} Then scan the array.`,
        "jim-1",
        { final: lateFinal },
      );
      assert.equal(nodes.captionsBar.hidden, true);
    }

    t.mock.timers.tick(60000);
    assert.equal(nodes.captionsBar.hidden, true);
  });
}

test("a dismissed completed Jim turn stays hidden", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  finishJim(t, nodes, "Use a hash map.");
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
  finishJim(t, nodes, "Try sorting.");
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, false);
});

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
    // History is covered separately; this fixture isolates input side effects.
    replayingEditorHistory: true,
    drawingTurn: null,
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
  finishJim(t, nodes, "Try another approach.");
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
  finishJim(t, nodes, jimText);
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

test("candidate speech pauses a live Jim turn without retiring it", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1");
  captions.updateCaptions("you", "I will count the values.", "you-1");

  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  t.mock.timers.tick(300);
  assert.equal(
    nodes.captionsText.textContent,
    "[You]: I will count the values.",
  );

  const text = "Use a hash map and count each value.";
  captions.updateCaptions("interviewer", text, "jim-1", {
    final: true,
  });
  finishJim(t, nodes, text);

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, `[Jim]: ${text}`);
});

test("a final-only Jim turn still reveals gradually", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });

  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a ha");
  assert.equal(nodes.captionsBar.hidden, false);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Use a hash map.");

  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, false);
});

for (const candidateId of ["you-1", null]) {
  test(`candidate speech retires a revealing final Jim turn (${candidateId ?? "no ID"})`, async (t) => {
    const { captions, nodes } = await setupCaptions(t);
    const text = "Use a hash map and count each value.";
    const candidateText = "I will count the values.";

    captions.updateCaptions("interviewer", text, "jim-1", {
      final: true,
    });
    assert.notEqual(nodes.captionsText.textContent, `[Jim]: ${text}`);

    captions.updateCaptions("you", candidateText, candidateId);
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

    t.mock.timers.tick(300);
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

    for (const final of [false, true]) {
      captions.updateCaptions(
        "interviewer",
        `${text} Then scan the array.`,
        "jim-1",
        { final },
      );
      assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);
      assert.equal(nodes.captionsBar.hidden, false);
    }

    t.mock.timers.tick(11699);
    assert.equal(nodes.captionsBar.hidden, false);
    t.mock.timers.tick(1);
    assert.equal(nodes.captionsBar.hidden, true);
  });
}

test("a new candidate segment retires a live Jim turn", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";
  const candidateText = "Now I will compare complements.";

  captions.updateCaptions("you", "I will count the values.", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  captions.updateCaptions("you", candidateText, "you-2");

  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  t.mock.timers.tick(300);
  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      `${jimText} Then scan the array.`,
      "jim-1",
      { final },
    );
    assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);
    assert.equal(nodes.captionsBar.hidden, false);
  }

  t.mock.timers.tick(11699);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);
});

test("a pending code edit does not dismiss the next Jim turn", async (t) => {
  const { captions, nodes } = await setupCaptions(t);

  captions.updateCaptions(
    "interviewer",
    "Use a hash map and count each value.",
    "jim-1",
  );
  captions.dismissCaptions();

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2", {
    final: true,
  });
  finishJim(t, nodes, "Try sorting.");

  t.mock.timers.tick(13000);
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");

  for (const final of [false, true]) {
    captions.updateCaptions(
      "interviewer",
      "Use a hash map and count each value. Then scan the array.",
      "jim-1",
      { final },
    );
    assert.equal(nodes.captionsBar.hidden, false);
    assert.equal(nodes.captionsText.textContent, "[Jim]: Try sorting.");
  }
});

test("a pending code edit preserves the candidate idle deadline", async (t) => {
  const { captions, nodes } = await setupCaptions(t);
  const jimText = "Use a hash map and count each value.";
  const candidateText = "I will count the values.";

  captions.updateCaptions("you", "I will", "you-1");
  captions.updateCaptions("interviewer", jimText, "jim-1");
  captions.updateCaptions("you", candidateText, "you-1");

  t.mock.timers.tick(11000);
  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, false);
  assert.equal(nodes.captionsText.textContent, `[You]: ${candidateText}`);

  t.mock.timers.tick(999);
  assert.equal(nodes.captionsBar.hidden, false);
  t.mock.timers.tick(1);
  assert.equal(nodes.captionsBar.hidden, true);

  captions.updateCaptions("interviewer", jimText, "jim-1", {
    final: true,
  });
  assert.equal(nodes.captionsBar.hidden, false);

  finishDismissal(t, nodes, jimText);
  t.mock.timers.tick(60000);
  assert.equal(nodes.captionsBar.hidden, true);
});

function fakeUnclearButton() {
  return {
    hidden: true,
    disabled: false,
    title: "",
    attributes: {},
    click: null,
    setAttribute(name, value) {
      this.attributes[name] = value;
    },
    addEventListener(type, handler) {
      if (type === "click") this.click = handler;
    },
  };
}

async function setupUnclear(t) {
  const captions = await import(
    `../../web/captions.js?copy=${++captionCopies}`
  );
  const button = fakeUnclearButton();
  const nodes = {
    captionsText: { textContent: "" },
    captionsBar: { hidden: true },
    captionUnclear: button,
  };
  captions.initCaptions({ nodes });
  t.mock.timers.enable({ apis: ["setTimeout"] });
  return { captions, button, nodes };
}

test("the Unclear button marks and unmarks the current Jim turn", async (t) => {
  const { captions, button } = await setupUnclear(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  assert.equal(button.hidden, false);

  button.click();
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.deepEqual(captions.unclearLineIds(), ["jim-1"]);

  button.click();
  assert.equal(button.attributes["aria-pressed"], "false");
  assert.deepEqual(captions.unclearLineIds(), []);
});

test("candidate speech hides the Unclear button and a new Jim turn starts unmarked", async (t) => {
  const { captions, button } = await setupUnclear(t);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  button.click();
  captions.updateCaptions("you", "Okay.", "you-1");
  assert.equal(button.hidden, true);

  captions.updateCaptions("interviewer", "Try sorting.", "jim-2");
  assert.equal(button.hidden, false);
  assert.equal(button.attributes["aria-pressed"], "false");
  assert.deepEqual(captions.unclearLineIds(), ["jim-1"]);
});

test("the Unclear button stops marking at the cap", async (t) => {
  const { captions, button } = await setupUnclear(t);

  for (let turn = 1; turn <= MAX_UNCLEAR_LINES; turn++) {
    captions.updateCaptions("interviewer", `Line ${turn}.`, `jim-${turn}`, {
      final: true,
    });
    button.click();
  }
  assert.equal(captions.unclearLineIds().length, MAX_UNCLEAR_LINES);

  captions.updateCaptions("interviewer", "One more line.", "jim-extra", {
    final: true,
  });
  assert.equal(button.attributes["aria-disabled"], "true");
  button.click();
  assert.equal(captions.unclearLineIds().length, MAX_UNCLEAR_LINES);
  assert.ok(!captions.unclearLineIds().includes("jim-extra"));
});

function focusOn(button, nodes, editor = {}) {
  const document = { activeElement: button };
  button.ownerDocument = document;
  const target = (name, options = {}) => ({
    isConnected: true,
    disabled: false,
    ...options,
    focus() {
      document.activeElement = nodes[name];
    },
  });
  nodes.editor = target("editor", editor);
  nodes.board = target("board");
  return document;
}

test("candidate speech moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  button.click();
  assert.equal(document.activeElement, button);

  captions.updateCaptions("you", "Okay.", "you-1");
  assert.equal(button.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

test("the cap leaves focus on the Unclear button it marks unavailable", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  for (let turn = 1; turn <= MAX_UNCLEAR_LINES; turn++) {
    captions.updateCaptions("interviewer", `Line ${turn}.`, `jim-${turn}`, {
      final: true,
    });
    button.click();
  }
  assert.equal(document.activeElement, button);

  captions.updateCaptions("interviewer", "One more line.", "jim-extra", {
    final: true,
  });
  assert.equal(button.attributes["aria-disabled"], "true");
  assert.equal(document.activeElement, button);
});

test("the idle timeout moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  // A Jim turn that never turns final keeps the idle timeout armed.
  captions.updateCaptions("interviewer", "Go on.", "jim-1");
  t.mock.timers.tick(12000);
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

test("a code edit moves focus off the Unclear button it hides", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Use a hash map.", "jim-1", {
    final: true,
  });
  t.mock.timers.tick(1000);
  captions.dismissCaptions();
  assert.equal(nodes.captionsBar.hidden, true);
  assert.equal(document.activeElement, nodes.editor);
});

for (const [name, editor] of [
  ["disabled", { disabled: true }],
  ["removed", { isConnected: false }],
]) {
  test(`focus skips a ${name} editor for the board`, async (t) => {
    const { captions, button, nodes } = await setupUnclear(t);
    const document = focusOn(button, nodes, editor);

    captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
    captions.updateCaptions("you", "Okay.", "you-1");
    assert.equal(document.activeElement, nodes.board);
  });
}

test("a mouse click on Unclear hands focus back to the editor", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
  button.click({ detail: 1 });
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.equal(document.activeElement, nodes.editor);
});

test("Space or Enter on Unclear keeps focus on the button", async (t) => {
  const { captions, button, nodes } = await setupUnclear(t);
  const document = focusOn(button, nodes);

  captions.updateCaptions("interviewer", "Go on.", "jim-1", { final: true });
  button.click({ detail: 0 });
  assert.equal(button.attributes["aria-pressed"], "true");
  assert.equal(document.activeElement, button);
});
