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

async function start(
  t,
  followUps = [],
  managed = false,
  patchText = null,
  manifestFiles = null,
) {
  const bundle = mkdtempSync(join(tmpdir(), "codetrial-workbench-test-"));
  const writers = [];
  const own = (child) => {
    const closed = new Promise((resolve) => child.once("close", resolve));
    writers.push({ child, closed });
    return child;
  };
  let page;
  t.after(async () => {
    try {
      await page?.close();
    } finally {
      for (const { child, closed } of writers.reverse()) {
        if (child.pid && child.exitCode === null && child.signalCode === null)
          child.kill();
        await closed;
      }
      // The reviewer can write presence files until these processes close.
      rmSync(bundle, { recursive: true, force: true });
    }
  });
  const files = ["src/example.rs", "tests/example.rs"];
  writeFileSync(
    join(bundle, "manifest.json"),
    JSON.stringify({
      head: "a".repeat(40),
      committed: true,
      pr: null,
      files:
        manifestFiles ||
        files.map((path) => ({ path, additions: 1, deletions: 1 })),
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
  } else {
    // The default would pick a Codex worker here when one is installed.
    args.push("--reviewer", "manual");
  }
  const server = own(
    spawn(python, [helper, "serve", "--port", "0", ...args], {
      cwd: root,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    }),
  );
  const lines = createInterface({ input: server.stdout });
  const [url] = await once(lines, "line", { signal: t.signal });
  lines.close();
  page = await browser.newPage();
  await page.goto(url);
  await page.locator("#workspace").waitFor({ state: "visible" });
  const inspect = () =>
    JSON.parse(
      execFileSync(python, [helper, "inspect", ...args], { encoding: "utf8" }),
    );
  return { bundle, args, page, url, inspect, files, server, own };
}

test(
  "the real web page sends an explanation to a waiting agent and displays its feedback",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, args, bundle, inspect, own } = await start(t);
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
    const waiting = own(
      spawn(python, [
        helper,
        "wait",
        ...args,
        "--after",
        String(saved.revision),
        "--timeout",
        "10",
      ]),
    );
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
      "Sent and saved, but no reviewer is listening, so nothing will appear here yet. Tell your agent in the terminal to read the submission.",
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
    assert.equal(await page.locator("#send").textContent(), "Retry");
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

const SENT_ANSWER =
  "The current workflow loses the submission handoff, which is why the previous record has to stay readable after a failed write.";

test(
  "repeated Send clicks while the answer check is pending submit only once",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t);
    let releaseCheck;
    const checksHeld = new Promise((resolve) => (releaseCheck = resolve));
    let checkStarted;
    const checking = new Promise((resolve) => (checkStarted = resolve));
    let requests = 0;
    await page.route("**/api/check", async (route) => {
      checkStarted();
      await checksHeld;
      await route.continue();
    });
    await page.route("**/api/review", async (route) => {
      requests++;
      await route.continue();
    });
    await page.locator('textarea[name="motivation"]').fill(SENT_ANSWER);
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    await page.locator("#send").click();
    await checking;
    await page.locator("#send").evaluate((button) => button.click());
    releaseCheck();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Sent, waiting" }).waitFor();
    assert.equal(requests, 1);
    assert.equal(inspect().status, "awaiting_review");
    assert.equal(await page.locator("#error").isVisible(), false);
  },
);

test(
  "refresh cannot race a send while its answer check is pending",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t);
    let releaseCheck;
    const checksHeld = new Promise((resolve) => (releaseCheck = resolve));
    let checkStarted;
    const checking = new Promise((resolve) => (checkStarted = resolve));
    await page.route("**/api/check", async (route) => {
      checkStarted();
      await checksHeld;
      await route.continue();
    });
    await page.locator('textarea[name="motivation"]').fill(SENT_ANSWER);
    await page.locator("#save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    await page.evaluate(() => {
      window.__sessionReads = 0;
      const fetch = window.fetch;
      window.fetch = (...args) => {
        if (args[0] === "/api/session") window.__sessionReads++;
        return fetch(...args);
      };
    });
    await page.locator("#send").click();
    await checking;
    const reads = await page.evaluate(() => {
      document.getElementById("refresh").click();
      return window.__sessionReads;
    });
    releaseCheck();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    assert.equal(reads, 0, "refresh must wait for the send to finish");
    assert.equal(inspect().status, "awaiting_review");
  },
);

test(
  "saving and reflecting cannot race a pending send check",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t, [], true);
    await page.locator('textarea[name="motivation"]').fill(SENT_ANSWER);
    await page.locator("#send").click();
    await page.locator("#reflection-form").waitFor({ state: "visible" });
    await page.reload();
    await page.locator("#reflection-form").waitFor({ state: "visible" });
    await page.locator("#self-grade").selectOption("got");
    let releaseCheck;
    const checksHeld = new Promise((resolve) => (releaseCheck = resolve));
    let checkStarted;
    const checking = new Promise((resolve) => (checkStarted = resolve));
    await page.route("**/api/check", async (route) => {
      checkStarted();
      await checksHeld;
      await route.continue();
    });
    await page.evaluate(() => {
      window.__otherWrites = 0;
      const fetch = window.fetch;
      window.fetch = (...args) => {
        if (["/api/answers", "/api/reflection"].includes(args[0]))
          window.__otherWrites++;
        return fetch(...args);
      };
    });
    await page.locator("#send").click();
    await checking;
    const writes = await page.evaluate(() => {
      document.getElementById("reflection-form").requestSubmit();
      document.getElementById("answers-form").requestSubmit();
      return window.__otherWrites;
    });
    releaseCheck();
    await page.getByText(/already have feedback/).waitFor();
    assert.equal(writes, 0, "other writes must wait for the send to finish");
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(inspect().status, "explanation_complete");
  },
);

