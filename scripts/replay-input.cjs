const fs = require("node:fs");
const { TextDecoder } = require("node:util");

class ReplayInputError extends Error {
  constructor(message, reason) {
    super(message);
    this.reason = reason;
  }
}

function readReplayFile(filename, limit, label) {
  const fail = (message, reason) => {
    throw new ReplayInputError(`${label}: ${message}`, reason);
  };
  let fd;
  try {
    fd = fs.openSync(
      filename,
      fs.constants.O_RDONLY |
        (fs.constants.O_NOFOLLOW ?? 0) |
        (fs.constants.O_NONBLOCK ?? 0),
    );
    const before = fs.fstatSync(fd, { bigint: true });
    if (!before.isFile())
      fail("input is not a bounded regular file", "invalid-input-file");
    if (before.size > BigInt(limit))
      fail("input exceeds byte limit", "input-limit");
    // One spare byte detects growth without zero-filling the whole limit.
    const bytes = Buffer.alloc(Number(before.size) + 1);
    let size = 0;
    while (size < bytes.length) {
      const count = fs.readSync(fd, bytes, size, bytes.length - size, null);
      if (!count) break;
      size += count;
    }
    const after = fs.fstatSync(fd, { bigint: true });
    if (size > limit) fail("input exceeds byte limit", "input-limit");
    if (
      BigInt(size) !== BigInt(before.size) ||
      after.size !== before.size ||
      after.mtimeNs !== before.mtimeNs ||
      after.ctimeNs !== before.ctimeNs
    )
      fail("input changed while reading", "input-changed");
    return bytes.subarray(0, size);
  } catch (error) {
    if (error instanceof ReplayInputError) throw error;
    const failure = new ReplayInputError(
      `${label}: ${error.code === "ELOOP" ? "input symlink refused" : "input is not a readable bounded regular file"}`,
      error.code === "ENOENT" ? "file-missing" : "invalid-input-file",
    );
    if (
      typeof error.code === "string" &&
      /^[A-Z][A-Z0-9_]{0,30}$/.test(error.code)
    )
      failure.code = error.code;
    throw failure;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

function decodeReplayJson(bytes, label) {
  let text;
  let value;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
      bytes,
    );
    value = JSON.parse(text);
  } catch {
    throw new ReplayInputError(
      `${label}: input is not UTF-8 JSON`,
      "invalid-json",
    );
  }
  const stack = [];
  for (const match of text.matchAll(/"(?:\\[\s\S]|[^"\\])*"|[{}\[\]]/g)) {
    const token = match[0];
    if (token === "{") stack.push(new Set());
    else if (token === "[") stack.push(null);
    else if (token === "}" || token === "]") stack.pop();
    else if (
      text
        .slice(match.index + token.length)
        .trimStart()
        .startsWith(":")
    ) {
      const key = JSON.parse(token);
      if (key === "__proto__")
        throw new ReplayInputError(
          `${label}: input has a forbidden object key`,
          "forbidden-key",
        );
      if (stack.at(-1).has(key))
        throw new ReplayInputError(
          `${label}: input has a duplicate object key`,
          "duplicate-key",
        );
      stack.at(-1).add(key);
    }
  }
  return value;
}

module.exports = { readReplayFile, decodeReplayJson };
