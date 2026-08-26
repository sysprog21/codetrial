# Avatar contract

Jim's browser-rendered avatar. Presentation only: Gemini Live, LiveKit audio and
transcription, barge-in, and interviewer behavior are unchanged by anything here.
No rendered frame is published as a LiveKit track, sent to the server, or stored.

Related documents: [third-party notices](../THIRD-PARTY-NOTICES.md) for asset
licenses and attribution; `web/vendor/avatar/README.md` for reproducible bundle
sources.

## Status

Shipping. The renderer, behavior mapping, fallback, and vendored model are all
in the repo.

## Model

| Field | Value |
|---|---|
| File | `web/vendor/avatar/jim.vrm` |
| Size | 10,917,800 bytes (10.92 MB) |
| SHA-256 | `624d0d554bc205bbdc33e22a68a2c3c20edebb3e573011ead8878a65e5329b23` |
| Spec version | VRM 1.0 |
| Triangles | 45,058 |
| Materials | 17 |
| Texture resolution | 1024x1024 maximum |
| glTF extensions | `VRMC_vrm`, `VRMC_springBone`, `VRMC_node_constraint`, `VRMC_materials_mtoon`, `KHR_materials_unlit`, `KHR_texture_transform`, `KHR_materials_emissive_strength` |

See the [third-party notices](../THIRD-PARTY-NOTICES.md) for the model's source,
license, attribution, and redistribution terms.

### Where this deviates from the budget, and why that was accepted

17 materials against a budget of 1 to 3, and `VRMC_springBone` physics against
"minimal physics." The associated licensing decision is recorded in the
[third-party notices](../THIRD-PARTY-NOTICES.md).

Revisit the budget when a commissioned model exists, not before. If render cost
becomes a problem, measure it first.

### Requirements on any replacement

Budget for a commissioned replacement: upper body only, 1 to 3 materials, 512
to 1024 px textures, minimal physics, 5 to 15 MB.

The delivered VRM must be uncompressed glTF: no `KHR_draco_mesh_compression`,
no `KHR_texture_basisu`, no `EXT_meshopt_compression`. The vendored bundle
registers no decoder for any of them, and `GLTFLoader` rejects outright with
"no DRACOLoader instance provided" rather than degrading. This matters because
a 5 to 15 MB budget is exactly the pressure that pushes an author toward Draco
or KTX2, and discovering it at delivery time would mean re-vendoring the bundle
with three more decoders for one model. The shipped model satisfies this.

Document the replacement's source, license, attribution requirements, and
redistribution rights in the [third-party notices](../THIRD-PARTY-NOTICES.md).

## Dependencies

The renderer uses the vendored bundle at `web/vendor/avatar/three-vrm.js`.
Its components, licenses, and reproducible source details are listed in
[the third-party notices](../THIRD-PARTY-NOTICES.md).

### Why a bundle instead of an import map

`@pixiv/three-vrm` and `GLTFLoader.js` import `three` by bare specifier. A
browser resolves that only through an import map, import maps cannot be loaded
from a `src` URL, and so one would have to be inlined into
`web/interview.html`. `src/web.rs` sets `script-src 'self'` with no
`'unsafe-inline'` and no nonce, so an inline import map would be blocked, and
the alternative is loosening the page's script policy for a build-time
convenience. Bundling resolves the specifiers at vendor time and leaves the CSP
alone.

The same reasoning applies to `web/recording/index.html` when recording task 8a
builds it: import the same vendored bundle, do not add an import map.

## Fallback contract

`#jim-avatar` carries `data-avatar-state`, and that attribute is the contract.
`web/avatar/avatar.js` owns the value; `web/styles.css` owns the appearance.

| State | Meaning | What the candidate sees |
|---|---|---|
| `loading` | the page is asking whether a model exists | neutral panel |
| `ready` | a VRM is loaded and rendering | the canvas |
| `unavailable` | no model, no WebGL, a load failure, or a load timeout | neutral panel, with the reason in `#jim-avatar-note` |
| `stopped` | the interview ended or the room was left | neutral panel |

Only `ready` hides the neutral panel, so a state nobody has invented yet
degrades to the panel rather than to an empty box.

Every failure lands on `unavailable`: a missing `jim.vrm`, a browser with no
WebGL, a corrupt model, and a model server that accepts the connection and then
hangs are one path, not four. The load timeout is `LOAD_TIMEOUT_MS`, 30000 ms.

The renderer is imported only after a `HEAD` on the model URL succeeds, so a
checkout with no `jim.vrm` never downloads the 730 KB bundle.

