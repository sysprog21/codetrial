import { test } from "node:test";
import assert from "node:assert/strict";

import { storeReportPracticeFocus } from "../../web/problem-picker.js";
import { functionBody, read } from "./source.js";

test("replay Practice this stores the selected item with its problem and stays on failure", async () => {
  const body = functionBody(read("web/replay.js"), "loadReport");

  const first = {
    phase: "Coding",
    weakness: "Trace the algorithm",
    drill: "Trace one example",
    successCriterion: "Explain every step",
  };

  const second = {
    phase: "Test",
    weakness: "Test boundaries",
    drill: "Build a boundary test table",
    successCriterion: "Predict each output",
  };

  const report = { improvementPlan: [first, second] };

  // The last case keeps the problem id on the stored row alone, a shape the
  // row is guaranteed to have and the payload is not.
  for (const [storageFails, idOnRow] of [
    [false, false],
    [true, false],
    [false, true],
  ]) {
    let click;
    const stored = [];
    const status = { textContent: "" };

    const buttons = [0, 1].map((index) => ({
      dataset: { practiceFocus: String(index) },
      parentElement: {
        querySelector: () => status,
      },
      addEventListener(type, handler) {
        assert.equal(type, "click");
        if (index === 1) click = handler;
      },
    }));

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

    const storage = {
      setItem(key, value) {
        if (storageFails) throw new Error("storage blocked");
        stored.push([key, JSON.parse(value)]);
      },
    };

    const fakeWindow = { location: { href: "" } };

    const loadReport = new Function(
      "selected",
      "drawn",
      "nodes",
      "latest",
      "fetch",
      "sanitizeReport",
      "reportMarkup",
      "unclearLinesOf",
      "storeReportPracticeFocus",
      "tabStorage",
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
            reports: [
              {
                ...(idOnRow ? { problemId: "example" } : {}),
                payload: {
                  interviewId: "interview-1",
                  problemTitle: "Example",
                  ...(idOnRow ? {} : { problemId: "example" }),
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
      storeReportPracticeFocus,
      storage,
      fakeWindow,
      () => {},
      () => Promise.resolve(),
    );

    await loadReport("interview-1", "recording-1");
    click();

    assert.deepEqual(
      stored,
      storageFails
        ? []
        : [
            [
              "codetrial.selectedPracticeFocus",
              { ...second, problemId: "example" },
            ],
          ],
    );
    assert.equal(fakeWindow.location.href, storageFails ? "" : "/");
    assert.equal(
      status.textContent,
      storageFails ? "Could not save this practice focus." : "",
    );
  }
});