test(
  "a send nobody is listening for says so, keeps Send usable and offers the resume line",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page } = await start(t);
    await page.locator('textarea[name="motivation"]').fill(SENT_ANSWER);
    await page.locator("#send").click();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    const handoff = page.locator("#handoff");
    assert.equal(await handoff.getAttribute("data-tone"), "away");
    assert.equal(await page.locator("#resume-copy").isVisible(), true);
    assert.equal(await page.locator("#send").textContent(), "Sent, waiting");
    assert.notEqual(
      await page.locator("#send").getAttribute("aria-disabled"),
      "true",
    );
    // The banner is not a second live region: one status region carries it.
    assert.equal(await page.locator("#handoff[role]").count(), 0);
    // An edit while it waits makes the button a plain Send again.
    await page
      .locator('textarea[name="motivation"]')
      .pressSequentially(" More.");
    assert.equal(await page.locator("#send").textContent(), "Send to reviewer");
    assert.equal(await handoff.getAttribute("data-tone"), "idle");
  },
);

test(
  "a reviewer that signalled and then went quiet is not called absent",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, bundle, inspect } = await start(t);
    await page.locator('textarea[name="motivation"]').fill(SENT_ANSWER);
    const signal = (state, seconds) =>
      writeFileSync(
        join(bundle, "explanation-agent.json"),
        JSON.stringify({
          identity: inspect().identity,
          state,
          until: Date.now() / 1000 + seconds,
        }),
      );
    await page.locator("#send").click();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    signal("reading", 600);
    await page.locator('#handoff[data-tone="working"]').waitFor();
    assert.match(
      await page.locator("#waiting-time").textContent(),
      /on this page$/,
    );
    // The signal is past its time: the server calls it lapsed, whatever this
    // page has or has not watched.
    signal("reading", -5);
    await page.locator('#handoff[data-tone="away"]').waitFor();
    assert.match(
      await page.locator("#reviewer-presence").textContent(),
      /No reviewer signal for 2 minutes/,
    );
    assert.match(
      await page.locator("#feedback-status").textContent(),
      /stopped signalling/,
    );
    assert.equal(await page.locator("#resume-copy").isVisible(), false);
  },
);

test(
  "a refused send is said beside the answer, with focus on it, and nothing is sent",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, inspect } = await start(t);
    const box = page.locator('textarea[name="motivation"]');
    await box.fill("Too short to say anything");
    await page.locator('#answer-coach[data-state="short"]').waitFor();
    await page.locator("#send").click();
    await page.locator('#answer-coach[data-refused="true"]').waitFor();
    // The message is said again a frame later, so wait for the words.
    await page.waitForFunction(() =>
      /at least 12 words/.test(
        document.getElementById("coach-message").textContent,
      ),
    );
    assert.equal(await page.locator("#error").isVisible(), false);
    assert.equal(
      await box.evaluate((node) => node === document.activeElement),
      true,
    );
    assert.notEqual(inspect().status, "awaiting_review");
    // Typing on clears the ring, and an answer that can be sent is marked so.
    await box.fill(SENT_ANSWER);
    await page
      .locator(
        '#answer-coach[data-state="good"],#answer-coach[data-state="ungrounded"]',
      )
      .waitFor();
    assert.equal(
      await page.locator("#answer-coach").getAttribute("data-refused"),
      null,
    );
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
      await reference.addStyleTag({ content: read("web/theme.css") });
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
    for (const name of ["theme.css", "styles.css"]) {
      const css = await page.request.get(new URL(`/${name}`, page.url()).href);
      assert.equal(await css.text(), read(`web/${name}`));
    }
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
    assert.match(notice, /signs this review and sends its/);
    assert.doesNotMatch(notice, /explain one passage/);
    assert.doesNotMatch(notice, /label/);
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
        "Replace the existing behavior so earlier evidence stays available when writing fails, which is why the previous record has to stay readable after a failed write.",
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
      "Replace the existing behavior so earlier evidence stays available when writing fails, which is why the previous record has to stay readable after a failed write.",
    );
    await page.locator('[data-path="tests/example.rs"]').click();
    assert.equal(
      await page.locator('[name="reading:regression"]').inputValue(),
      "A failed write must preserve the existing evidence, which is why the previous record has to stay readable after a failed write.",
    );
    await page.locator("#all-diff").click();
    assert.equal(
      await page.locator('[name="motivation"]').inputValue(),
      "Replace the existing behavior so earlier evidence stays available when writing fails, which is why the previous record has to stay readable after a failed write.",
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
    const shown = await page.locator("#diff").textContent();
    assert.match(shown, /src\/example\.rs/);
    assert.match(shown, /renamed from src\/old\.rs/);
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
      /signs this review and sends its/,
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
    // The intent record is edited beside the notice, which it leaves showing:
    // finishing must not close the server on what was typed there either.
    await box.fill(text);
    await page.locator("#save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    await openIntent(page);
    await page.locator("#intent-goal").fill("Keep the why next to the change.");
    await page.locator("#close-window").click();
    await page
      .getByText("Save your latest edit before finishing", { exact: false })
      .waitFor();
    assert.equal(await page.locator("#finished").isVisible(), false);
    assert.equal(server.exitCode, null);
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

const modified = (path, lines, extra = "") =>
  `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n${extra}${lines}`;

test(
  "the diff shows line numbers and marks the words that changed",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const patch = modified(
      "src/example.rs",
      "@@ -10,3 +10,3 @@ fn total()\n keep\n-let total = max(a, b);\n+let total = max(a, b, c);\n tail\n",
    );
    const { page } = await start(t, [], false, patch, [
      { path: "src/example.rs", additions: 1, deletions: 1 },
    ]);
    await page.locator("#diff .row.k-a").waitFor();
    assert.equal(
      await page.locator("#diff mark.word.add").first().textContent(),
      ", c",
    );
    assert.equal(
      await page.locator("#diff .row.k-d .num").first().textContent(),
      "11",
    );
    assert.equal(
      await page.locator("#diff .row.k-a .num").nth(1).textContent(),
      "11",
    );
    assert.match(
      await page.locator("#diff .hunk").textContent(),
      /@@ -10,3 \+10,3 @@fn total\(\)/,
    );
    assert.match(
      await page.locator("#diff-summary").textContent(),
      /\+1 \u22121/,
    );
  },
);

test(
  "lines moved between files are neutral, marked in text, and say where they went",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const func = [
      "def compute(total, counted):",
      "    result = max(total, counted)",
      "    return result + len(str(total))",
      "    # keep the larger reading",
    ];
    const patch =
      modified(
        "a.py",
        `@@ -1,5 +1,1 @@\n head\n${func.map((x) => `-${x}\n`).join("")}`,
      ) +
      modified(
        "b.py",
        `@@ -1,1 +1,5 @@\n head\n${func.map((x) => `+${x}\n`).join("")}`,
      );
    const { page } = await start(t, [], false, patch, [
      { path: "a.py", additions: 0, deletions: 4 },
      { path: "b.py", additions: 4, deletions: 0 },
    ]);
    await page.locator("#diff .moved").first().waitFor();
    // One file is shown at first: the four lines that left a.py.
    assert.equal(await page.locator("#diff .moved").count(), 4);
    await page.locator("#all-diff").click();
    await page.waitForFunction(
      () => document.querySelectorAll("#diff .moved").length === 8,
    );
    assert.equal(await page.locator("#diff .moved").count(), 8);
    assert.equal(
      await page.locator("#diff .moved .sign").first().textContent(),
      "\u00bb",
    );
    assert.match(
      await page.locator("#diff").textContent(),
      /4 lines moved from a\.py:2/,
    );
    assert.match(
      await page.locator("#diff").textContent(),
      /4 lines moved to b\.py:2/,
    );
    assert.match(
      await page.locator("#diff-summary").textContent(),
      /8 lines moved/,
    );
  },
);