## Behavior

Phase 1 reads only `lk.agent.state`, the three values LiveKit already publishes:
`listening`, `thinking`, `speaking`. An unrecognized state falls back to
`listening`. `questioning`, `encouraging`, and `challenging` wait for
`src/agent.rs` to publish a documented `lk.avatar.state`; inventing them in the
browser would be the avatar guessing at the interviewer's intent.

| Constant | Value | Why |
|---|---|---|
| `ANALYSER_FFT_SIZE` | 512 | ~10.7 ms at 48 kHz |
| `ANALYSER_WINDOW` | 5 frames | smooths syllable gaps without lagging speech |
| `MOUTH_OPEN_THRESHOLD` | 0.04 | below this is room tone |
| `MOUTH_FULL_AMPLITUDE` | 0.32 | above this the jaw is already open |
| `BLINK_INTERVAL_MS` | 4200 | |
| `BLINK_DURATION_MS` | 140 | |
| `BREATH_PERIOD_MS` | 4000 | |
| `BREATH_AMPLITUDE` | 0.02 | radians of head pitch |
| `GAZE_BOUND` | 0.35 rad | |
| `HEAD_TILT_BOUND` | 0.12 rad | the head follows the eyes at a third of the travel |
| `TRANSITION_MS` | 200 | a time constant, not a deadline: ~63% applied after one, visually done at three |

Blink and breath are deterministic functions of the clock, which is the only
reason a fake-clock test can assert them exactly. A random blink interval would
look better and prove nothing.

The mouth is driven by an analyser on Jim's own track, built with
`createMediaStreamSource(new MediaStream([track.mediaStreamTrack]))`.
`createMediaElementSource` is the obvious call and the wrong one: it returns
silence for a MediaStream-backed element, so the mouth would never open and
nothing would say why. The candidate's microphone is never observed.

The analyser is built only for a track whose participant passes `isAgent`, and
torn down only for that same track. `playRemoteAudio` fires for every remote
audio track, so without the first guard the mouth followed whichever arrived
first, and without the second any other participant leaving killed lip sync for
the rest of the session.

The mouth is not eased at all. The analyser already averages `ANALYSER_WINDOW`
frames, and running that through the transition filter as well put two lags
between Jim's voice and his jaw. Only the state-driven channels, which change in
steps, are eased.

Audio drives the jaw, and the gate defaults to open. `setMouth` used to be
gated on `setSpeaking(true)` having been called, and `setSpeaking` is only ever
called from `updateAgentState`, which returns early when it cannot find the
agent participant. One stale "Waiting" pill and Jim sat through an entire
interview talking with his mouth shut, which reads to a candidate as an
interviewer they cannot converse with. Agent state may mute the mouth; it may
not be required to un-mute it. Anything that must be inferred from a channel
that can go silent belongs on the side of the default that fails visible.

The mouth closes immediately, without easing, on any of: the agent leaving the
`speaking` state, which is what barge-in looks like from here; Jim's track being
unsubscribed; the report appearing; the room being left. A jaw easing shut while
the candidate has the floor reads as Jim talking over them.

`prefers-reduced-motion: reduce` drops the blink, the breathing, and the easing
between states. It keeps the mouth, because that is a speech cue rather than
decoration.

## Interfaces

`createAvatar({ mount, loadModel, now, timeoutMs, reducedMotion, onState })`
exposes four behavior calls, `setMouth(value)`, `setGaze(target)`,
`setExpression(state)`, and `setSpeaking(value)`, plus `frame(at)`,
`destroy()`, `ready`, `state()`, and `frames()` for lifecycle. There is no
generic avatar interface and no 2D adapter; add one when a second renderer
exists, not before.

`setGaze` is an override that outlives the next `setExpression`. Passing null
hands the eyes back to the agent state. `setExpression` used to reassign the
gaze from the state table, which silently discarded every `setGaze`.

`onState` fires synchronously inside the same call that writes
`data-avatar-state`. Anything deriving UI from the phase must use it rather than
the `ready` promise, which resolves a microtask later and still fires after a
`destroy()`.

`loadModel` resolves to `{ apply(pose), dispose(), frames() }`. One `apply` per
frame with the whole pose beats eight setters: the frame reaches the renderer
atomically, and a test can assert it by value. The pose is
`{ mouth, blink, gaze: {x, y}, headTilt: {x, y}, breath, expression, agentState }`,
where `expression` is a weight per preset name, every weight is in 0..1, and
every angle is inside the bounds above.

