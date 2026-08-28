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
│  · report + history        │  test_results, report      │
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

The browser sends code updates and test outcomes over LiveKit's data channel;
the agent receives structured code, not editor screenshots. Candidate code runs
locally for Python and JavaScript. C, C++, and Java runs use Compiler Explorer,
so source code leaves the browser for those languages.

Camera and microphone are required to start an interview. Audio and code
snapshots are kept in memory only unless recording is enabled, which is off by
default and described under [Recording](#recording). Candidate video is not
forwarded to Gemini
unless `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED=true` is set. Face-presence
analysis runs locally in the browser; when unsupported, it is reported as
unavailable rather than inferred. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)
for bundled software, models, licenses, and integrity verification.
Jim's avatar model is Seed-san by VirtualCast, Inc.; its required credit and
license are recorded in that notice.

## Dependencies by lifecycle

| Phase | Required | Notes |
|---|---|---|
| Build time | Rust stable, `curl` or `wget`, and `sha256sum` or `shasum` | Cargo resolves application crates from `Cargo.lock`. `make build` fetches the checksum-pinned browser assets on the first build. |
| Test time | Build-time tools, Node.js 18+, and Python 3 | Node runs browser and fixture checks; Python verifies generated problem-bank files. `npm ci` adds ESLint and Playwright for the full local browser gate. `shellcheck`, `actionlint`, and `cargo-audit` are optional: each gate reports that it skipped rather than failing without them. |
| Runtime | The compiled `codetrial` binary and its `web/` assets | No Node.js, Python, or `node_modules` is required. Rust dependencies are compiled into the binary; browser dependencies are vendored and checksum-pinned. |

Running an interview also requires a LiveKit Cloud project and a Google AI Studio
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

The binary loads `config/codetrial.env.local` automatically. Use `--config PATH`
for another file, or `CODETRIAL_SKIP_CONFIG=1` to disable that behavior.

## Run

```bash
INTERVIEW_ROOM_NAME=interview-local make serve
```

Open <http://localhost:3000>, choose a problem and duration, then complete the
media preflight. `INTERVIEW_ROOM_NAME` pins every interview to that one room
name, which is what a local run wanting a stable URL usually wants; without it
each session gets a random room. The pin is dropped for a session if its LiveKit
project has run out of connection minutes, so a spent project costs the stable
URL rather than the interview. See [docs/providers.md](docs/providers.md).

| Command | Use |
|---|---|
| `cargo run -- serve` | Local web server and interviewer in one process |
| `cargo run -- web` | Web server that dispatches an interviewer for each room |
| `cargo run -- run-livekit ROOM` | Interviewer only |
| `cargo run -- check-gemini` | Verify Gemini credentials |

`serve` is for local work and refuses `NODE_ENV=production`; use `web` in
production. A `web` process handles at most 16 concurrent interviews.

Set `CODETRIAL_COMPILER_EXPLORER_ENABLED=false` to leave C, C++, and Java
editors available while disabling their remote test runs.

## Interview behavior

Jim asks short, targeted questions between logical blocks, offers progressive
conceptual hints, and uses the latest test run in the final assessment. Voice
responses stop when the candidate interrupts. Say “can I get a hint?” when
needed; hints affect the communication score.

Candidates can present the interview in Google Meet by sharing the CodeTrial
tab with tab audio enabled. Meet owns the shared tab after that point, and
face-presence analysis is disabled for the session. See the
[manual check](docs/meet-tab-audio-manual-check.md) for the supported flow.

## Development

```bash
make build           # release build
make serve           # local server and interviewer
make web             # web server only
make indent          # format Rust; the gate runs cargo fmt --check
make clean           # remove build output
make fetch-vendor    # fetch pinned browser assets
make verify-vendor   # verify vendor checksums
make check           # test gate plus live Gemini credential check
./scripts/test.sh    # credential-free CI gate
```

Every update to `main` publishes a `latest` prerelease with one self-contained
`codetrial` executable for Linux, macOS, and Windows. It serves the browser
application from inside the binary, and falls back to those built-in assets per
file, so a partially populated web tree is filled in rather than 404ing.

Assets on disk win where they exist. That root is `web/` relative to the working
directory unless `CODETRIAL_WEB_DIR` names another, so running the binary from a
checkout picks up edits with no environment variable set.

The macOS build is unsigned. A downloaded copy carries the quarantine attribute
and refuses to open until you clear it:

```shell
xattr -d com.apple.quarantine codetrial-aarch64-apple-darwin
```

`web/problems/`, `web/judges/`, and the problem cards in `web/index.html` are
generated from `problem-bank/`. Regenerate them after editing that source, or
the gate fails on the drift:

