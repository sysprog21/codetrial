// The browser worker's half of the run budget protocol in
// scripts/interview-run-budget.py.
//
// A scripted action asks the launcher for a permit before it is dispatched and
// does nothing when the answer is anything but an exact grant. The guard never
// retries: a request whose reply was lost may have been granted, and
// dispatching on a guess is how a budget is overrun. The count lives in the
// launcher, so a page reload, or a fresh guard in a restarted worker, asks the
// launcher where the run stands instead of starting again from one.
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const net = require("net");

const PROTOCOL_VERSION = 1;
const PERMIT_TIMEOUT_MS = 1000;
const ENV_NAMES = [
  "BROWSER_CHECK_RUN_MANIFEST",
  "BROWSER_CHECK_RUN_ID",
  "BROWSER_CHECK_RUN_MANIFEST_SHA256",
  "BROWSER_CHECK_BUDGET_SOCKET",
];

class BudgetDenied extends Error {}

function exchange(socketPath, request, timeoutMs = PERMIT_TIMEOUT_MS) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let data = "";
    const conn = net.createConnection(socketPath);
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      conn.destroy();
      if (error) reject(error);
      else resolve(value);
    };
    const timer = setTimeout(
      () => finish(new BudgetDenied("no reply within the permit timeout")),
      timeoutMs,
    );
    conn.on("connect", () => conn.write(JSON.stringify(request) + "\n"));
    conn.on("data", (block) => {
      data += block.toString("utf8");
      const end = data.indexOf("\n");
      if (end < 0) return;
      try {
        const reply = JSON.parse(data.slice(0, end));
        if (
          reply === null ||
          typeof reply !== "object" ||
          reply.v !== PROTOCOL_VERSION
        )
          throw new Error("shape");
        finish(null, reply);
      } catch {
        finish(new BudgetDenied("malformed reply"));
      }
    });
    conn.on("error", (error) =>
      finish(new BudgetDenied(`launcher unreachable: ${error.code}`)),
    );
    conn.on("close", () =>
      finish(new BudgetDenied("connection closed before a reply")),
    );
  });
}

class RunBudget {
  constructor(socketPath, runId, digest) {
    this.socketPath = socketPath;
    this.runId = runId;
    this.digest = digest;
    this.dead = null;
    this.nextSeq = null;
    // One permit at a time: two overlapping asks would carry the same sequence
    // and the second would drain the run.
    this.queue = Promise.resolve();
  }

  call(request) {
    return exchange(this.socketPath, {
      ...request,
      v: PROTOCOL_VERSION,
      run_id: this.runId,
      manifest_sha256: this.digest,
    });
  }

  // Joins the run at the launcher's count, which is what a reload relies on.
  async sync() {
    const status = await this.call({ op: "status" });
    if (status.ok !== true || !Number.isInteger(status.count)) {
      this.dead = "unusable status reply";
      throw new BudgetDenied(this.dead);
    }
    this.nextSeq = status.count + 1;
    return this;
  }

  async launcherAlive() {
    try {
      return (await this.call({ op: "ping" })).ok === true;
    } catch {
      return false;
    }
  }

  permit(actionId) {
    const turn = this.queue.then(() => this.permitNow(actionId));
    this.queue = turn.catch(() => {});
    return turn;
  }

  async permitNow(actionId) {
    if (this.dead) throw new BudgetDenied(`guard is dead: ${this.dead}`);
    const seq = this.nextSeq;
    try {
      const reply = await this.call({
        op: "permit",
        action_id: actionId,
        seq,
      });
      if (reply.granted !== true) {
        throw new BudgetDenied(String(reply.reason || "denied"));
      }
      if (reply.action_id !== actionId || reply.seq !== seq) {
        throw new BudgetDenied("reply does not match the request");
      }
    } catch (error) {
      this.dead = error.message;
      throw error;
    }
    this.nextSeq = seq + 1;
  }

  // The permit comes first; nothing is dispatched on a denial.
  async guarded(actionId, dispatch) {
    await this.permit(actionId);
    return dispatch();
  }
}

// Null when the run has no budget, so a plain check is untouched. Anything
// short of the full set of variables, or a manifest that is not the one the
// launcher hashed, throws before the worker dispatches anything.
async function loadRunBudget(env = process.env) {
  const present = ENV_NAMES.filter((name) => env[name]);
  if (present.length === 0) return null;
  if (present.length !== ENV_NAMES.length) {
    const missing = ENV_NAMES.filter((name) => !env[name]);
    throw new BudgetDenied(`run budget is incomplete, missing ${missing}`);
  }
  let raw;
  try {
    raw = fs.readFileSync(env.BROWSER_CHECK_RUN_MANIFEST);
  } catch (error) {
    throw new BudgetDenied(`cannot read the run manifest: ${error.code}`);
  }
  const digest = crypto.createHash("sha256").update(raw).digest("hex");
  if (digest !== env.BROWSER_CHECK_RUN_MANIFEST_SHA256) {
    throw new BudgetDenied("manifest hash does not match the launcher's");
  }
  let manifest;
  try {
    manifest = JSON.parse(raw.toString("utf8"));
  } catch {
    throw new BudgetDenied("the run manifest is not JSON");
  }
  if (manifest.run_id !== env.BROWSER_CHECK_RUN_ID) {
    throw new BudgetDenied("run ID does not match the manifest");
  }
  const budget = new RunBudget(
    env.BROWSER_CHECK_BUDGET_SOCKET,
    manifest.run_id,
    digest,
  );
  budget.actions = manifest.actions;
  budget.fixtureSha256 = manifest.fixture_sha256;
  return budget.sync();
}

/// Calls `onLost` once the launcher has missed `misses` pings in a row. One
/// slow reply is load, not loss: the ping shares the permit lock with the join
/// re-hash and its fsync. Returns the timer, which the caller may unref.
function watchLauncher(budget, { onLost, everyMs = 3000, misses = 2 }) {
  let missed = 0;
  let lost = false;
  const timer = setInterval(async () => {
    if (lost) return;
    missed = (await budget.launcherAlive()) ? 0 : missed + 1;
    if (missed < misses) return;
    lost = true;
    clearInterval(timer);
    await onLost();
  }, everyMs);
  return timer;
}

module.exports = { BudgetDenied, RunBudget, loadRunBudget, watchLauncher };
