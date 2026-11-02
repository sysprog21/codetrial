import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
const require = createRequire(import.meta.url);
const {
  readReplayFile,
  decodeReplayJson,
} = require("../../scripts/replay-input.cjs");
function file(t, bytes) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "replay-input-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const name = path.join(dir, "input");
  fs.writeFileSync(name, bytes);
  return name;
}
test("regular input accepts the exact byte cap and refuses one more byte", (t) => {
  const name = file(t, Buffer.alloc(8, 1));
  assert.equal(readReplayFile(name, 8, "replay input").length, 8);
  fs.appendFileSync(name, Buffer.from([1]));
  assert.throws(
    () => readReplayFile(name, 8, "replay input"),
    /bounded regular/,
  );
});
test("growth below the read cap is still detected by the final stat", (t) => {
  const name = file(t, "abc");
  const stat = fs.fstatSync;
  let grew = false;
  t.mock.method(fs, "fstatSync", (fd, options) => {
    const result = stat(fd, options);
    if (!grew) {
      grew = true;
      fs.appendFileSync(name, "d");
    }
    return result;
  });
  assert.throws(
    () => readReplayFile(name, 8, "replay input"),
    /changed while reading/,
  );
  assert.equal(grew, true);
});
test("same-size rewrite with an observed metadata change refuses", (t) => {
  const name = file(t, "abc");
  const stat = fs.fstatSync;
  let changed = false;
  t.mock.method(fs, "fstatSync", (fd, options) => {
    const result = stat(fd, options);
    if (!changed) {
      changed = true;
      fs.writeFileSync(name, "def");
      fs.utimesSync(name, 1, 1);
    }
    return result;
  });
  assert.throws(
    () => readReplayFile(name, 8, "replay input"),
    /changed while reading/,
  );
  assert.equal(changed, true);
});
test("shared JSON decoding validates nested keys and encoding without coercion", () => {
  assert.deepEqual(
    decodeReplayJson(Buffer.from('{"a":[{"b":1},{"b":2}]}'), "input"),
    { a: [{ b: 1 }, { b: 2 }] },
  );
  for (const json of [
    '{"a":{"b":1,"b":2}}',
    '{"a":1,"\\u0061":2}',
    '{"a":[{"__proto__":{}}]}',
  ])
    assert.throws(
      () => decodeReplayJson(Buffer.from(json), "input"),
      /duplicate object key|forbidden object key/,
    );
  assert.throws(
    () => decodeReplayJson(Buffer.from([0xff]), "input"),
    /not UTF-8 JSON/,
  );
});

test("JSON BOMs cannot be stripped into an accepted payload", () => {
  assert.throws(
    () => decodeReplayJson(Buffer.from('\ufeff{"a":1}'), "input"),
    /not UTF-8 JSON/,
  );
});