```bash
python3 scripts/gen-problems.py
python3 scripts/gen-problem-cards.py
```

`scripts/top-interview-150.json` records which problems the study plan asks
for. Refresh it from LeetCode with:

```bash
python3 scripts/gen-problems.py --sync-study-plan
```

The sync refuses to write when the plan and `problem-bank/` disagree, naming
the problems each side is missing. Port those first. `--check` holds the
committed manifest to the same rule, so drift fails the gate offline.

Two commands cover the porting. `--plan-drift` asks LeetCode what changed
without writing anything, and `--scaffold SLUG` prints the `problems.json` and
`judges.json` entries for one problem, filled in as far as LeetCode's metadata
reaches:

```bash
python3 scripts/gen-problems.py --plan-drift
python3 scripts/gen-problems.py --scaffold reverse-linked-list-ii
```

It stops where it has to. The statement, examples and constraints stay empty
because the fetcher never asks LeetCode for problem prose, and each judge case
carries its input without an expected value, because `exampleTestcases` is
inputs only. Both are written by someone who has read the problem.

The end-to-end browser check additionally needs Playwright and Chromium:

```bash
npm ci && npx playwright install chromium
scripts/browser-check.sh
```

Checks that need credentials or a running service stay out of the gate and are
run on their own: `scripts/server-check.sh`, `gemini-check.sh`,
`parity-check.sh`, `report-parity-check.sh`, `visual-parity-check.sh`,
`recording-integration.sh`, and `recording-provision-check.sh`.

## Configuration

`config/codetrial.env.example` documents the variables that belong in a config
file. `NODE_ENV`, `CODETRIAL_SKIP_CONFIG`, and `INTERVIEW_ROOM_NAME` are set in
the environment instead. The common ones are:

| Variable | Default | Purpose |
|---|---|---|
| `CODETRIAL_WEB_ADDR` | `127.0.0.1:3000` | Listen address |
| `CODETRIAL_DURATION_MIN` | `45` | Default interview length (10–90) |
| `GEMINI_LIVE_MODEL` | `gemini-3.1-flash-live-preview` | Realtime interviewer model |
| `GEMINI_REPORT_MODEL` | `gemini-3.1-flash-lite` | Report model |
| `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED` | `false` | Forward candidate video to Gemini |
| `CODETRIAL_COMPILER_EXPLORER_ENABLED` | `true` | Enable remote C, C++, and Java runs |

Two more matter in production. `SESSION_SECRET` signs the session cookie and its
built-in default is a published string, so with `NODE_ENV=production` the server
refuses to start until you set a real one. `CODETRIAL_TRUSTED_PROXY_HOPS`
defaults to `0`, and `X-Forwarded-For` is read only once you set it to the number
of proxies in front of the server.

Reports are stored in browser storage and, for signed-in users, in local SQLite.
GitHub handles are self-declared unless OAuth is configured; they are not proof
of identity. An account holds at most 200 reports of 64 KiB each; `POST
/api/reports` answers `507` once full, and rewriting a report the account
already owns keeps working at the ceiling so a finished interview can always
save.

Serving more than one LiveKit project from one deployment is in
[docs/providers.md](docs/providers.md).

## Recording

Recording is off by default. Enabling it requires a separate LiveKit project,
private GCS staging bucket, Shared Drive, service account, GitHub OAuth, and
candidate consent. The full configuration, lifecycle, retention policy, and
credentialed acceptance check are in [docs/recording-contract.md](docs/recording-contract.md).

## Further documentation

| Document | Covers |
|---|---|
| [Third-party notices](THIRD-PARTY-NOTICES.md) | Bundled software, model attribution, licenses, and checksum verification |
| [Avatar contract](docs/avatar-contract.md) | Renderer behavior, asset limits, privacy, and accessibility |
| [Provider pooling](docs/providers.md) | Spreading rooms over several LiveKit projects |
| [Recording contract](docs/recording-contract.md) | Provisioning, consent, delivery, retention, and operations |
| [Google Meet manual check](docs/meet-tab-audio-manual-check.md) | Verifying tab-audio presentation |
| [LiveKit connection troubleshooting](docs/livekit-connection-troubleshooting.md) | Telling four connection failures apart, and checking credentials without the browser |

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
- No audio: check browser and system output devices, then the microphone mute state.
- Report error: verify `GOOGLE_API_KEY`; the agent log contains the exact cause.
- Gemini model not found: set `GEMINI_LIVE_MODEL` to a current model from
  <https://ai.google.dev/gemini-api/docs/live>.
