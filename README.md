# CodeTrial

A live technical interview simulator. Jim, the AI interviewer, reads your editor
as you type, listens while you think out loud, interjects with targeted
engineering questions, steps in when you stall, warns you near the end, and
produces a scored hire/no-hire report when the session closes.

Test cases run inside the interview and the interviewer sees the results: a
pass/fail digest streams over a LiveKit data topic, so Jim reacts to a red or
green run, and the final report grades against what actually executed rather
than a reading of the code.

The whole system is one Rust binary. It serves the static browser app, mints
LiveKit tokens, and runs the interviewer agent.

## How it works

```
┌────────────────────────────┐       WebRTC         ┌─────────────────────┐
│ Browser (static, no build) │  mic + camera ─────▶ │ LiveKit Cloud       │
│  · editor + syntax colors  │                      │ (SFU)               │
│  · problem panel, timer    │                      └─────┬───────────────┘
│  · test runners            │  data channel              │
│  · report + history        │  code_update / control /   │
└────────────┬───────────────┘  test_results / report     ▼
             │
             │ /api/*                ┌─────────────────────┐
             ▼                       │ Rust agent          │
┌────────────────────────────┐       │ (LiveKit runner)    │
│ Rust web server            │       │  Gemini Live   ◀────┤ voice, barge-in,
│  · token minting           │       │  Gemini Flash  ◀────┤ vision, scoring
│  · GitHub handle           │       └─────────────────────┘
│  · SQLite report history   │
└────────────────────────────┘
```

The editor is streamed to the agent as structured text over LiveKit's data
channel rather than as pixels: it is cheaper in tokens and more reliable for
code. Camera and microphone are required before the interview starts; screen
sharing is not part of the flow.

Audio and code snapshots are relayed in memory only. Candidate video is not sent
to Gemini by default; `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED=true` opts the
server into forwarding candidate video frames. Reports are kept in browser
storage, and synced to a local SQLite database when signed in. API keys stay in
server-side env files and are never sent to the browser.

Camera integrity analysis is scheduled in a browser Worker. Frames reach it as
a transferred `VideoFrame` or `ImageBitmap` handle, so no pixels are copied on
the way; a browser with neither sends timing only, and face detection is
reported unavailable rather than guessed at. Detection itself runs in a nested
worker on vendored MediaPipe. This does not require `SharedArrayBuffer`,
WebGPU, server-side computer vision, or uploading raw media for integrity
analysis.

Face presence uses MediaPipe Face Detection locally in the browser worker. The
vendored package is `@mediapipe/face_detection` version `0.4.1646425229`, from
<https://www.npmjs.com/package/@mediapipe/face_detection>, licensed
Apache-2.0. Every shipped byte is pinned in
`web/vendor/face-detection/SHA256SUMS` and checked by `make verify-vendor`: the
detector is the integrity signal, so what it is built from is worth recording.

The JS loaders are committed; the 11.7 MB of wasm, TFLite model, and graph
binary beside them are downloaded by `scripts/fetch-vendor.sh`, which
`make build`, `make serve`, `make web`, and `./scripts/test.sh` all run first.
`web/vendor/face-detection/FETCH` names the version and the files; SHA256SUMS
stays the contract, and a download is written into `web/vendor/` only after its
bytes hash to the pin, so an interrupted transfer or a bad mirror cannot leave
the browser a wasm nobody pinned. A checkout that already has the bytes skips
the download, so this costs one 1.5-second fetch per clone and nothing after.
A first build therefore needs network access; every later one does not.

The room client is `livekit-client` version `2.20.0`, the `dist/livekit-client.umd.js`
build from <https://www.npmjs.com/package/livekit-client>, licensed Apache-2.0
and pinned in `web/vendor/SHA256SUMS`. `web/interview.html` loads it as a plain
script, so the vendored bytes are the only copy there is; record the version
with the hash, because a hash alone cannot tell you what to re-download.

### Where candidate code executes