test(
  "side by side pairs removed and added lines, and whitespace-only edits can be hidden",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const patch = modified(
      "src/example.rs",
      "@@ -1,3 +1,3 @@\n keep\n-    x = 1\n+        x = 1\n-old_word here\n+new_word here\n",
    );
    const { page } = await start(t, [], false, patch, [
      { path: "src/example.rs", additions: 2, deletions: 2 },
    ]);
    await page.locator("#diff .row").first().waitFor();
    assert.equal(await page.locator("#diff .row.k-d").count(), 2);
    await page.locator("#diff-ws").check();
    assert.equal(await page.locator("#diff .row.k-d").count(), 1);
    assert.equal((await page.locator("#diff .row.k-c").count()) >= 2, true);
    await page.locator("#diff-ws").uncheck();
    await page.locator("#diff-mode").click();
    assert.equal(
      await page.locator("#diff-mode").getAttribute("aria-pressed"),
      "true",
    );
    assert.equal(await page.locator("#diff .row.split").count(), 2);
    assert.match(
      await page.locator("#diff .row.split").first().textContent(),
      /x = 1.*x = 1/,
    );
    await page.reload();
    await page.locator("#diff .row.split").first().waitFor();
  },
);

test(
  "the next and previous buttons walk the changes",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const patch = modified(
      "src/example.rs",
      "@@ -1,9 +1,9 @@\n-a1\n+b1\n k\n k\n k\n-a2\n+b2\n k\n k\n k\n-a3\n+b3\n",
    );
    const { page } = await start(t, [], false, patch, [
      { path: "src/example.rs", additions: 3, deletions: 3 },
    ]);
    await page.locator("#diff .row").first().waitFor();
    assert.match(
      await page.locator("#diff-position").textContent(),
      /of 3 changes/,
    );
    await page.locator("#diff-next").click();
    assert.match(await page.locator("#diff-position").textContent(), /^1 of 3/);
    await page.locator("#diff-next").click();
    await page.locator("#diff-next").click();
    await page.locator("#diff-next").click();
    assert.match(await page.locator("#diff-position").textContent(), /^3 of 3/);
    await page.locator("#diff-prev").click();
    assert.match(await page.locator("#diff-position").textContent(), /^2 of 3/);
  },
);

test(
  "a large diff is drawn completely, in pieces, without dropping a line",
  { timeout: 60000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const count = 6000;
    const body = Array.from(
      { length: count },
      (_, i) => `+line number ${i}\n`,
    ).join("");
    const patch = `diff --git a/big.txt b/big.txt\nnew file mode 100644\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,${count} @@\n${body}`;
    const { page } = await start(t, [], false, patch, [
      { path: "big.txt", additions: count, deletions: 0 },
    ]);
    await page.waitForFunction(
      (n) => document.querySelectorAll("#diff .row.k-a").length === n,
      count,
      { timeout: 30000 },
    );
    assert.match(
      await page.locator("#diff .row.k-a").last().textContent(),
      new RegExp(`line number ${count - 1}`),
    );
    assert.equal(await page.locator("#diff-notice").isVisible(), false);
  },
);

