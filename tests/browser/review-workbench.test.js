import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { execFileSync, spawn } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { launchChromium, read, root } from "./source.js";

const helper = join(root, ".claude/skills/codetrial-review/workbench.py");
const python = process.env.PYTHON || "python3";
let browser;
before(async () => {
  browser = await launchChromium();
});
after(async () => {
  await browser?.close();
});

async function start(t, followUps = [], managed = false, patchText = null) {
  const bundle = mkdtempSync(join(tmpdir(), "codetrial-workbench-test-"));
  t.after(() => rmSync(bundle, { recursive: true, force: true }));
  const files = ["src/example.rs", "tests/example.rs"];
  writeFileSync(
    join(bundle, "manifest.json"),
    JSON.stringify({
      head: "a".repeat(40),
      committed: true,
      pr: null,
      files: files.map((path) => ({ path, additions: 1, deletions: 1 })),
      commit_sizes: {
        head: "a".repeat(40),
        complete: true,
        commits: [
          { sha: "a".repeat(40), known_lines: 200, complete: true, files: [] },
        ],
      },
    }),
  );
  writeFileSync(
    join(bundle, "pr.diff"),
    patchText ||
      files
        .map(
          (path) =>
            `diff --git a/${path} b/${path}\n@@ -1 +1 @@\n-old\n+<img src=x onerror=alert(1)>\n`,
        )
        .join(""),
  );
  const questions = join(bundle, "questions.json");
  writeFileSync(questions, JSON.stringify(followUps));
  const args = ["--bundle", bundle, "--questions", questions];
  const env = { ...process.env };
  if (managed) {
    const bin = join(bundle, "bin");
    mkdirSync(bin);
    const cli = join(bin, "codex");
    writeFileSync(
      cli,
      `#!/usr/bin/env python3
import json, sys
from pathlib import Path
if sys.argv[1:3] == ['login', 'status']: sys.exit(0)
sys.stdin.read()
Path(sys.argv[sys.argv.index('--output-last-message')+1]).write_text(json.dumps({'notes':[{'field':'motivation','text':'The key change preserves the author goal; caller rollback is still unverified.'}]}))
`,
    );
    chmodSync(cli, 0o700);
    env.PATH = `${bin}:${env.PATH}`;
    args.push("--reviewer", "codex");
  }
  const server = spawn(python, [helper, "serve", "--port", "0", ...args], {
    cwd: root,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  t.after(async () => {
    if (server.exitCode === null) {
      const exited = once(server, "exit");
      server.kill();
      await exited;
    }
  });
  const lines = createInterface({ input: server.stdout });
  const [url] = await once(lines, "line", { signal: t.signal });
  lines.close();
  const page = await browser.newPage();
  t.after(() => page.close());
  await page.goto(url);
  await page.locator("#workspace").waitFor({ state: "visible" });
  const inspect = () =>
    JSON.parse(
      execFileSync(python, [helper, "inspect", ...args], { encoding: "utf8" }),
    );
  return { bundle, args, page, url, inspect, files, server };
}

test(
  "the real web page sends an explanation to a waiting agent and displays its feedback",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, args, bundle, inspect } = await start(t);
    await page.getByText(/Commit threshold reached .*at least 200/).waitFor();
    assert.equal(inspect().total, 4);
    assert.equal(await page.locator("#answers-form textarea").count(), 1);
    await page.locator(".file-button").nth(1).click();
    assert.equal(await page.locator("#answers-form textarea").count(), 1);
    for (const input of await page
      .locator("#overall-answers textarea, #question-answers textarea")
      .all()) {
      await input.fill(
        "The failure keeps the old record; the regression check exercises that path.",
      );
    }
    assert.equal(await page.locator("#diff img").count(), 0);
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    const saved = inspect();
    assert.equal(saved.status, "ready_to_send");
    const waiting = spawn(python, [
      helper,
      "wait",
      ...args,
      "--after",
      String(saved.revision),
      "--timeout",
      "10",
    ]);
    t.after(() => {
      if (waiting.exitCode === null) waiting.kill();
    });
    let received = "";
    waiting.stdout.on("data", (chunk) => {
      received += chunk;
    });
    const done = once(waiting, "exit");
    await page.locator("#send").click();
    const [code] = await done;
    assert.equal(code, 0);
    const submission = JSON.parse(received);
    assert.equal(submission.answers.motivation, saved.answers.motivation);
    // Held by the reviewer and not answered: the page shows it is waiting, and
    // keeps the motion out of reach of someone who asked for none.
    await page.locator("#waiting").waitFor({ state: "visible" });
    const animation = () =>
      page
        .locator(".waiting-dots i")
        .first()
        .evaluate((dot) => getComputedStyle(dot).animationName);
    assert.equal(await animation(), "waiting-bounce");
    await page.emulateMedia({ reducedMotion: "reduce" });
    assert.equal(await animation(), "waiting-breathe");
    assert.equal(
      await page
        .locator(".waiting-bar")
        .evaluate((bar) => getComputedStyle(bar, "::after").animationName),
      "none",
    );
    await page.emulateMedia({ reducedMotion: "no-preference" });
    const feedback = join(bundle, "feedback.json");
    writeFileSync(
      feedback,
      JSON.stringify({
        identity: submission.identity,
        answer_digest: submission.answer_digest,
        notes: [
          {
            field: "motivation",
            text: "Trace the write failure through the caller before choosing a retry.",
          },
        ],
      }),
    );
    execFileSync(python, [
      helper,
      "feedback",
      ...args,
      "--request-id",
      submission.request_id,
      "--file",
      feedback,
    ]);
    await page
      .getByText(
        "Trace the write failure through the caller before choosing a retry.",
        { exact: false },
      )
      .waitFor();
    assert.equal(await page.locator("#waiting").isVisible(), false);
    assert.equal(inspect().status, "reflection_needed");
    await page.locator("#self-grade").selectOption("missed");
    await page
      .locator("#correction")
      .fill("I have not verified the caller's retry behavior.");
    await page.locator("#reflect-save").click();
    await page
      .getByText(
        "Reflection recorded. This is participation evidence, not code approval.",
      )
      .waitFor();
    assert.equal(inspect().status, "explanation_complete");
    await page
      .locator('textarea[name="motivation"]')
      .fill("An unsaved correction while the reviewer adds context.");
    await page
      .getByText(
        "Earlier feedback is shown below. Your latest answers still need review.",
        { exact: true },
      )
      .waitFor();
    const nextFeedback = JSON.parse(readFileSync(feedback, "utf8"));
    nextFeedback.notes[0].text =
      "The caller also needs to preserve the previous record.";
    writeFileSync(feedback, JSON.stringify(nextFeedback));
    execFileSync(python, [
      helper,
      "feedback",
      ...args,
      "--request-id",
      submission.request_id,
      "--file",
      feedback,
    ]);
    await page
      .getByText("The caller also needs to preserve the previous record.", {
        exact: false,
      })
      .waitFor();
    assert.equal(await page.locator("#self-grade").inputValue(), "");
    assert.equal(await page.locator("#correction").inputValue(), "");
    assert.equal(
      await page.locator('textarea[name="motivation"]').inputValue(),
      "An unsaved correction while the reviewer adds context.",
    );
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    assert.equal(inspect().status, "ready_to_send");
    await page.reload();
    await page.locator("#workspace").waitFor({ state: "visible" });
    assert.equal(
      await page.locator('textarea[name="motivation"]').inputValue(),
      "An unsaved correction while the reviewer adds context.",
    );
  },
);

