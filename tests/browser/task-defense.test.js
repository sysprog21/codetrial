import test from "node:test";
import assert from "node:assert/strict";
import { defenseSections } from "../../web/task-defense.js";
import { launchChromium, startStaticServer } from "./source.js";

// What the result area shows of a defense: suggestions, item by item.

test("a defense result shows each item's suggested marks", () => {
  const sections = defenseSections({
    notice: "Generated on the student's machine.",
    items: [
      {
        id: "q1",
        file: "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala",
        line: 77,
        explain: { suggestedMark: 2, reason: "Named the stall signal." },
        predict: { suggestedMark: 3 },
      },
      {
        id: "q2",
        file: "4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala",
        line: 216,
        explain: { suggestedMark: null, insufficientReason: "missing_turns" },
        predict: { suggestedMark: null, insufficientReason: "missing_run" },
      },
    ],
    bug: { suggestedMark: 0 },
  });
  assert.deepEqual(sections, [
    ["Defense", "Generated on the student's machine."],
    [
      "q1: 4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala:77",
      "Explain: 2. Predict and confirm: 3. Named the stall signal.",
    ],
    [
      "q2: 4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala:216",
      "Explain: none (missing_turns). Predict and confirm: none (missing_run).",
    ],
    ["Bug", "Suggested mark: 0."],
  ]);
});

test("any other result has no defense sections", () => {
  assert.deepEqual(defenseSections(undefined), []);
  assert.deepEqual(defenseSections({ unavailable: "a run was going" }), []);
});

test("temporary locks preserve inputs and a new panel replaces the old one", async (t) => {
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  const { server, base } = await startStaticServer();
  try {
    const page = await browser.newPage();
    await page.goto(`${base}/task.html`, { waitUntil: "commit" });
    await page.setContent('<section id="defense"></section>');
    await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      const root = document.querySelector("#defense");
      root.hidden = false;
      const view = (id) => ({
        stamp: { studentId: id, head: "123456789012" },
        lines: [
          {
            id: "q1",
            file: "CPU.scala",
            line: 1,
            near: ["x"],
            nearFirst: 1,
            quote: "x",
            change: "delete",
            deletes: true,
            specs: [],
            prediction: null,
            begun: true,
          },
        ],
        bugBegun: true,
        bug: { task: "cpu", programs: [], built: false },
        sims: [],
      });
      window.makePanel = (id) =>
        mountDefense(
          root,
          async () => view(id),
          (error) => {
            throw error;
          },
        );
      window.panel = window.makePanel("FIRST");
      await window.panel.refresh();
    });
    const how = page.locator("#defense textarea");
    await how.fill("my explanation");
    await page.evaluate(() => window.panel.setLocked(true));
    assert.equal(await how.isDisabled(), true);
    await page.evaluate(() => window.panel.setLocked(false));
    assert.equal(await how.isDisabled(), false);
    assert.equal(await how.inputValue(), "my explanation");
    assert.equal(
      await page
        .getByRole("button", { name: "Run with the change" })
        .isDisabled(),
      true,
    );
    await page.evaluate(async () => {
      window.panel.destroy();
      window.panel = window.makePanel("SECOND");
      await window.panel.refresh();
    });
    assert.match(await page.locator("#defense").innerText(), /SECOND/);
    assert.equal(await page.locator("#defense textarea").inputValue(), "");
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
});