test(
  "a diff the page cannot account for line by line is shown as written, with a warning",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    // The header promises 5 lines each side; the body holds 2.
    const patch = modified("src/example.rs", "@@ -1,5 +1,5 @@\n-old\n+new\n");
    const { page } = await start(t, [], false, patch, [
      { path: "src/example.rs", additions: 1, deletions: 1 },
    ]);
    await page.locator("#diff-notice").waitFor({ state: "visible" });
    assert.match(
      await page.locator("#diff-notice").textContent(),
      /could not account for every line/,
    );
    assert.equal((await page.locator("#diff .row.raw").count()) > 0, true);
    assert.match(await page.locator("#diff").textContent(), /@@ -1,5 \+1,5 @@/);
  },
);

test(
  "a function moved with new names and one edited line says so and keeps the edit visible",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const before = [
      "def compute(total, counted):",
      "    result = max(total, counted)",
      "    if result > limit(total):",
      "        result = limit(total)",
      "    return result + len(str(total))",
    ];
    const after = before.map((x) => x.replaceAll("total", "amount"));
    after[1] = "    result = min(amount, counted)";
    const patch =
      modified(
        "a.py",
        `@@ -1,6 +1,1 @@\n head\n${before.map((x) => `-${x}\n`).join("")}`,
      ) +
      modified(
        "b.py",
        `@@ -1,1 +1,6 @@\n head\n${after.map((x) => `+${x}\n`).join("")}`,
      );
    const { page } = await start(t, [], false, patch, [
      { path: "a.py", additions: 0, deletions: 5 },
      { path: "b.py", additions: 5, deletions: 0 },
    ]);
    await page.locator("#all-diff").click();
    await page.locator("#diff .moved-note").nth(1).waitFor();
    const note = await page.locator("#diff .moved-note").nth(1).textContent();
    assert.match(note, /5 lines moved from a\.py:2/);
    assert.match(note, /names changed: .*total \u2192 amount/);
    // The lines that changed are still edits, not hidden in the move.
    assert.equal(
      (await page.locator("#diff .row.k-a:not(.moved)").count()) >= 1,
      true,
    );
    assert.equal(
      await page.locator("#diff .row.k-a mark.word").first().isVisible(),
      true,
    );
  },
);

test(
  "a diff that fails to load can be retried and then opens the workspace",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { url } = await start(t);
    const page = await browser.newPage();
    t.after(() => page.close());
    let failures = 1;
    await page.route("**/api/diff", (route) =>
      failures-- > 0 ? route.abort() : route.continue(),
    );
    await page.goto(url);
    await page.locator("#retry").waitFor({ state: "visible" });
    assert.equal(await page.locator("#workspace").isVisible(), false);
    await page.locator("#retry").click();
    await page.locator("#workspace").waitFor({ state: "visible" });
    await page.locator("#diff .row").first().waitFor();
  },
);

test(
  "every line of a multi-line quote is marked as the passage asked about",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const patch = modified(
      "src/example.rs",
      "@@ -1,3 +1,3 @@\n keep\n-old_one\n+new_one\n-old_two\n+new_two\n",
    );
    const { page } = await start(
      t,
      [
        {
          id: "multi",
          path: "src/example.rs",
          prompt: "Why were these two lines changed together?",
          quote: "+new_one\n-old_two",
        },
      ],
      false,
      patch,
      [{ path: "src/example.rs", additions: 2, deletions: 2 }],
    );
    await page.locator("#diff .row.asked").first().waitFor();
    const marked = await page
      .locator("#diff .row.asked .code")
      .allTextContents();
    assert.deepEqual(
      marked.map((x) => x.trim()),
      ["new_one", "old_two"],
    );
  },
);

const CACHE_DIFF = `diff --git a/src/cache.rs b/src/cache.rs
--- a/src/cache.rs
+++ b/src/cache.rs
@@ -10,5 +10,6 @@ fn get
 fn get(key: &str) -> Option<Value> {
-    db.load(key)
+    let hit = cache.lookup(key);
+    hit.or_else(|| db.load(key))
 }
${" "}
 fn put() {}
`;