`expression` is a map rather than a `{name, weight}` pair because a single
shared weight applied the incoming preset at the outgoing one's value during a
transition, and cut the outgoing one to zero instead of fading it. Writing every
preset every frame is also what lets the renderer forget which one it set last.

The timeout races the load; it cannot cancel it. Any model the avatar does not
adopt, whether because the timeout won or because `destroy()` came first, is
disposed when it eventually arrives. Without that it keeps a WebGL context, and
a page gets about 16.

`web/avatar/vrm.js` is the only first-party file that imports Three.js, and
`tests/browser/avatar.test.js` asserts that. If `web/avatar/avatar.js` ever
imports it, the node tests stop being able to run at all.

## Placement and pose

The avatar floats over the editor's top-right (`#jim-stage`, fixed), not in the
sidebar. The sidebar already carries the problem, timer, status pill, captions,
controls and the Meet panel; adding a face made it crowded, and the point of
the avatar is that the candidate glances at it while working, so it belongs
where their eyes already are. The stage takes no pointer events, so it can
never swallow a click meant for the editor, and the credit link re-enables them
for itself. Below 1200px wide the stage is hidden rather than allowed to cover
code.

Every VRM loads in a T-pose and the format ships no idle animation, so
`loadVrm` drops the upper arms once at load. This is model-independent: without
it any model stands in frame with its arms straight out. It does not help a
model whose arms are rigged props driven by `VRMC_node_constraint`, because
those constraints are applied inside `vrm.update()`, after and on top of these
rotations. Seed-san's robot arms are exactly that case, which is one more entry
in the cost column for a borrowed mascot.

### Captions

`#captions-bar` lives inside `#jim-stage`, under the avatar. It sizes to the
turn rather than clamping: a fixed `max-height` cut sentences in half behind a
scrollbar nobody reaches for mid-interview. `CAPTION_MAX_CHARS` already bounds
the text, so the remaining `min(40vh, 20rem)` cap is a backstop, not the normal
case.

It is a live subtitle, not a log. The transcript tab keeps every turn, so the
bar hides after `CAPTION_IDLE_HIDE_MS` (12 s) with nobody speaking and returns
on the next word. `init` arms the timer for the "Listening..." placeholder too,
or it would sit over the editor for the whole interview having said nothing.

## Checks

`node --test tests/browser/avatar.test.js` covers the behavior with a fake
clock, a fake mount, and a fake model. It cannot see WebGL, a canvas, or a
layout, and no assertion in it should claim to.

`BROWSER_CHECK_FLOW=avatar ./scripts/browser-check.sh` is the only check that
runs the renderer. It asserts the mount leaves `loading` and then asserts
whichever branch it landed on. The `ready` branch reads a frame counter the
page exposes as `window.__codetrialAvatarFrames` and requires it to advance,
because a canvas that exists proves nothing about a loop that runs; an earlier
version sampled `toDataURL` once, never took a second sample, and compared the
first against a length threshold.

`connect-src` must contain `blob:`, asserted in `tests/web.rs`. GLTFLoader
mints a `blob:` URL per embedded texture and `ImageBitmapLoader` reads it with
`fetch`, which `connect-src` governs, so without it every texture in the model
fails and the avatar degrades to the neutral panel with no stated reason. No
other check can catch this until a model ships.

## Privacy

No face or voice data is collected for avatar control. The analyser reads Jim's
synthesized speech, never the candidate's microphone, and no candidate emotion
is inferred or persisted. The model and every animation stay in static browser
assets. Render work is capped at one canvas.

## Accessibility and the render loop

The canvas is `role="img"` with the label "Jim, the AI interviewer". It carried
`aria-hidden="true"` while it had no name, which was the right answer then: an
unlabeled canvas announced to a screen reader is worse than a hidden one. A
picture of a person whose whole useful content is "this is the interviewer"
needs one sentence, not a live region.

The render loop stops scheduling frames while the document is hidden and starts
again on `visibilitychange`. Browsers already throttle `requestAnimationFrame`
in a background tab, so what this buys is the analyser read and the humanoid
update stopping too. There is one door into the loop, `resumeAvatar`, which
refuses to open a second one and refuses to open any while the model is still
loading: `createAvatar` returns before the model has arrived, and a visibility
change during that window would otherwise start a loop that poses nothing.

The media contract is unchanged by any of this. No first-party script captures a
canvas stream, the only tracks published are the candidate's microphone and
camera, and the recording template renders the same avatar from the same
vendored bundle rather than subscribing to a second one.