// The panel in a real page, against a fake server: what it shows, what it
// sends, and that learner code is shown as text and never as markup.
test("the defense panel predicts, runs, locks and lets go in a browser", async (t) => {
  const { launchChromium, startStaticServer } = await import("./source.js");
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  let server;
  try {
    const hosted = await startStaticServer();
    server = hosted.server;
    const page = await browser.newPage();
    await page.goto(hosted.base);
    const result = await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      const item = {
        id: "q1",
        file: "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala",
        line: 3,
        quote: "io.stall := a && b",
        exercise: "4.2",
        near: [
          "class Hazard {",
          "  // <img src=x onerror=alert(1)>",
          "  io.stall := a && b",
        ],
        nearFirst: 1,
        change: "`&&` becomes `||`",
        deletes: false,
        specs: ["mycpu.pipeline.CPUSpec", "mycpu.pipeline.HazardSpec"],
        prediction: null,
        begun: true,
      };
      const view = (over = {}) => ({
        stamp: {
          studentId: "F74121234",
          head: "0123456789abcdef",
          auditor: false,
        },
        lines: [{ ...item, ...over }],
        runs: [],
        bugBegun: true,
        bug: {
          task: "4-pipeline",
          variant: "Forward",
          programs: ["fib.elf"],
          built: false,
          buildError: null,
        },
        sims: [],
        claim: null,
        ...over.extra,
      });
      const sent = [];
      const failures = [];
      const api = async (path, body) => {
        sent.push([path, body]);
        if (path === "/api/defense/predict")
          return view({
            prediction: { at: 1, fails: body.fails, how: body.how },
          });
        if (path === "/api/defense/run")
          return view({
            extra: {
              runs: [
                {
                  item: "q1",
                  prediction: { fails: ["mycpu.pipeline.HazardSpec"] },
                  outcome: {
                    kind: "tests",
                    failed: ["mycpu.pipeline.HazardSpec"],
                  },
                },
              ],
            },
          });
        return view();
      };
      const root = document.createElement("section");
      document.body.replaceChildren(root);
      const panel = mountDefense(root, api, (error) => failures.push(error));
      await panel.refresh();
      const injected = root.querySelector("img") !== null;
      const runButton = () =>
        [...root.querySelectorAll("button")].find(
          (b) => b.textContent === "Run with the change",
        );
      const runDisabledBefore = runButton().disabled;
      root.querySelector(
        "fieldset.defense-specs input[value='mycpu.pipeline.HazardSpec']",
      ).checked = true;
      root.querySelector("textarea").value = "the stall is lost";
      // Typed before the prediction, as a learner would, through input events.
      const typed = root.querySelector("input.defense-edit");
      typed.value = "io.stall := a || b";
      typed.dispatchEvent(new Event("input"));
      const recordButton = [...root.querySelectorAll("button")].find(
        (b) => b.textContent === "Record prediction",
      );
      recordButton.focus();
      recordButton.click();
      // The panel renders once the request it sent is answered; a bounded
      // wait, so a panel that never redraws fails instead of hanging.
      const rendered = async (ready) => {
        for (let turn = 0; !ready(); turn++) {
          if (turn > 5000) throw new Error("the panel never redrew");
          await new Promise((r) => setTimeout(r, 0));
        }
      };
      await rendered(() => root.querySelector(".defense-recorded"));
      const recorded = root.querySelector(".defense-recorded")?.textContent;
      const announced = root.querySelector("[role=status]").textContent;
      const refocused = document.activeElement?.dataset?.focus;
      // A claim with no cycle is final and wrong, so it is never sent.
      const before = sent.length;
      [...root.querySelectorAll("button")]
        .find((b) => b.textContent === "State the bug")
        .click();
      const unsentClaim = sent.length === before;
      const claimAdvice = root.querySelector("[role=status]").textContent;
      // The panel was drawn again by the prediction's answer.
      const keptEdit = root.querySelector("input.defense-edit").value;
      runButton().focus();
      runButton().click();
      await rendered(() => root.querySelector(".defense-runs li"));
      const runs = root.querySelector(".defense-runs").textContent;
      // The run consumed the prediction, so Run comes back disabled and focus
      // moves on to the next prediction instead of falling to the page.
      const afterRun = document.activeElement?.dataset?.focus;
      panel.setLocked(true);
      const locked = root.querySelector(".defense-controls").disabled;
      panel.destroy();
      return {
        injected,
        runDisabledBefore,
        recorded,
        keptEdit,
        announced,
        refocused,
        unsentClaim,
        claimAdvice,
        runs,
        afterRun,
        locked,
        emptied: root.childElementCount === 0 && root.hidden,
        sent,
        failures: failures.length,
      };
    });
    assert.equal(result.injected, false, "learner code is text, never markup");
    assert.equal(result.runDisabledBefore, true, "no run before a prediction");
    assert.match(result.recorded, /HazardSpec; the stall is lost/);
    assert.equal(
      result.keptEdit,
      "io.stall := a || b",
      "the redraw keeps the edit",
    );
    assert.equal(result.announced, "Prediction recorded.");
    assert.equal(
      result.refocused,
      "q1:record",
      "focus returns after the redraw",
    );
    assert.equal(result.unsentClaim, true);
    assert.match(result.claimAdvice, /cycle as a whole number/);
    assert.match(result.runs, /failed: HazardSpec/);
    assert.equal(result.afterRun, "q1:record", "focus moves on after a run");
    assert.equal(result.locked, true);
    assert.equal(result.emptied, true);
    assert.equal(result.failures, 0);
    assert.deepEqual(result.sent.slice(1), [
      [
        "/api/defense/predict",
        {
          item: "q1",
          fails: ["mycpu.pipeline.HazardSpec"],
          compiles: true,
          how: "the stall is lost",
        },
      ],
      [
        "/api/defense/run",
        {
          item: "q1",
          edited: "io.stall := a || b",
          specs: ["mycpu.pipeline.CPUSpec", "mycpu.pipeline.HazardSpec"],
        },
      ],
    ]);
  } finally {
    server?.close();
    await browser.close();
  }
});