| Language | Runner | Notes |
|---|---|---|
| Python | Pyodide (CPython on WebAssembly) in a Worker | runs in the browser, served from `web/vendor/pyodide/` |
| JavaScript | Web Worker | runs in the browser |
| C, C++, Java | [Compiler Explorer](https://godbolt.org) | code leaves the machine |

Browser runs are terminated after 5 seconds, so an infinite loop costs one test
run rather than the interview. C, C++ and Java are compiled and executed by
Compiler Explorer, which means that source leaves the machine; the editor
toolbar says so. Set `CODETRIAL_COMPILER_EXPLORER_ENABLED=false` to keep the
C, C++ and Java editors available while withdrawing compiled-language test
runs. A page can still set `globalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL`
before the module loads to redirect those runs; an empty value withdraws them.

Verdicts use per-problem checkers rather than string equality, so any valid
answer passes: either index order for Two Sum, any correct palindrome, a valid
topological order, an operation sequence for LRU cache. The bank holds 150
problems, including function-style and class-style prompts. C++ and Java class-style
problems use the same disclosed Compiler Explorer path; C class-style tests
remain staged.

## Requirements

- Rust (stable). Check with `rustup show`.
- Node 18 or newer, for the browser test suite.
- Python 3, for the problem-bank generators.
- A LiveKit Cloud project and a Google AI Studio key (both have free tiers that
  cover ordinary use: 1,000 agent minutes per month and the free realtime model).

## Setup

### 1. LiveKit Cloud

Create a project at <https://cloud.livekit.io>, then open Settings, Keys, Create
key. Note the WebSocket URL (`wss://….livekit.cloud`), the API key, and the API
secret.

### 2. Google AI Studio

Create an API key at <https://aistudio.google.com/apikey>.

### 3. Configuration file

```bash
cp config/codetrial.env.example config/codetrial.env.local
```

Fill in the LiveKit and Google values. The binary loads this file automatically
when it exists; `--config PATH` points at a different one, and
`CODETRIAL_SKIP_CONFIG=1` disables the automatic load.

To spread rooms over more than one LiveKit project, add
`config/codetrial.env.<id>` per extra project, each with its own `LIVEKIT_URL`,
`LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` and optionally `GOOGLE_API_KEY`. The id
is the part after the last dot and must be lowercase letters and digits: it
appears in the room name (`interview-<id>-xxxxxxxx`), which is how
`codetrial run-livekit ROOM` knows which project to join. Give both processes
the same config directory, which is `config/` by default and otherwise the
directory holding the `--config` file; the agent refuses a room naming a
project it cannot see rather than joining the wrong one. A file that is
incomplete or unreadable is reported and skipped, and never stops the primary
project from serving.

### 4. GitHub handle

Every interviewee must provide a GitHub username before the server mints a
LiveKit token. The server records that username in SQLite and signs a browser
session cookie; it does not require a GitHub OAuth app.

The handle is a label, not an identity. Nothing verifies it, so treat it as
self-declared: each recorded login gets its own account row, and typing a
handle someone else used does not reach their reports. Report history lives
with the session cookie, so it is per browser and lasts as long as the cookie.
Attributing an interview to a real person needs OAuth.

```
SESSION_SECRET=…                    # optional locally; signs the session cookie
CODETRIAL_DB_PATH=codetrial.db      # optional; where account records live
```

Both values have local defaults, and the default `SESSION_SECRET` is a
published string: with `NODE_ENV=production` the server refuses to start until
you set a real one. `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` remain
optional if you want to re-enable the OAuth redirect flow at `/api/login`.

An account stores at most 200 reports, each capped at 64 KiB. `POST
/api/reports` answers `507` once an account is full, and rewriting a report the
account already owns keeps working at the ceiling so a finished interview can
always save. The cap exists because that route is an authenticated write with
no rate limit where the client picks its own row ids, which is otherwise an
unbounded database.

## Running

```bash
INTERVIEW_ROOM_NAME=interview-local make serve
```

Open <http://localhost:3000>, choose a problem and a duration, and complete the
media preflight. Camera, microphone, and output are mandatory before the
interview starts.

Say "can I get a hint?" when stuck; hints count against the communication score.
Click End interview, or let the clock reach 0:00, to get the report: coding
score, communication score, hire/no-hire, and specific improvement areas.

`serve` runs one agent in one room and is a local mode. It refuses to start with
`NODE_ENV=production`; `web` is the production mode.

| Command | Purpose |
|---|---|
| `cargo run -- serve` | web server and agent together, one fixed room |
| `cargo run -- web` | web server, dispatching an interviewer per room |
| `cargo run -- run-livekit ROOM` | agent only, joining `ROOM` |
| `cargo run -- check-gemini` | opens a Gemini Live session and exits, to verify credentials |

`web` names a fresh room per interview, so it is the only process that can know
where to send an interviewer, and it sends one itself when `GOOGLE_API_KEY` is
set. Without that key it serves the web side only and says so on startup: an
interviewer then has to reach each room from somewhere else, and since nothing
publishes those room names, that shape only works with a LiveKit agent worker
registered against the project. One `web` process hosts at most 16 interviews at
once and refuses further dispatch with `dispatch_refused` rather than growing
the bill without limit.

An interviewer leaves a room five minutes after joining if no candidate arrives,
and ninety seconds after the candidate leaves for good. A candidate who closes
the tab therefore stops costing Gemini minutes without waiting out the clock.

All modes accept `--config`, `--web-addr`, `--web-dir`, `--room-prefix`, and
`--duration-min`. Flags win over the config file, which wins over the
environment.

To disable C, C++ and Java execution through Compiler Explorer while keeping
their editors selectable:

```bash
CODETRIAL_COMPILER_EXPLORER_ENABLED=false make web
```

## During the interview

1. While you type and narrate, Jim stays quiet and reads the editor, interjecting
   between logical blocks with a single targeted question ("why a hash map over a
   plain array on line 12?").
2. Go silent and stop typing for 15 seconds and Jim steps in: "walk me through
   what you're thinking right now."
3. A vague answer gets one precise push-back on depth: complexity, unbalanced
   trees, an edge case.
4. Hints are conceptual and progressive, anchored to the line you are stuck on.
   Never the algorithm, never code.
5. Run tests at any point. All green and Jim raises the bar; failures get one
   nudge toward the pattern in the failing cases, never the bug or the fix. The
   latest run feeds the report's coding score.

Voice replies stay under three sentences, avoid reading code aloud, and stop the
moment you interrupt, which Gemini Live handles natively.

### Presenting in Google Meet

A human interviewer can watch a CodeTrial interview from Google Meet without
CodeTrial being in Meet's audio path at all. LiveKit plays Jim into the
CodeTrial tab, and Meet re-shares that tab:

```text
Jim audio -> LiveKit -> CodeTrial tab audio
CodeTrial tab video + tab audio -> Meet "Share a tab" + "Share tab audio"
Candidate microphone -> LiveKit -> interviewer, by joining the room
```

Open "Present in Google Meet" in the interview sidebar for the five steps. The
two that Meet gets wrong by default: share **A tab**, not a window or the whole
screen, and enable **Share tab audio** before clicking Share. Without tab audio
the interviewer sees the session but never hears Jim.

One microphone cannot serve both applications. CodeTrial keeps it for LiveKit so
Jim can hear you, which means Meet does not get it, and the interviewer hears the
candidate by joining the interview room rather than through Meet. Meet carries
the shared tab.

Tick "I will present this interview in Google Meet" during the media check if you
plan to share the tab. The preflight still proves your camera works and that you
are in front of it, then CodeTrial stops the camera and hands the device to Meet,
because Meet cannot open a camera CodeTrial is holding. Face-presence integrity
does not run for that interview and the report records
`CAMERA_RELEASED_TO_PRESENTER`, so an interview with no face evidence is never
mistaken for one that was watched.

CodeTrial never presents itself as a microphone to Meet: there is no audio mixer,
no virtual device, and no `MediaStreamDestination` bridge. `tests/browser/meet-tab-audio.test.js` pins
which first-party scripts may call `getUserMedia` and rejects
`createMediaStreamDestination` and `getDisplayMedia` anywhere outside
`web/vendor/`, so adding one of those is a deliberate edit to the test rather
than a quiet commit. It reads source text, so it does not catch every shape a
mixer could take; treat it as a tripwire, not a proof.

The sidebar's output selector only chooses which speaker plays Jim inside the
CodeTrial tab. It needs Chrome's `setSinkId` and says so when the browser lacks
it, and the device list fills in only after the media preflight grants access.

Wear headphones. On speakers, the candidate microphone re-captures Jim and the
interviewer hears him twice.

Once the tab is shared, Google Meet controls who receives the interview.
Recordings or copies made in Meet, or by anyone in the call, are outside
CodeTrial's control. The manual verification procedure is
`docs/meet-tab-audio-manual-check.md`.

### Jim's avatar

Jim has a browser-rendered upper-body avatar in the interview sidebar. It is
presentation only. Gemini Live, LiveKit audio and transcription, and barge-in
are unchanged by it, no rendered frame is published as a LiveKit video track or
sent to the server, and there is no server GPU work.

It is rendered locally from Three.js 0.185.1 and `@pixiv/three-vrm` 3.5.5, both
MIT, vendored as one pre-bundled ES module at `web/vendor/avatar/three-vrm.js`
and pinned in `web/vendor/avatar/SHA256SUMS`. Licenses ship beside it;
`web/vendor/avatar/README.md` records the tarballs and the command that
reproduces the bundle byte for byte.

Jim's face is **Seed-san, by VirtualCast, Inc.**, from the VRM Consortium's
specification repository, under the
[VRM Public License 1.0](https://vrm.dev/licenses/1.0/). The model's own
embedded metadata is the grant: `allowRedistribution: true`,
`commercialUsage: "corporation"`, `avatarPermission: "everyone"`,
`modification: "allowModificationRedistribution"`, and
`creditNotation: "required"`. Credit is a license term, not a courtesy, which
is why VirtualCast is named here, under the avatar in the interview page, and
in `web/vendor/avatar/LICENSE-jim-vrm.txt`. Removing any of those breaks the
license. This is a borrowed mascot standing in for a commissioned model;
`docs/avatar-contract.md` records why it was accepted and what it costs.

A browser without WebGL, or a checkout whose model is missing or corrupt, shows
a neutral panel saying Jim is present by voice. The renderer is imported only
after the model URL answers, so a checkout with no model never downloads the
bundle at all.

The mouth is driven by an analyser on Jim's own audio track. The candidate's
microphone is never observed, no face or voice data is collected for avatar
control, and no candidate emotion is inferred or persisted. `prefers-reduced-motion:
reduce` drops the blink, the breathing, and the easing between states, and keeps
the mouth, which is a speech cue rather than decoration.

`docs/avatar-contract.md` records the states, the constants, the pose interface,
and what still has to be measured once a model exists.

## Development shortcuts

The Makefile keeps the common commands short:

```bash
make build       # release build
make check       # the test gate, plus a live Gemini credential check
make indent      # format and lint: cargo fmt + clippy, ruff, shfmt
make serve       # run the web server and agent together
make web         # run the web server only
make fetch-vendor  # download the pinned MediaPipe binaries (build does this)
make verify-vendor # hash every vendored file against its SHA256SUMS pin
make clean       # remove Cargo build artifacts
```

`make check` runs the normal test gate and the live Gemini credential check, so
it needs the configured LiveKit and Google credentials. CI runs the credential-free
gate directly with `./scripts/test.sh`.

`./scripts/test.sh` runs every gate even after one fails, and prints the ones
that failed on the last line. It used to stop at the first, which meant a single
missing vendored file reported nothing about the other three hundred tests.

## Tests

```bash
./scripts/test.sh
```

This is the whole gate, and CI runs exactly this: the vendored-binary fetch and
hash check, `cargo fmt --check`, `cargo clippy -D warnings`, the Rust test
suite, the browser suite under `node --test`, the problem-bank generators and
the wire fixtures in `--check` mode, the TODO dependency graph, and syntax
checks for the shell and Playwright scripts.

Two of those are worth naming. `scripts/gen-wire-fixtures.mjs --check` fails
when a data-channel payload builder in `web/lib.js` changes without
regenerating the fixture the Rust consumer is tested against; every topic has
two implementations, and `tests/fixtures/README.md` records what happened the
last time they were only tested against themselves.
`scripts/check-todo-deps.py` fails when a task in the working task list names a
dependency that does not exist or when the dependency graph closes a cycle. The
task list is untracked, so the check skips when it is absent, which it is in a
fresh clone.

Checks that need credentials, a browser download, or a running binary stay out
of the default run:

```bash
npm install && npx playwright install chromium   # once, for the browser checks

scripts/server-check.sh          # HTTP surface against a live binary
scripts/browser-check.sh         # Playwright end-to-end through a real page
                                 # BROWSER_CHECK_AGENT=dispatch covers production:
                                 # "web" mints a room and staffs it itself
scripts/visual-parity-check.sh   # screenshots against tests/golden/visual
scripts/parity-check.sh          # data-topic and payload parity
scripts/report-parity-check.sh   # report shape parity
scripts/gemini-check.sh          # live Gemini credential check
```

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` | required | LiveKit project credentials |
| `GOOGLE_API_KEY` | required | Google AI Studio key |
| `GEMINI_LIVE_MODEL` | `gemini-3.1-flash-live-preview` | realtime voice model |
| `GEMINI_REPORT_MODEL` | `gemini-3.1-flash-lite` | grader that writes the report |
| `GEMINI_VOICE` | `Puck` | interviewer voice |
| `GEMINI_SILENCE_MS` | `700` | silence before Gemini decides the candidate has finished speaking; lower replies sooner, higher lets people pause mid-sentence |
| `CODETRIAL_WEB_ADDR` | `127.0.0.1:3000` | listen address |
| `CODETRIAL_WEB_DIR` | `web` | static asset root |
| `CODETRIAL_ROOM_PREFIX` | `interview` | prefix for generated room names |
| `CODETRIAL_DURATION_MIN` | `45` | default interview length, clamped to 10 to 90 |
| `CODETRIAL_COMPILER_EXPLORER_ENABLED` | `true` | set to `false` to withdraw C, C++ and Java test runs; invalid nonblank values fail closed |
| `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED` | `false` | opt in to forwarding candidate video frames to Gemini |
| `INTERVIEW_ROOM_NAME` | unset | fixed room for local runs; ignored when `NODE_ENV=production` |
| `NODE_ENV` | unset | `production` marks cookies `Secure` and makes `serve` refuse to start |
| `CODETRIAL_TRUSTED_PROXY_HOPS` | `0` | proxies in front of the server; `X-Forwarded-For` is read only when set |
| `SESSION_SECRET` | local default | signs the session cookie; required when `NODE_ENV=production`, since the default is published |
| `CODETRIAL_DB_PATH` | `codetrial.db` | SQLite account and report records |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | unset | optional OAuth redirect support for `/api/login` |

`/api/token` requires a recorded GitHub username session and is capped at 30
requests per minute per client; `POST /api/login`, which is where those
sessions come from, is capped separately at the same rate. Behind a reverse
proxy, set `CODETRIAL_TRUSTED_PROXY_HOPS` or every request will be charged
to the proxy.

Interviewer timing rules (silence threshold, interjection cooldowns, review
interval) are constants at the top of `src/agent.rs`.

## Layout

```
src/            Rust: web server, LiveKit runner, Gemini client, prompts, problem bank
web/            browser app, served as-is with no build step
problem-bank/   problem and judge source of truth; web/problems/ and web/judges/ are generated from it
scripts/        the test gate and the credentialed checks
tests/          Rust suites plus the browser suite under tests/browser
```

`web/problems/<id>.json`, `web/judges/<id>.json` and the problem cards in
`web/index.html` are generated from `problem-bank/problems.json` and
`problem-bank/judges.json` by `scripts/gen-problems.py` and
`scripts/gen-problem-cards.py`. The gate fails if they drift. The interviewer's
own problem notes in `src/agent.rs` are hand-written and pinned against
`tests/golden/problems.json`.

The browser fetches one statement and one judge per interview rather than a
module holding all 150. A judge carries the expected output of every test case,
so bundling them handed each candidate the answers to every other problem in the
bank. Their own problem's judge is still reachable, because the runner is in the
browser; `apply_test_results` in `src/agent.rs` says what that means for the
report.

## Troubleshooting

- "Waiting for interviewer" never resolves: the agent is not running, is in a
  different room, or points at a different LiveKit project. Start it with the
  same `LIVEKIT_URL` and keys.
- No audio: check the system and browser output device, and confirm the mic pill
  is not muted.
- The report shows an error: usually a missing or invalid `GOOGLE_API_KEY`. The
  agent log gives the exact cause.
- "Model not found" from Gemini: Google rotated the preview model id. Set
  `GEMINI_LIVE_MODEL` to a current id from
  <https://ai.google.dev/gemini-api/docs/live>.
- The first C, C++ or Java run is slow: the test drawer shows the compile state
  while Compiler Explorer builds a harness it has not seen. Cached runs usually
  return in a few hundred milliseconds.
