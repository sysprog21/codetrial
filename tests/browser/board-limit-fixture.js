import assert from "node:assert/strict";
import { MAX_STROKES, MAX_STROKES_MESSAGE } from "../../web/whiteboard.js";

// Stroke limit refusal announcement, frame cancellation, and message removal.
// Both Example drawing and whiteboard run this shared plan via limitProbe.
export async function exerciseLimitAnnouncement(
  page,
  { sharedStatus = false } = {},
) {
  // One evaluate is one task: calls named together cannot be split by a frame.
  const run = (...calls) =>
    page.evaluate((calls) => {
      let last;
      for (const [name, ...args] of calls)
        last = globalThis.limitProbe[name](...args);
      return last;
    }, calls);
  // Two frames, so a frame queued before the call has run.
  const frames = () =>
    page.evaluate(
      () =>
        new Promise((done, fail) => {
          const timer = setTimeout(
            () => fail(new Error("probe: frame timeout")),
            2000,
          );
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              clearTimeout(timer);
              done();
            }),
          );
        }),
    );
  const text = () =>
    page.evaluate(() => globalThis.limitProbe.status.textContent);
  const count = () => run(["count"]);
  const updates = () => page.evaluate(() => globalThis.limitProbe.updates);
  const full = async (why) =>
    assert.equal(await count(), MAX_STROKES, `probe: ${why}`);
  // Every refusal after the first has to be a change a screen reader hears.
  const announcedAgain = async (what) => {
    await page.evaluate(() => (globalThis.limitProbe.updates = []));
    await run(["refuse"]);
    await frames();
    assert.deepEqual(
      await updates(),
      ["", MAX_STROKES_MESSAGE],
      `${what}: a repeated refusal must empty the line, then set the message`,
    );
  };

  // The first refusal is announced.
  await run(["fill"]);
  await full("the board is full");
  await run(["refuse"]);
  await frames();
  assert.equal(
    await text(),
    MAX_STROKES_MESSAGE,
    "pen: the first refusal is announced",
  );

  // The same text written again is not a change, so a repeated refusal must
  // empty the line and then set it, as two updates.
  await page.evaluate(() => {
    const probe = globalThis.limitProbe;
    probe.updates = [];
    new MutationObserver(() =>
      probe.updates.push(probe.status.textContent),
    ).observe(probe.status, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  });
  await announcedAgain("pen");

  // The eraser is refused by the same full board, and a screen-reader user
  // reaches for it as readily as for the pen.
  await run(["useTool", "eraser"]);
  await announcedAgain("eraser");
  await run(["useTool", "pen"]);

  // Repainting a board that is still full is no reason to take it back.
  await run(["repaint"]);
  await frames();
  assert.equal(
    await text(),
    MAX_STROKES_MESSAGE,
    "repaint: a board that is still full keeps the message",
  );

  // Undo after the message is displayed: taking away the reason clears it.
  await run(["makeRoom"]);
  assert.equal(await count(), MAX_STROKES - 1, "probe: Undo made room");
  await frames();
  assert.equal(
    await text(),
    "",
    "Undo: a displayed limit message is cleared when room is made",
  );

  // Undo in the same task as a refusal: cancelling the pending frame keeps the
  // message away when the slot is filled again before the frame runs. A second
  // refusal in the same task verifies that the first frame was cancelled.
  await run(["fill"]);
  await full("the board is full again before pending Undo");
  await run(["refuse"], ["refuse"], ["makeRoom"], ["fill"]);
  assert.equal(await count(), MAX_STROKES, "probe: slot filled again");
  await frames();
  assert.equal(
    await text(),
    "",
    "Undo: the pending message is cancelled and the line stays empty",
  );

  // Clear is the other edit that makes room. Filling the board again in the
  // same task leaves only the cancelled frame to keep the message away.
  await run(["fill"]);
  await full("the board is full again before Clear");
  await run(["refuse"], ["clear"], ["fill"]);
  assert.equal(await count(), MAX_STROKES, "probe: slot filled again");
  await frames();
  assert.equal(
    await text(),
    "",
    "Clear: the pending message is cancelled and the line stays empty",
  );

  // A lock after the message is displayed clears it.
  await run(["fill"]);
  await full("the board is full again before displayed lock");
  await run(["refuse"]);
  await frames();
  assert.equal(
    await text(),
    MAX_STROKES_MESSAGE,
    "probe: limit message displayed before lock",
  );
  await run(["lock", true], ["repaint"]);
  await frames();
  assert.equal(
    await text(),
    "",
    "lock: a displayed limit message is cleared when the board is locked",
  );
  await run(["lock", false]);

  // A lock in the same task as a refusal cancels the pending frame. Unlocking
  // in that task too leaves only the cancelled frame to keep the message away.
  await run(["fill"]);
  await full("the board is full again before pending lock");
  await run(["refuse"], ["lock", true], ["repaint"], ["lock", false]);
  await frames();
  assert.equal(
    await text(),
    "",
    "lock: the pending message is cancelled and stays away after unlocking",
  );

  // A line that also reports other things keeps them: only the limit message
  // is the board's to take back.
  if (sharedStatus) {
    await run(["makeRoom"]);
    assert.equal(await count(), MAX_STROKES - 1, "probe: Undo made room");
    const sync = "Drawing kept here; it will sync when connected.";
    await run(["setStatus", sync], ["repaint"]);
    await frames();
    assert.equal(
      await text(),
      sync,
      "shared status: a message that is not the limit message is left alone",
    );

    // A sync message arriving before the frame runs is preserved.
    await run(["fill"]);
    await full("the board is full again before sync override");
    await run(["refuse"], ["setStatus", sync]);
    await frames();
    assert.equal(
      await text(),
      sync,
      "shared status: a sync message written before the frame is preserved",
    );

    // A host lock without a repaint drops the pending limit message. Only the
    // Example drawing's frame checks `locked()`; the whiteboard's does not.
    await run(["fill"]);
    await full("the board is full again before host lock");
    await run(["refuse"], ["setLocked", true]);
    await frames();
    assert.equal(
      await text(),
      "",
      "lock: a host lock without repaint drops the pending limit message",
    );
    await run(["setLocked", false]);

    // Finishing the board in the same task drops the pending limit message.
    await run(["fill"]);
    await full("the board is full again before finish");
    await run(["flush"]);
    await run(["refuse"], ["finish"]);
    await frames();
    assert.equal(
      await text(),
      "",
      "finish: a stopped board drops the pending limit message",
    );
  }
}