test(
  "a reference note with a diagram, table and quoted code is drawn from the pinned diff",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page, args, bundle, inspect } = await start(
      t,
      [],
      false,
      CACHE_DIFF,
      [{ path: "src/cache.rs", additions: 2, deletions: 1 }],
    );
    await page
      .locator('textarea[name="motivation"]')
      .fill(
        "Every get reached the database, so the cache lookup has to come first and only a miss may fall through to it.",
      );
    await page.locator("#send").click();
    await page.getByText("Sent to your review task", { exact: true }).waitFor();
    const sent = inspect();
    const note = [
      "The lookup now precedes the database, see @src/cache.rs:11.",
      "",
      "```flow LR",
      "get -> cache: looked up first @src/cache.rs:11",
      "cache --> <img src=x onerror=alert(1)>: on a miss",
      "```",
      "",
      "| Option | Cost |",
      "| --- | --- |",
      "| Read through | one lookup @src/cache.rs:12 |",
      "",
      "```code src=src/cache.rs lines=11-12",
      "```",
    ].join("\n");
    const feedback = join(bundle, "feedback.json");
    writeFileSync(
      feedback,
      JSON.stringify({
        identity: sent.identity,
        answer_digest: sent.answer_digest,
        notes: [{ field: "motivation", text: note }],
      }),
    );
    execFileSync(python, [
      helper,
      "feedback",
      ...args,
      "--request-id",
      sent.request_id,
      "--file",
      feedback,
    ]);
    await page.locator(".ex-figure svg").waitFor();
    // The picture has a text equivalent, and the guess is named as one.
    assert.match(
      (await page.locator(".ex-figure svg").getAttribute("aria-label")) ?? "",
      /get to cache/,
    );
    assert.equal(await page.locator(".ex-edges li").count(), 2);
    assert.equal(await page.locator(".ex-edge.ex-inferred").count(), 1);
    assert.equal(await page.locator(".ex-edge:not(.ex-inferred)").count(), 1);
    assert.match(
      (await page.locator(".ex-edges li .ex-tag").textContent()) ?? "",
      /inferred/,
    );
    // A citation in prose is a button too, not the raw "@path:N" text.
    assert.equal(await page.locator(".ex-note-body > p .ex-cite").count(), 1);
    assert.equal(await page.getByText("@src/cache.rs").count(), 0);
    assert.equal(await page.locator(".ex-table tbody tr").count(), 1);
    assert.equal(await page.locator(".ex-code .ex-row.k-a").count(), 2);
    // A note is data: its markup is shown, never parsed.
    assert.equal(await page.locator("#feedback-list img").count(), 0);
    await page.getByText("<img src=x onerror=alert(1)>").first().waitFor();
    // A citation opens its line in the diff.
    await page.locator("#diff .file-toggle").click();
    await page.locator(".ex-edges .ex-cite").first().click();
    await page.locator("#diff .cited").first().waitFor();
    assert.equal(await page.locator("#diff .file-body").isVisible(), true);
    assert.match(
      (await page.locator("#diff .cited").first().textContent()) ?? "",
      /cache\.lookup/,
    );
  },
);

test("citation waits for all chunks and the latest request wins", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t);
  const result = await page.evaluate(async () => {
    const { DiffView } = await import("/diffview.js");
    const frames = [];
    const original = window.requestAnimationFrame;
    window.requestAnimationFrame = (callback) => frames.push(callback);
    try {
      const root = document.createElement("div");
      document.body.append(root);
      const view = new DiffView({
        root,
        notice: document.createElement("div"),
        summary: document.createElement("div"),
        position: document.createElement("div"),
      });
      const lines = Array.from({ length: 500 }, (_, i) => ({
        k: "a",
        n: i + 1,
        t: `line ${i + 1}`,
      }));
      view.setModel({
        problems: [],
        files: [
          {
            path: "a",
            status: "modified",
            additions: 500,
            deletions: 0,
            hunks: [
              {
                old_start: 1,
                old_count: 0,
                new_start: 1,
                new_count: 500,
                lines,
              },
            ],
          },
        ],
      });
      view.show({ path: "a" });
      view.reveal({ side: "new", start: 2, end: 3 });
      view.reveal({ side: "new", start: 398, end: 402 });
      while (frames.length) frames.shift()();
      return [...root.querySelectorAll(".cited")].map((row) =>
        Number(row.dataset.n),
      );
    } finally {
      window.requestAnimationFrame = original;
    }
  });
  assert.deepEqual(result, [398, 399, 400, 401, 402]);
});

test("a poll that changes nothing leaves a focused citation where it was", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page, args, bundle, inspect } = await start(
    t,
    [],
    false,
    CACHE_DIFF,
    [{ path: "src/cache.rs", additions: 2, deletions: 1 }],
  );
  await page
    .locator('textarea[name="motivation"]')
    .fill(
      "The lookup has to come first, and only a miss may fall through to the database.",
    );
  await page.locator("#send").click();
  await page.getByText("Sent to your review task", { exact: true }).waitFor();
  const sent = inspect();
  const feedback = join(bundle, "feedback.json");
  writeFileSync(
    feedback,
    JSON.stringify({
      identity: sent.identity,
      answer_digest: sent.answer_digest,
      notes: [
        {
          field: "motivation",
          text: "The lookup comes first, see @src/cache.rs:11.",
        },
      ],
    }),
  );
  execFileSync(python, [
    helper,
    "feedback",
    ...args,
    "--request-id",
    sent.request_id,
    "--file",
    feedback,
  ]);
  const chip = page.locator(".ex-note-body .ex-cite");
  await chip.waitFor();
  await chip.focus();
  await page.evaluate(() => {
    window.__chip = document.activeElement;
  });
  // The page polls every five seconds; wait for one that carries no new feedback.
  // Wait for an actual poll, and for the page to have handled it.
  await afterPoll(page);
  const kept = await page.evaluate(
    () => document.activeElement === window.__chip && window.__chip.isConnected,
  );
  assert.equal(kept, true, "the citation button was rebuilt and lost focus");
});

