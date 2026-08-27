# LiveKit connection troubleshooting

A record of one debugging session where the browser could not hear Jim, what
was actually wrong, and what was changed. The interesting part is not the fix,
which is four lines, but that four different failures wore the same symptom:
the interview page falling back to offline practice mode.

## The symptom is not the diagnosis

`connect` in `web/interview.js` puts the thrown message into the banner, so
most failures do say why. The one that does not is the one that cost this
session: a CSP refusal reaches the SDK as `Failed to fetch`, and the banner
repeats it faithfully. A policy fault then reads as a network fault. The
console is where the cases separate, and the exact error string is what tells
them apart:

| Console error | Layer | Meaning |
|---|---|---|
| `Failed to fetch` on `/api/token` | this server | the Rust process is not running, or not reachable |
| `Refused to connect ... Content Security Policy` | this server's CSP | the origin is not in `connect-src` |
| `401 invalid token` | LiveKit | the API secret does not match the API key |
| `401 invalid API key for domain` | LiveKit | the key belongs to a different project |
| `429 Too Many Requests` | LiveKit | rate limited; every retry extends it |
| `Abort handler called` | browser | the WebSocket handshake was interrupted locally |

The two 401s are worth reading closely. They are different messages for
different faults, and mistaking one for the other sends you to the wrong
project.

## The change

`content_security_policy` in `src/web/policy.rs` now also names
`*.livekit.cloud` when the configured host is a LiveKit Cloud URL.

The configured `LIVEKIT_URL` is the project's own hostname, `<slug>.livekit.cloud`.
That is not where the signaling socket ends up. LiveKit Cloud redirects the
client to a regional subdomain, and the JS SDK walks a list of them when one
fails:

```
conversation-xxxxxxxx.otokyo1b.production.livekit.cloud
conversation-xxxxxxxx.oosaka1a.production.livekit.cloud
conversation-xxxxxxxx.ohyderabad1a.production.livekit.cloud
...
```

The policy named only the two origins derived from the configured URL, so every
one of those regional hosts was refused by the browser before a request left
it. The page reported `could not establish signal connection: Failed to fetch`,
which reads like a network fault and is not one.

CSP `*.livekit.cloud` matches subdomains at any depth under that base, so one
pattern covers every region present and any region added later. The guard keeps
it narrow: a self-hosted LiveKit at `wss://livekit.example.com` gets no
wildcard, because nothing about that deployment implies sibling hosts are
equally trusted.

Both schemes are pushed for the same reason the exact origins already were: the
SDK opens the socket over `wss://` and then calls the same host over HTTPS for
region settings, and naming only one lets the socket open and then blocks the
rest.

### Verification

- `tests/web.rs`: `responses_carry_baseline_security_headers`,
  `production_policy_names_no_loopback_origins`, and
  `the_recording_template_is_reachable_under_a_policy_that_permits_its_room`
  all pass. Each asserts with `contains`, and this change only adds entries, so
  no existing assertion moved.
- `cargo fmt --check` and `cargo clippy --lib` clean.
- Confirmed against the live symptom: the CSP refusal disappeared from the
  browser console and the request reached LiveKit.

`tests/web.rs` pins the Cloud wildcard in
`responses_carry_baseline_security_headers`; `src/web/policy.rs` also covers
both Cloud domains and rejects lookalike or self-hosted hosts.

## What was ruled out, and how

Recorded because each of these looked plausible enough to act on, and acting on
the wrong one costs a rebuild or a new cloud project.

**A stale binary.** Credentials are read at runtime from
`config/codetrial.env.local`; they are never compiled in. Editing that file
needs a restart, never a rebuild. The same binary produced a valid token with
one set of credentials and `invalid token` with another, which is only possible
if the binary is not the variable.

**A skewed clock.** JWTs carry `nbf` and `exp`, and WSL2 clocks drift across
host sleep, which rejects tokens as `invalid token` with a correct secret.
Compared `date -u` in WSL against the `Date` header from the LiveKit host: one
second apart. Not this.

**A wrong project.** Signed a token by hand with each key/secret pair and called
`ListRooms` on each host. The control (old key against old host) returned
`200`, which is what makes the other results meaningful. The failing pair
returned `invalid token` while a genuinely mismatched pair returned
`invalid API key for domain`. LiveKit recognised the key as belonging to that
project, so only the signature could be wrong, so only the secret could be
wrong.

**`check-gemini` as proof of LiveKit credentials.** It is not. It opens a
Gemini Live session and prints the pool it would use; it never authenticates
against LiveKit. It passing says nothing about `LIVEKIT_API_SECRET`.

## Verifying credentials without the browser

Signing a management token by hand and calling `ListRooms` checks a key/secret
pair against a project without touching `/rtc/validate`, which is the
endpoint the browser rate limits. Useful when the page is already 429'd: the
management API answers while joins are refused, and the same call lists room
participants, which is how you find out whether the agent is in the room.

That last question is the one worth asking first. An agent showing
`kind: AGENT`, `state: ACTIVE`, `lk.agent.state: listening` proves the server,
Gemini, and the agent's own LiveKit connection all work, and narrows everything
remaining to the browser.

## Rate limiting

Repeated failures make this worse in a way that is easy to miss. One page load
attempts every region in turn, so a handful of reloads is dozens of requests
against `/rtc/validate`, and the limit tightens. The agent is unaffected: it
does not call that endpoint, and it stays in the room while the browser is
locked out.

When a `429` appears, close the tab rather than reloading it, and leave it
closed. A background tab may still be retrying.

## Configuration changed during this session

`config/codetrial.env.local` in both checkouts was repointed at a new LiveKit
project. The file is covered by `config/*.env.*` in `.gitignore` and no
credential appears in this document or anywhere else in the repository.

## Resolved

A full interview ran end to end on the new project. The server log is the
evidence:

```
joined room=interview-local identity=interviewer-interview-local
starting interview: room=interview-local problem=two-sum duration=30min
timing: room and Gemini session ready 0.78s after the candidate joined
timing: 0.01s from the candidate finishing to the reply starting
...
interview ended by the browser: room=interview-local topic=control
```

The browser joined the room, Jim answered turn after turn (`from the candidate
finishing to the reply starting`), barge-in worked (`Gemini cut its own turn`),
and the session ended on the candidate pressing End interview. Voice runs both
ways: the microphone reaches the agent and Jim's audio reaches the browser.

The `Abort handler called` handshake failure did not recur once the working key
was in place, so it was a transient interruption during the handshake rather
than a standing fault - a retry cleared it. It never needed a code change.

## The fixes, in the end

| # | Fault | Resolution |
|---|---|---|
| 1 | CSP blocked LiveKit regional subdomains | `src/web/policy.rs`, `connect-src` adds `*.livekit.cloud` |
| 2 | Old project rate limited (429) | moved to a fresh LiveKit project |
| 3 | First new secret was wrong (43 chars) -> 401 | reissued the key, verified before writing |
| 4 | Browser WebSocket handshake aborted | cleared on retry; no change needed |

Only #1 was a defect in this codebase. The rest were environment and
credentials. The one change that stays is the CSP wildcard.