test("a waveform draws the signals that move first, each as a bit or a bus", async () => {
  const { waveLanes } = await import("../../web/task-defense.js");
  const lanes = waveLanes({
    first: 10,
    fields: ["core.pc", "core.stall", "uart.txd", "retire.valid"],
    rows: [
      [0x100, 0, 1, 1],
      [0x104, 1, 1, 0],
      [0x104, 0, 1, 1],
    ],
  });
  assert.deepEqual(
    lanes.map((lane) => [lane.name, lane.moves, lane.bit]),
    [
      ["core.pc", true, false],
      ["core.stall", true, true],
      ["retire.valid", true, true],
      ["uart.txd", false, true],
    ],
  );
  const many = waveLanes(
    {
      first: 0,
      fields: Array.from({ length: 20 }, (_, n) => `f${n}`),
      rows: [Array(20).fill(0)],
    },
    12,
  );
  assert.equal(many.length, 12);
});

test("the panel draws a run's waveform and its test output in a browser", async (t) => {
  const { launchChromium, startStaticServer } = await import("./source.js");
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  let server;
  try {
    const hosted = await startStaticServer();
    server = hosted.server;
    const page = await browser.newPage();
    await page.goto(hosted.base);
    const drawn = await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      const view = {
        stamp: {
          studentId: "F74121234",
          head: "0123456789abcdef",
          auditor: false,
        },
        lines: [
          {
            id: "q1",
            file: "4-pipeline/src/main/scala/A.scala",
            line: 2,
            quote: "a && b",
            exercise: null,
            near: ["x", "a && b"],
            nearFirst: 1,
            change: "`&&` becomes `||`",
            deletes: false,
            specs: ["p.ASpec"],
            prediction: null,
            begun: true,
          },
        ],
        runs: [
          {
            item: "q1",
            prediction: { fails: ["p.ASpec"], compiles: true },
            outcome: { kind: "tests", failed: ["p.ASpec"] },
            summary:
              "[info] - stalls *** FAILED ***\n[info] Tests: succeeded 0, failed 1\n",
          },
        ],
        bugBegun: true,
        bug: {
          task: "4-pipeline",
          variant: "Forward",
          programs: ["fib.elf"],
          built: true,
          buildError: null,
        },
        sims: [
          {
            model: "bug",
            program: "fib.elf",
            first: 4,
            count: 3,
            retire: [[5, "pc=0000000c x"]],
            error: null,
            table: {
              first: 4,
              fields: ["core.pc", "core.flush"],
              rows: [
                [16, 0],
                [20, 1],
                [20, 0],
              ],
              end: {},
            },
          },
        ],
        simulations: 1,
        claim: null,
      };
      const root = document.createElement("section");
      document.body.replaceChildren(root);
      const panel = mountDefense(
        root,
        async () => view,
        () => {},
      );
      await panel.refresh();
      const wave = root.querySelector("svg.defense-waveform");
      return {
        label: wave?.getAttribute("aria-label"),
        names: [...wave.querySelectorAll("g > text:first-child")].map(
          (t) => t.textContent,
        ),
        bits: wave.querySelectorAll("polyline.bit").length,
        buses: wave.querySelectorAll("polygon.bus").length,
        output: root.querySelector(".defense-runs details pre")?.textContent,
      };
    });
    assert.equal(
      drawn.label,
      "Waveform of 2 signals over 3 cycles from cycle 4",
    );
    assert.deepEqual(drawn.names, ["core.pc", "core.flush"]);
    assert.equal(drawn.bits, 1);
    assert.equal(drawn.buses, 2, "core.pc holds 0x10, then 0x14");
    assert.match(drawn.output, /Tests: succeeded 0, failed 1/);
  } finally {
    server?.close();
    await browser.close();
  }
});