// Starts a review, answers it, and has the reviewer reply with `note`.
async function referenced(t, patch, files, note) {
  const ctx = await start(t, [], false, patch, files);
  await ctx.page
    .locator('textarea[name="motivation"]')
    .fill(
      "The lookup has to come first, and only a miss may fall through to the database.",
    );
  await ctx.page.locator("#send").click();
  await ctx.page
    .getByText("Sent to your review task", { exact: true })
    .waitFor();
  const sent = ctx.inspect();
  const feedback = join(ctx.bundle, "feedback.json");
  writeFileSync(
    feedback,
    JSON.stringify({
      identity: sent.identity,
      answer_digest: sent.answer_digest,
      notes: [{ field: "motivation", text: note }],
    }),
  );
  execFileSync(python, [
    helper,
    "feedback",
    ...ctx.args,
    "--request-id",
    sent.request_id,
    "--file",
    feedback,
  ]);
  return ctx;
}
// The page polls every five seconds: wait for one, and for the page to have handled it.
// The intent record is a disclosure: open it the way a reader does.
async function openIntent(page) {
  const details = page.locator("#intent-details");
  await details.waitFor({ state: "attached" });
  if (!(await details.evaluate((node) => node.open)))
    await page.locator("#intent-details > summary").click();
  await page.locator("#intent-form").waitFor();
}

async function afterPoll(page) {
  const response = await page.waitForResponse(
    (r) => r.request().method() === "GET" && r.url().endsWith("/api/session"),
    { timeout: 15000 },
  );
  await response.finished();
  // The page renders what it received before its next two frames.
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
}
const CACHE_FILES = [{ path: "src/cache.rs", additions: 2, deletions: 1 }];

test("a label of the widest letters still fits inside its picture", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const wide = "W".repeat(48);
  const note = ["LR", "TB"]
    .flatMap((dir) => [
      "```flow " + dir,
      `a -> b: ${wide} @src/cache.rs:11`,
      "```",
      "",
    ])
    .join("\n");
  const { page } = await referenced(t, CACHE_DIFF, CACHE_FILES, note);
  await page.locator(".ex-figure svg").first().waitFor();
  const out = await page.evaluate(() =>
    [...document.querySelectorAll(".ex-figure svg")].map((svg) => {
      const [x, y, w, h] = svg.getAttribute("viewBox").split(" ").map(Number);
      return [...svg.querySelectorAll("text")]
        .map((text) => text.getBBox())
        .filter(
          (b) =>
            b.x < x - 0.5 ||
            b.y < y - 0.5 ||
            b.x + b.width > x + w + 0.5 ||
            b.y + b.height > y + h + 0.5,
        ).length;
    }),
  );
  assert.deepEqual(
    out,
    [0, 0],
    "text drawn outside the viewBox, so it would be clipped",
  );
});

test("a citation jump moves focus to the line and says where it went", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await referenced(
    t,
    CACHE_DIFF,
    CACHE_FILES,
    "The lookup comes first, see @src/cache.rs:11-12.",
  );
  await page.locator(".ex-note-body .ex-cite").click();
  await page.locator("#diff .cited").first().waitFor();
  const state = await page.evaluate(() => ({
    focused: document.activeElement?.classList.contains("cited"),
    status: document.getElementById("diff-notice").textContent,
    live: document.getElementById("diff-notice").getAttribute("role"),
  }));
  assert.equal(state.focused, true, "focus stayed on the citation");
  assert.match(state.status, /Showing src\/cache\.rs, lines 11\u201312\./);
  assert.equal(state.live, "status");
});

test("a citation to a line the whitespace filter hides turns the filter off", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const patch = `diff --git a/src/ws.rs b/src/ws.rs
--- a/src/ws.rs
+++ b/src/ws.rs
@@ -1,3 +1,3 @@
 fn f() {
-    return 1;
+        return 1;
 }
`;
  const { page } = await referenced(
    t,
    patch,
    [{ path: "src/ws.rs", additions: 1, deletions: 1 }],
    "It only re-indents @src/ws.rs:o2.",
  );
  await page.locator("#diff-ws").check();
  await page.locator(".ex-note-body .ex-cite").click();
  await page.locator("#diff .cited").first().waitFor();
  assert.equal(await page.locator("#diff-ws").isChecked(), false);
  assert.match(
    (await page.locator("#diff .cited").first().textContent()) ?? "",
    /return 1/,
  );
});

test("a citation finds its own file when two files share a line number", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t);
  const marked = await page.evaluate(async () => {
    const { DiffView } = await import("/diffview.js");
    const root = document.createElement("div");
    document.body.append(root);
    const view = new DiffView({
      root,
      notice: document.createElement("div"),
      summary: document.createElement("div"),
      position: document.createElement("div"),
    });
    const file = (path) => ({
      path,
      status: "modified",
      additions: 1,
      deletions: 0,
      hunks: [
        {
          old_start: 1,
          old_count: 0,
          new_start: 1,
          new_count: 2,
          lines: [
            { k: "c", n: 1, o: 1, t: "one" },
            { k: "a", n: 2, o: null, t: `${path} two` },
          ],
        },
      ],
    });
    view.setModel({ problems: [], files: [file("a.rs"), file("b.rs")] });
    view.show({});
    view.reveal({ path: "b.rs", side: "new", start: 2, end: 2 });
    return [...root.querySelectorAll(".cited")].map((row) => row.textContent);
  });
  assert.equal(marked.length, 1);
  assert.match(marked[0], /b\.rs two/);
});

const GUARD_DIFF = `diff --git a/src/cache.rs b/src/cache.rs
--- a/src/cache.rs
+++ b/src/cache.rs
@@ -1,6 +1,3 @@
 fn get(key: &str) -> Option<Value> {
-    if key.is_empty() {
-        return None;
-    }
     db.load(key)
 }
`;
const GUARD_FILES = [{ path: "src/cache.rs", additions: 0, deletions: 3 }];

