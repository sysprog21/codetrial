# CodeTrial

CodeTrial is a live technical-interview simulator. Its AI interviewer, Jim,
observes the candidate's code and test results, conducts a voice interview, and
produces a scored report at the end of the session.

The project is a single Rust binary that serves the browser application, mints
LiveKit tokens, and runs the interviewer agent.

## Architecture

```text
┌────────────────────────────┐ WebRTC: mic + camera ┌─────────────────────┐
│ Browser (static, no build) ├─────────────────────▶│ LiveKit Cloud       │
│  · editor + syntax colors  ├─────────────────────▶│ (SFU)               │
│  · problem panel, timer    │  data channel        └─────┬───────────────┘
│  · test runners            │  code_update, control,     │
│  · whiteboard              │  test_results, report,     │
│  · report + history        │  board_image               │
└────────────┬───────────────┘                            ▼
             │                            ┌──────────────────────────────────┐
             │ /api/*                     │ Rust agent (LiveKit runner)      │
             ▼                            │  · Gemini Live: voice, vision    │
┌────────────────────────────┐            │  · Gemini Flash: report scoring  │
│ Rust web server            │            └──────────────────────────────────┘
│  · token minting           │
│  · GitHub handle           │
│  · SQLite report history   │
└────────────────────────────┘
```

The browser sends code updates and test outcomes over LiveKit's data channel, so
the agent receives structured code rather than editor screenshots. Python and
JavaScript run locally; C, C++, and Java run through Compiler Explorer, so
source code leaves the browser for those three.

