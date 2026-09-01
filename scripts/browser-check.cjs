// `domcontentloaded`, not `networkidle`, on every navigation below.
//
// The interview page fetches Pyodide, a VRM avatar and the MediaPipe face
// models, tens of megabytes that most of these flows never touch, and
// `networkidle` waits for all of it before the first assertion runs. Every
// `goto` here is already followed by `clearMediaGate`, which blocks on the gate
// becoming visible and then hidden, or by a `waitFor` on the heading the flow
// cares about. Those wait for the thing being tested; `networkidle` waited for
// the whole page and cost the offline flow about fifty seconds of it.
const { chromium } = require(process.env.PLAYWRIGHT_PATH);
const fs = require("fs");
const http = require("http");
const { spawn } = require("child_process");
let livekitServerSdk;

function redact(text) {
  let output = String(text);
  for (const key of ["LIVEKIT_API_KEY", "LIVEKIT_API_SECRET", "GOOGLE_API_KEY"]) {
    const value = process.env[key];
    if (value) output = output.split(value).join("[redacted]");
  }
  return output.replace(/[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}/g, "[jwt]");
}

function stopProcessGroup(child) {
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    child.kill();
  }
}

/// The interview page gates the room join on local media, with no bypass, so
/// every run clears it the way a candidate would. Fake browser devices drive
/// the level meter.
async function clearMediaGate(page) {
  const gate = page.locator("#audio-check");
  await gate.waitFor({ state: "visible", timeout: 30000 });
  await page.getByRole("button", { name: "Play test tone" }).click();
  await page.getByRole("button", { name: "I heard it" }).click();
  if ((await page.locator("#audio-step-output p").innerText()) !== "Output — Confirmed") {
    throw new Error("output confirmation did not render immediately");
  }
  const join = page.getByRole("button", { name: "Start interview" });
  await join.waitFor();
  for (let i = 0; i < 60 && (await join.isDisabled()); i++) {
    await page.waitForTimeout(500);
  }
  if (await join.isDisabled()) {
    throw new Error(`media gate never opened: ${await page.locator("#audio-check-status").textContent()}`);
  }
  await join.click();
  await gate.waitFor({ state: "hidden" });
}

async function listRoomParticipants(roomName) {
  livekitServerSdk ||= requireLivekitServerSdk();
  const host = process.env.LIVEKIT_URL.replace(/^wss:/, "https:");
  const client = new livekitServerSdk.RoomServiceClient(host, process.env.LIVEKIT_API_KEY, process.env.LIVEKIT_API_SECRET);
  return {
    client,
    participants: await client.listParticipants(roomName),
  };
}

function isAgentParticipant(participant) {
  return participant.permission?.agent === true || String(participant.kind) === "AGENT";
}

