import { test } from "node:test";
import assert from "node:assert/strict";
import {
  practiceFocusAccountOwner,
  practiceFocusFrom,
} from "../../web/problem-picker.js";
import { functionBody, read } from "./source.js";

test("replay Practice this carries the exact selected improvement", async () => {
  const source = read("web/replay.js");
  const body = functionBody(source, "loadReport");

  const first = {
    phase: "Coding",
    weakness: "Implement the traversal",
    drill: "Trace one example",
    successCriterion: "Identify every connected region",
    durationMin: 10,
  };

  const second = {
    phase: "Test",
    weakness: "Test boundary cases",
    drill: "Build a boundary test table",
    successCriterion: "Predict every boundary-case result",
    durationMin: 15,
  };

  const report = {
    improvementPlan: [first, second],
  };

  const buttons = [0, 1].map((index) => {
    let click;
    return {
      dataset: { practiceFocus: String(index) },
      addEventListener(type, handler) {
        assert.equal(type, "click");
        click = handler;
      },
      click() {
        return click();
      },
    };
  });

  const nodes = {
    code: { hidden: true },
    board: {
      toDataURL() {
        throw new Error("board should not be read");
      },
    },
    report: {
      textContent: "",
      innerHTML: "",
      querySelectorAll(selector) {
        assert.equal(selector, "[data-practice-focus]");
        return buttons;
      },
    },
  };

  const stored = [];

  const fakeWindow = {
    location: { href: "" },
  };

  const loadReport = new Function(
    "selected",
    "drawn",
    "nodes",
    "latest",
    "fetch",
    "sanitizeReport",
    "reportMarkup",
    "unclearLinesOf",
    "storageArea",
    "storeSelectedPracticeFocus",
    "practiceFocusAccountOwner",
    "window",
    "mountFinalCodeCopy",
    "loadOriginalProblem",
    `${body}\n}\nreturn loadReport;`,
  )(
    "recording-1",
    false,
    nodes,
    { language: "javascript", code: "return 1;" },
    async () => ({
      ok: true,
      async json() {
        return {
          accountId: "A",
          reports: [
            {
              payload: {
                id: "report-1",
                interviewId: "interview-1",
                problemTitle: "Example",
                problemId: "example",
                report,
              },
            },
          ],
        };
      },
    }),
    (value) => value,
    () => "<section></section>",
    () => [],
    (name) => {
      assert.equal(name, "sessionStorage");
      return null;
    },
    (storage, owner, focus, interviewMode, sourceReportId) => {
      assert.equal(owner, "account:A");
      assert.equal(storage, null);
      assert.equal(sourceReportId, "report-1");
      stored.push(practiceFocusFrom(focus, interviewMode));
      return true;
    },
    practiceFocusAccountOwner,
    fakeWindow,
    () => {},
    () => Promise.resolve(null),
  );

  await loadReport("interview-1", "recording-1");

  await buttons[1].click();

  assert.deepEqual(stored, [
    {
      phase: second.phase,
      weakness: second.weakness,
      drill: second.drill,
      successCriterion: second.successCriterion,
    },
  ]);

  assert.equal("durationMin" in stored[0], false);
  assert.equal(fakeWindow.location.href, "/");
});

