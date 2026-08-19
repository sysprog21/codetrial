# Cross-implementation fixtures

`integrity-chain.json` is real output from `web/lib.js`'s
`integrityEventPayload`, consumed by the real Rust verifier in
`tests/agent.rs::browser_generated_integrity_events_all_verify_in_the_agent`.

It exists because the integrity hash has two implementations and each was only
ever tested against itself, so a divergence between them passed a green gate
and silently emptied the evidence section of every camera interview.

Do not hand-edit the hashes. Regenerate:

```sh
node -e '
import("./web/lib.js").then(async (lib) => {
  const fs = await import("node:fs");
  let prev = { seq: 0, hash: "" };
  const inputs = [
    { type: "SESSION_START", source: "media", severity: "info" },
    { type: "MEDIA_PREFLIGHT_PASSED", source: "preflight", severity: "info" },
    { type: "INTEGRITY_HEARTBEAT", source: "media", severity: "info",
      detail: "analyzer=source=camera;analysis=face_detect,tracking;frames=3;transport=ImageBitmap" },
    { type: "FACE_MISSING", source: "camera", severity: "warning", durationMs: 2000, detail: "faces=0" },
    { type: "INTEGRITY_HEARTBEAT", source: "media", severity: "info",
      detail: "analyzer=source=camera;analysis=tracking;frames=1;transport=ImageBitmap" },
  ];
  const events = [];
  for (const input of inputs) {
    const ev = await lib.integrityEventPayload(input, prev);
    events.push(ev);
    prev = { seq: ev.seq, hash: ev.hash };
  }
  fs.writeFileSync("tests/fixtures/integrity-chain.json", JSON.stringify(events, null, 2) + "\n");
});'
```

Keep at least one `detail` at the length bound. That is the case that broke.