The lobby also offers a whiteboard interview, which takes the same problem bank
and the same six steps and swaps the editor and the test runner for a board.
Nothing runs: the candidate draws their examples and traces one by hand. The
board is exported as an image a moment after each stroke settles and reaches
the interviewer over its own byte stream on the same data channel, and
`read_board` puts the latest one back in front of it on request. The final
board is attached to the report request, so the reviewer grades the drawing
rather than an empty editor, and the recording keeps the drawing as the
strokes that made it, which is what lets the replay redraw any moment of it.
How to hold one is under [Whiteboard interviews](#whiteboard-interviews).

Audio and code snapshots stay in memory unless [recording](#recording) is
enabled, which is off by default. Candidate video reaches Gemini only with
`CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED=true`, and never in a whiteboard
interview, where the board travels the stream a camera frame would.
Face-presence analysis runs in the browser and reports itself unavailable
rather than guessing.

## Dependencies by lifecycle

| Phase | Required | Notes |
|---|---|---|
| Build time | Rust stable, `curl` or `wget`, and `sha256sum` or `shasum`; Linux also needs Clang 21+ and GLib development headers | Cargo resolves application crates from `Cargo.lock`. `make build` fetches the checksum-pinned browser assets on the first build. |
| Test time | Build-time tools, Node.js 18+, and Python 3.9+ | Node runs browser and fixture checks; Python verifies generated problem-bank files. `npm ci` adds pinned Prettier, ESLint and Playwright for the full local browser gate. `ruff`, `shellcheck`, `shfmt`, `commentflow`, `actionlint`, and `cargo-audit` are optional: each gate reports that it skipped rather than failing without them. CI installs all of them, so those lanes are enforced on a pull request whatever a checkout can run; locally the `actionlint` lane also takes its container image. |
| Runtime | The compiled `codetrial` binary and a config file | No Node.js, Python, or `node_modules` is required. Rust dependencies are compiled into the binary; browser dependencies are vendored, checksum-pinned, and embedded, so a `web/` directory is optional and only overrides what is already inside. |

On Linux x86_64, `make` downloads the checksum-pinned WebRTC compiler into
`target/clang` and uses it unless `CXX` is already set (requires `curl`,
`sha256sum`, `flock`, and `tar` with xz support). Plain `cargo` needs it named:
`CXX="$PWD/target/clang/bin/clang++" cargo build`, or `CXX` set to an installed
Clang 21+ compiler.

Running an interview also needs a LiveKit Cloud project and a Google AI Studio
API key. C, C++, and Java test runs additionally use the remote Compiler
Explorer service.

## Setup

1. Create a LiveKit project at <https://cloud.livekit.io> and a Google AI
   Studio key at <https://aistudio.google.com/apikey>.
2. Create local configuration:

   ```bash
   cp config/codetrial.env.example config/codetrial.env.local
   ```

3. Set `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, and
   `GOOGLE_API_KEY` in that file.

One installation keeps its config file and its account database together in a
`config/` directory: `config/` in a checkout, and a `config/` beside the
executable for a downloaded binary. Use `--config PATH` to point at a different
file. Extra `codetrial.env.*` files in the same directory are optional providers
used for pooling.

If `web` mode finds no config file at all, it serves a Setup page and prints the
URL to open, so steps 2 and 3 above can be skipped: fill in your keys there and
it writes `config/codetrial.env.local` for you. That page serves on a loopback
address only and refuses any other, so writing the file yourself is the way to
deploy for other people. See [docs/install.md](docs/install.md).

## Run

Prebuilt binaries for Linux, macOS, and Windows are published for every build of
`main`. What they carry, the config file each needs beside it, and the platform
notes are in [docs/install.md](docs/install.md). From a checkout instead:

```bash
INTERVIEW_ROOM_NAME=interview-local make web
```

Open <http://localhost:3000>, choose a problem and duration, then complete the
media preflight. `INTERVIEW_ROOM_NAME` pins every interview to that one room
name, which is what a local run wanting a stable URL usually wants; without it
each session gets a random room. The pin is dropped for a session if its LiveKit
project has run out of connection minutes, so a spent project costs the stable
URL rather than the interview. See [docs/providers.md](docs/providers.md).

| Command | Use |
|---|---|
| `cargo run -- web` | The server; hosts interviewers too when `GOOGLE_API_KEY` is set |
| `cargo run -- run-livekit ROOM` | Interviewer only, for one named room |
| `cargo run -- check-gemini` | Verify Gemini credentials |
| `codetrial --version` | Print the version and the commit the binary was built from |

`web` is the only serving mode, local and deployed alike: the difference between
the two is the configuration, not the command. A `web` process hosts agents for
at most `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` interviews at once, 16 by default;
past that, dispatch is refused and the candidate waits.

Set `CODETRIAL_COMPILER_EXPLORER_ENABLED=false` to withdraw C, C++, and Java,
whose tests run remotely: their language tabs are disabled, since code that
cannot be run cannot complete the REACTO Test step.

## Interview behavior

Jim asks short, targeted questions between logical blocks, offers progressive
conceptual hints, and uses the latest test run in the final assessment. Voice
responses stop when the candidate interrupts. Say "can I get a hint?" when
needed. In a coding-only session Jim never ends the interview early: only the
timer or the candidate's End interview button closes it.

The media preflight always requires confirmed output and a working microphone,
and its Back to lobby button releases every device without starting. For an
interview that is not recorded, a candidate may continue without a camera when
it is unavailable or declined; the signed integrity trail and the report record
that neutral condition and why. A recorded interview still requires its camera
before it can start.

Candidates can present the interview in Google Meet by sharing the CodeTrial tab
with tab audio enabled. Meet owns the shared tab after that, and face-presence
analysis is disabled for the session. See the
[manual check](docs/meet-tab-audio-manual-check.md) for the supported flow.

The lobby offers 30, 45, and 60 minutes and the endpoints accept 10 to 90.
Recording lowers that ceiling to `CODETRIAL_RECORDING_MAX_MINUTES`, and the
lobby is told the ceiling rather than left to discover it. The rules, and why
the offered length and the enforced length come from one function, are in
[docs/interview-length.md](docs/interview-length.md).

## Whiteboard interviews

A whiteboard interview practices the round where there is a marker and a blank
board instead of an editor. Nothing compiles and no test runs, so the answer is
what you can draw, say aloud and defend by tracing an example by hand.

1. In the lobby, pick a problem, a length and a loop as usual, then press
   **Whiteboard** beside **Editor** in the same row. The note under the row
   confirms the switch. Start the interview and complete the media preflight.
2. The board takes the place of the editor. It accepts strokes only while the
   interview is live: not before the room is joined, not while it is paused,
   and not during the behavioral round of a Coding + behavioral loop.
3. Draw with the mouse, a stylus or a finger. A tablet with a pen is closest to
   a real board: while the pen is down, a palm resting on the screen is ignored,
   and a second finger never joins the first finger's stroke.
4. Use the toolbar above the board:

   | Control | What it does |
   |---|---|
   | Pen colors | Black, red, blue and green. Write in black and mark up in the others. |
   | Eraser | Rubs out what it passes over. The three size buttons beside it pick a size and the eraser together: small for one character, medium for one line, large for a region. Pick a pen color, or press Eraser again, to draw again. |
   | Undo, Redo | Step back and forward one stroke or erasure at a time. |
   | Clear board | Wipes the board in one step, which Undo brings back. |

5. Talk while you draw. Jim hears you live and is sent the whole board a moment
   after you stop drawing, so pause briefly after a figure you want discussed.
   When a mark is hard to read, Jim asks what it stands for rather than
   guessing, and what you said while drawing it is how it gets read.
6. Follow the checklist, which renames the six steps for the board: Repeat,
   Example (draw one ordinary case and one edge case), Approach (draw the idea
   and its cost), Trace (walk an example through the drawing), Edge cases, and
   Complexity. Pseudocode or a diagram is fine for Approach; Trace is where you
   prove it, so step through your own example and say what each variable holds.
7. Clear the board between steps whenever it fills up. The board is captured
   as each step closes, so a clear costs nothing already graded.

Write large and leave space between figures: the board reaches Jim as an image,
and small, crowded handwriting is the hardest part of it to read. Say "can I get a hint?" exactly as in an editor interview.

The report opens on the board, step by step: the board as each step closed, its
score and Jim's notes, then your final board. The work score is named Board work
and grades the drawing and the trace instead of code. The step images are not
saved with the report, so a report reopened from history says so in their place.

## Scoring

A report model reads the interview brief (the final code, the transcript, the
test runs, the hints and Jim's running notes) and writes the report. Every
score is that model's judgment of the evidence, not a formula, and every claim
in the feedback has to point at something in that evidence.

| Score | What it assesses |
|---|---|
| Coding, 0 to 100 | Correctness of the final code read against the problem (not the pass count), edge cases, the algorithm and its reasoning, implementation quality, testing, optimization, and how the approach compares with the optimal one. An empty or non-functional editor caps it below 30. |
| Communication, 0 to 100 | How clearly you narrated: restating the problem, working an example, explaining the algorithm and its complexity, predicting tests, discussing optimization, and answering follow-ups accurately. STAR completeness counts only when a behavioral question was asked. |
| Decision | HIRE only for a working, reasonably optimal solution and clear communication, judged against a mid-level onsite bar. The practice level you pick changes the wording of the summary, not the bar. |

Speech is machine-transcribed, so filler words, accent, phrasing, and typing
speed are ignored. Camera and microphone state are recorded as session
conditions and never read as confidence, eye contact, or body language.

The report also scores each phase of the framework you worked through: Repeat,
Example, Algorithm, Coding, Test, and Optimizations for REACTO, and Situation,
Task, Action, and Result for STAR when a behavioral question was asked. A phase
with no evidence is shown as not assessed, never as zero.

| Band | Meaning |
|---|---|
| 90 to 100 | Complete, precise, and independent |
| 75 to 89 | Sound with a minor gap |
| 60 to 74 | Partially demonstrated with a material gap |
| 40 to 59 | Weak or substantially incomplete |
| 0 to 39 | Observed incorrect or missing despite a clear opportunity |

Phase scores are coaching feedback. They are not weighted parts of the Coding
or Communication score, and they have not been [calibrated](docs/rubric-calibration.md)
against human reviewers, so do not read them as hiring evidence.

The report model sees how many hints you had, how far up the three-rung hint
ladder you went, and how many Jim volunteered rather than you asked for. It
treats them as context and never as a fixed deduction; a hint you asked for and
then relied on weighs more than one Jim offered unprompted.

## Development

```bash
make build           # release build
make web             # run the server
make indent          # format Rust, shell, Python, HTML and JavaScript; run Clippy
make clean           # remove build output
make fetch-vendor    # fetch pinned browser assets
make verify-vendor   # verify vendor checksums
make check           # test gate plus live Gemini credential check
make hooks           # install the git hooks; uninstall-hooks removes them
./scripts/test.sh    # credential-free CI gate
```

The gate ends by printing what it did not check, because several lanes are
optional and a run that skips them still exits 0. That summary, the formatter
chain, the git hooks, the generated problem files, and where a slow run's time
actually goes are in [docs/development.md](docs/development.md).

## Configuration

`config/codetrial.env.example` documents the variables that belong in a config
file; `NODE_ENV`, `INTERVIEW_ROOM_NAME` and `CODETRIAL_GEMINI_REST_BASE` are set
in the environment instead.
The common ones:

| Variable | Default | Purpose |
|---|---|---|
| `CODETRIAL_WEB_ADDR` | `127.0.0.1:3000` | Listen address |
| `CODETRIAL_DURATION_MIN` | `45` | Interview length preselected in the lobby (10–90); see [interview length](docs/interview-length.md) |
| `GEMINI_LIVE_MODEL` | `gemini-3.1-flash-live-preview` | Realtime interviewer model |
| `GEMINI_REPORT_MODEL` | `gemini-3.1-flash-lite` | Report model |
| `CODETRIAL_MAX_INTERIM_REVIEWS` | `6` | Quiet-pause report-model reviews per interview; `0` disables them and `72` is the maximum |
| `CODETRIAL_GEMINI_REPLY_TIMEOUT_S` | `45` | Seconds an owed interviewer reply may go without output before the Live socket is replaced (20–120); see [degradation controls](docs/provider-cost-and-degradation.md) |
| `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED` | `false` | Forward candidate video to Gemini, one low-resolution frame in five seconds; ignored in a whiteboard interview |
| `CODETRIAL_COMPILER_EXPLORER_ENABLED` | `true` | Enable remote C, C++, and Java runs |
| `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` | `16` | Interviews one `web` process hosts agents for |

Two more matter in production. `SESSION_SECRET` signs the session cookie, and
its built-in default is a string published in this repository, so the server
refuses to start on it with `NODE_ENV=production`, on any listen address that is
not loopback, or with `CODETRIAL_TRUSTED_PROXY_HOPS` above zero. A server other
machines can reach signs cookies anyone can forge, whether or not its operator
remembered to say it was production, and a loopback socket behind a proxy is
reachable too. Naming the built-in value explicitly does not satisfy the check.
`CODETRIAL_TRUSTED_PROXY_HOPS` defaults to `0`, and `X-Forwarded-For` is read
only once you set it to the number of proxies in front of the server.

Reports are stored in browser storage and, for signed-in users, in local SQLite.
GitHub handles are self-declared unless OAuth is configured; they are not proof
of identity. An account holds at most 200 reports of 64 KiB each; `POST
/api/reports` answers `507` once full, and rewriting a report the account
already owns keeps working at the ceiling so a finished interview can always
save.

Serving more than one LiveKit project from one deployment is in
[docs/providers.md](docs/providers.md).

The report and the quiet-pause reviews can be written by a model on your own
hardware instead. Point `CODETRIAL_GEMINI_REST_BASE` at a server that answers
Gemini's `generateContent`, such as `scripts/gemini-shim.py` in front of
llama.cpp's `llama-server`, which needs `--jinja` for the tool calls; the
shim's docstring has the commands. The live interviewer still talks to
Gemini. Any base other than Google's gets longer report deadlines, 45 seconds
a call and 250 in all instead of 20 and 125, since a 12B model on one 16 GB GPU
takes 14 to 32 seconds per report.

The shim carries function calls too, so the interviewer behaviour check in
[docs/development.md](docs/development.md#checks-outside-the-gate) runs against
the same base. With Gemma 4, start it with `--thinking off`. That check names no
thinking budget, and with thinking on gemma-4-12b sometimes repeated itself to
the output limit or put its reasoning in the reply. With it off the check
passed 18 of 21 problem runs across seven runs, about 24 seconds a run against
240; each miss was a second hint request answered without calling `log_hint`.

## Recording

Recording is off by default. Enabling it requires a separate LiveKit project,
private GCS staging bucket, Shared Drive, service account, GitHub OAuth, and
candidate consent. It also caps how long an interview can run:
`CODETRIAL_RECORDING_MAX_MINUTES` (45 by default) becomes the longest length the
lobby offers, described in [docs/interview-length.md](docs/interview-length.md).
The full configuration, lifecycle, retention policy, and credentialed acceptance
check are in [docs/recording-contract.md](docs/recording-contract.md).

The credentialed recording acceptance records frame dimensions, rate, bitrate,
and duration from `ffprobe`. Candidate-visible seconds, audio-source count, and
production-avatar rendering are operator attestations until recorder-side
telemetry exists; they are not presented as measurements.

## Integrity evidence

Integrity features produce evidence for a person to review. Nothing here scores
a candidate, and nothing here is a claim that anybody cheated.

The replay page shows a *response window* for each interviewer turn, with what
that number is worth written beside it: whose clock measured it, what bounds how
long one can run, and why a long one is a place to go and watch rather than a
finding. Overlay detection and gaze reading are refused, and the reasons are
recorded rather than left implicit. See
[docs/integrity-evidence.md](docs/integrity-evidence.md).

## Further documentation

| Document | Covers |
|---|---|
| [Contributing](CONTRIBUTING.md) | Reporting a bug, the commit and title rules, opening a pull request |
| [Development](docs/development.md) | The gate, formatters, hooks, generated files, releases |
| [Installing a binary](docs/install.md) | Published binaries, platform notes, the config beside them |
| [Interview length](docs/interview-length.md) | What the lobby offers and what the endpoints enforce |
| [Integrity evidence](docs/integrity-evidence.md) | Response windows, and what CodeTrial declines to look for |
| [Third-party notices](THIRD-PARTY-NOTICES.md) | Bundled software, licenses, checksums |
| [Avatar contract](docs/avatar-contract.md) | Renderer behavior, asset limits, privacy, accessibility |
| [Provider pooling](docs/providers.md) | Spreading rooms over several LiveKit projects |
| [Recording contract](docs/recording-contract.md) | Provisioning, consent, delivery, retention, operations |
| [Google Meet manual check](docs/meet-tab-audio-manual-check.md) | Verifying tab-audio presentation |
| [Response window manual check](docs/integrity-response-window-check.md) | Reading a real replay's windows as a person would |
| [LiveKit troubleshooting](docs/livekit-connection-troubleshooting.md) | Telling four connection failures apart |
| [Observable delivery policy](docs/observable-delivery-policy.md) | What a report may and may not assess |
| [Interview contract versions](docs/interview-contract-versions.md) | The five versions every report carries |
| [Evidence runtime](docs/evidence-runtime.md) | The session evidence ledger and how code and test observations are recorded |
| [Adding a problem](docs/adding-a-problem.md) | Add an imported or original interview exercise |
| [Rubric calibration](docs/rubric-calibration.md) | Calibration status of the framework scores |
| [Provider cost and degradation](docs/provider-cost-and-degradation.md) | Gemini budgets, restarts, concurrency |

## Repository layout

```text
src/            Rust web server, interviewer, and problem bank
web/            Static browser application
problem-bank/   Problem and judge source of truth
config/         Env file templates and per-provider credentials
docs/           Contracts and operational notes
scripts/        Build, verification, and integration checks
tests/          Rust and browser tests
```

## Troubleshooting

- Waiting for interviewer: start the agent in the same LiveKit room and project.
- No audio: check browser and system output devices, then the microphone
  mute state.
- Report error: verify `GOOGLE_API_KEY`; the agent log contains the exact cause.
- Gemini model not found: set `GEMINI_LIVE_MODEL` to a current model from
  <https://ai.google.dev/gemini-api/docs/live>.