function agentParticipantIdentities(participants) {
  return participants
    .filter(isAgentParticipant)
    .map((participant) => participant.identity)
    .sort();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/// scripts/session-cookie.sh hands over a bare `name=value`, attributes already
/// stripped, so this only has to split on the first `=`.
function sessionCookieHeader(cookie) {
  const [name, ...value] = String(cookie || "").split("=");
  if (!name || value.length === 0) return null;
  return { name, value: value.join("=") };
}

async function isolateRustAgent(roomName, rustAgentIdentity, timeoutMs = 120000) {
  const started = Date.now();
  let lastAgentParticipants = [];
  while (Date.now() - started < timeoutMs) {
    let roomState;
    try {
      roomState = await listRoomParticipants(roomName);
    } catch (error) {
      if (error.status === 404 || error.code === "not_found") {
        await sleep(500);
        continue;
      }
      throw error;
    }
    let removed = false;
    for (const participant of roomState.participants) {
      if (isAgentParticipant(participant) && participant.identity !== rustAgentIdentity) {
        await roomState.client.removeParticipant(roomName, participant.identity);
        removed = true;
      }
    }
    if (removed) {
      await sleep(500);
      continue;
    }
    lastAgentParticipants = agentParticipantIdentities(roomState.participants);
    if (lastAgentParticipants.length === 1 && lastAgentParticipants[0] === rustAgentIdentity) {
      return lastAgentParticipants;
    }
    await sleep(500);
  }
  throw new Error(`room agents not isolated\n${JSON.stringify(lastAgentParticipants)}`);
}

(async () => {
  let browser;
  let rustAgent;
  let compilerExplorerMock;
  let agentDone = false;
  try {
    browser = await chromium.launch({
      args: [
        "--use-fake-device-for-media-stream",
        "--use-fake-ui-for-media-stream",
        ...(process.env.BROWSER_CHECK_BARGE_AUDIO_FILE
          ? [`--use-file-for-fake-audio-capture=${process.env.BROWSER_CHECK_BARGE_AUDIO_FILE}`]
          : []),
      ],
    });
  } catch (error) {
    console.error(String(error && error.message ? error.message : error));
    console.error("Set PLAYWRIGHT_PATH to an installed Playwright module path and install Chromium for that runner.");
    process.exit(1);
  }

  try {
    const page = await browser.newPage();
    // Playwright defaults both of these to 30s, which is generous until the
    // machine is busy and then is not. `interview.js` is a module with a
    // top-level await, so DOMContentLoaded does not fire until the problem
    // fetch and the whole module graph have resolved; on a loaded runner that
    // navigation alone has been measured past thirty seconds, and the check
    // failed on the clock rather than on anything it was asserting.
    //
    // One number in one place rather than a timeout argument sprinkled down the
    // flows. It costs nothing when things are fast, because every wait returns
    // as soon as its condition holds. It costs two minutes only when something
    // is genuinely broken, which is a bill worth paying to stop a green gate
    // going red for being run at a busy moment.
    page.setDefaultTimeout(120000);
    page.setDefaultNavigationTimeout(120000);
    // One boolean, not every URL of the run. The avatar flow only asks whether
    // the pinned model actually arrived, and accumulating thousands of request
    // strings to answer that grew unboundedly for no gain.
    //
    // Read off the response and not the request, because that is the whole
    // distinction the assertion below rests on: a request that never got a 200
    // is an offline runner, which is allowed to fall back, while a 200 means
    // the bytes were in the browser's hands and a neutral panel is a bug.
    // 304 counts: a revalidated response is the browser being handed the model
    // out of its own HTTP cache, which is delivery. Anything else, including a
    // 4xx and a request that never got an answer, leaves this false.
    let modelDelivered = false;
    // Imported, not scraped. `model.js` owns the pinned URL and has no imports
    // of its own, so a dynamic import reads the real value; matching a regex
    // against the source would pin its formatting instead, and break on a
    // reflow with an error about a missing export.
    const { MODEL_URL } = await import("../web/avatar/model.js");
    page.on("response", (response) => {
      const delivered = response.status() === 200 || response.status() === 304;
      if (response.url() === MODEL_URL && delivered) modelDelivered = true;
    });
    // The avatar degrades to a neutral panel on every failure, by design, which
    // means a broken model and an absent one look identical from the DOM. The
    // console is the only place that says which, so a failure quotes it.
    const consoleErrors = [];
    page.on("console", (message) => {
      const text = message.text();
      // One prefix, not an allowlist of English sentences. The previous filter
      // named two diagnostics by their opening words and silently dropped the
      // other two, which is the swallowed-signal bug it exists to catch,
      // reproduced one layer out. Everything else at warning level here is GL
      // and fake-media noise.
      const ours = text.startsWith("codetrial ");
      if (message.type() === "error" || (message.type() === "warning" && ours)) {
        consoleErrors.push(text);
      }
    });
    page.on("pageerror", (error) => consoleErrors.push(String(error)));
    // "status of 500" with no URL is not a diagnosis, so pair every failing
    // response with the thing that was being fetched.
    page.on("response", (response) => {
      if (response.status() >= 400) consoleErrors.push(`HTTP ${response.status()} ${response.url()}`);
    });
    page.on("requestfailed", (request) =>
      consoleErrors.push(`request failed ${request.url()}: ${request.failure()?.errorText}`));
    const sessionCookie = sessionCookieHeader(process.env.BROWSER_CHECK_SESSION_COOKIE);
    if (sessionCookie) {
      await page.context().addCookies([{
        ...sessionCookie,
        url: process.env.BASE_URL,
        httpOnly: true,
        sameSite: "Lax",
      }]);
    }
    let compilerExplorerBaseUrl = process.env.BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL;
    if (compilerExplorerBaseUrl === "mock") {
      compilerExplorerMock = await startCompilerExplorerMock();
      compilerExplorerBaseUrl = compilerExplorerMock.url;
    }
    // Not in mock mode. `web/runners.js` reads this global once, when the
    // module evaluates, and `/runtime-config.js` assigns it too; which of the
    // two lands first is not fixed, so injecting a third writer made the run
    // pass or fail depending on load order. Under interception the page needs
    // no injection at all: it computes the real origin, exactly as in
    // production, and the request is answered before it leaves.
    if (compilerExplorerBaseUrl !== "__default__" && !compilerExplorerMock) {
      await page.addInitScript((baseUrl) => {
        globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = baseUrl;
      }, compilerExplorerBaseUrl);
    }
    if (compilerExplorerMock) {
      // Route interception, not the init script above.
      //
      // `/runtime-config.js` assigns the same global, and it loads after any
      // init script, so the page always ended up with the server's origin and
      // the mock server sat there receiving nothing. Counting its requests said
      // zero while the check passed, because the real service returns the same
      // answers the mock was built to fake: the flow was hitting godbolt.org
      // and calling itself hermetic.
      //
      // Intercepting by URL pattern does not care what the page computed. The
      // browser still believes it is talking to the real origin, so the CSP,
      // which is built from that same origin, is exercised rather than
      // sidestepped.
      await page.route("**/api/compiler/**", async (route) => {
        const request = route.request();
        const response = await fetch(`${compilerExplorerMock.url}${new URL(request.url()).pathname}`, {
          method: request.method(),
          headers: { "Content-Type": "application/json" },
          body: request.postData() ?? undefined,
        });
        await route.fulfill({
          status: response.status,
          contentType: "application/json",
          body: await response.text(),
        });
      });
    }
    const mode = process.env.BROWSER_CHECK_AGENT;
    const flow = process.env.BROWSER_CHECK_FLOW;
    // Real LiveKit and Gemini credentials are in play, so a fallback to offline
    // practice is a failure rather than a graceful degradation. In `rust` this
    // script starts the agent; in `dispatch` the server does, which is the
    // whole point of that mode.
    const credentialed = mode === "rust" || mode === "dispatch";
    let agentFailure;
    const agentOutput = [];
    let sawRustMetadataConfig = false;
    const never = new Promise(() => {});
    let roomName;
    let rustAgentIdentity;
    let rustAgentParticipants = [];
    function startRustAgent(room) {
      // The same `--config` the server was started with. `provider_dir` is
      // the config file's directory, so an agent left to find its own would
      // route against a different pool than the server it is answering for.
      rustAgent = spawn("cargo", [
        "run",
        "--quiet",
        "--",
        "run-livekit",
        room,
        "--config",
        process.env.BROWSER_CHECK_CONFIG_PATH,
      ], {
        cwd: process.env.ROOT,
        detached: true,
        env: process.env,
        stdio: ["ignore", "pipe", "pipe"],
      });
      for (const stream of [rustAgent.stdout, rustAgent.stderr]) {
        stream.on("data", (chunk) => {
          const text = redact(chunk);
          if (text.includes("problem=two-sum duration=20min")) sawRustMetadataConfig = true;
          agentOutput.push(text);
          while (agentOutput.join("").length > 12000) agentOutput.shift();
        });
      }
      agentFailure = new Promise((_, reject) => {
        rustAgent.once("exit", (code, signal) => {
          if (!agentDone && (code !== 0 || signal)) {
            reject(new Error(
              `rust agent exited early: code=${code} signal=${signal}\n${agentOutput.join("")}`,
            ));
          }
        });
      });
      // The agent can die before anything races this promise, and an
      // unobserved rejection takes the whole check down with a stack trace
      // instead of the agent output above.
      agentFailure.catch(() => {});
    }
    async function runAndExpectPassing(expected = 4, timeout = 30000) {
      await page.getByRole("button", { name: /Run tests/ }).click();
      await page.getByRole("button", { name: "Run tests" }).waitFor({ timeout });
      try {
        await page.getByText(`Test results · ${expected}/${expected}`).waitFor({ timeout });
      } catch (error) {
        const label = await page.locator("#results-label").innerText().catch(() => "");
        const body = await page.locator("#results-body").innerText().catch(() => "");
        throw new Error(`expected ${expected}/${expected} test results, got ${label}\n${body}`);
      }
    }
    // Flow-first, before the mode dispatch. This is the only check that can see
    // WebGL at all; the node tests drive the same module with an injected
    // loader and no renderer.
    if (flow === "avatar") {
      // Explicit, because the avatar is hidden below a CSS breakpoint and
      // Playwright's default 1280x720 sits close enough to it that the check
      // would silently depend on a default nobody chose.
      // The browser launches with --use-fake-ui-for-media-stream, which is why
      // no other flow in this file grants permissions.
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(`${process.env.BASE_URL}/interview?problem=two-sum&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.locator("#jim-avatar").waitFor({ timeout: 30000 });
      // Leaving "loading" is the contract. Which terminal state it lands on is
      // the repo's business, and both are asserted below.
      await page
        .waitForFunction(() => document.querySelector("#jim-avatar")?.dataset.avatarState !== "loading", null, { timeout: 60000 })
        .catch(() => { throw new Error("the avatar never left its loading state"); });

      const avatarState = await page.locator("#jim-avatar").getAttribute("data-avatar-state");
      const canvases = await page.locator("#jim-avatar canvas").count();
      const fallbackVisible = await page.locator("#jim-avatar-fallback").isVisible();
      const note = (await page.locator("#jim-avatar-note").innerText()).trim();

      if (avatarState === "ready") {
        if (canvases !== 1) throw new Error(`a rendered avatar needs exactly one canvas, found ${canvases}`);
        if (fallbackVisible) throw new Error("the neutral panel is still covering a rendered avatar");
        // Counts real render calls. The previous version sampled toDataURL
        // once, waited, and then compared that one sample against a length
        // threshold, so it had no "after" and passed on a cleared buffer.
        // Reading pixels back would need preserveDrawingBuffer; a counter the
        // renderer increments is cheaper and unambiguous.
        const first = await page.evaluate(() => window.__codetrialAvatarFrames?.() ?? null);
        if (first === null) throw new Error("the page exposed no avatar frame counter");
        // Waits for the condition instead of sleeping past it: the next frame
        // lands in about 16 ms, so a fixed 500 ms sleep spent most of itself
        // waiting for something already true.
        await page
          .waitForFunction((from) => (window.__codetrialAvatarFrames?.() ?? 0) > from, first, { timeout: 5000 })
          .catch(() => { throw new Error(`the avatar render loop is not running: stuck at ${first} frames`); });
      } else if (avatarState === "unavailable") {
        if (canvases !== 0) throw new Error("an unavailable avatar must not leave a canvas behind");
        if (!fallbackVisible) throw new Error("the neutral panel must be visible when the avatar is unavailable");
        if (!/avatar is unavailable/.test(note)) throw new Error(`the neutral panel must say why, got: ${note}`);
        if (modelDelivered) {
          // The model is fetched from a third-party host now, so an offline or
          // firewalled runner reaching the neutral panel is correct behavior
          // and not a failure. A 200 for the pinned URL removes that excuse:
          // the bytes arrived, and everything after that is this repo's, which
          // is a broken avatar wearing the same panel as an unreachable one.
          throw new Error(
            `the pinned model was delivered but did not render:\n${consoleErrors.join("\n") || "(no console errors captured)"}`,
          );
        }
        console.log("avatar: the pinned model was never delivered, so the neutral panel is the correct result");
      } else {
        throw new Error(`unexpected avatar state: ${avatarState}`);
      }
      return;
    }

    if (mode === "home") {
      await page.goto(process.env.BASE_URL, { waitUntil: "domcontentloaded" });
      await page.getByRole("heading", { name: "Practice a live technical interview" }).waitFor();
      await page.getByRole("button", { name: "Start interview" }).waitFor();
      await page.getByText("Valid Parentheses").waitFor();
      return;
    }

    if (mode === "offline") {
      await page.goto(`${process.env.BASE_URL}/interview?problem=two-sum&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Two Sum", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      // The `""` branch that used to be here is gone. It set the global from an
      // init script and expected "not wired up yet", which cannot work: the
      // page also loads /runtime-config.js, which assigns the same global, so
      // the injected value never survived and the assertion timed out. Nothing
      // ran it, so nobody found out.
      //
      // Both halves of that path are covered where they are cheap and correct.
      // tests/web.rs asserts the server emits the disabled runtime-config, and
      // tests/browser/dom-contract.test.js re-imports web/runners.js with the
      // global set to "" and asserts every compiled language reports itself not
      // wired up. Neither has a load order to lose.
      if (compilerExplorerMock) {
        await page.getByRole("button", { name: "C", exact: true }).click();
        await page.getByLabel("Code editor").fill(`#include <stdlib.h>
int* twoSum(int* nums, int numsSize, int target, int* returnSize) {
    int* out = malloc(sizeof(int) * 2);
    for (int i = 0; i < numsSize; i++) {
        for (int j = i + 1; j < numsSize; j++) {
            if (nums[i] + nums[j] == target) {
                out[0] = i;
                out[1] = j;
                *returnSize = 2;
                return out;
            }
        }
    }
    *returnSize = 0;
    return out;
}
`);
        await runAndExpectPassing(4, 120000);
        await page.goto(`${process.env.BASE_URL}/interview?problem=min-stack&duration=20`, { waitUntil: "domcontentloaded" });
        await clearMediaGate(page);
        await page.getByRole("heading", { name: "Min Stack", level: 1 }).waitFor();
        await page.getByText("Offline", { exact: true }).waitFor();
        await page.getByRole("button", { name: "C++" }).click();
        await page.getByLabel("Code editor").fill(`#include <vector>
using namespace std;
class MinStack {
    vector<int> values;
    vector<int> minimums;
public:
    MinStack() {}
    void push(int value) {
        values.push_back(value);
        minimums.push_back(minimums.empty() ? value : min(value, minimums.back()));
    }
    void pop() {
        values.pop_back();
        minimums.pop_back();
    }
    int top() { return values.back(); }
    int getMin() { return minimums.back(); }
};
`);
        await runAndExpectPassing(4, 120000);
        await page.goto(`${process.env.BASE_URL}/interview?problem=binary-search-tree-iterator&duration=20`, { waitUntil: "domcontentloaded" });
        await clearMediaGate(page);
        await page.getByRole("heading", { name: "Binary Search Tree Iterator", level: 1 }).waitFor();
        await page.getByText("Offline", { exact: true }).waitFor();
        await page.getByRole("button", { name: "Java", exact: true }).click();
        await page.getByLabel("Code editor").fill(`class BSTIterator {
    private final ArrayDeque<TreeNode> stack = new ArrayDeque<>();
    public BSTIterator(TreeNode root) { pushLeft(root); }
    private void pushLeft(TreeNode node) {
        while (node != null) {
            stack.push(node);
            node = node.left;
        }
    }
    public int next() {
        TreeNode node = stack.pop();
        pushLeft(node.right);
        return node.val;
    }
    public boolean hasNext() { return !stack.isEmpty(); }
}
`);
        await runAndExpectPassing(3, 120000);
        return;
      }
      if (compilerExplorerBaseUrl !== "__default__") {
        await page.getByRole("button", { name: "C++" }).click();
        await page.getByLabel("Code editor").fill(`class Solution {
public:
    vector<int> twoSum(vector<int>& nums, int target) {
        return {0, 1};
    }
};
`);
        await page.getByRole("button", { name: /Run tests/ }).click();
        await page.getByRole("button", { name: "Run tests" }).waitFor({ timeout: 120000 });
        await page.getByText("Couldn't run your code").waitFor({ timeout: 120000 });
        await page.getByText(/Compiler Explorer (run failed|did not respond)/).waitFor({ timeout: 120000 });
        return;
      }
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`let i = 0;
function twoSum() {
  return spec.cases[i++].expected;
}
`);
      await page.getByRole("button", { name: /Run tests/ }).click();
      await page.getByRole("button", { name: "Run tests" }).waitFor();
      await page.getByText("Test results · 0/4").waitFor();
      await page.getByLabel("Code editor").fill(`function twoSum(nums, target) {
  const seen = new Map();
  for (let i = 0; i < nums.length; i++) {
    const want = target - nums[i];
    if (seen.has(want)) return [seen.get(want), i];
    seen.set(nums[i], i);
  }
  return [];
}
`);
      await runAndExpectPassing();
      await page.getByRole("button", { name: "C++" }).click();
      await page.getByLabel("Code editor").fill(`class Solution {
public:
    vector<int> twoSum(vector<int>& nums, int target) {
        unordered_map<int, int> seen;
        for (int i = 0; i < (int)nums.size(); i++) {
            int want = target - nums[i];
            if (seen.count(want)) return {seen[want], i};
            seen[nums[i]] = i;
        }
        return {};
    }
};
`);
      await runAndExpectPassing(4, 120000);
      await page.getByLabel("Code editor").fill(`class Solution {
public:
    vector<int> twoSum(vector<int>& nums, int target) {
        return {
    }
};
`);
      await page.getByRole("button", { name: /Run tests/ }).click();
      await page.getByRole("button", { name: "Run tests" }).waitFor({ timeout: 120000 });
      await page.getByText("Couldn't run your code").waitFor({ timeout: 120000 });
      await page.locator("#results-body pre").filter({ hasText: /Compilation failed|expected|error/i }).waitFor({ timeout: 120000 });
      await page.getByRole("button", { name: "C", exact: true }).click();
      await page.getByLabel("Code editor").fill(`#include <stdlib.h>
int* twoSum(int* nums, int numsSize, int target, int* returnSize) {
    int* out = malloc(sizeof(int) * 2);
    for (int i = 0; i < numsSize; i++) {
        for (int j = i + 1; j < numsSize; j++) {
            if (nums[i] + nums[j] == target) {
                out[0] = i;
                out[1] = j;
                *returnSize = 2;
                return out;
            }
        }
    }
    *returnSize = 0;
    return out;
}
`);
      await runAndExpectPassing(4, 120000);
      await page.getByRole("button", { name: "Python" }).click();
      await page.getByLabel("Code editor").fill(`class Solution:
    def twoSum(self, nums, target):
        seen = {}
        for i, value in enumerate(nums):
            want = target - value
            if want in seen:
                return [seen[want], i]
            seen[value] = i
        return []
`);
      await runAndExpectPassing(4, 120000);
      await page.getByRole("button", { name: "Transcript" }).click();
      await page.locator("p").filter({ hasText: /^Jim$/ }).first().waitFor();
      await page.locator("p").filter({ hasText: /^You$/ }).first().waitFor();

      await page.goto(`${process.env.BASE_URL}/interview?problem=merge-sorted-array&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Merge Sorted Array", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function merge(nums1, m, nums2, n) {
  let write = m + n - 1;
  let left = m - 1;
  let right = n - 1;
  while (right >= 0) {
    if (left >= 0 && nums1[left] > nums2[right]) {
      nums1[write--] = nums1[left--];
    } else {
      nums1[write--] = nums2[right--];
    }
  }
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=remove-duplicates-from-sorted-array-ii&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Remove Duplicates from Sorted Array II", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function removeDuplicates(nums) {
  let write = 0;
  for (const value of nums) {
    if (write < 2 || nums[write - 2] !== value) {
      nums[write++] = value;
    }
  }
  return "5";
}
`);
      await page.getByRole("button", { name: /Run tests/ }).click();
      await page.getByRole("button", { name: "Run tests" }).waitFor();
      await page.getByText("Test results · 0/3").waitFor();
      await page.getByLabel("Code editor").fill(`function removeDuplicates(nums) {
  let write = 0;
  for (const value of nums) {
    if (write < 2 || nums[write - 2] !== value) {
      nums[write++] = value;
    }
  }
  return write;
}
`);
      await runAndExpectPassing(3);

      await page.goto(`${process.env.BASE_URL}/interview?problem=merge-two-sorted-lists&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Merge Two Sorted Lists", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function mergeTwoLists(list1, list2) {
  const dummy = new ListNode();
  let tail = dummy;
  while (list1 && list2) {
    if (list1.val <= list2.val) {
      tail.next = list1;
      list1 = list1.next;
    } else {
      tail.next = list2;
      list2 = list2.next;
    }
    tail = tail.next;
  }
  tail.next = list1 || list2;
  return dummy.next;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=copy-list-with-random-pointer&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Copy List with Random Pointer", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function copyRandomList(head) {
  if (!head) return null;
  const copies = new Map();
  for (let node = head; node; node = node.next) copies.set(node, new _Node(node.val));
  for (let node = head; node; node = node.next) {
    copies.get(node).next = node.next ? copies.get(node.next) : null;
    copies.get(node).random = node.random ? copies.get(node.random) : null;
  }
  return copies.get(head);
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=rotate-list&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Rotate List", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function rotateRight(head, k) {
  if (!head || !head.next) return head;
  let tail = head;
  let length = 1;
  while (tail.next) {
    tail = tail.next;
    length++;
  }
  k %= length;
  if (k === 0) return head;
  tail.next = head;
  let steps = length - k;
  while (steps-- > 0) tail = tail.next;
  const next = tail.next;
  tail.next = null;
  return next;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=invert-binary-tree&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Invert Binary Tree", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function invertTree(root) {
  if (!root) return null;
  const left = invertTree(root.left);
  root.left = invertTree(root.right);
  root.right = left;
  return root;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=construct-binary-tree-from-preorder-and-inorder-traversal&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Construct Binary Tree from Preorder and Inorder Traversal", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function buildTree(preorder, inorder) {
  const positions = new Map(inorder.map((value, index) => [value, index]));
  let preIndex = 0;
  function build(left, right) {
    if (left > right) return null;
    const value = preorder[preIndex++];
    const root = new TreeNode(value);
    const mid = positions.get(value);
    root.left = build(left, mid - 1);
    root.right = build(mid + 1, right);
    return root;
  }
  return build(0, inorder.length - 1);
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=populating-next-right-pointers-in-each-node-ii&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Populating Next Right Pointers in Each Node II", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function connect(root) {
  let level = root;
  while (level) {
    const dummy = new _Node(0);
    let tail = dummy;
    for (let node = level; node; node = node.next) {
      if (node.left) tail = tail.next = node.left;
      if (node.right) tail = tail.next = node.right;
    }
    level = dummy.next;
  }
  return root;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=binary-search-tree-iterator&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Binary Search Tree Iterator", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`var BSTIterator = function(root) {
  this.stack = [];
  this.pushLeft(root);
};

BSTIterator.prototype.pushLeft = function(node) {
  while (node) {
    this.stack.push(node);
    node = node.left;
  }
};

BSTIterator.prototype.next = function() {
  const node = this.stack.pop();
  this.pushLeft(node.right);
  return node.val;
};

BSTIterator.prototype.hasNext = function() {
  return this.stack.length > 0;
};
`);
      await runAndExpectPassing(3);

      await page.goto(`${process.env.BASE_URL}/interview?problem=lowest-common-ancestor-of-a-binary-tree&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Lowest Common Ancestor of a Binary Tree", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function lowestCommonAncestor(root, p, q) {
  if (!root || root === p || root === q) return root;
  const left = lowestCommonAncestor(root.left, p, q);
  const right = lowestCommonAncestor(root.right, p, q);
  if (left && right) return root;
  return left || right;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=binary-tree-zigzag-level-order-traversal&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Binary Tree Zigzag Level Order Traversal", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function zigzagLevelOrder(root) {
  if (!root) return [];
  const rows = [];
  let queue = [root];
  let leftToRight = true;
  while (queue.length) {
    const next = [];
    const row = [];
    for (const node of queue) {
      if (leftToRight) row.push(node.val);
      else row.unshift(node.val);
      if (node.left) next.push(node.left);
      if (node.right) next.push(node.right);
    }
    rows.push(row);
    queue = next;
    leftToRight = !leftToRight;
  }
  return rows;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=validate-binary-search-tree&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Validate Binary Search Tree", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function isValidBST(root) {
  function valid(node, low, high) {
    if (!node) return true;
    if (node.val <= low || node.val >= high) return false;
    return valid(node.left, low, node.val) && valid(node.right, node.val, high);
  }
  return valid(root, -Infinity, Infinity);
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=clone-graph&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Clone Graph", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function cloneGraph(node) {
  if (!node) return null;
  const copies = new Map();
  function clone(current) {
    if (copies.has(current)) return copies.get(current);
    const copy = new _Node(current.val);
    copies.set(current, copy);
    copy.neighbors = current.neighbors.map(clone).reverse();
    return copy;
  }
  return clone(node);
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=course-schedule-ii&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Course Schedule II", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function findOrder(numCourses, prerequisites) {
  const graph = Array.from({ length: numCourses }, () => []);
  const indegree = Array(numCourses).fill(0);
  for (const [course, prerequisite] of prerequisites) {
    graph[prerequisite].push(course);
    indegree[course]++;
  }
  const queue = [];
  for (let course = numCourses - 1; course >= 0; course--) {
    if (indegree[course] === 0) queue.push(course);
  }
  const order = [];
  while (queue.length) {
    const course = queue.pop();
    order.push(course);
    for (const next of graph[course]) {
      if (--indegree[next] === 0) queue.push(next);
    }
  }
  return order.length === numCourses ? order : [];
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=implement-trie-prefix-tree&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Implement Trie (Prefix Tree)", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`var Trie = function() {
  this.children = new Map();
  this.word = false;
};

Trie.prototype.insert = function(word) {
  let node = this;
  for (const char of word) {
    if (!node.children.has(char)) node.children.set(char, new Trie());
    node = node.children.get(char);
  }
  node.word = true;
};

Trie.prototype.search = function(word) {
  const node = this.find(word);
  return Boolean(node && node.word);
};

Trie.prototype.startsWith = function(prefix) {
  return Boolean(this.find(prefix));
};

Trie.prototype.find = function(text) {
  let node = this;
  for (const char of text) {
    node = node.children.get(char);
    if (!node) return null;
  }
  return node;
};
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=design-add-and-search-words-data-structure&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Design Add and Search Words Data Structure", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`var WordDictionary = function() {
  this.children = new Map();
  this.word = false;
};

WordDictionary.prototype.addWord = function(word) {
  let node = this;
  for (const char of word) {
    if (!node.children.has(char)) node.children.set(char, new WordDictionary());
    node = node.children.get(char);
  }
  node.word = true;
};

WordDictionary.prototype.search = function(word) {
  const dfs = (node, index) => {
    if (index === word.length) return node.word;
    const char = word[index];
    if (char !== ".") {
      const next = node.children.get(char);
      return Boolean(next && dfs(next, index + 1));
    }
    for (const next of node.children.values()) {
      if (dfs(next, index + 1)) return true;
    }
    return false;
  };
  return dfs(this, 0);
};
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=combination-sum&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Combination Sum", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function combinationSum(candidates, target) {
  candidates.sort((a, b) => a - b);
  const results = [];
  function dfs(start, remain, path) {
    if (remain === 0) {
      results.push([...path].reverse());
      return;
    }
    for (let i = start; i < candidates.length && candidates[i] <= remain; i++) {
      path.push(candidates[i]);
      dfs(i, remain - candidates[i], path);
      path.pop();
    }
  }
  dfs(0, target, []);
  return results;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=permutations&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Permutations", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function permute(nums) {
  const results = [];
  function dfs(path, used) {
    if (path.length === nums.length) {
      results.unshift([...path]);
      return;
    }
    for (let i = nums.length - 1; i >= 0; i--) {
      if (used[i]) continue;
      used[i] = true;
      path.push(nums[i]);
      dfs(path, used);
      path.pop();
      used[i] = false;
    }
  }
  dfs([], Array(nums.length).fill(false));
  return results;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=generate-parentheses&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Generate Parentheses", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function generateParenthesis(n) {
  const results = [];
  function dfs(open, close, path) {
    if (path.length === n * 2) {
      results.unshift(path);
      return;
    }
    if (open < n) dfs(open + 1, close, path + "(");
    if (close < open) dfs(open, close + 1, path + ")");
  }
  dfs(0, 0, "");
  return results;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=n-queens-ii&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "N-Queens II", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function totalNQueens(n) {
  let count = 0;
  const cols = new Set();
  const diagA = new Set();
  const diagB = new Set();
  function dfs(row) {
    if (row === n) {
      count++;
      return;
    }
    for (let col = 0; col < n; col++) {
      if (cols.has(col) || diagA.has(row + col) || diagB.has(row - col)) continue;
      cols.add(col);
      diagA.add(row + col);
      diagB.add(row - col);
      dfs(row + 1);
      cols.delete(col);
      diagA.delete(row + col);
      diagB.delete(row - col);
    }
  }
  dfs(0);
  return count;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=word-search&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Word Search", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function exist(board, word) {
  function dfs(row, col, index) {
    if (index === word.length) return true;
    if (row < 0 || col < 0 || row === board.length || col === board[0].length) return false;
    if (board[row][col] !== word[index]) return false;
    const saved = board[row][col];
    board[row][col] = "#";
    const found = dfs(row + 1, col, index + 1)
      || dfs(row - 1, col, index + 1)
      || dfs(row, col + 1, index + 1)
      || dfs(row, col - 1, index + 1);
    board[row][col] = saved;
    return found;
  }
  for (let row = 0; row < board.length; row++) {
    for (let col = 0; col < board[0].length; col++) {
      if (dfs(row, col, 0)) return true;
    }
  }
  return false;
}
`);
      await runAndExpectPassing();

      await page.goto(`${process.env.BASE_URL}/interview?problem=convert-sorted-array-to-binary-search-tree&duration=20`, { waitUntil: "domcontentloaded" });
      await clearMediaGate(page);
      await page.getByRole("heading", { name: "Convert Sorted Array to Binary Search Tree", level: 1 }).waitFor();
      await page.getByText("Offline", { exact: true }).waitFor();
      await page.getByRole("button", { name: "JavaScript" }).click();
      await page.getByLabel("Code editor").fill(`function sortedArrayToBST(nums) {
  function build(left, right) {
    if (left > right) return null;
    const mid = Math.floor((left + right) / 2);
    return new TreeNode(nums[mid], build(left, mid - 1), build(mid + 1, right));
  }
  return build(0, nums.length - 1);
}
`);
      await runAndExpectPassing();

      await page.getByRole("button", { name: "End interview" }).click();
      await page.getByRole("heading", { name: "No evaluation" }).waitFor({ timeout: 30000 });
      const report = await page.evaluate(() => JSON.parse(localStorage.getItem("codetrial_history") || "[]")[0]?.report || null);
      if (!report?.incomplete || "codingScore" in report || "decision" in report) throw new Error("offline activity was presented as personalized evaluation");
      return;
    }

    await page.context().grantPermissions(["microphone", "camera"], { origin: process.env.BASE_URL });
    if (credentialed) {
      const fixedRoom = process.env.BROWSER_CHECK_FIXED_ROOM === "1";
      if (fixedRoom) {
        roomName = process.env.INTERVIEW_ROOM_NAME;
        rustAgentIdentity = `interviewer-${roomName}`;
        sawRustMetadataConfig = true;
      } else {
        await page.addInitScript(() => {
          const originalFetch = window.fetch.bind(window);
          window.fetch = async (input, init) => {
            const url = typeof input === "string" ? input : (input && input.url) || "";
            const response = await originalFetch(input, init);
            if (url.endsWith("/api/token") && response.ok) {
              response.clone().json().then((body) => {
                window.__BROWSER_CHECK_TOKEN_RESPONSE = body;
              }).catch(() => {});
            }
            return response;
          };
        });
      }
    }

    await page.goto(`${process.env.BASE_URL}/interview?problem=two-sum&duration=20`);
    await clearMediaGate(page);
    if (credentialed && !rustAgentIdentity) {
      roomName = await page
        .waitForFunction(() => window.__BROWSER_CHECK_TOKEN_RESPONSE?.roomName, null, { timeout: 30000 })
        .then((handle) => handle.jsonValue())
        .catch(() => null);
      if (!roomName) throw new Error("server /api/token did not return a room name");
      rustAgentIdentity = `interviewer-${roomName}`;
      // In dispatch mode the server starts the interviewer, and this script
      // must not: a second one in the room would be evicted by the first.
      if (mode === "rust") startRustAgent(roomName);
    }
    await Promise.race([
      (async () => {
        if (mode === "rust") {
          rustAgentParticipants = await isolateRustAgent(roomName, rustAgentIdentity);
          const started = Date.now();
          while (!sawRustMetadataConfig && Date.now() - started < 120000) {
            await page.waitForTimeout(500);
          }
          if (!sawRustMetadataConfig) {
            throw new Error(`rust agent did not use candidate metadata\n${agentOutput.join("")}`);
          }
        }
        // With real credentials the room join must actually succeed. Offline
        // practice mode is a legitimate fallback when there are no credentials
        // and a silent failure when there are: a `ReferenceError` thrown after
        // a successful connect once put every interview into it, and the page
        // looked fine because Jim still greeted through the room that stayed
        // open underneath. Nothing in the gate could see that, so it is checked
        // here, where credentials exist.
        if (credentialed) {
          const offline = consoleErrors.filter((line) => /codetrial connect_failed/.test(line));
          if (offline.length) {
            throw new Error(`the interview fell back to offline practice with real credentials:\n${offline.join("\n")}`);
          }
          const pill = await page.locator("#agent-state").innerText().catch(() => "");
          if (/Offline/.test(pill)) throw new Error("the interview joined no room; the status pill reads Offline");
        }
        const problemTitle = await page.getByRole("heading", { name: "Two Sum", level: 1 }).innerText();
        // Scoped to the pill. The captions element also renders the literal
        // word "Listening" as its placeholder, so an unscoped text match hits
        // two elements; it only ever looked unambiguous because the pill was
        // stuck on "Waiting" while the connect bug was live.
        await page.locator("#agent-state").filter({ hasText: /Listening|Thinking|Speaking/ }).waitFor({ timeout: 120000 });
        if (mode === "dispatch") {
          // Who actually staffed the room. The pill turning green only proves
          // that some agent arrived; this proves it was the one this server
          // dispatched, which is the difference between the fix working and a
          // stray worker registered on the LiveKit project covering for it.
          const staffing = (await listRoomParticipants(roomName)).participants;
          rustAgentParticipants = agentParticipantIdentities(staffing);
          const identities = staffing.map((participant) => participant.identity);
          if (!identities.includes(rustAgentIdentity)) {
            throw new Error(
              `the server did not staff the room it minted; participants: ${JSON.stringify(identities)}`,
            );
          }
        }
        await page.getByRole("button", { name: "Transcript" }).click();
        await page.getByText("No conversation yet").waitFor({ state: "detached", timeout: 120000 });
        let testResultText = null;
        let candidateSegmentCount = null;
        let agentAfterCandidate = false;
        if (flow === "report") {
          await page.getByRole("button", { name: /Run tests/ }).click();
          await page.getByText(/Test results · \d+\/\d+|Couldn't run your code/).waitFor({ timeout: 180000 });
          testResultText = await page.locator("text=/Test results · \\d+\\/\\d+|Couldn't run your code/").first().innerText();
          if (mode === "rust") {
            rustAgentParticipants = await isolateRustAgent(roomName, rustAgentIdentity, 10000);
          }
          await page.getByRole("button", { name: "End interview" }).click();
          await page.getByRole("heading", { name: "Your performance packet" }).waitFor({ timeout: 240000 });
        } else if (flow === "barge") {
          await page.locator("#agent-state").filter({ hasText: "Speaking" }).waitFor({ timeout: 120000 });
          const candidateLabels = page.locator("p").filter({ hasText: /^You$/ });
          await candidateLabels.first().waitFor({ timeout: 180000 });
          candidateSegmentCount = await candidateLabels.count();
          const started = Date.now();
          while (!agentAfterCandidate && Date.now() - started < 180000) {
            agentAfterCandidate = await page.evaluate(() => {
              const paragraphs = [...document.querySelectorAll("p")].map((node) => node.textContent?.trim() ?? "");
              const firstYou = paragraphs.findIndex((text) => text === "You");
              if (firstYou === -1) return false;
              return paragraphs
                .slice(firstYou + 1)
                .some((text) => text === "Jim" || /\b(go ahead|listening|stopped|proceed|take the floor|sure)\b/i.test(text));
            });
            if (!agentAfterCandidate) await page.waitForTimeout(500);
          }
          if (!agentAfterCandidate) {
            throw new Error(`agent did not respond after candidate barge-in\n${agentOutput.join("")}`);
          }
        }
        if (process.env.BROWSER_CHECK_CAPTURE) {
          if (mode === "rust" && flow !== "report") {
            rustAgentParticipants = await isolateRustAgent(roomName, rustAgentIdentity, 10000);
          }
          const speakerLabels = (await page.locator("p").filter({ hasText: /^(Jim|You)$/ }).allInnerTexts()).map((label) => label.trim().toLowerCase());
          const report = flow === "report"
            ? await page.evaluate(() => {
                const history = JSON.parse(localStorage.getItem("codetrial_history") ?? "[]");
                return history[0]?.report ?? null;
              })
            : null;
          const agentState = flow === "report"
            ? null
            : (await page.locator("#agent-state").innerText()).replace(/\u2026/g, "");
          fs.writeFileSync(process.env.BROWSER_CHECK_CAPTURE, JSON.stringify({
            mode,
            flow,
            problemTitle,
            agentState,
            testResultText,
            transcriptSegmentCount: speakerLabels.length,
            firstTranscriptSpeaker: speakerLabels[0] ?? null,
            candidateSegmentCount,
            agentAfterCandidate,
            rustAgentParticipants,
            report,
          }, null, 2));
        }
      })(),
      agentFailure ?? never,
    ]).catch(async (error) => {
      console.error(redact(await page.locator("body").innerText().catch(() => "")));
      if (roomName) {
        const participants = await listRoomParticipants(roomName)
          .then((roomState) => roomState.participants)
          .catch((apiError) => [`list failed: ${apiError.message}`]);
        console.error(redact(JSON.stringify(participants, null, 2)));
      }
      if (agentOutput.length > 0) {
        console.error(redact(agentOutput.join("")));
      }
      if (process.env.SERVER_LOG && fs.existsSync(process.env.SERVER_LOG)) {
        const serverLog = fs.readFileSync(process.env.SERVER_LOG, "utf8");
        if (serverLog) console.error(redact(serverLog));
      }
      throw error;
    });
    agentDone = true;
  } finally {
    agentDone = true;
    if (rustAgent) stopProcessGroup(rustAgent);
    if (compilerExplorerMock) await new Promise((resolve) => compilerExplorerMock.server.close(resolve));
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});

function requireLivekitServerSdk() {
  return require("livekit-server-sdk");
}

function startCompilerExplorerMock() {
  const server = http.createServer((request, response) => {
    const headers = {
      "Access-Control-Allow-Headers": "content-type",
      "Access-Control-Allow-Methods": "POST, OPTIONS",
      "Access-Control-Allow-Origin": "*",
      "Content-Type": "application/json",
    };
    if (request.method === "OPTIONS") {
      response.writeHead(204, headers).end();
      return;
    }
    if (request.method !== "POST" || !request.url.startsWith("/api/compiler/")) {
      response.writeHead(404).end();
      return;
    }
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const payload = JSON.parse(body || "{}");
      const source = payload.source || "";
      const executes = payload.options?.compilerOptions?.executorRequest === true
        && payload.options?.filters?.execute === true;
      let stdout;
      if (
        executes
        && payload.options?.userArguments === "-O2 -std=c17"
        && request.url.includes("/api/compiler/cclang1910/")
        && source.includes("int* twoSum")
        && source.includes("int main(void)")
      ) {
        stdout = "{\"results\":[{\"actual\":[0,1],\"timeMs\":1},{\"actual\":[1,2],\"timeMs\":1},{\"actual\":[0,1],\"timeMs\":1},{\"actual\":[0,2],\"timeMs\":1}]}";
      } else if (
        executes
        && payload.options?.userArguments === "-O2 -std=c++20"
        && request.url.includes("/api/compiler/g162/")
        && [
          "class MinStack",
          "jsonFragments(actual)",
          "MinStack instance{}",
          "instance.push(-2)",
          "instance.pop();",
          "instance.getMin()",
          "instance.top()",
        ].every((pattern) => source.includes(pattern))
      ) {
        stdout = "{\"results\":[{\"actual\":[null,null,null,null,-3,null,0,-2],\"timeMs\":1},{\"actual\":[null,null,null,null,1,null,1,null,2],\"timeMs\":1},{\"actual\":[null,null,null,3,3,null,5,5],\"timeMs\":1},{\"actual\":[null,null,null,null,-1,-1],\"timeMs\":1}]}";
      } else if (
        executes
        && payload.options?.userArguments === ""
        && request.url.includes("/api/compiler/java2501/")
        && [
          "class BSTIterator",
          "new BSTIterator(treeNode(new Integer[]{7, 3, 15, null, null, 9, 20}))",
          "actual.add(jsonAny(instance.next()))",
          "actual.add(jsonAny(instance.hasNext()))",
        ].every((pattern) => source.includes(pattern))
      ) {
        stdout = "{\"results\":[{\"actual\":[null,3,7,true,9,true,15,true,20,false],\"timeMs\":1},{\"actual\":[null,true,1,false],\"timeMs\":1},{\"actual\":[null,1,2,3,false],\"timeMs\":1}]}";
      } else {
        response.writeHead(400, headers);
        response.end(JSON.stringify({ code: 1, stderr: "unexpected mock Compiler Explorer request" }));
        return;
      }
      response.writeHead(200, headers);
      response.end(JSON.stringify({
        code: 0,
        didExecute: true,
        stdout,
      }));
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ server, url: `http://127.0.0.1:${server.address().port}` });
    });
  });
}