test("a prediction that the change will not compile, a refusal and an answer over the limit", async (t) => {
  const { launchChromium, startStaticServer } = await import("./source.js");
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  let server;
  try {
    const hosted = await startStaticServer();
    server = hosted.server;
    const page = await browser.newPage();
    await page.goto(hosted.base);
    const result = await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      const item = {
        id: "q1",
        file: "1-single-cycle/src/main/scala/A.scala",
        line: 7,
        quote: "val k = 4.U",
        exercise: null,
        near: ["val k = 4.U"],
        nearFirst: 7,
        change: "`4.U` becomes `5.U`",
        deletes: false,
        specs: ["p.ASpec", "p.BSpec"],
        prediction: null,
        begun: true,
      };
      const base = {
        stamp: { studentId: "F74121234", head: "0123456789abcdef" },
        lines: [item],
        runs: [],
        bugBegun: true,
        bug: { task: "1-single-cycle", programs: ["fib.elf"], built: true },
        sims: [],
        claim: null,
      };
      let view = base;
      const sent = [];
      const api = async (path, body) => {
        if (body) sent.push([path, body]);
        if (path === "/api/defense/predict") {
          view = {
            ...base,
            lines: [{ ...item, prediction: { ...body, at: 1 } }],
          };
        }
        if (path === "/api/defense/run")
          throw { code: "defense_refused", message: "The run was refused." };
        if (path === "/api/defense/claim")
          view = {
            ...view,
            claim: { cycle: 9, signal: "s", rule: "r", exercise: "1.2" },
          };
        return view;
      };
      const root = document.createElement("section");
      document.body.replaceChildren(root);
      const panel = mountDefense(root, api, () => {});
      await panel.refresh();
      const status = () => root.querySelector("[role=status]").textContent;
      const settle = async (ready) => {
        for (let turn = 0; !ready(); turn++) {
          if (turn > 5000) throw new Error("the panel never redrew");
          await new Promise((r) => setTimeout(r, 0));
        }
      };
      const button = (name) =>
        [...root.querySelectorAll("button")].find(
          (b) => b.textContent === name,
        );
      const label =
        root.querySelector("input.defense-edit").labels[0].textContent;
      // Over the server's byte limit though under 1024 characters.
      const how = root.querySelector("textarea");
      how.value = "\u932f".repeat(400);
      button("Record prediction").click();
      const long = status();
      const unsentLong = sent.length === 0;
      root.querySelector(
        "fieldset.defense-specs input[value='p.ASpec']",
      ).checked = true;
      const noBuild = [...root.querySelectorAll("label")]
        .find((l) => l.textContent.includes("will not compile"))
        .querySelector("input");
      noBuild.checked = true;
      how.value = "the width no longer matches";
      button("Record prediction").click();
      await settle(() => root.querySelector(".defense-recorded"));
      const recorded = root.querySelector(".defense-recorded").textContent;
      // A refused run says why beside the control, not only atop the page.
      button("Run with the change").click();
      await settle(() => status() === "The run was refused.");
      // A stated claim moves focus to what was stated.
      const fields = root.querySelectorAll(".defense-claim input");
      fields[0].value = "9";
      fields[1].value = "s";
      fields[2].value = "r";
      fields[3].value = "1.2";
      button("State the bug").focus();
      button("State the bug").click();
      await settle(() => button("Bug stated"));
      const afterClaim = document.activeElement?.dataset?.focus;
      // A run row whose prediction said the change would not compile.
      panel.destroy();
      const rows = document.createElement("section");
      document.body.replaceChildren(rows);
      const shown = mountDefense(
        rows,
        async () => ({
          ...base,
          runs: [
            {
              item: "q1",
              prediction: { fails: [], compiles: false },
              outcome: { kind: "compileError" },
            },
          ],
        }),
        () => {},
      );
      await shown.refresh();
      return {
        label,
        long,
        unsentLong,
        recorded,
        afterClaim,
        sent,
        row: rows.querySelector(".defense-runs li").textContent,
      };
    });
    assert.equal(result.label, "Line 7 after your change");
    assert.match(result.long, /too long: keep it within 1024 bytes/);
    assert.equal(
      result.unsentLong,
      true,
      "an answer over the limit is not sent",
    );
    assert.match(result.recorded, /the change will not compile/);
    assert.deepEqual(result.sent[0], [
      "/api/defense/predict",
      {
        item: "q1",
        fails: [],
        compiles: false,
        how: "the width no longer matches",
      },
    ]);
    assert.equal(result.afterClaim, "bug:stated");
    assert.equal(result.row, "Predicted no build; the change did not compile");
  } finally {
    server?.close();
    await browser.close();
  }
});

