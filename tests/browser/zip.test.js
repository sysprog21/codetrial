import test from "node:test";
import assert from "node:assert/strict";
import { crc32, zipStore } from "../../web/zip.js";
import { readZip } from "./zip-reader.js";

test("the CRC-32 is the one ZIP and gzip use", () => {
  assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
  assert.equal(crc32(new Uint8Array()), 0);
});

test("a ZIP holds its files whole and in order, whatever their sizes", () => {
  const big = Uint8Array.from({ length: 100_000 }, (_, at) => (at * 7) & 255);
  const files = [
    { name: "result.json", bytes: new TextEncoder().encode('{"a":1}') },
    { name: "dir/recording.webm", bytes: big },
    { name: "empty", bytes: new Uint8Array() },
  ];
  const zip = zipStore(files, Date.UTC(2026, 10, 24, 12, 30, 40));
  const read = readZip(zip);
  assert.deepEqual(
    read.map((file) => file.name),
    files.map((file) => file.name),
  );
  for (const [at, file] of files.entries())
    assert.deepEqual(Buffer.from(read[at].bytes), Buffer.from(file.bytes));
});

test("a name that is not plain ASCII, or that leaves its folder, is refused", () => {
  const bytes = new Uint8Array([1]);
  // The escapes spell a Chinese name without putting one in this file.
  for (const name of [
    "\u7b54\u8fa8.webm",
    "caf\u00e9.txt",
    "/etc/x",
    "a/../b",
    "",
    "tab\t",
  ])
    assert.throws(() => zipStore([{ name, bytes }]), /plain ASCII/, name);
});

test("an empty ZIP is still a ZIP", () => {
  assert.deepEqual(readZip(zipStore([])), []);
});

test("the reader refuses a local header that disagrees with the directory", () => {
  const zip = zipStore([{ name: "a.txt", bytes: new Uint8Array([1, 2, 3]) }]);
  // A flipped bit in the local CRC, and one in the local method.
  for (const at of [14, 8]) {
    const bad = Uint8Array.from(zip);
    bad[at] ^= 1;
    assert.throws(() => readZip(bad), /local header differs|not a plain/);
  }
});
