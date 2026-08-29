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
| Runtime | The compiled `codetrial` binary and a config file | No Node.js, Python, or `node_modules` is required. Rust dependencies are compiled into the binary; browser dependencies are vendored, checksum-pinned, and embedded, so a `web/` directory is optional and only overrides what is already inside. |

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

The binary requires `codetrial.env.local`, searching `./config/` before the
current directory. Use `--config PATH` for an alternate primary config file.
Extra `codetrial.env.*` files in the same directory are optional providers used
for pooling.

## Prebuilt binaries

Every push to `main` publishes a prerelease tagged `main-<commit sha>` under
<https://github.com/sysprog21/codetrial/releases>, newest first. The tag names
the commit and never moves, and a release that has already published is left
alone rather than rebuilt over, so a download link keeps serving the bytes it
served the first time. Only the most recent few are kept: the `release` job in
`.github/workflows/check.yml` names the retention, and older prereleases are
deleted with their tags. Nothing is required at runtime beyond the binary
itself: the browser application, its vendored assets, and the WASM are compiled
in, so there is no Node.js, no `node_modules`, and no `web/` directory to
unpack alongside it.

```bash
# Linux
tar -xzf codetrial-x86_64-unknown-linux-gnu.tar.gz
mv codetrial-x86_64-unknown-linux-gnu codetrial

# macOS
unzip codetrial-aarch64-apple-darwin.zip
xattr -d com.apple.quarantine codetrial-aarch64-apple-darwin
mv codetrial-aarch64-apple-darwin codetrial
```

A config file has to sit beside it. The binary will not run on environment
variables alone, and it refuses to start without LiveKit credentials rather than
booting a server that cannot mint a token. There is no `config/codetrial.env.example`
to copy outside a checkout, so write the three required keys yourself:

```bash
cat >codetrial.env.local <<'EOF'
LIVEKIT_URL=wss://YOUR-PROJECT.livekit.cloud
LIVEKIT_API_KEY=...
LIVEKIT_API_SECRET=...
GOOGLE_API_KEY=...
EOF
./codetrial web   # http://127.0.0.1:3000
```

`GOOGLE_API_KEY` is the optional one, and it decides which of two things this
process is. With it, the server hosts interviewers itself. Without it, it serves
the web side only and every room waits for an agent to join from elsewhere, and
it says so on startup. There is no matching line for the hosting case, so a
silent start is the one that hosts interviewers. `--config PATH` names the file
if you would rather keep it somewhere else; see
[Configuration](#configuration) for the rest.

Platform notes:

- macOS binaries carry only the ad-hoc signature the linker applies, which is
  what lets an arm64 binary run at all. They are not Developer ID signed and not
  notarized, because that needs a paid Apple Developer Program membership this
  project does not have. A downloaded `.zip` carries the quarantine attribute
  and Gatekeeper refuses it, so clear the attribute as above or build from
  source.
- Windows binaries are unsigned. SmartScreen warns on first run.
- Only `x86_64` Linux, `arm64` macOS, and `x86_64` Windows are published. Build
  from source for anything else.

## Run

Building from a checkout instead:

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
responses stop when the candidate interrupts. Say “can I get a hint?” when
needed; hints affect the communication score.

Candidates can present the interview in Google Meet by sharing the CodeTrial
tab with tab audio enabled. Meet owns the shared tab after that point, and
face-presence analysis is disabled for the session. See the
[manual check](docs/meet-tab-audio-manual-check.md) for the supported flow.

## Development

```bash
make build           # release build
make web             # run the server
make indent          # format Rust; the gate runs cargo fmt --check
make clean           # remove build output
make fetch-vendor    # fetch pinned browser assets
make verify-vendor   # verify vendor checksums
make check           # test gate plus live Gemini credential check
./scripts/test.sh    # credential-free CI gate
```

The [prerelease](#prebuilt-binaries) is published by the `build` and `release`
jobs in `.github/workflows/check.yml`, which run only on a push to `main`. No
repository secrets are involved: the macOS binary ships with the linker's ad-hoc
signature and nothing else, so a fork builds the same artifacts this repository
does. Developer ID signing and notarization would need a paid Apple Developer
Program membership, and the download instructions clear quarantine instead.

The embed falls back per file rather than wholesale, so a partially populated
web tree is filled in from the built-in copy rather than 404ing. Assets on disk
win where they exist. That root is `web/` relative to the working directory
unless `CODETRIAL_WEB_DIR` names another, so running the binary from a checkout
picks up edits with no environment variable set.

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
file. `NODE_ENV` and `INTERVIEW_ROOM_NAME` are set in
the environment instead. The common ones are:

| Variable | Default | Purpose |
|---|---|---|
| `CODETRIAL_WEB_ADDR` | `127.0.0.1:3000` | Listen address |
| `CODETRIAL_DURATION_MIN` | `45` | Default interview length (10–90) |
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
