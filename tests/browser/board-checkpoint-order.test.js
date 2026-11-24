// A whiteboard checkpoint whose upload fails is held and sent after the
// checkpoints taken later, so the order the agent receives them in is not the
// order they were taken in. The report credits writing on the board only when
// the Repeat checkpoint is the first, so each checkpoint carries the order it
// was taken in. This drives the page's own capture, hold and retry with a
// room whose Repeat upload fails twice, and checks the uploads that went out
// against the board wire fixture, whose last two cases the agent's test
// (`a_repeat_checkpoint_sent_after_example_is_still_first` in
// tests/unit/livekit/board.rs) reads to show the report still sees Repeat
// first.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { launchChromium, read, startStaticServer } from "./source.js";

let browser;
let server;
let base;

// Only the served copy exposes state setup. Capture, hold, retry and the
// stream header are the production ones; no live room is used.
const setup = `
globalThis.checkpointTest = {
  uploads: [],
  start(failures) {
    nodes.audioCheck.hidden = true;
    state.phase = "live";
    state.interviewId = "checkpoint-order-test";
    state.connected = true;
    const left = { ...failures };
    state.room = { localParticipant: { streamBytes: async (options) => {
      // A failed write is still closed, as the page closes it, so only a
      // close after a write that went through counts as sent.
      const upload = { options, failed: false, sent: false };
      this.uploads.push(upload);
      return {
        write: async () => {
          const phase = options.attributes.checkpoint;
          if (left[phase] > 0) {
            left[phase] -= 1;
            upload.failed = true;
            throw new Error("upload failed");
          }
        },
        close: async () => { upload.sent = !upload.failed; },
      };
    } } };
    board.ready = true;
    board.model.push("#101418", 3, [10, 10, 20, 20]);
    paintBoard();
  },
  async complete(phases) {
    captureBoardCheckpoints(phases);
    await board.publishing;
  },
  async settle() {
    queueBoardPublish();
    await board.publishing;
  },
  sent() {
    return this.uploads
      .filter((upload) => upload.sent)
      .map((upload) => upload.options.attributes);
  },
  held() {
    return [...board.heldCheckpoints.keys()];
  },
};
`;

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer({
    handle(request, response, url) {
      if (url.pathname === "/interview.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(read("web/interview.js") + setup);
        return true;
      }
      if (url.pathname === "/api/session") {
        response.setHeader("content-type", "application/json");
        response.end('{"signedIn":false}');
        return true;
      }
      return false;
    },
  }));
});

after(async () => {
  await browser?.close();
  await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
});

test("a repeat checkpoint that fails twice keeps the order it was taken in", async (t) => {
  if (!browser) {
    t.skip("playwright chromium unavailable");
    return;
  }
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  });
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/*", (route) =>
    route
      .request()
      .url()
      .startsWith(base + "/")
      ? route.continue()
      : route.abort(),
  );
  await page.goto(
    `${base}/interview.html?problem=chargeback-pair-match&mode=whiteboard`,
  );
  await page.waitForFunction(() => Boolean(globalThis.checkpointTest));

  const result = await page.evaluate(async () => {
    const test = globalThis.checkpointTest;
    test.start({ repeat: 2 });
    // Repeat is taken and its upload fails, so it is held.
    await test.complete(["repeat"]);
    const heldAfterRepeat = test.held();
    // Example is taken; the held Repeat is retried ahead of it and fails
    // again, and Example goes out.
    await test.complete(["repeat", "example"]);
    const heldAfterExample = test.held();
    // The next board retries Repeat, which now goes out, after Example.
    await test.settle();
    return {
      heldAfterRepeat,
      heldAfterExample,
      heldAtEnd: test.held(),
      sent: test.sent(),
    };
  });
  assert.deepEqual(errors, []);
  assert.deepEqual(result.heldAfterRepeat, ["repeat"]);
  assert.deepEqual(result.heldAfterExample, ["repeat"]);
  assert.deepEqual(result.heldAtEnd, []);

  // Example arrived first and Repeat after it, each with the order it was
  // taken in, as the fixture's last two cases say, and then the board itself.
  const checkpoints = result.sent.filter((attributes) => attributes.checkpoint);
  const fixture = JSON.parse(read("tests/fixtures/board-stream.json"));
  const expected = fixture.cases
    .slice(3)
    .map(({ options: { attributes } }) => ({
      checkpoint: attributes.checkpoint,
      taken: attributes.taken,
    }));
  assert.deepEqual(
    checkpoints.map(({ checkpoint, taken }) => ({ checkpoint, taken })),
    expected,
  );
  assert.deepEqual(expected, [
    { checkpoint: "example", taken: "2" },
    { checkpoint: "repeat", taken: "1" },
  ]);
  assert.equal(
    result.sent.at(-1).checkpoint,
    undefined,
    "the board itself last",
  );
});
