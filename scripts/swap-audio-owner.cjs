"use strict";

// The one owner of the decoded-audio marker and its settle, shared by the soak,
// voice and long-interview flows. It lives apart from browser-check.cjs so its
// ordering, cancellation and page-failure behavior can be driven with a page
// that is only an object with `evaluate`.

const { installedSwapKeys, judgeSwapAudio } = require("./soak-judge.cjs");

const POLL_MS = 250;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/// Marks each installed swap in the page once, as soon as its end line is
/// read, so the sample that answers it can only open on a Speaking transition
/// after that point. Read late, never early: a transition between the install
/// and this call is missed and the next one is used instead, which can fail a
/// swap whose reply was quicker than a poll but never passes one on audio from
/// before it. It runs on its own clock, not the callers' loops: those wait on
/// log parsing and room-service calls, and a swap read late can miss the
/// transition that answered it. `halt` ends the loop on a failure path;
/// `stop` ends it, rethrows a marking failure and marks what is left.
///
/// `newLines` returns the log gained since its last call and `agentOutput` is
/// the agent's retained output; the swaps are read from both. `keysOf` and
/// `wait` are the real parser and timer unless a caller says otherwise.
function startSwapMarker({
  page,
  roomName,
  agentOutput,
  newLines,
  keysOf = (text) => installedSwapKeys(text, roomName),
  wait = sleep,
  pollMs = POLL_MS,
}) {
  const marked = new Set();
  let marking = true;
  let markError = null;
  const markNewSwaps = async () => {
    for (const key of keysOf(`${agentOutput.join("")}\n${newLines()}`)) {
      if (marked.has(key)) continue;
      marked.add(key);
      await page.evaluate((swap) => globalThis.__markJimSwap?.(swap), key);
    }
  };
  const loop = (async () => {
    while (marking) {
      await wait(pollMs);
      if (!marking) break;
      try {
        await markNewSwaps();
      } catch (error) {
        markError = error;
        marking = false;
      }
    }
  })();
  const halt = async () => {
    marking = false;
    await loop;
  };
  return {
    halt,
    async stop() {
      await halt();
      if (markError) throw markError;
      await markNewSwaps();
    },
  };
}

/// The decoded audio each answered swap owes: the page is given the rest of
/// the grace for the first Speaking transition after the last swap, then
/// cancels what still waits, and the result is judged against the episodes.
async function judgeSettledSwapAudio(page, replies, label, { graceMs, log }) {
  const rearms = await page.evaluate(
    (waitMs) => globalThis.__settleJimSwapAudio(waitMs),
    graceMs,
  );
  const audio = judgeSwapAudio(replies, rearms);
  for (const e of audio.episodes) {
    log(
      `${label} episode ${e.episode} audio: ${e.status} ${e.reasons.join("; ")}`.trimEnd(),
    );
  }
  return audio;
}

module.exports = { judgeSettledSwapAudio, startSwapMarker };