test("the change is shown as three debts, and a removed check opens at its old line", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t, [], false, GUARD_DIFF, GUARD_FILES);
  await page.locator(".debt-card").first().waitFor();
  assert.deepEqual(await page.locator(".debt-card h3").allTextContents(), [
    "Code",
    "People",
    "Intent",
  ]);
  const code = page.locator(".debt-card").nth(0);
  assert.match(
    (await code.textContent()) ?? "",
    /2 removed lines look like a check or an early exit/,
  );
  assert.match(
    (await page.locator(".debt-card").nth(2).textContent()) ?? "",
    /No changed test file was recognized/,
  );
  // Observations and questions, never a score: look at the cards, not the heading
  // that says so.
  assert.doesNotMatch(
    (await page.locator("#debt-layers").textContent()) ?? "",
    /score|rating|\d+\s*\/\s*10/i,
  );
  await code.locator(".ex-cite").first().click();
  await page.locator("#diff .cited").first().waitFor();
  assert.match(
    (await page.locator("#diff .cited").first().textContent()) ?? "",
    /key\.is_empty\(\)/,
  );
  assert.match(
    (await page.locator("#diff-notice").textContent()) ?? "",
    /Showing src\/cache\.rs, old line 2\./,
  );
});

test("the intent record is the contributor's own, survives a reload and copies as text", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page, inspect } = await start(t, [], false, GUARD_DIFF, GUARD_FILES);
  await openIntent(page);
  const before = inspect();
  await page.evaluate(() => {
    window.__copied = null;
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (text) => (window.__copied = text) },
      configurable: true,
    });
  });
  assert.equal(
    await page.locator("#intent-copy").isDisabled(),
    true,
    "nothing written yet",
  );
  await page
    .locator("#intent-goal")
    .fill("A miss must still reach the database.");
  await page
    .locator("#intent-check")
    .fill("tests/cache_test.rs::miss_reaches_db");
  // A poll must not overwrite what is being typed: wait for one, and for the page to handle it.
  await afterPoll(page);
  assert.equal(
    await page.locator("#intent-goal").inputValue(),
    "A miss must still reach the database.",
  );
  await page.locator("#intent-save").click();
  await page.getByText("Saved on this computer", { exact: true }).waitFor();
  await page.reload();
  // A record already started opens by itself, so it is not hidden on return.
  await page.locator("#intent-goal").waitFor();
  assert.equal(
    await page.locator("#intent-details").evaluate((node) => node.open),
    true,
  );
  await page.waitForFunction(
    () => document.getElementById("intent-goal").value !== "",
  );
  assert.equal(
    await page.locator("#intent-goal").inputValue(),
    "A miss must still reach the database.",
  );
  assert.equal(
    await page.locator("#intent-check").inputValue(),
    "tests/cache_test.rs::miss_reaches_db",
  );
  await page.evaluate(() => {
    window.__copied = null;
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (text) => (window.__copied = text) },
      configurable: true,
    });
  });
  await page.locator("#intent-copy").click();
  const copied = await page.evaluate(() => window.__copied);
  assert.equal(
    copied,
    "### Why this change exists\n\n- **Goal:** A miss must still reach the database.\n- **Checked by:** tests/cache_test.rs::miss_reaches_db\n",
  );
  // It is not part of the answers, so nothing the gate reads has moved.
  const after = inspect();
  for (const key of ["answer_digest", "identity", "revision", "status"])
    assert.equal(after[key], before[key], key);
});

test("typing during a save is kept, not overwritten by the save or the next poll", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t, [], false, GUARD_DIFF, GUARD_FILES);
  await openIntent(page);
  // The save is held in flight until the test has typed, then released.
  let release;
  const held = new Promise((resolve) => (release = resolve));
  await page.route("**/api/intent", async (route) => {
    await held;
    await route.continue();
  });
  await page.locator("#intent-goal").fill("First draft.");
  await page.locator("#intent-save").click();
  await page.locator("#intent-goal").pressSequentially(" More.");
  await page.waitForFunction(
    () =>
      document.getElementById("intent-status").textContent ===
      "Unsaved changes",
    null,
    { timeout: 5000 },
  );
  release();
  await page.waitForResponse((r) => r.url().endsWith("/api/intent"));
  await afterPoll(page);
  assert.equal(
    await page.locator("#intent-goal").inputValue(),
    "First draft. More.",
  );
  assert.equal(
    await page.locator("#intent-status").textContent(),
    "Unsaved changes",
  );
});

test("the intent record copies every answer under its label, in one line each", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t);
  const [full, blank] = await page.evaluate(async () => {
    const { intentMarkdown } = await import("/debtlens.js");
    return [
      intentMarkdown({
        goal: "A\n   B",
        check: "E",
      }),
      intentMarkdown({ goal: "  " }),
    ];
  });
  assert.equal(
    full,
    "### Why this change exists\n\n- **Goal:** A B\n- **Checked by:** E\n",
  );
  assert.equal(blank, "", "nothing written, nothing to copy");
});

test("the copy button says why it is off, and a citation jump is announced", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await referenced(
    t,
    CACHE_DIFF,
    CACHE_FILES,
    "The lookup comes first, see @src/cache.rs:11.",
  );
  await openIntent(page);
  assert.equal(await page.locator("#intent-copy").isDisabled(), true);
  assert.equal(
    await page.locator("#intent-status").textContent(),
    "Write an answer to enable copying.",
  );
  await page.locator("#intent-goal").fill("x");
  assert.equal(await page.locator("#intent-copy").isDisabled(), false);
  await page.locator(".ex-note-body .ex-cite").click();
  await page.waitForFunction(
    () =>
      /Showing src\/cache\.rs, line 11\./.test(
        document.getElementById("diff-live").textContent,
      ),
    null,
    { timeout: 5000 },
  );
});

