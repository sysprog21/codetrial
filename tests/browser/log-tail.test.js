import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import logTail from "../../scripts/log-tail.cjs";

const { createLogTail } = logTail;

function withLog(run) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "log-tail-"));
  try {
    run(path.join(dir, "server.log"));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("each read returns the complete lines gained since the last, once", () => {
  withLog((file) => {
    const tail = createLogTail(file);
    assert.equal(tail(), "", "a log that is not there yet");
    fs.writeFileSync(file, "one\ntwo\n");
    assert.equal(tail(), "one\ntwo\n");
    assert.equal(tail(), "", "nothing new");
    fs.appendFileSync(file, "three\n");
    assert.equal(tail(), "three\n");
  });
});

test("a line still being written is held back until its newline", () => {
  withLog((file) => {
    const tail = createLogTail(file);
    fs.writeFileSync(file, "whole\nhal");
    assert.equal(tail(), "whole\n");
    fs.appendFileSync(file, "f");
    assert.equal(tail(), "");
    fs.appendFileSync(file, " a line\nnext");
    assert.equal(tail(), "half a line\n");
    fs.appendFileSync(file, "\n");
    assert.equal(tail(), "next\n");
  });
});

test("a character split across two reads is not broken", () => {
  withLog((file) => {
    const tail = createLogTail(file);
    // Two bytes per letter, which is the point: the first read ends inside one.
    const bytes = Buffer.from("αβγ\n", "utf8");
    fs.writeFileSync(file, bytes.subarray(0, 3));
    assert.equal(tail(), "");
    fs.appendFileSync(file, bytes.subarray(3));
    assert.equal(tail(), "αβγ\n");
  });
});

test("a log that was replaced is read from its start", () => {
  withLog((file) => {
    const tail = createLogTail(file);
    // Truncated in place, with half a line of the old log still held back.
    fs.writeFileSync(file, "a long first log\nand half a li");
    assert.equal(tail(), "a long first log\n");
    fs.writeFileSync(file, "new\n");
    assert.equal(tail(), "new\n");
    // Rotated: another file under the same name, and a longer one, so its
    // size alone would read as the old log having grown.
    const next = `${file}.next`;
    fs.writeFileSync(next, "the next log, longer than the last\nand more\n");
    fs.renameSync(next, file);
    assert.equal(tail(), "the next log, longer than the last\nand more\n");
    assert.equal(tail(), "");
  });
});

test("a log that goes away reads as empty and is picked up when it returns", () => {
  withLog((file) => {
    const tail = createLogTail(file);
    fs.writeFileSync(file, "one\n");
    assert.equal(tail(), "one\n");
    fs.rmSync(file);
    assert.equal(tail(), "");
    fs.mkdirSync(file);
    assert.equal(tail(), "", "a directory in its place cannot be read");
    fs.rmdirSync(file);
    fs.writeFileSync(file, "back\n");
    assert.equal(tail(), "back\n");
  });
});
