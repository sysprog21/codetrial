// Keep complete diagnostic lines before the browser harness trims its stderr tail.
const fs = require("node:fs");
const { StringDecoder } = require("node:string_decoder");

const PREFIXES = {
  floor: ["codetrial floor "],
  latency: [
    "codetrial latency ",
    "codetrial replay_gate ",
    "interview clock: ",
    "replacement start: ",
    "replacement end: ",
  ],
};
const LINE_BYTES = 4096;

class DiagnosticCapture {
  constructor(path, family = "floor") {
    if (!Object.hasOwn(PREFIXES, family))
      throw new Error("unknown diagnostic family");
    this.path = path;
    this.family = family;
    this.prefixes = PREFIXES[family];
    this.pending = "";
    this.pendingBytes = 0;
    this.tail = "";
    this.diagnostic = false;
    this.decoder = new StringDecoder("utf8");
    this.discarding = false;
    this.fd = null;
    this.closed = false;
    this.needsSeparator = false;
  }

  write(line) {
    if (this.closed) throw new Error("diagnostic capture is closed");
    this.fd ??= fs.openSync(this.path, "a", 0o600);
    const bytes = Buffer.from((this.needsSeparator ? "\n" : "") + line);
    try {
      if (fs.writeSync(this.fd, bytes) !== bytes.length)
        throw new Error("short diagnostic write");
      this.needsSeparator = false;
    } catch (error) {
      this.needsSeparator = true;
      throw error;
    }
  }

  incomplete(reason) {
    if (!/^[a-z][a-z0-9-]{0,63}$/.test(reason))
      throw new Error("invalid diagnostic failure category");
    this.write(
      `codetrial capture_incomplete family=${this.family} reason=${reason}\n`,
    );
  }

  push(chunk) {
    if (this.closed) throw new Error("diagnostic capture is closed");
    const text = Buffer.isBuffer(chunk) ? this.decoder.write(chunk) : chunk;
    for (const part of text.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
      const complete = part.endsWith("\n");
      const scanned = this.tail + part;
      this.diagnostic ||= this.prefixes.some((prefix) =>
        scanned.includes(prefix),
      );
      this.tail = scanned.slice(-32);
      if (!this.discarding) {
        this.pendingBytes += Buffer.byteLength(part);
        if (this.pendingBytes > LINE_BYTES) {
          this.discarding = true;
          this.pending = "";
        } else this.pending += part;
      }
      if (!complete) continue;
      try {
        if (this.diagnostic) {
          if (this.discarding) this.incomplete("oversized-line");
          else if (
            !this.prefixes.some((prefix) => this.pending.startsWith(prefix))
          )
            this.incomplete("unframed-line");
          else if (this.pending.includes("\ufffd"))
            this.incomplete("invalid-utf8");
          else this.write(this.pending);
        }
      } finally {
        this.resetLine();
      }
    }
  }

  resetLine() {
    this.pending = "";
    this.pendingBytes = 0;
    this.tail = "";
    this.diagnostic = false;
    this.discarding = false;
  }

  discontinuity(reason) {
    try {
      this.incomplete(reason);
    } finally {
      this.resetLine();
      this.decoder = new StringDecoder("utf8");
    }
  }

  close() {
    if (this.closed) return;
    try {
      this.push(this.decoder.end());
      if (
        this.diagnostic ||
        (this.pending.length > 0 &&
          this.prefixes.some((prefix) => prefix.startsWith(this.pending)))
      )
        this.incomplete(
          this.discarding ? "oversized-line" : "unterminated-line",
        );
    } finally {
      this.closed = true;
      if (this.fd !== null) fs.closeSync(this.fd);
      this.fd = null;
    }
  }
}

// Each pipe has its own line buffer. Completion includes every close/error
// path, so the caller can await drainage after stopping its own child group.
function captureDiagnosticStream(stream, paths) {
  const captures = Object.entries(paths)
    .filter(([, path]) => path)
    .map(([family, path]) => new DiagnosticCapture(path, family));
  let failure = null;
  const fail = (reason) => {
    failure ??= reason;
    for (const capture of captures) {
      if (capture.closed) continue;
      try {
        capture.incomplete(reason);
      } catch {
        failure = "write-failed";
      }
    }
  };
  const closed = new Promise((resolve) => {
    stream.on("data", (chunk) => {
      if (failure) return;
      try {
        for (const capture of captures) capture.push(chunk);
      } catch {
        fail("write-failed");
      }
    });
    stream.once("error", () => fail("stream-error"));
    stream.once("close", () => {
      for (const capture of captures) {
        try {
          capture.close();
        } catch {
          failure = "write-failed";
        }
      }
      resolve(failure);
    });
  });
  return { closed, incomplete: fail };
}

