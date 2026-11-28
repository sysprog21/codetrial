// Waits for browser and process tests that name what they wait for.
//
// A test that bounds a wait by counting frames or by a short wall-clock
// deadline measures how busy the runner is rather than the page: on a loaded
// CI runner frames arrive seconds apart and a 140 ms transition "does not
// settle" inside 1500 ms. These helpers wait for the observable state and keep
// the timeout only as the backstop that turns a hang into a named failure, so
// it is generous enough that only a real hang reaches it.

const BACKSTOP_MS = 30000;

// Races `work` against the backstop, which names what was being waited for: a
// bare timeout never does. The timer is cleared whichever side wins.
export function withBackstop(work, what) {
  let timer = null;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(
      () =>
        reject(
          new Error(`timed out after ${BACKSTOP_MS} ms waiting for ${what}`),
        ),
      BACKSTOP_MS,
    );
  });
  return Promise.race([work, expired]).finally(() => clearTimeout(timer));
}

// Polls `probe` until it returns something truthy. The backstop bounds the
// whole wait, a probe that never settles included.
export async function until(probe, what) {
  let stopped = false;
  const poll = async () => {
    while (!stopped && !(await probe())) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  };
  try {
    await withBackstop(poll(), what);
  } finally {
    stopped = true;
  }
}

// Resolves once every CSS transition and animation running on the locator's
// element has finished or been cancelled, and a frame has followed. Reading the
// computed style first flushes pending style so a transition the caller just
// triggered has started, and the wait is on the animations themselves, not on
// how often frames are delivered. The backstop covers the final frame too.
export async function animationsFinished(locator, what) {
  await withBackstop(
    locator.evaluate(async (el) => {
      // eslint-disable-next-line no-unused-expressions -- the read is the flush
      getComputedStyle(el).transform;
      await Promise.all(
        el
          .getAnimations()
          .map((animation) => animation.finished.catch(() => {})),
      );
      await new Promise(requestAnimationFrame);
    }),
    what,
  );
}
