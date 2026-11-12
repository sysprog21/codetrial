import { test } from "node:test";
import assert from "node:assert/strict";
import { downloadMarkdown, reportFilename } from "../../web/download.js";
import { savedAttemptReport } from "../../web/progress.js";
import { functionBody, read } from "./source.js";

for (const [path, name] of [
  ["web/app.js", "exportSavedReport"],
  ["web/interview.js", "downloadReport"],
]) {
  function harness() {
    const lookups = [];
    const downloads = [];
    const button = { disabled: false };
    const done = { disabled: false };
    const scope = {
      state: {
        downloadingReport: false,
        categoryComparisonReady: Promise.resolve(),
        reportSaveComplete: true,
      },
      nodes: {
        report: {
          querySelector: (selector) => (selector === "#done" ? done : button),
        },
      },
      requestTimeoutMs: 10_000,
      pendingReportDownloads: new Set(),
      selectionKey: (attempt) => attempt.id || attempt,
      loadPageMapWithin: () =>
        new Promise((resolve, reject) => {
          lookups.push({ resolve, reject });
        }),
      reportMarkdown: () => "# Report",
      savedAttemptReport,
      buildMarkdown: () => "# Report",
      reportFilename,
      problem: { page: "two-sum" },
      downloadMarkdown: (...args) => downloads.push(args),
    };
    const download = new Function(
      "scope",
      `with (scope) {
      ${name === "exportSavedReport" ? `${functionBody(read(path), "downloadSavedReport")}\n}` : ""}
      ${functionBody(read(path), name)}
      }
      return ${name};
    }`,
    )(scope);
    const guardedDownload = (...args) =>
      name === "exportSavedReport" ? download(...args, null) : download();
    const attempt = { report: {}, problemId: "two-sum", at: "2026-10-23" };
    const run = () =>
      name === "exportSavedReport"
        ? guardedDownload(attempt, button)
        : guardedDownload();
    return {
      run,
      button,
      done,
      lookups,
      downloads,
      scope,
      download: (attempt, button) => guardedDownload(attempt, button),
      attempt,
    };
  }

  test(`${name} exports once while pending and permits a later download`, async () => {
    const h = harness();
    const first = h.run();
    assert.equal(h.button.disabled, true);
    if (name === "downloadReport") assert.equal(h.done.disabled, true);
    await h.run();
    await h.run();
    assert.equal(h.lookups.length, 1);
    assert.equal(h.downloads.length, 0);
    h.lookups[0].resolve({});
    await first;
    assert.equal(h.downloads.length, 1);
    assert.equal(h.button.disabled, false);
    if (name === "downloadReport") assert.equal(h.done.disabled, false);
    const next = h.run();
    assert.equal(h.button.disabled, true);
    h.lookups[1].resolve(null);
    await next;
    assert.equal(h.downloads.length, 2);
    assert.equal(h.button.disabled, false);
  });

  for (const failure of ["lookup", "export"]) {
    test(`${name} restores the button after ${failure} fails`, async () => {
      const h = harness();
      const error = new Error("download failed");
      const originalDownload = h.scope.downloadMarkdown;
      if (failure === "export")
        h.scope.downloadMarkdown = () => {
          throw error;
        };
      const pending = h.run();
      const rejected = assert.rejects(pending, error);
      if (failure === "lookup") h.lookups[0].reject(error);
      else h.lookups[0].resolve(null);
      await rejected;
      assert.equal(h.button.disabled, false);
      if (name === "downloadReport") assert.equal(h.done.disabled, false);
      h.scope.downloadMarkdown = originalDownload;
      const retry = h.run();
      h.lookups[1].resolve(null);
      await retry;
      assert.equal(h.downloads.length, 1);
      assert.equal(h.button.disabled, false);
    });
  }

  if (name === "exportSavedReport") {
    test("batch and row downloads share the pending guard for one report", async () => {
      const h = harness();
      h.attempt.id = "saved-report";
      const batch = h.download(h.attempt);
      await h.run();
      await h.download({ ...h.attempt });
      assert.equal(h.lookups.length, 1);
      h.lookups[0].resolve(null);
      await batch;
      const retry = h.run();
      assert.equal(h.button.disabled, true);
      h.lookups[1].resolve(null);
      await retry;
      assert.equal(h.downloads.length, 2);
      assert.equal(h.button.disabled, false);
    });

    test("different saved reports can export while another lookup is pending", async () => {
      const h = harness();
      const first = h.download({ ...h.attempt, id: "first" });
      const second = h.download({ ...h.attempt, id: "second" });
      assert.equal(h.lookups.length, 2);
      h.lookups[0].resolve(null);
      h.lookups[1].resolve(null);
      await Promise.all([first, second]);
      assert.equal(h.downloads.length, 2);
    });
  }
}

test("report filenames use local timestamps and safe scenario names", () => {
  assert.equal(
    reportFilename("scenario/../name?", new Date(2026, 0, 2, 1, 30)),
    "interview-report-scenario----name--2026-01-02-013000.md",
  );
  assert.equal(
    reportFilename("", new Date(2026, 10, 12, 23, 30)),
    "interview-report-past-interview-2026-11-12-233000.md",
  );
});

test("report filenames distinguish times on the same local calendar day", () => {
  assert.equal(
    reportFilename("two-sum", new Date(2026, 9, 14, 1, 2, 3)),
    "interview-report-two-sum-2026-10-14-010203.md",
  );
  assert.equal(
    reportFilename("two-sum", new Date(2026, 9, 14, 15, 30, 0)),
    "interview-report-two-sum-2026-10-14-153000.md",
  );
});

test("report filenames follow the local midnight boundary", () => {
  assert.equal(
    reportFilename("two-sum", new Date(2026, 9, 14, 23, 59, 59)),
    "interview-report-two-sum-2026-10-14-235959.md",
  );
  assert.equal(
    reportFilename("two-sum", new Date(2026, 9, 15, 0, 0, 0)),
    "interview-report-two-sum-2026-10-15-000000.md",
  );
});

test("markdown downloads release their anchor and object URL after the click", async (t) => {
  const events = [];
  let blob;
  const anchor = {
    click() {
      events.push(["click", this.href, this.download]);
    },
    remove() {
      events.push(["remove"]);
    },
  };
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createElement(tag) {
        assert.equal(tag, "a");
        return anchor;
      },
      body: {
        append(node) {
          assert.equal(node, anchor);
          events.push(["append"]);
        },
      },
    },
  });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, "document", previous);
    else delete globalThis.document;
  });
  t.mock.method(URL, "createObjectURL", (value) => {
    blob = value;
    return "blob:report";
  });
  t.mock.method(URL, "revokeObjectURL", (url) => events.push(["revoke", url]));
  t.mock.timers.enable({ apis: ["setTimeout"] });
  downloadMarkdown("# Saved report\n", "report.md");
  assert.equal(blob.type, "text/markdown");
  assert.equal(await blob.text(), "# Saved report\n");
  assert.deepEqual(events, [
    ["append"],
    ["click", "blob:report", "report.md"],
    ["remove"],
  ]);
  t.mock.timers.runAll();
  assert.deepEqual(events.at(-1), ["revoke", "blob:report"]);
  assert.equal(events.length, 4);
});