test("an item shows its overview and colored Chisel until Begin opens it", async (t) => {
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  const { server, base } = await startStaticServer();
  try {
    const page = await browser.newPage();
    await page.goto(`${base}/task.html`, { waitUntil: "commit" });
    await page.setContent('<section id="defense"></section>');
    await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      const item = {
        id: "q1",
        file: "2-rv32im/src/main/scala/mycpu/rv32im/CPU.scala",
        line: 2,
        quote: "val active = !reset.asBool && !halted",
        exercise: "2.1",
        near: [
          "val halted = RegInit(false.B) // kept",
          "val active = !reset.asBool && !halted",
          'when (io.x === 3.U) { r := Wire(UInt(32.W)) } // <b>"no"</b>',
        ],
        nearFirst: 1,
        change: "`&&` becomes `||`",
        deletes: false,
        specs: ["mycpu.rv32im.T2HaltSpec"],
        prediction: null,
        begun: false,
      };
      let view = {
        stamp: { studentId: "F74121234", head: "0123456789abcdef" },
        lines: [item],
        runs: [],
        bugBegun: false,
        hints: { allowed: 2, given: {} },
        bug: {
          task: "4-pipeline",
          variant: "Forward",
          programs: [],
          built: false,
        },
        sims: [],
        claim: null,
      };
      window.sent = [];
      const api = async (path, body) => {
        if (body) {
          window.sent.push([path, body]);
          if (body.item === "q1")
            view = {
              ...view,
              lines: [{ ...item, begun: true }],
              hints: { allowed: 2, given: { q1: 1 } },
            };
          else view = { ...view, bugBegun: true };
        }
        return view;
      };
      const panel = mountDefense(
        document.querySelector("#defense"),
        api,
        () => {},
      );
      await panel.refresh();
    });
    const q1 = page.locator("article").first();
    await q1.getByText("Overview").waitFor();
    // Four steps, and what a hint costs when the package allows them.
    assert.equal(await q1.locator("ol li").count(), 5);
    assert.match(
      await q1.locator("ol li").last().textContent(),
      /hint .*\(2 in the whole defense\).*2 at most/,
    );
    assert.match(await q1.locator("ol").textContent(), /`&&` becomes `\|\|`/);
    assert.equal(
      await page.locator("input.defense-edit").count(),
      0,
      "no controls before Begin",
    );
    assert.equal(
      await page.getByRole("button", { name: "Begin the bug item" }).count(),
      1,
    );

    // The grammar loads and the lines are drawn again, colored, as text.
    await page.locator(".tok-chisel").first().waitFor();
    const kinds = await page.$$eval(
      "pre.defense-near span[class^=tok-]",
      (spans) => spans.map((span) => `${span.className}:${span.textContent}`),
    );
    for (const expected of [
      "tok-chisel:RegInit",
      "tok-literal:false",
      "tok-number:B",
      "tok-keyword:val",
      "tok-hardware-op:===",
      "tok-hardware-op::=",
      "tok-chisel:Wire",
      "tok-chisel-type:UInt",
      "tok-number:32",
      "tok-comment:// kept",
    ])
      assert.ok(kinds.includes(expected), `${expected} in ${kinds.join(" ")}`);
    assert.equal(
      await page.locator("pre.defense-near b").count(),
      0,
      "code is text",
    );
    assert.equal(
      await page.locator("pre.defense-near").textContent(),
      '   1  val halted = RegInit(false.B) // kept\n   2  val active = !reset.asBool && !halted\n   3  when (io.x === 3.U) { r := Wire(UInt(32.W)) } // <b>"no"</b>\n',
    );

    await page.getByRole("button", { name: "Begin item 1" }).click();
    await page.locator("input.defense-edit").waitFor();
    assert.deepEqual(await page.evaluate(() => window.sent), [
      ["/api/defense/begin", { item: "q1" }],
    ]);
    assert.equal(
      await page.evaluate(() => document.activeElement?.dataset?.focus),
      "q1:opened",
      "focus moves to the opened item",
    );
    assert.match(
      await page.locator("[data-focus='q1:opened']").textContent(),
      / · 1 hint$/,
    );

    // The bug item opens the same way, onto its heading.
    await page.getByRole("button", { name: "Begin the bug item" }).click();
    await page.getByRole("button", { name: "Build the two models" }).waitFor();
    assert.deepEqual((await page.evaluate(() => window.sent)).at(-1), [
      "/api/defense/begin",
      { item: "bug" },
    ]);
    assert.equal(
      await page.evaluate(() => document.activeElement?.dataset?.focus),
      "bug:opened",
    );
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("a panel that cannot load tries again, and offers no Begin once items are over", async (t) => {
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  const { server, base } = await startStaticServer();
  try {
    const page = await browser.newPage();
    await page.clock.install();
    await page.goto(`${base}/task.html`, { waitUntil: "commit" });
    await page.setContent('<section id="defense"></section>');
    await page.evaluate(async () => {
      const { mountDefense } = await import("/task-defense.js");
      window.asked = 0;
      const api = async () => {
        window.asked += 1;
        if (window.asked === 1) throw new Error("offline");
        if (window.asked > 2) throw { code: "defense_closed" };
        return {
          stamp: { studentId: "F74121234", head: "0123456789abcdef" },
          lines: [
            {
              id: "q1",
              file: "2-rv32im/src/main/scala/mycpu/rv32im/CPU.scala",
              line: 2,
              quote: "val active = !halted",
              exercise: "2.1",
              near: ["val active = !halted"],
              nearFirst: 2,
              change: "`!` is removed",
              deletes: false,
              specs: [],
              prediction: null,
              begun: false,
            },
          ],
          runs: [],
          bugBegun: false,
          itemsOpen: false,
          hints: { allowed: 0, given: {} },
          bug: {
            task: "4-pipeline",
            variant: null,
            programs: [],
            built: false,
          },
          sims: [],
          claim: null,
        };
      };
      const panel = mountDefense(
        document.querySelector("#defense"),
        api,
        () => {},
      );
      window.panel = panel;
      await panel.refresh();
    });
    await page
      .getByText("The panel could not load; it tries again shortly.")
      .waitFor();
    await page.clock.runFor(5000);
    const over = page.getByRole("button", { name: "The items are over" });
    await over.first().waitFor();
    assert.equal(await over.count(), 2, "the line item and the bug item");
    assert.ok(await over.first().isDisabled());
    assert.equal(await page.evaluate(() => window.asked), 2);
    // Once the defense is closed the panel stays as drawn and asks no more.
    await page.evaluate(() => window.panel.refresh());
    await page.clock.runFor(20000);
    assert.equal(await page.evaluate(() => window.asked), 3);
    assert.equal(await over.count(), 2);
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
});