test("unsaved intent text asks before the page is left, and saving clears it", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const { page } = await start(t, [], false, GUARD_DIFF, GUARD_FILES);
  await openIntent(page);
  const prevented = () =>
    page.evaluate(() => {
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    });
  assert.equal(await prevented(), false, "nothing typed, nothing to lose");
  await page.locator("#intent-goal").fill("Written, not saved.");
  assert.equal(await prevented(), true, "typed text would be lost");
  await page.locator("#intent-save").click();
  await page.getByText("Saved on this computer", { exact: true }).waitFor();
  assert.equal(await prevented(), false);
});

test(
  "the rail follows the review, and the reference sits beside the explanation it answers",
  { timeout: 30000 },
  async (t) => {
    if (!browser) return t.skip("playwright chromium unavailable");
    const { page } = await start(t, [], true);
    await page.setViewportSize({ width: 1400, height: 900 });
    const step = (name) =>
      page.locator(`#review-rail button[data-step="${name}"]`);
    const current = () =>
      page
        .locator('#review-rail button[aria-current="step"]')
        .getAttribute("data-step");
    assert.equal(await current(), "read");
    const box = page.locator('[name="motivation"]');
    const text =
      "Keep earlier evidence available when a write fails, which is why the previous record has to stay readable after a failed write.";
    await box.fill(text);
    assert.equal(await current(), "explain");
    assert.match(await step("read").textContent(), /\(done\)/);
    // Ctrl with Enter sends from the answer; plain Enter would only add a line.
    await box.press("Control+Enter");
    await page.locator("#feedback-list li").first().waitFor();
    await page.waitForFunction(
      () =>
        document.querySelector('#review-rail [aria-current="step"]')?.dataset
          .step === "compare",
    );
    // The reply is said once and marked on the rail; focus stays where it was.
    assert.match(
      await page.locator("#rail-live").textContent(),
      /Reviewer feedback arrived/,
    );
    assert.equal(
      await step("compare").evaluate((b) => b.classList.contains("rail-new")),
      true,
    );
    // Beside the reference: the explanation exactly as the reviewer received it.
    const yours = page.locator("#your-explanation");
    await yours.waitFor({ state: "visible" });
    assert.equal(await page.locator("#yours-text").textContent(), text);
    const ref = await page.locator("#feedback-list").boundingBox();
    const aside = await yours.boundingBox();
    assert.ok(
      aside.x >= ref.x + ref.width,
      "beside, not below, on a wide screen",
    );
    // A step is a heading to land on, and visiting Compare clears the mark.
    await step("compare").click();
    assert.equal(
      await page.evaluate(() => document.activeElement.id),
      "feedback-heading",
    );
    assert.equal(
      await step("compare").evaluate((b) => b.classList.contains("rail-new")),
      false,
    );
    await page.locator("#self-grade").selectOption("got");
    await page.locator("#reflect-save").click();
    await page.locator("#completion-notice").waitFor({ state: "visible" });
    assert.equal(await current(), "keep");
    // The intent record is closed until wanted; its summary says how far it is.
    assert.equal(
      await page.locator("#intent-summary-state").textContent(),
      "Optional · not started",
    );
    await openIntent(page);
    // A record that answers only one question is still a record.
    await page.locator("#intent-check").fill("make test");
    await page.locator("#intent-save").click();
    await page.getByText("Saved on this computer", { exact: true }).waitFor();
    assert.equal(await step("keep").getAttribute("data-state"), "done");
    await page.locator("#intent-goal").fill("Keep the why next to the change.");
    assert.match(
      await page.locator("#intent-summary-state").textContent(),
      /^2 of 2 answered · unsaved changes$/,
    );
    assert.equal(await step("keep").getAttribute("data-state"), "current");
    // Cmd with Enter saves as Ctrl does.
    await page.locator("#intent-goal").press("Meta+Enter");
    await page.waitForFunction(() =>
      /· saved$/.test(
        document.getElementById("intent-summary-state").textContent,
      ),
    );
    assert.match(
      await page.locator("#intent-summary-state").textContent(),
      /^2 of \d+ answered · saved$/,
    );
    assert.equal(await step("keep").getAttribute("data-state"), "done");
    // An edit after feedback: the quote stays, and says it is the older text.
    await box.fill(text + " Edited.");
    assert.equal(await current(), "explain");
    assert.match(
      await page.locator("#yours-note").textContent(),
      /changed it since/,
    );
    // Sent again, the new reply is said again and the quote is current.
    await page.evaluate(() => {
      document.getElementById("rail-live").textContent = "";
    });
    await box.press("Meta+Enter");
    await page.waitForFunction(() =>
      /Reviewer feedback arrived/.test(
        document.getElementById("rail-live").textContent,
      ),
    );
    assert.equal(await page.locator("#yours-note").textContent(), "");
    // On a narrow screen the explanation follows the reference instead.
    await page.setViewportSize({ width: 600, height: 900 });
    const narrowRef = await page.locator("#feedback-list").boundingBox();
    const narrowAside = await yours.boundingBox();
    assert.ok(narrowAside.y >= narrowRef.y + narrowRef.height - 1);
    assert.equal(
      await page
        .locator("#review-rail")
        .evaluate((rail) => getComputedStyle(rail).position),
      "static",
    );
  },
);
