// Joins sampled clocks only; transport delay and provider activity are separate evidence.
class ReplayClockError extends Error {
  constructor(reason) {
    super(`replay clock: ${reason}`);
    this.reason = reason;
  }
}

function requireValue(condition, reason) {
  if (!condition) throw new ReplayClockError(reason);
}

function number(value) {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    Math.abs(value) <= Number.MAX_SAFE_INTEGER
  );
}

function whole(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function intersect(intervals) {
  const low = Math.max(...intervals.map((interval) => interval.low));
  const high = Math.min(...intervals.map((interval) => interval.high));
  requireValue(low <= high, "clock-drift");
  return { low, high };
}

function interval(center, uncertainty) {
  const low = center - uncertainty;
  const high = center + uncertainty;
  requireValue(number(low) && number(high), "unsafe-clock-range");
  return { low, high };
}

function clockId(value) {
  return typeof value === "string" && /^[a-zA-Z0-9:._-]{1,256}$/.test(value);
}

function hull(intervals) {
  return {
    low: Math.min(...intervals.map((item) => item.low)),
    high: Math.max(...intervals.map((item) => item.high)),
  };
}

function createClockJoin({ harness, provider, maxUncertaintyMs }) {
  requireValue(
    number(maxUncertaintyMs) &&
      maxUncertaintyMs > 0 &&
      maxUncertaintyMs <= 5000,
    "invalid-uncertainty-limit",
  );
  for (const samples of [harness, provider]) {
    requireValue(
      Array.isArray(samples) && samples.length >= 2 && samples.length <= 4096,
      "missing-or-excessive-samples",
    );
  }
  const harnessId = harness[0]?.clock_id;
  const providerId = provider[0]?.clock_id;
  requireValue(
    clockId(harnessId) && clockId(providerId),
    "missing-clock-identity",
  );
  requireValue(harnessId !== providerId, "mixed-clock-domain");
  const local = harness.map((sample) => {
    requireValue(
      sample &&
        sample.clock_id === harnessId &&
        number(sample.mono_ms) &&
        sample.mono_ms >= 0 &&
        whole(sample.unix_ms) &&
        number(sample.uncertainty_ms) &&
        sample.uncertainty_ms >= 1 &&
        sample.uncertainty_ms <= maxUncertaintyMs,
      "invalid-harness-sample",
    );
    return interval(sample.unix_ms - sample.mono_ms, sample.uncertainty_ms);
  });
  const start = provider[0]?.start_unix_ms;
  requireValue(whole(start), "missing-provider-anchor");
  const remote = provider.flatMap((sample) => {
    requireValue(
      sample &&
        sample.clock_id === providerId &&
        sample.start_unix_ms === start &&
        whole(sample.emitted_ms) &&
        whole(sample.emitted_unix_ms) &&
        whole(sample.clock_uncertainty_ms) &&
        sample.clock_uncertainty_ms >= 2 &&
        sample.clock_uncertainty_ms <= maxUncertaintyMs,
      "invalid-provider-sample",
    );
    return [
      interval(
        sample.emitted_unix_ms - sample.emitted_ms,
        sample.clock_uncertainty_ms,
      ),
    ];
  });
  for (let i = 1; i < harness.length; i++) {
    requireValue(
      harness[i].mono_ms > harness[i - 1].mono_ms,
      "unordered-harness-samples",
    );
  }
  const firstRemote = Math.min(...provider.map((sample) => sample.emitted_ms));
  const lastRemote = Math.max(...provider.map((sample) => sample.emitted_ms));
  requireValue(firstRemote < lastRemote, "unbracketed-provider-clock");
  // Intersections detect inconsistency; their narrowing is not extra timing precision.
  intersect(local);
  intersect(remote);
  const h = hull(local);
  const p = hull(remote);
  const delta = { low: p.low - h.high, high: p.high - h.low };
  requireValue(number(delta.low) && number(delta.high), "unsafe-clock-range");
  const uncertainty = delta.high - delta.low + 1;
  requireValue(uncertainty <= maxUncertaintyMs, "excessive-clock-uncertainty");
  const firstHarness = harness[0].mono_ms;
  const lastHarness = harness.at(-1).mono_ms;
  return {
    clock_id: harnessId,
    provider_clock_id: providerId,
    uncertainty_ms: uncertainty,
    providerWindow(window) {
      requireValue(
        window &&
          window.clock_id === harnessId &&
          number(window.low) &&
          number(window.high) &&
          window.low < window.high &&
          window.low >= firstHarness &&
          window.high <= lastHarness,
        "invalid-harness-window",
      );
      requireValue(
        window.low - uncertainty >= firstHarness &&
          window.high + uncertainty <= lastHarness,
        "window-near-harness-boundary",
      );
      const result = {
        low: window.low - delta.high - 1,
        high: window.high - delta.low,
      };
      requireValue(
        number(result.low) &&
          number(result.high) &&
          result.low >= firstRemote &&
          result.high <= lastRemote,
        "window-outside-provider-samples",
      );
      return result;
    },
    project(eventMs, eventClockId) {
      requireValue(eventClockId === providerId, "event-clock-mismatch");
      requireValue(whole(eventMs), "invalid-event-time");
      requireValue(
        eventMs >= firstRemote && eventMs <= lastRemote,
        "event-outside-provider-samples",
      );
      const result = {
        low: eventMs + delta.low,
        high: eventMs + 1 + delta.high,
      };
      requireValue(
        number(result.low) && number(result.high),
        "unsafe-clock-range",
      );
      requireValue(
        result.low >= firstHarness && result.high <= lastHarness,
        "event-outside-harness-samples",
      );
      return result;
    },
  };
}

module.exports = { createClockJoin, ReplayClockError };
