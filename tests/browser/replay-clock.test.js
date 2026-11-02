import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const {
  createClockJoin,
  ReplayClockError,
} = require("../../scripts/replay-clock.cjs");
const local = (mono_ms, unix_ms = 100000 + mono_ms, uncertainty_ms = 1) => ({
  clock_id: "node:test",
  mono_ms,
  unix_ms,
  uncertainty_ms,
});
const remote = (emitted_ms, extra = {}) => ({
  clock_id: "producer:test",
  start_unix_ms: 100000,
  emitted_ms,
  emitted_unix_ms: 100000 + emitted_ms,
  clock_uncertainty_ms: 2,
  ...extra,
});
function inputs() {
  return {
    harness: [local(0), local(3000)],
    provider: [remote(0), remote(2500)],
    maxUncertaintyMs: 10,
  };
}
function rejects(args, reason) {
  assert.throws(
    () => createClockJoin(args),
    (e) => e instanceof ReplayClockError && e.reason === reason,
  );
}
const project = (join, ms) => join.project(ms, "producer:test");
test("joins the event clock independent of delayed emission, retaining millisecond quantization", () => {
  const args = inputs();
  args.provider = [remote(2500), remote(0), remote(2000)];
  const join = createClockJoin(args);
  assert.deepEqual(project(join, 1000), { low: 997, high: 1004 });
  assert.equal(join.uncertainty_ms, 7);
});
test("same timestamp in another monotonic clock maps through independent wall samples", () => {
  const args = inputs();
  args.harness = [local(500, 100000), local(3500, 103000)];
  assert.deepEqual(project(createClockJoin(args), 1000), {
    low: 1497,
    high: 1504,
  });
});
test("detects local sampled drift and refuses mixed anchors without enforcing an old wall offset", () => {
  let args = inputs();
  args.harness[1].unix_ms += 3;
  rejects(args, "clock-drift");
  args = inputs();
  args.provider[1].emitted_unix_ms += 5;
  rejects(args, "clock-drift");
  args = inputs();
  args.provider[1].start_unix_ms += 1;
  rejects(args, "invalid-provider-sample");
  args = inputs();
  for (const sample of args.provider) sample.emitted_unix_ms += 100;
  assert.deepEqual(project(createClockJoin(args), 1000), {
    low: 1097,
    high: 1104,
  });
});
test("uses a conservative hull rather than false precision from intersecting samples", () => {
  const args = inputs();
  args.harness[1].unix_ms += 2;
  assert.deepEqual(project(createClockJoin(args), 1000), {
    low: 995,
    high: 1004,
  });
  args.harness[1].unix_ms++;
  rejects(args, "clock-drift");
});
test("full uncertainty width includes event quantization and is held to the configured limit", () => {
  const args = inputs();
  args.maxUncertaintyMs = 7;
  assert.equal(createClockJoin(args).uncertainty_ms, 7);
  args.maxUncertaintyMs = 6;
  rejects(args, "excessive-clock-uncertainty");
});
test("does not extrapolate beyond either sampled clock", () => {
  const join = createClockJoin(inputs());
  assert.throws(() => project(join, 2501), /event-outside-provider-samples/);
  assert.throws(() => project(join, 0), /event-outside-harness-samples/);
  assert.throws(() => project(join, -1), /invalid-event-time/);
  const args = inputs();
  args.provider = [remote(0), remote(4000)];
  assert.throws(
    () => project(createClockJoin(args), 3000),
    /event-outside-harness-samples/,
  );
});
test("refuses absent, duplicate-only, excessive and unordered sampling", () => {
  let args = inputs();
  args.harness = [];
  rejects(args, "missing-or-excessive-samples");
  args = inputs();
  args.provider = [remote(0), remote(0)];
  rejects(args, "unbracketed-provider-clock");
  args = inputs();
  args.harness.reverse();
  rejects(args, "unordered-harness-samples");
  args = inputs();
  args.provider = Array.from({ length: 4097 }, (_, i) => remote(i));
  rejects(args, "missing-or-excessive-samples");
});
test("missing/null/coerced and unsafe numbers cannot become clock anchors", () => {
  for (const value of [
    null,
    undefined,
    "100000",
    NaN,
    Infinity,
    -1,
    Number.MAX_SAFE_INTEGER + 1,
  ]) {
    let args = inputs();
    args.provider[0].start_unix_ms = value;
    rejects(args, "missing-provider-anchor");
    args = inputs();
    args.harness[0].unix_ms = value;
    rejects(args, "invalid-harness-sample");
  }
  for (const value of [null, "1", NaN, Infinity, -1, 5001]) {
    const args = inputs();
    args.maxUncertaintyMs = value;
    rejects(args, "invalid-uncertainty-limit");
  }
});
test("harness/page and producer identities cannot be mixed or omitted", () => {
  let args = inputs();
  delete args.harness[0].clock_id;
  rejects(args, "missing-clock-identity");
  args = inputs();
  args.harness[1].clock_id = "page:test";
  rejects(args, "invalid-harness-sample");
  args = inputs();
  args.provider[1].clock_id = "producer:other";
  rejects(args, "invalid-provider-sample");
  const join = createClockJoin(inputs());
  assert.throws(() => join.project(1000, "page:test"), /event-clock-mismatch/);
  assert.throws(() => join.project(1000), /event-clock-mismatch/);
});
test("a join snapshots its bracketing bounds rather than retaining mutable caller arrays", () => {
  const args = inputs();
  const join = createClockJoin(args);
  args.harness[0].mono_ms = 2000;
  args.harness.at(-1).mono_ms = 100;
  assert.deepEqual(project(join, 1000), { low: 997, high: 1004 });
});

