// Run with: node --test tests/browser/task-code-limit.test.js
//
// The browser refuses a buffer over TASK_MAX_CODE_BYTES before it is sent, and
// the agent refuses `code.len() > MAX_CODE_BYTES` after it arrives, in UTF-8
// bytes. `taskCodeTooLarge` decides most buffers from their UTF-16 length and
// encodes only the ones in between, so the shortcut has to give the answer the
// byte count gives, at the limit and one either side of it, for every width.

import { test } from "node:test";
import assert from "node:assert/strict";

import { TASK_MAX_CODE_BYTES, taskCodeTooLarge } from "../../web/lib.js";

// One to four UTF-8 bytes; the last is two UTF-16 units. The last entry mixes
// all four.
const UNITS = ["a", "é", "漢", "\u{1f600}", "aé漢\u{1f600}"];

// Exactly `bytes` UTF-8 bytes: whole `unit`s, then ASCII for what is left.
function withBytes(bytes, unit) {
  const whole = Math.floor(bytes / Buffer.byteLength(unit));
  const text = unit.repeat(whole);
  return text + "a".repeat(bytes - Buffer.byteLength(text));
}

test("taskCodeTooLarge agrees with the UTF-8 byte count at and around the limit", () => {
  // Empty and one byte reach the shortcut that never encodes; the offsets
  // reach the encoded middle and the shortcut that always says too large.
  const sizes = [0, 1];
  for (let offset = -3; offset <= 3; offset += 1) {
    sizes.push(TASK_MAX_CODE_BYTES + offset);
  }
  for (const unit of UNITS) {
    for (const bytes of sizes) {
      const text = withBytes(bytes, unit);
      assert.equal(Buffer.byteLength(text), bytes);
      assert.equal(
        taskCodeTooLarge(text),
        bytes > TASK_MAX_CODE_BYTES,
        `${bytes} bytes of ${JSON.stringify(unit)}`,
      );
    }
  }
});
