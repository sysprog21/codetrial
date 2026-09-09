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

The browser sends code updates and test outcomes over LiveKit's data channel, so
the agent receives structured code rather than editor screenshots. Python and
JavaScript run locally; C, C++, and Java run through Compiler Explorer, so
source code leaves the browser for those three.

Camera and microphone are required to start. Audio and code snapshots stay in
memory unless [recording](#recording) is enabled, which is off by default.
Candidate video reaches Gemini only with
`CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED=true`. Face-presence analysis runs in
the browser and reports itself unavailable rather than guessing.

Jim's avatar model is Seed-san by VirtualCast, Inc.; its required credit and
license are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md), along with
every other bundled asset and its checksum.

## Dependencies by lifecycle

| Phase | Required | Notes |
|---|---|---|
| Build time | Rust stable, `curl` or `wget`, and `sha256sum` or `shasum` | Cargo resolves application crates from `Cargo.lock`. `make build` fetches the checksum-pinned browser assets on the first build. |
| Test time | Build-time tools, Node.js 18+, and Python 3 | Node runs browser and fixture checks; Python verifies generated problem-bank files. `npm ci` adds ESLint and Playwright for the full local browser gate. `ruff`, `shellcheck`, `shfmt`, `commentflow`, `actionlint`, and `cargo-audit` are optional: each gate reports that it skipped rather than failing without them. CI installs all of them, so those lanes are enforced on a pull request whatever a checkout can run; locally the `actionlint` lane also takes its container image. |
| Runtime | The compiled `codetrial` binary and a config file | No Node.js, Python, or `node_modules` is required. Rust dependencies are compiled into the binary; browser dependencies are vendored, checksum-pinned, and embedded, so a `web/` directory is optional and only overrides what is already inside. |

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
executable for a downloaded binary. The binary looks in both, and in two older
places it no longer writes to — `codetrial.env.local` in the current directory
and beside the executable. Use `--config PATH` to point at a different file. Extra `codetrial.env.*` files in the same directory are optional
providers used for pooling. If `web` mode finds no config file at all, it serves
a Setup page instead and prints the URL to open — you can skip steps 2–3 above,
fill in your keys there, and it writes them to `config/codetrial.env.local` for you. That page
serves on a loopback address only; a start that would put it on any other
address refuses instead, and writing the file yourself is the way to deploy for
other people. See [docs/install.md](docs/install.md).

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

`web` is the only serving mode, local and deployed alike: the difference between
the two is the configuration, not the command. A `web` process hosts agents for
at most `CODETRIAL_MAX_CONCURRENT_INTERVIEWS` interviews at once, 16 by default;
past that, dispatch is refused and the candidate waits.

Set `CODETRIAL_COMPILER_EXPLORER_ENABLED=false` to leave C, C++, and Java
editors available while disabling their remote test runs.

## Interview behavior

Jim asks short, targeted questions between logical blocks, offers progressive
conceptual hints, and uses the latest test run in the final assessment. Voice
responses stop when the candidate interrupts. Say "can I get a hint?" when
needed; hints affect the communication score.

Candidates can present the interview in Google Meet by sharing the CodeTrial tab
with tab audio enabled. Meet owns the shared tab after that, and face-presence
analysis is disabled for the session. See the
[manual check](docs/meet-tab-audio-manual-check.md) for the supported flow.

The lobby offers 30, 45, and 60 minutes and the endpoints accept 10 to 90.
Recording lowers that ceiling to `CODETRIAL_RECORDING_MAX_MINUTES`, and the
lobby is told the ceiling rather than left to discover it. The rules, and why
the offered length and the enforced length come from one function, are in
[docs/interview-length.md](docs/interview-length.md).

## Development

```bash
make build           # release build
make web             # run the server
make indent          # run the formatters; the gate checks their result
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
file; `NODE_ENV` and `INTERVIEW_ROOM_NAME` are set in the environment instead.
The common ones:

| Variable | Default | Purpose |
|---|---|---|
| `CODETRIAL_WEB_ADDR` | `127.0.0.1:3000` | Listen address |
| `CODETRIAL_DURATION_MIN` | `45` | Interview length preselected in the lobby (10–90); see [interview length](docs/interview-length.md) |
| `GEMINI_LIVE_MODEL` | `gemini-3.1-flash-live-preview` | Realtime interviewer model |
| `GEMINI_REPORT_MODEL` | `gemini-3.1-flash-lite` | Report model |
| `CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED` | `false` | Forward candidate video to Gemini |
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
| [Development](docs/development.md) | The gate, formatters, hooks, generated files, releases |
| [Installing a binary](docs/install.md) | Published binaries, platform notes, the config beside them |
| [Interview length](docs/interview-length.md) | What the lobby offers and what the endpoints enforce |
| [Integrity evidence](docs/integrity-evidence.md) | Response windows, and what CodeTrial declines to look for |
| [Third-party notices](THIRD-PARTY-NOTICES.md) | Bundled software, model attribution, licenses, checksums |
| [Avatar contract](docs/avatar-contract.md) | Renderer behavior, asset limits, privacy, accessibility |
| [Provider pooling](docs/providers.md) | Spreading rooms over several LiveKit projects |
| [Recording contract](docs/recording-contract.md) | Provisioning, consent, delivery, retention, operations |
| [Google Meet manual check](docs/meet-tab-audio-manual-check.md) | Verifying tab-audio presentation |
| [Response window manual check](docs/integrity-response-window-check.md) | Reading a real replay's windows as a person would |
| [LiveKit troubleshooting](docs/livekit-connection-troubleshooting.md) | Telling four connection failures apart |
| [Observable delivery policy](docs/observable-delivery-policy.md) | What a report may and may not assess |
| [Interview contract versions](docs/interview-contract-versions.md) | The five versions every report carries |
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