test("inverse windows include uncertainty margins at both harness bracket edges", () => {
  const args = inputs();
  args.provider = [remote(0), remote(4000)];
  const join = createClockJoin(args);
  assert.throws(
    () => join.providerWindow({ clock_id: "node:test", low: 10, high: 2995 }),
    /window-near-harness-boundary/,
  );
  assert.throws(
    () => join.providerWindow({ clock_id: "node:test", low: 6, high: 2000 }),
    /window-near-harness-boundary/,
  );
  const bounds = join.providerWindow({
    clock_id: "node:test",
    low: 7,
    high: 2993,
  });
  assert.deepEqual(bounds, { low: 3, high: 2996 });
  assert.deepEqual(project(join, bounds.low), { low: 0, high: 7 });
  assert.deepEqual(project(join, bounds.high), { low: 2993, high: 3000 });
  assert.deepEqual(
    join.providerWindow({ clock_id: "node:test", low: 1000.5, high: 2000.5 }),
    { low: 996.5, high: 2003.5 },
  );
});

test("inverse windows reject invalid bounds or a substituted clock domain", () => {
  const join = createClockJoin(inputs());
  for (const window of [
    { clock_id: "page:test", low: 1000, high: 2000 },
    { clock_id: "node:test", low: -1, high: 2000 },
    { clock_id: "node:test", low: NaN, high: 2000 },
    { clock_id: "node:test", low: 2000, high: 1000 },
    { clock_id: "node:test", low: 1000, high: Infinity },
  ])
    assert.throws(() => join.providerWindow(window), /invalid-harness-window/);
  const args = inputs();
  args.provider.forEach((sample) => {
    sample.clock_id = "node:test";
  });
  rejects(args, "mixed-clock-domain");
  const missingRounding = inputs();
  missingRounding.harness[0].uncertainty_ms = 0;
  rejects(missingRounding, "invalid-harness-sample");
  const missingProviderRounding = inputs();
  missingProviderRounding.provider[0].clock_uncertainty_ms = 1;
  rejects(missingProviderRounding, "invalid-provider-sample");
});