test(
  "a key change prompt replaces the generic question with one answer",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t, [
      {
        id: "motivation-gap",
        path: "src/example.rs",
        prompt: "Why is the existing behavior insufficient?",
      },
    ]);
    assert.equal(await page.locator("#answers-form textarea").count(), 1);
    await page
      .getByText("Why is the existing behavior insufficient?", { exact: true })
      .waitFor();
    await page
      .locator('textarea[name="motivation"]')
      .fill(
        "The existing behavior prevents the intended workflow, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#send").click();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    assert.equal(inspect().status, "awaiting_review");
  },
);

test(
  "reviewer failures are visible and can be explicitly retried",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, bundle, inspect } = await start(t);
    await page
      .locator('textarea[name="motivation"]')
      .fill(
        "The current workflow loses the submission handoff, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#send").click();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    assert.equal(
      await page.locator("#feedback-status").textContent(),
      "Saved and awaiting reviewer feedback",
    );
    const before = inspect();
    writeFileSync(
      join(bundle, "reviewer-job.json"),
      JSON.stringify({
        identity: before.identity,
        request_id: before.request_id,
        state: "failed",
      }),
    );
    writeFileSync(
      join(bundle, "explanation-agent.json"),
      JSON.stringify({
        identity: before.identity,
        state: "error",
        until: Date.now() / 1000 + 60,
        error: "reviewer_failed",
      }),
    );
    await page.getByText(/The reviewer could not finish/).waitFor();
    await page.locator("#send").click();
    await page.waitForFunction(
      (requestId) =>
        document.querySelector("#send").getAttribute("aria-disabled") !==
        "true",
      before.request_id,
    );
    assert.notEqual(inspect().request_id, before.request_id);
  },
);

