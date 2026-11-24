import test from "node:test";
import assert from "node:assert/strict";
import {
  codeText,
  defenseSubmission,
  transcriptText,
  youtubeUrl,
} from "../../web/submission.js";
import { readZip, sha256 } from "./zip-reader.js";

test("a YouTube video or playlist link is kept, anything else is refused", () => {
  for (const good of [
    "https://youtu.be/0MiU9z_UDc8",
    "https://www.youtube.com/watch?v=0MiU9z_UDc8&t=10s",
    "https://m.youtube.com/watch?v=0MiU9z_UDc8",
    "https://www.youtube.com/live/0MiU9z_UDc8",
    "https://www.youtube.com/playlist?list=PLabcdefghijklmnop",
  ])
    assert.equal(youtubeUrl(` ${good} `), new URL(good).href, good);
  assert.equal(youtubeUrl("   "), null);
  for (const bad of [
    "http://youtu.be/0MiU9z_UDc8",
    "https://example.com/watch?v=0MiU9z_UDc8",
    "https://youtu.be.example.com/0MiU9z_UDc8",
    "https://user:pw@youtu.be/0MiU9z_UDc8",
    "https://youtu.be/short",
    "https://youtu.be:8443/0MiU9z_UDc8",
    "https://www.youtube.com/watch",
    "0MiU9z_UDc8",
    "javascript:alert(1)",
  ])
    assert.throws(() => youtubeUrl(bad), /YouTube link/, bad);
});

test("the transcript names who spoke, the turn and the UTC time", () => {
  const text = transcriptText(
    [
      { at: 1795465020, role: "learner", turnId: "turn-1", text: "id flush" },
      { at: 1795465030, role: "jim", turnId: null, text: "Which cycle?" },
    ],
    "Heading.",
  );
  assert.equal(
    text,
    "Heading.\n\n[2026-11-23T20:17:00Z] Learner (turn-1): id flush\n\n[2026-11-23T20:17:10Z] Jim: Which cycle?\n",
  );
});

const defense = {
  items: [
    {
      id: "item-1",
      file: "src/Core.scala",
      line: 12,
      exercise: "4.4",
      quote: "val x = a + b",
      change: "swap the operands",
      changedLine: "val x = b + a",
      predict: {
        runs: [
          {
            edited: "val x = b + a",
            prediction: { fails: ["t1"], compiles: true, how: "wrong order" },
          },
        ],
      },
    },
    {
      id: "item-2",
      file: "src/Alu.scala",
      line: 3,
      quote: "x",
      change: "delete it",
      changedLine: null,
      predict: { runs: [] },
    },
  ],
  bug: {
    claim: {
      program: "add.elf",
      cycle: 14,
      signal: "id flush",
      rule: "r",
      exercise: "4.4",
    },
  },
  stamp: { studentId: "B12345678", head: "abc123" },
};

test("the code file lists what was typed and predicted and the bug claim", () => {
  const text = codeText(defense, "Heading.");
  assert.match(text, /item-1 {2}src\/Core\.scala:12 \(Exercise 4\.4\)/);
  assert.match(text, /Original line: val x = a \+ b/);
  assert.match(text, /Typed \(run 1\): val x = b \+ a/);
  assert.match(text, /fails t1; compiles\. wrong order/);
  assert.match(text, /Change asked: {2}delete it \(the line is removed\)/);
  assert.match(text, /\(nothing was run\)/);
  assert.match(text, /program add\.elf, cycle 14, signal "id flush"/);
  assert.match(codeText({ items: [], bug: {} }, "H"), /No claim was stated/);
});

test("a run without retained details is not reported as no run", () => {
  const text = codeText(
    {
      items: [{ id: "q", runs: [], predict: { runs: [{ matched: null }] } }],
      bug: {},
    },
    "H",
  );
  assert.match(text, /run details were not retained/);
  assert.doesNotMatch(text, /nothing was run/);
});

test("a submission holds code, transcript and result, each named by its hash", async () => {
  const result = {
    assessmentMode: "defense",
    taskAssessment: {
      setId: "mycpu",
      setVersion: 3,
      sessionId: "abc-123",
      taskId: "defense",
    },
    defense,
  };
  const submission = await defenseSubmission({
    result,
    transcript: [
      { at: 1795465020, role: "learner", turnId: "turn-1", text: "hi" },
    ],
    recordingUrl: "https://youtu.be/0MiU9z_UDc8",
    when: Date.UTC(2026, 10, 24, 1, 2, 3),
  });
  assert.equal(submission.name, "defense-submission-mycpu-v3-abc-123.zip");
  const files = readZip(submission.bytes);
  assert.deepEqual(
    files.map((file) => file.name),
    ["result.json", "code.txt", "transcript.txt", "manifest.json"],
  );
  const manifest = JSON.parse(Buffer.from(files[3].bytes).toString());
  assert.deepEqual(manifest, submission.manifest);
  assert.equal(manifest.recordingUrl, "https://youtu.be/0MiU9z_UDc8");
  assert.equal(manifest.studentId, "B12345678");
  assert.equal(manifest.createdAt, "2026-11-24T01:02:03.000Z");
  for (const file of files.slice(0, 3)) {
    const named = manifest.files.find((entry) => entry.name === file.name);
    assert.deepEqual(
      { bytes: named.bytes, sha256: named.sha256 },
      { bytes: file.bytes.length, sha256: sha256(file.bytes) },
    );
  }
  assert.deepEqual(JSON.parse(Buffer.from(files[0].bytes).toString()), result);
  assert.match(
    Buffer.from(files[2].bytes).toString(),
    /transcript is incomplete.*before a reload or crash/is,
  );
});

test("a name made from the stamp stays plain ASCII", async () => {
  const submission = await defenseSubmission({
    result: { taskAssessment: { setId: "答辨/../x y", sessionId: "" } },
    transcript: [],
    recordingUrl: null,
    when: Date.UTC(2026, 0, 2, 3, 4, 5),
  });
  assert.match(submission.name, /^[\x20-\x7e]+\.zip$/);
  assert.doesNotMatch(submission.name, /[/ ]/);
});