test("replay binds Practice this to the report account", async () => {
  const source = read("web/replay.js");
  const body = functionBody(source, "loadReport");

  const item = {
    phase: "Test",
    weakness: "Test boundary cases",
    drill: "Build a boundary test table",
    successCriterion: "Predict every boundary-case result",
  };

  let click;
  const button = {
    dataset: { practiceFocus: "0" },
    addEventListener(type, handler) {
      assert.equal(type, "click");
      click = handler;
    },
  };

  const nodes = {
    code: { hidden: true },
    board: {
      toDataURL() {
        throw new Error("board should not be read");
      },
    },
    report: {
      textContent: "",
      innerHTML: "",
      querySelectorAll() {
        return [button];
      },
    },
  };

  const fakeWindow = {
    location: { href: "" },
  };

  let storedOwner = null;

  const loadReport = new Function(
    "selected",
    "drawn",
    "nodes",
    "latest",
    "fetch",
    "sanitizeReport",
    "reportMarkup",
    "unclearLinesOf",
    "storageArea",
    "storeSelectedPracticeFocus",
    "practiceFocusAccountOwner",
    "window",
    "mountFinalCodeCopy",
    "loadOriginalProblem",
    `${body}\n}\nreturn loadReport;`,
  )(
    "recording-1",
    false,
    nodes,
    { language: "javascript", code: "return 1;" },
    async () => ({
      ok: true,
      async json() {
        return {
          accountId: "B",
          reports: [
            {
              payload: {
                id: "report-2",
                interviewId: "interview-1",
                problemTitle: "Example",
                problemId: "example",
                report: { improvementPlan: [item] },
              },
            },
          ],
        };
      },
    }),
    (value) => value,
    () => "<section></section>",
    () => [],
    () => null,
    (storage, owner, _focus, _interviewMode, sourceReportId) => {
      assert.equal(storage, null);
      assert.equal(sourceReportId, "report-2");
      storedOwner = owner;
      return true;
    },
    practiceFocusAccountOwner,
    fakeWindow,
    () => {},
    () => Promise.resolve(),
  );

  await loadReport("interview-1", "recording-1");
  await click();

  assert.equal(storedOwner, "account:B");
  assert.equal(fakeWindow.location.href, "/");
});

test("replay stays on the report when Practice this cannot be stored", async () => {
  const source = read("web/replay.js");
  const body = functionBody(source, "loadReport");
  const item = {
    phase: "Test",
    weakness: "Test boundary cases",
    drill: "Build a boundary test table",
    successCriterion: "Predict every boundary-case result",
  };
  const status = {
    hidden: true,
    textContent: "",
  };
  let click;
  const button = {
    dataset: { practiceFocus: "0" },
    parentElement: {
      querySelector(selector) {
        assert.equal(selector, "[data-practice-focus-status]");
        return status;
      },
    },
    addEventListener(type, handler) {
      assert.equal(type, "click");
      click = handler;
    },
  };
  const nodes = {
    code: { hidden: true },
    board: {
      toDataURL() {
        throw new Error("board should not be read");
      },
    },
    report: {
      textContent: "",
      innerHTML: "",
      querySelectorAll() {
        return [button];
      },
    },
  };
  const fakeWindow = {
    location: { href: "" },
  };
  const loadReport = new Function(
    "selected",
    "drawn",
    "nodes",
    "latest",
    "fetch",
    "sanitizeReport",
    "reportMarkup",
    "unclearLinesOf",
    "storageArea",
    "storeSelectedPracticeFocus",
    "practiceFocusAccountOwner",
    "window",
    "mountFinalCodeCopy",
    "loadOriginalProblem",
    `${body}\n}\nreturn loadReport;`,
  )(
    "recording-1",
    false,
    nodes,
    { language: "javascript", code: "return 1;" },
    async () => ({
      ok: true,
      async json() {
        return {
          accountId: "A",
          reports: [
            {
              payload: {
                id: "report-1",
                interviewId: "interview-1",
                problemTitle: "Example",
                problemId: "example",
                report: { improvementPlan: [item] },
              },
            },
          ],
        };
      },
    }),
    (value) => value,
    () => "<section></section>",
    () => [],
    () => null,
    () => false,
    practiceFocusAccountOwner,
    fakeWindow,
    () => {},
    () => Promise.resolve(),
  );
  await loadReport("interview-1", "recording-1");
  await click();
  assert.equal(fakeWindow.location.href, "");
  assert.equal(status.hidden, false);
  assert.equal(status.textContent, "Could not save this practice focus.");
});