test(
  "an older tab keeps unsaved explanations when a newer answer is saved",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, url, inspect } = await start(t);
    const other = await browser.newPage();
    t.after(() => other.close());
    await other.goto(url);
    await other.locator("#workspace").waitFor({ state: "visible" });
    await other
      .locator('textarea[name="motivation"]')
      .fill("My local reasoning must survive the conflict.");
    await page
      .locator('textarea[name="motivation"]')
      .fill("A newer explanation from the first tab.");
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    await other.locator("#save").click();
    await other.locator("#error").waitFor({ state: "visible" });
    assert.equal(
      await other.locator('textarea[name="motivation"]').inputValue(),
      "My local reasoning must survive the conflict.",
    );
    assert.equal(
      inspect().answers.motivation,
      "A newer explanation from the first tab.",
    );
  },
);

test(
  "the workspace uses lobby colors in both themes and fits a narrow screen",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page } = await start(t);
    const reference = await browser.newPage();
    t.after(() => reference.close());
    for (const colorScheme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme });
      await reference.emulateMedia({ colorScheme });
      await reference.setContent('<body class="practice-page"></body>');
      await reference.addStyleTag({ content: read("web/styles.css") });
      const colors = (target) =>
        target.locator("body").evaluate((el) => {
          const css = getComputedStyle(el);
          return [
            "--page",
            "--surface",
            "--ink",
            "--accent",
            "--radius-lg",
          ].map((key) => css.getPropertyValue(key));
        });
      assert.deepEqual(await colors(page), await colors(reference));
    }
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
    );
    const css = await page.request.get(new URL("/styles.css", page.url()).href);
    assert.equal(
      await css.text(),
      readFileSync(join(root, "web/styles.css"), "utf8"),
    );
  },
);

test(
  "managed browser submission returns a reference and accepts human uncertainty",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(
      t,
      [
        {
          id: "intent",
          path: "src/example.rs",
          prompt: "Why preserve this record on write failure?",
          quote: "-old",
        },
      ],
      true,
    );
    assert.equal(await page.locator("#answers-form textarea").count(), 1);
    await page
      .locator('[name="motivation"]')
      .fill(
        "Keep earlier evidence available when a write fails, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#confidence").selectOption("probably");
    await page.locator("#send").click();
    await page
      .getByText(
        "The key change preserves the author goal; caller rollback is still unverified.",
        { exact: false },
      )
      .waitFor();
    assert.equal(inspect().status, "reflection_needed");
    await page.locator("#self-grade").selectOption("partly");
    await page.locator("#correction").fill("I have not verified rollback yet.");
    await page.locator("#reflect-save").click();
    await page
      .getByText(
        "Reflection recorded. This is participation evidence, not code approval.",
      )
      .waitFor();
    const state = inspect();
    assert.equal(state.status, "explanation_complete");
    assert.equal(state.submission.answers.confidence, "probably");
    assert.equal(state.reflection.grade, "partly");
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(
      await page.locator("#completion-notice").getAttribute("role"),
      "status",
    );
    const notice = await page.locator("#completion-notice").innerText();
    assert.match(notice, /close this window and continue submitting/i);
    assert.match(notice, /pr-description\.md/);
    assert.equal(await page.locator("#close-window").isVisible(), true);
    assert.equal(
      await page.locator("#reflect-save").textContent(),
      "Confirmed",
    );
    assert.equal(
      await page.locator("#reflect-save").getAttribute("aria-disabled"),
      "true",
    );
    assert.equal(
      await page
        .locator("#reflect-save")
        .evaluate((button) => button === document.activeElement),
      true,
    );
    assert.match(
      await page.locator("#reviewer-presence").textContent(),
      /No further response is needed/,
    );
    await page.locator("#reflect-save").evaluate((button) => button.click());
    assert.equal(inspect().revision, state.revision);
    await page.reload();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    for (const colorScheme of ["dark", "light"]) {
      await page.emulateMedia({ colorScheme });
      assert.equal(
        await page.locator("#completion-notice").evaluate((element) => {
          const style = getComputedStyle(element);
          const probe = document.createElement("span");
          probe.style.color = "var(--good)";
          element.append(probe);
          const color = getComputedStyle(probe).color;
          probe.remove();
          return style.borderTopColor === color;
        }),
        true,
      );
    }
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
    );
    await page
      .locator('[name="motivation"]')
      .fill(
        "A revised explanation requires review again, which is why the previous record has to stay readable after a failed write.",
      );
    assert.equal(await page.locator("#completion-notice").isVisible(), false);
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    assert.equal(inspect().status, "ready_to_send");
    assert.equal(await page.locator("#completion-notice").isVisible(), false);
  },
);

