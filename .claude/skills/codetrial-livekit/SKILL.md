---
name: codetrial-livekit
description: Where CodeTrial's LiveKit code lives and how to change it - the Rust interviewer's room loop, join tokens, agent dispatch, the management API, agent state, egress and the vendored transport, looking LiveKit facts up for the pinned crate, and upgrading the livekit crates. Use when touching src/livekit*, src/dispatch.rs, src/token.rs or src/web/token.rs, debugging "can't hear Jim", a 401, a 429 or an empty room, checking LiveKit credentials, asking whether LiveKit supports something, or reaching for lk agent debugger, console, simulate or deploy, which need an Agents SDK project this repo does not have. Data-channel topics are codetrial-web; validating a change or bringing the server up is codetrial-verify.
---

# LiveKit in CodeTrial

The interviewer is a Rust participant: the `web` server mints the browser's
join token and dispatches one agent per room, which joins with the `livekit`
crate and bridges the room to Gemini Live. No Agents SDK worker exists, so `lk
agent` cannot drive it. A room change is checked by the credentialed browser
lanes in codetrial-verify, and a full live interview only when those cannot
show it.

## Where LiveKit is touched

| Concern | Where |
| --- | --- |
| The room loop, media, and the management API over HTTP | `src/livekit.rs` and `src/livekit/`, whose module header maps the regions |
| Turn state and what an event does to it | `src/livekit/turn/` (`RuntimeActivity`; its intent methods are under `turn/activity/`, the latency and timing observers in `turn/probes.rs`); ask the owner for the effect, do not write its fields. The candidate's receivers and queued audio are `src/livekit/streams.rs`. After moving any of these run `tools/check-structure` ([Structure lints](../codetrial-improve/rust.md#structure-lints)) |
| Join tokens | `src/token.rs`, served by `src/web/token.rs` |
| One agent per room, no-show release | `src/dispatch.rs` |
| Agent state the page reads (`lk.agent.state`) | set in `src/livekit.rs`, decided by `docs/avatar-contract.md` |
| Recording through egress | `src/recording/`, decided by `docs/recording-contract.md` |
| The SDK's transport patch | `vendor/livekit-net`, whose README says what changed and when to delete it |

`docs/providers.md` covers connection minutes and refused credentials.

## Look a fact up for the crate this tree pins

A remembered LiveKit flag, default or signature is a guess, and a wrong one
costs a full rebuild. Check, in this order:

1. `--help` on the installed `lk`, for any CLI question.
2. The source of the version `Cargo.lock` pins, in the cargo registry, for
   anything the Rust SDK does. LiveKit's docs mostly show the Python and Node
   SDKs, so they say what the server and protocol do, not this crate's API.
3. `lk docs` for server behavior, limits and changelogs.

```bash
lk docs search "room composite egress"   # see lk docs --help
```

Say so when a fact could not be confirmed, and mark a call site written
without a lookup as unverified.

## When the browser cannot hear Jim

Read the console's exact error string, not the banner, against the table in
`docs/livekit-connection-troubleshooting.md`, and never reload on a `429`:
every retry extends the limit. Check first whether the agent is in the room:

```bash
lk room participants list --room "$room"   # see --help
```

`lk` reads `LIVEKIT_URL`, `LIVEKIT_API_KEY` and `LIVEKIT_API_SECRET` from the
environment. The primary project's are in `config/codetrial.env.local`, and
each extra project's in its own file (`docs/providers.md`). Load one the way
`scripts/browser-check.sh` does (`set -a`, source the file, `set +a`) in the
command's own shell, and never print, paste or log them. Checking a key and
secret against a project without the browser is in the troubleshooting doc's
[credentials section](../../../docs/livekit-connection-troubleshooting.md#verifying-credentials-without-the-browser).

## Rules for room code

- Time to first audio is what the candidate feels. Measure it with the phases
  in `docs/latency-observability.md` before changing anything that claims to
  speed it up.
- The nudges and the watchdog are set in `docs/provider-cost-and-degradation.md`
  and when a turn ends in `docs/turn-timing.md`; change silence behavior there
  rather than by feel.
- The model interprets; Rust owns the state. The timer, the report, integrity
  evidence and every effect stay in code, and a tool call is the model's
  reading, not proof.
- Work after the interview (the report and its delivery) completes inside the
  room's lifecycle, or a dropped connection cuts it off.
- Each subcommand builds its tokio runtime in `src/main.rs`, and room code runs
  on it already async: never build another runtime under it or block on a
  future inside the loop, which stalls media for every room the process hosts.
- The lifecycle logs name their room (`room=`, as in `src/dispatch.rs`), so one
  interview can be traced from dispatch to report. Give a new lifecycle line the
  room too.

## Upgrading the LiveKit crates

The crates are pinned in `Cargo.lock`, and a bump is slow to build
(`docs/development.md` says why). Read the changelog first, check whether the
bump meets the `vendor/livekit-net` README's removal condition, then run the
Rust lanes and a live interview through codetrial-verify: some regressions only
show in a real room with a real browser.

## Local evidence at closeout

Before ending this invocation, follow the shared
[local evidence contract](../codetrial-conventions/SKILL.md#local-evidence-at-closeout)
and explicitly record its report using
[experience.md](../codetrial-review/experience.md). Include this skill in
`skills_used`; a combined invocation may use one report for its examined head.
Local experience is a lead to recheck, never a maintainer ruling.