async function drainDiagnosticStreams(streams, timeoutMs = 5000) {
  if (!streams.length) return;
  let timer;
  try {
    const result = await Promise.race([
      Promise.all(streams.map((stream) => stream.closed)),
      new Promise((resolve) => {
        timer = setTimeout(() => resolve(null), timeoutMs);
      }),
    ]);
    if (result === null) {
      for (const stream of streams) stream.incomplete("drain-timeout");
      throw new Error(
        "agent diagnostic pipes did not close before the drain deadline",
      );
    }
    if (result.some(Boolean))
      throw new Error("agent diagnostic capture failed");
  } finally {
    clearTimeout(timer);
  }
}

function readFloorRecords(path) {
  return fs
    .readFileSync(path, "utf8")
    .split("\n")
    .slice(0, -1)
    .filter((line) => line.startsWith("codetrial floor "))
    .map((line) =>
      Object.fromEntries(
        line
          .split(" ")
          .slice(2)
          .map((field) => field.split("=")),
      ),
    );
}

function assertInterruptedOverlap(path, room) {
  if (
    !readFloorRecords(path).some(
      (fields) =>
        fields.room === room &&
        fields.kind === "overlap" &&
        fields.outcome === "interrupted" &&
        /^\d+$/.test(fields.overlap_ms),
    )
  ) {
    throw new Error(
      "barge replay has no measured interrupted floor overlap; missing or misaligned activity",
    );
  }
}

module.exports = {
  PREFIXES,
  DiagnosticCapture,
  captureDiagnosticStream,
  drainDiagnosticStreams,
  assertInterruptedOverlap,
  readFloorRecords,
};

// The server owns dispatched-agent stderr. Follow from byte zero until cleanup,
// including the bytes written after the browser exits or reports a failure.
if (require.main === module) {
  const [source, target, doneFile, latencyTarget] = process.argv.slice(2);
  const captures = [new DiagnosticCapture(target)];
  if (latencyTarget)
    captures.push(new DiagnosticCapture(latencyTarget, "latency"));
  let offset = 0;
  let identity = null;
  let missingReported = false;
  const parentPid = process.ppid;
  let stopDeadline = null;
  const buffer = Buffer.alloc(65536);
  function poll(final = false) {
    let fd;
    try {
      fd = fs.openSync(source, "r");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      if (!missingReported && (identity !== null || final)) {
        for (const capture of captures) capture.discontinuity("source-missing");
        missingReported = true;
      }
      return;
    }
    try {
      const stat = fs.fstatSync(fd);
      const nextIdentity = `${stat.dev}:${stat.ino}`;
      if (
        (identity !== null && identity !== nextIdentity) ||
        stat.size < offset
      ) {
        for (const capture of captures)
          capture.discontinuity("source-replaced-or-truncated");
        offset = 0;
      }
      identity = nextIdentity;
      let count;
      while ((count = fs.readSync(fd, buffer, 0, buffer.length, offset))) {
        offset += count;
        for (const capture of captures) capture.push(buffer.subarray(0, count));
      }
    } finally {
      fs.closeSync(fd);
    }
  }
  const timer = setInterval(() => {
    poll();
    if (doneFile && fs.existsSync(doneFile)) finish();
    else if (process.ppid !== parentPid) finish("parent-lost");
    else if (stopDeadline !== null && Date.now() >= stopDeadline)
      finish("drain-timeout");
  }, 50);
  function finish(reason) {
    clearInterval(timer);
    poll(true);
    for (const capture of captures) {
      if (reason) capture.incomplete(reason);
      capture.close();
    }
    process.exit(reason ? 1 : 0);
  }
  function interrupted() {
    if (!doneFile) return finish();
    // A group signal also reaches the server. Let shell cleanup wait for that
    // server before declaring capture complete; bound an abandoned cleanup.
    stopDeadline ??= Date.now() + 5000;
  }
  process.on("SIGTERM", interrupted);
  process.on("SIGINT", interrupted);
  poll();
}