test(
  "file navigation changes the question and preserves separate answers and confirmation",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(
      t,
      [
        {
          id: "core",
          path: "src/example.rs",
          quote: "-old",
          prompt: "Why replace the existing behavior?",
        },
        {
          id: "regression",
          path: "tests/example.rs",
          quote: "-old",
          prompt: "What regression should this test catch?",
        },
      ],
      true,
    );
    await page
      .locator('[name="motivation"]')
      .fill(
        "Keep earlier evidence available when writing fails, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator('[data-path="tests/example.rs"]').click();
    await page
      .getByText("What regression should this test catch?", { exact: true })
      .waitFor();
    assert.equal(await page.locator("#answers-form textarea").count(), 1);
    await page
      .locator('[name="reading:regression"]')
      .fill(
        "A failed write must preserve the existing evidence, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#main-question").click();
    assert.equal(
      await page.locator('[name="motivation"]').inputValue(),
      "Keep earlier evidence available when writing fails, which is why the previous record has to stay readable after a failed write.",
    );
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.equal(
      await page.locator('[name="reading:regression"]').inputValue(),
      "A failed write must preserve the existing evidence, which is why the previous record has to stay readable after a failed write.",
    );
    await page.locator("#all-diff").click();
    assert.equal(
      await page.locator('[name="motivation"]').inputValue(),
      "Keep earlier evidence available when writing fails, which is why the previous record has to stay readable after a failed write.",
    );
    await page.locator('[data-path="tests/example.rs"]').click();
    await page.locator("#send").click();
    await page
      .getByText(
        "The key change preserves the author goal; caller rollback is still unverified.",
        { exact: false },
      )
      .waitFor();
    await page.locator("#self-grade").selectOption("partly");
    await page
      .locator("#correction")
      .fill("The caller rollback still needs verification.");
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    const confirmed = inspect();
    await page.locator('[data-path="src/example.rs"]').click();
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.equal(inspect().revision, confirmed.revision);
    assert.equal(await page.locator("#completion-notice").isVisible(), true);
    await page.reload();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.equal(
      await page.locator('[name="reading:regression"]').inputValue(),
      "A failed write must preserve the existing evidence, which is why the previous record has to stay readable after a failed write.",
    );
    await page
      .locator('[name="reading:regression"]')
      .fill("The rollback boundary remains unclear.");
    assert.equal(await page.locator("#completion-notice").isVisible(), false);
  },
);

