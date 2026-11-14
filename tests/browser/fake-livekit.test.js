import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { topics } from "../../web/lib.js";
import { FAKE_LIVEKIT, fakeLiveKit } from "./source.js";

for (const startupReady of [false, true]) {
  test(`shared LiveKit fixture preserves publication with readiness ${startupReady ? "enabled" : "disabled"}`, async () => {
    const window = {};
    runInNewContext(
      startupReady ? fakeLiveKit({ startupReady: true }) : FAKE_LIVEKIT,
      { window, TextEncoder, TextDecoder },
      { timeout: 1000 },
    );
    const room = new window.LivekitClient.Room();
    const notices = [];
    room.on("DataReceived", (bytes, sender, kind, topic) => {
      assert.ok(bytes instanceof Uint8Array);
      notices.push({
        packet: JSON.parse(new TextDecoder().decode(bytes)),
        sender,
        kind,
        topic,
        publications: window.__published.length,
      });
    });
    await room.connect();
    await room.localParticipant.publishTrack({ kind: "audio" });
    assert.equal(notices.length, 0);
    assert.equal(room.remoteParticipants.size, Number(startupReady));
    const publish = (topic, payload) =>
      room.localParticipant.publishData(
        new TextEncoder().encode(JSON.stringify(payload)),
        { topic },
      );
    await publish(topics.code, { code: "candidate work" });
    assert.equal(notices.length, 0);
    await publish(topics.integrity, { seq: 1 });
    assert.equal(notices.length, Number(startupReady));
    await publish(topics.integrity, { seq: 2 });
    await publish(topics.control, { type: "startup_sync" });
    assert.equal(notices.length, Number(startupReady));
    assert.equal(window.__published.length, 4);
    assert.deepEqual(JSON.parse(JSON.stringify(window.__published)), [
      { topic: topics.code, payload: { code: "candidate work" } },
      { topic: topics.integrity, payload: { seq: 1 } },
      { topic: topics.integrity, payload: { seq: 2 } },
      { topic: topics.control, payload: { type: "startup_sync" } },
    ]);
    if (startupReady) {
      const [notice] = notices;
      assert.deepEqual(notice.packet, {
        type: "interviewer_state",
        reconnecting: false,
        startup: "ready",
      });
      assert.equal(notice.topic, topics.control);
      assert.equal(notice.kind, undefined);
      assert.equal(notice.sender.identity, "agent-fake-room");
      assert.equal(notice.sender.kind, "AGENT");
      assert.equal(
        room.remoteParticipants.get(notice.sender.identity),
        notice.sender,
      );
      assert.equal(notice.publications, 2);
    }
    await room.disconnect();
  });
}