test(
  "renamed diffs show the selected file and accept its quoted question",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const patchText =
      "diff --git a/src/old.rs b/src/example.rs\nrename from src/old.rs\nrename to src/example.rs\n--- a/src/old.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n-old\n+new\n" +
      "diff --git a/tests/example.rs b/tests/example.rs\n@@ -1 +1 @@\n-unrelated-old\n+unrelated-new\n";
    const { page } = await start(
      t,
      [
        {
          id: "rename",
          path: "src/example.rs",
          prompt: "Why preserve behavior through this rename?",
          quote: "+new",
        },
      ],
      false,
      patchText,
    );
    assert.match(
      await page.locator("#diff").textContent(),
      /rename to src\/example.rs/,
    );
    assert.doesNotMatch(
      await page.locator("#diff").textContent(),
      /unrelated-new/,
    );
    await page
      .getByText("Why preserve behavior through this rename?", { exact: true })
      .waitFor();
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.match(await page.locator("#diff").textContent(), /unrelated-new/);
  },
);

test(
  "reflection fields stay bound to the saved text while confirmation is pending",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t, [], true);
    await page
      .locator('[name="motivation"]')
      .fill(
        "Preserve the existing record after failure, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#send").click();
    await page.locator("#reflection-form").waitFor({ state: "visible" });
    let saved, release;
    const committed = new Promise((resolve) => {
      saved = resolve;
    });
    const gate = new Promise((resolve) => {
      release = resolve;
    });
    await page.route("**/api/reflection", async (route) => {
      const response = await route.fetch();
      saved();
      await gate;
      await route.fulfill({ response });
    });
    await page.locator("#self-grade").selectOption("partly");
    await page
      .locator("#correction")
      .fill("The original correction is the one being confirmed.");
    await page.locator("#reflect-save").click();
    await committed;
    try {
      assert.equal(await page.locator("#self-grade").isDisabled(), true);
      assert.equal(
        await page.locator("#correction").evaluate((field) => field.readOnly),
        true,
      );
      await page.locator("#correction").press("End");
      await page.keyboard.type("unsaved new correction");
      assert.equal(
        await page.locator("#correction").inputValue(),
        inspect().reflection.correction,
      );
    } finally {
      release();
    }
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(
      await page.locator("#correction").inputValue(),
      inspect().reflection.correction,
    );
    assert.equal(
      await page.locator("#correction").evaluate((field) => field.readOnly),
      true,
    );
  },
);

test(
  "another tab's confirmation never confirms a different local reflection draft",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, url, inspect } = await start(t, [], true);
    await page
      .locator('[name="motivation"]')
      .fill(
        "Preserve records when writing fails, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#send").click();
    await page.locator("#reflection-form").waitFor({ state: "visible" });
    const other = await browser.newPage();
    t.after(() => other.close());
    await other.goto(url);
    await other.locator("#reflection-form").waitFor({ state: "visible" });
    await page.locator("#self-grade").selectOption("partly");
    await page.locator("#correction").fill("Unsaved local correction B.");
    await other.locator("#self-grade").selectOption("partly");
    await other
      .locator("#correction")
      .fill("Saved correction A from another tab.");
    await other.locator("#reflect-save").click();
    await other.locator("#completion-notice").waitFor({ state: "visible" });
    await page.locator("#refresh").click();
    await page
      .getByText(
        "A different reflection was saved elsewhere. Your draft is unchanged; review it before confirming.",
        { exact: true },
      )
      .waitFor();
    assert.equal(await page.locator("#completion-notice").isVisible(), false);
    assert.equal(
      await page.locator("#correction").inputValue(),
      "Unsaved local correction B.",
    );
    assert.equal(
      inspect().reflection.correction,
      "Saved correction A from another tab.",
    );
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(
      inspect().reflection.correction,
      "Unsaved local correction B.",
    );
    await other.locator("#refresh").click();
    await other.waitForFunction(
      () =>
        document.getElementById("correction").value ===
        "Unsaved local correction B.",
    );
    assert.equal(
      await other.locator("#correction").inputValue(),
      "Unsaved local correction B.",
    );
  },
);

test(
  "the page coaches the answer while it is written, before anything is sent",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page } = await start(t);
    const box = page.locator('textarea[name="motivation"]');
    await page
      .getByText("At least 12 words", { exact: true })
      .waitFor({ state: "visible" });
    assert.match(
      await page.locator("#coach-message").innerText(),
      /in your own words/,
    );
    await box.fill("Too short");
    await page.getByText("2 of 12 words", { exact: true }).waitFor();
    await box.fill(
      "The handler skips the escaping step so the stored value reaches the page untouched and runs",
    );
    await page
      .getByText("Nothing from the changed lines is mentioned yet", {
        exact: false,
      })
      .waitFor();
    assert.ok((await page.locator(".coach-chip").count()) > 0);
    await page.locator(".coach-chip").first().click();
    assert.match(await box.inputValue(), /`[A-Za-z_]+`$/);
    await page.getByText("Specific enough", { exact: false }).waitFor();
    assert.equal(await page.locator("#coach-anchors").isVisible(), false);
  },
);

test(
  "the core answer coach follows navigation to optional questions",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page } = await start(t, [
      {
        id: "core",
        path: "src/example.rs",
        quote: "-old",
        prompt: "Why replace the existing behavior?",
      },
      {
        id: "regression",
        path: "tests/example.rs",
        quote: "-old",
        prompt: "What regression should this test catch?",
      },
    ]);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.locator(".coach-chip").first().waitFor({ state: "visible" });
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.equal(await page.locator("#answer-coach").isVisible(), false);
    await page
      .locator('[name="reading:regression"]')
      .fill("Optional test context");
    assert.equal(await page.locator("#answer-coach").isVisible(), false);
    await page.locator("#main-question").click();
    await page.locator(".coach-chip").first().waitFor({ state: "visible" });
    await page.locator(".coach-chip").first().click();
    assert.match(
      await page.locator('[name="motivation"]').inputValue(),
      /`[A-Za-z_]+`/,
    );
    assert.deepEqual(errors, []);
  },
);

test(
  "finishing stops the server, leaves the token behind and says what comes next",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect, server } = await start(t, [], true);
    await page
      .locator('[name="motivation"]')
      .fill(
        "Keep earlier evidence available when a write fails, which is why the previous record has to stay readable after a failed write.",
      );
    await page.locator("#send").click();
    await page.locator("#self-grade").waitFor({ state: "visible" });
    await page.locator("#self-grade").selectOption("got");
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(inspect().status, "explanation_complete");
    await page.locator("#close-window").click();
    await page.locator("#finished").waitFor({ state: "visible" });
    assert.equal(await page.locator("#workspace").isVisible(), false);
    assert.doesNotMatch(page.url(), /token=/);
    assert.match(
      await page.locator("#finished").innerText(),
      /pr-description\.md/,
    );
    if (server.exitCode === null)
      await once(server, "exit", { signal: AbortSignal.timeout(8000) });
    assert.equal(server.exitCode, 0);
  },
);

test(
  "finishing refuses to leave unsaved edits behind",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect, server } = await start(t, [], true);
    const box = page.locator('textarea[name="motivation"]');
    const text =
      "Keep earlier evidence available when a write fails, which is why the previous record has to stay readable after a failed write.";
    await box.fill(text);
    await page.locator("#send").click();
    await page.locator("#self-grade").waitFor({ state: "visible" });
    await page.locator("#self-grade").selectOption("got");
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    await box.fill(text + " And one more edit that was never saved.");
    // An edit hides the notice and its button; a click that still arrives,
    // from a stale tab or a script, must not close the server on the edit.
    assert.equal(await page.locator("#completion-notice").isVisible(), false);
    await page.evaluate(() => document.getElementById("close-window").click());
    await page
      .getByText("Save your latest edit before finishing", { exact: false })
      .waitFor();
    assert.equal(await page.locator("#finished").isVisible(), false);
    assert.equal(server.exitCode, null);
    assert.equal(inspect().status, "explanation_complete");
  },
);

test(
  "the coach follows an answer saved from another tab",
  { timeout: 40000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, url } = await start(t);
    const words = (n) => Array(n).fill("alpha").join(" ");
    const box = page.locator('textarea[name="motivation"]');
    await box.fill(words(14));
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    await page.getByText("14 of 12 words", { exact: true }).waitFor();
    const other = await browser.newPage();
    t.after(() => other.close());
    await other.goto(url);
    await other.locator("#workspace").waitFor({ state: "visible" });
    await other.locator('textarea[name="motivation"]').fill(words(13));
    await other.locator("#save").click();
    await other.getByText("Saved on this computer", { exact: true }).waitFor();
    await page.getByText("13 of 12 words", { exact: true }).waitFor({
      timeout: 15000,
    });
  },
);
