# Task mode contract

Status: draft, revised before any distribution; versions below stay at 1. Task
mode is automatic wherever the server is reachable from this computer alone
(see Trust model), and has no switch. The interview prompt and report
contracts are unchanged.

## Trust model

Task mode runs on the learner's own machine. Each learner downloads a prebuilt
CodeTrial and runs it with `--task-mode`, bound to loopback for themselves
alone. The instructor's only infrastructure is a static website holding
encrypted packages.

That decides what task mode can promise. Everything the program enforces, it
enforces on a machine the learner controls: the encryption, the test rules,
the deadline and the review can all be read past or patched out by a learner
determined to. They exist to make peeking at reference solutions and breaking
the announced rules deliberate rather than easy, and every page and export says
so. A task result is generated on the learner's machine. It is not an
authenticated submission, a secure grade or proof that the rules were kept;
"recorded" below means recorded by the learner's own CodeTrial.

## Prerequisites

The learner provides, before taking any task:

- a prebuilt CodeTrial for their platform, run locally;
- their own Gemini API key and LiveKit credentials;
- a GitHub account;
- a supported desktop browser (Chromium or Firefox) with a microphone and a
  camera;
- the assignment link from the instructor's site, and its six-digit PIN.

A first run goes through Setup, which in task mode requires the Gemini key and
the LiveKit URL, key and secret, and checks each with one cheap call before
saving. Setup keeps the assignment link it was opened with and continues to it
afterwards. Task mode is off unless `codetrial web --task-mode` asks for it,
and is then on only when the server is reachable from this computer alone:
bound to a loopback address (`127.0.0.1` or `::1`), not behind a declared
proxy (`CODETRIAL_TRUSTED_PROXY_HOPS`), and set up neither to record nor to
share one fixed room. Its routes then also refuse a request whose `Host` does
not name `localhost`, `127.0.0.1` or `[::1]`, which keeps out a DNS name
rebound to the machine. Without `--task-mode` they answer `task_mode_off`, and
with it but anywhere else than this computer alone, `loopback_required`. One
CodeTrial process serves one learner and runs one attempt at a time, in the
one language the learner chose for it.

## Local recordings

Learners can optionally start a local recording during preparation, after
allowing the microphone and camera and before Start enters fullscreen. Choose
this CodeTrial tab and share tab audio in Chrome or Chromium to include Jim
and the existing task microphone. Anything visible in the captured tab,
including the camera preview during preparation, is included. Capture prompts are unavailable during work
because leaving fullscreen or hiding the page ends the attempt. Camera
sampling, calibrated look-away warnings and all task rules continue while
recording. Stopping recording stops only tab capture and the audio mix: it
does not stop the task camera or its monitoring. A camera or detector failure
still interrupts the attempt; a sustained look-away or hidden page still ends
it as invalid, with any available recording saved for download.

End recording or browser Stop sharing saves a WebM. Completion, a rule ending,
or room loss also finalizes it, with a download link retained beside the result.
If preparation has to replace the microphone and camera, it first finalizes
any current recording; start a fresh recording after retrying device setup.
When the connection is lost before Start is acknowledged and the server
confirms the task never began, preparation offers recording again and keeps
the previous download available.

The bytes stay in the current browser tab until downloaded and are never
uploaded. Download before reloading or closing the tab; the browser asks for
confirmation while recorded media is unsaved (a download the browser blocks
still counts as saved, so the status text points to the link), except during the attempt, when
leaving fullscreen would already end it. Result recovery cannot restore local
media. Recording failure does not end the task.

## Versions and ownership

The sidecar `promptVersion` is the platform composition version, not an
instructor revision counter; only integer 1 is supported and booleans are not
integers. Every change to a record, sidecar, set rule or PIN creates a new
immutable set version. Package format, prompt composition, wire protocol, review
schema and rubric are versioned independently of the interview tuple and are
all 1. `rubricProfile` maps `task-engagement-v1` to task rubric 1.

A task result carries `assessmentMode: "task"`; missing mode means interview
for legacy reports. The interview scorer never scores task results. Results
carry a `taskContract` object with integer `package`, `prompt`, `wire`,
`reportSchema` and `rubric` versions. Unknown or future versions display
feedback unavailable and never fall through to interview scoring.

## Default values

[task-defaults.json](../problem-bank/task-defaults.json) is the sole numeric
and text default table, set rules included. Python loads it directly; Rust
consumes the same artifact. Schema limits (target counts, identifier and
narrative byte lengths) are fixed protocol limits. Instructor-tunable values
are the set rules below; everything else (fetch timeouts, memory bounds,
capture capacities) belongs to CodeTrial, and no package can raise it.

No task control pauses the wall-clock deadline. Learner contexts are never
persisted: no localStorage drafts, no saved sessions. The latest attempt's
result, with its final code, stays in process memory for at most
`reviewRetentionSeconds` so a page that lost its room can still collect it,
and is dropped when that time is up even if no request arrives.

## Sidecar schema

The optional `task-mode.json` is an object keyed by known bank record IDs.
Unknown task IDs are rejected. An omitted entry selects the standard checks and
interaction prompt from `problem-bank/task-defaults.json`, no completion targets
or required cases, and the default hint maximum. Explicit null is invalid. A present entry requires every field
below and rejects unknown fields; nested objects are closed too.

| Field                 | Type and limits                                     |
| --------------------- | --------------------------------------------------- |
| `promptVersion`       | Integer 1 only                                      |
| `interactionPrompt`   | Nonblank UTF-8 string, at most `promptBytes` bytes  |
| `completionTargets`   | Array of 0..8 target objects                        |
| `understandingChecks` | Array with exactly the default `coreChecks` count   |
| `requiredCases`       | Array of 0..20 distinct exact source judge labels   |
| `maxHintRungs`        | Integer 0..default maximum                          |
| `rubricProfile`       | Literal `task-engagement-v1`                        |
| `languages`           | Optional; 1..5 distinct languages, default `python` |
| `optimization`        | Optional; `O0` or `O1`, needs C or C++              |
| `explainers`          | Optional; 1.. distinct names the page builds in     |

Set, target and check IDs match `[a-z][a-z0-9-]{0,63}`. Task record IDs also
permit an initial digit, preserving catalog IDs such as `3sum`. Target IDs and
check IDs are independently unique. A target has exactly `id`, `language`,
`marker`, `goal`; language is one of the task's languages, marker is 1..128
UTF-8 bytes matching `[A-Z][A-Z0-9_]*`, and goal is 1..1024 bytes of nonblank
text. A marker occurs exactly once as a whole token in its language's shipped
starter after variant renames, and two targets in one language cannot share it;
the same marker may name the matching place in another language's starter.
Check objects have exactly `id`, `question`; question is nonblank and at most
1024 UTF-8 bytes.

`languages` lists what the learner may choose from, in the order offered, from
`python`, `javascript`, `c`, `cpp` and `java`. Each needs a shipped starter, and
each starter is held to the code byte limit, since it is an attempt's first
revision. C runs only function judges, so a class task cannot list it. A
function task that lists C, C++ or Java needs `paramTypes`, with one type per
argument every case passes, and `returnType` in its judge, since the compiled
harness is built from them. The
learner chooses before calibration connects the room, the admission names the
choice, and it holds for the attempt: the starter, the targets offered, the
editor, the runs and the interviewer's projection all follow it. C, C++ and Java
runs compile on Compiler Explorer, as in interviews, so the learner's code and
the public cases leave the machine for them and the page says so before the
choice; reference material never does. A server with
`CODETRIAL_COMPILER_EXPLORER_ENABLED` off offers only the task's other
languages, and refuses a task with none as `language_unavailable`.

`optimization` lowers the compiler optimization level a C or C++ run is built
with, from the page's default to `O0` or `O1`, and can never raise it. A task
sets it when the cost of its code is the point, so the compiler does not hide
it; the page says so beside the compile notice, and the candidate's source has
no way to change it. Absent, the default applies. It needs C or C++ in
`languages`, since the other languages take no such flag.

`explainers` names what the page shows beside the brief to make the task easier
to picture. The only one is `cube`, a 2x2x2 cube the learner can turn: a 3D
view to drag round, the same cube unfolded, buttons and a notation box for
turns, and the position code the cube is in, with what each of its digits
says. Each explainer is built into the page, so a package names one and cannot
supply its own; an unknown or repeated name is refused, and absent, nothing is
shown. The cube's code is the default task's: seven cubie digits, then seven
twist digits, with the up-left-front corner held fixed.

Required cases resolve by exact **source judge label**, before variant renames,
not by index or fuzzy matching. Each label must resolve to exactly one case.
Validation binds the resulting case index in the immutable package; the public
projection uses the renamed display label. The page is given the bound
indexes, since a rename can make two shown labels equal, and the workspace
marks those cases among the public cases it shows.

Variables are exactly `{{title}}`, `{{language}}`, `{{target}}`, `{{phase}}`;
`language` is the attempt's.
Whitespace inside braces, unknown names and unmatched double braces are errors.
A single brace is literal. Values are substituted once, never recursively.
`target` is the currently requested target's goal, or the empty string; phase
is the server phase. Instructor text is subordinate to platform policy and
cannot grant access to reference material or completed code. Live and recovery
composition use the same validated sidecar and `problem-bank/task-prompt.txt`.
Initial prompts carry the hint count only; recovery resends delivered rungs,
never unreleased ones, and the platform serves one requested rung at a time.

Task records keep the existing bank format and structural validation, and
packaging additionally refuses unknown judge/case fields, unsupported checkers
or languages, missing Python starters and text over 32 KiB. The instructor's
`render-prompt --language` previews the prompt in any of a task's languages,
the first by default. Custom records
declare `origin: "original"` and omit published `title` and `examples`; the
variant supplies them. Catalog or study-plan membership is not required.

The interviewer (Jim) is given an explicit allowlist of title, brief,
contract, clarifications, hints up to the limit, completion targets and checks;
the workspace additionally shows the starter and public cases. `optimal`,
`pitfalls`, guide text and reference code ship inside the encrypted package for
the review's overlap check and are never shown to the learner or given to Jim.
Learner code and speech are untrusted content, not instructions.

## Set rules

An optional `task-set.json` in the bank holds the rules for the whole set; an
absent file or key takes the default from `task-defaults.json`. It is closed:

| Key               | Type and default                                                              |
| ----------------- | ----------------------------------------------------------------------------- |
| `durationMin`     | Integer within `MIN_DURATION_MIN`..`MAX_DURATION_MIN`; default `durationMin`  |
| `maxHintRungs`    | Integer 0..default maximum; lowers each task's sidecar value, never raises it |
| `closesAt`        | UTC time `YYYY-MM-DDTHH:MM:SSZ`, or null; default null                        |
| `lookAwaySeconds` | Integer 5..60; default 8                                                      |
| `rulesNote`       | Nonblank text of at most 1024 UTF-8 bytes, or null; default null              |

The effective hint maximum is the minimum of the sidecar value, the set value
and the variant ladder length. `closesAt` is checked at unlock and when Start
is admitted, against the `Date` header of the instructor site's response
rather than the learner's clock (the learner's clock only if the site sent
none), so an attempt admitted before the close may finish.
`lookAwaySeconds` is described under [Test rules](#test-rules). The default of
8 seconds is a starting value to be confirmed by the pilot, not a measured one.
`rulesNote` follows the platform's own statement of the rules, never replaces
it. The fullscreen and page-visibility rules are fixed and cannot be turned
off.

## Package format

A published set version is two files, beside its landing page (see
[Instructor workflow](#instructor-workflow)):

```text
sets/<setId>/<setVersion>/manifest.json
sets/<setId>/<setVersion>/tasks.enc
```

The manifest is closed and not encrypted: `packageVersion: 1`, `setId`,
`setVersion` (integer 1..2147483647), `salt` (standard base64 of 16 random
bytes), `ciphertextSha256` (lowercase hex) and `ciphertextBytes` (positive
integer). The encrypted plaintext is a closed object with `packageVersion: 1`,
`setId`, `setVersion`, `rules` (the complete resolved set rules), `problems`,
`judges`, `variants` and `sidecars`, using the existing bank structures.

The PIN is exactly six ASCII digits, leading zeros kept. The key is
PBKDF2-HMAC-SHA256 with 600,000 iterations and a 32-byte output. Its password is
the PIN; its salt is the compact UTF-8 JSON encoding of
`["codetrial-task-kdf-v1", setId, setVersion, salt]`, using the manifest's base64
salt string. The iteration count belongs to the package format, not the
manifest, so a website cannot set the work CodeTrial does; it is to be
confirmed against measured unlock time before the format is distributed.

`tasks.enc` is AES-256-GCM: a fresh random 12-byte nonce, then the ciphertext,
then the 16-byte tag. Associated data is the compact UTF-8 JSON encoding of
`["codetrial-task-set-v1", setId, setVersion]`. There is no separate PIN
verifier. A six-digit PIN is about twenty bits, so anybody holding the
ciphertext can find it offline, and every learner has it anyway: this keeps
out people who were never given the PIN and makes reading solutions a
deliberate act, and no more. Changing the PIN means a new set version.

## Instructor workflow

The bank is an authoring directory with `problems.json`, `judges.json`,
`variants.json`, and optionally `task-mode.json` and `task-set.json`.
`scripts/task-package.py` starts, validates, previews and builds it; it never
uploads, and finds its own Python, as
[development.md](development.md#instructor-task-packaging) describes.

```bash
scripts/task-package.py references
scripts/task-package.py new        # the default task, copied into ./tasks
scripts/task-package.py new --from two-sum --id pair-sum --languages python,javascript,cpp
scripts/task-package.py preview --task-id pair-sum
scripts/task-package.py build --site https://teacher.github.io/course
```

The bank is the directory `tasks` and the output the directory `publish`, both
in the current directory, unless `--bank` and `--output` name others. `preview`
and `render-prompt` (what Jim is told, in `--language` if the task offers more
than one) take the bank's only task when `--task-id` is left out, and name the
choices when there are several.

`references` lists what a task can start from: the shipped examples under
`problem-bank/task-examples/`, which show completion targets, required cases,
custom checks and several languages, then every catalog problem. `new` without
`--from` copies `rubik`, the default example. Unlike a catalog problem it is
not one function to write: the learner starts from a working cube solver in C
that is correct and too slow and too large for the compiler service, so a run
of it is reported as a timeout. The work is the one a service owner faces:
predict where the time goes, measure, change one thing at a time, and keep
every answer the same until the run finishes in time. The marked lines show
where the cost is (how often the table is built, what ranking a position
costs, what the queue holds, how each edge moves between positions) and are
guidance only. The judge compares
answers, so any implementation that returns the exact fewest turns for every
case passes, including one leaner than the reference. The interviewer asks the
learner to predict before running, to measure before changing, and afterwards
how they know no answer changed. C runs on Compiler Explorer, so a server with
it off refuses this task as `language_unavailable`. Whichever reference it
starts from, `new` copies the task into the bank, creating the bank if needed,
and writes the task's sidecar out in full, and a new bank's set rules, so each
field can be edited in place.
`--id` gives the copy its own id; it keeps the reference's origin, so the checks
that keep a catalog problem's source out of what the learner reads still apply
while the instructor rewrites it. Copies may share a LeetCode source number; task ids remain unique within the bank. `--languages` sets what the learner may choose
from, dropping targets marked in a language no longer offered. `new` refuses a
bank that is already invalid and an id the bank holds, validates the whole bank
with the task in it before changing any file, and leaves files it does not
change as they are. It stages the bank in `.task-new` and moves it into place
once it validates. If it stops after the bank validated, the next `new`
completes the move; if it stops before, the bank is unchanged and the next `new`
asks for `.task-new` to be deleted. It is not meant for two runs at once on one
bank.

`preview` shows what the learner sees and `render-prompt` what Jim is told; both
are for authoring and neither is required. `build` names the set after the
bank's directory (`tasks` unless `--bank` says otherwise), which must then be a
valid set id, and takes the version after
the last one the output holds, unless `--set-id` and `--version` say otherwise;
it checks both, the bank and the output before asking for the PIN, and prints the landing
page and each task's assignment link. The output is the only record of versions
it reads, so `--output` must be the directory that gets published: one that does
not hold the set yet starts at version 1, which `build` says before asking for
the PIN. `build` reads the PIN twice from a non-echoing prompt (or once from
stdin), never from arguments, and in one step validates the bank, encrypts it,
decrypts the result again to prove the round trip, writes the landing page and
scans the output. It refuses a version directory the output already holds, so a
version built there is never overwritten, and adds a new version beside the old
ones. The output holds only:

```text
publish/.nojekyll
publish/index.html
publish/sets/<setId>/<N>/index.html
publish/sets/<setId>/<N>/manifest.json
publish/sets/<setId>/<N>/tasks.enc
```

Each version has its own landing page, `sets/<setId>/<N>/index.html`, written
once with the version and never rewritten. It lists the tasks, the set's rules
in plain words, what a learner needs before starting, and for each task an
**Open in CodeTrial** link built from `--site`, with the same address as
copyable text. The root `index.html` is regenerated on every build and only
links to the published versions. Neither contains the PIN. `publish-scan DIR`
re-runs the scan alone and refuses anything the layout above does not name,
plaintext banks and sidecars included; `CNAME` and `README.md` at the root are
allowed.

The instructor uploads `publish/` as the root of a dedicated GitHub Pages
site, or any static HTTPS host, then shares the landing page and, separately,
the PIN. Pages sites are public; the encryption, not the hosting, is what keeps
casual readers out. Learners hand in exported results through the course's
usual channel; those files are self-reported.

## Learner flow

### The default task

With `--task-mode` the home address opens the default task, `rubik`, as a
guest: no sign-in, no PIN and no download. It is the task the program ships
(`problem-bank/task-examples/rubik.json`, the row `new` copies when no
`--from` names one), alone in a set called `default`, version 1, under the
default rules except for sixty minutes, since the task is a measured rewrite. Its assignment names the site `https://built-in.invalid/`;
`.invalid` never resolves, so no instructor's site can be mistaken for it. The
session names the task (`builtInTask`), so the page needs no link and goes
straight to the rules, the devices and the calibration, and then the task. The
interview lobby stays at `/index.html`.

A guest has an id of their own, which holds the attempt and its result, and the
result names them as `guest` with an id of 0 where an instructor's names a
GitHub login. A guest, or anyone not signed in with GitHub, may open the default
task and nothing else: any other set answers `authentication_required`, and
there is nothing for a guest to unlock.

On the default task the page offers the learner's instructor's tasks. The
learner signs in with GitHub, says which server and which set the instructor
named (the site, the set and its version, the three things an assignment link
carries), enters the PIN, and then goes through the same preparation and test.
How a server names its sets, so that the learner need not type them, is still to
be settled.

### Assignment links

An assignment is an address naming one version:

```text
http://localhost:3000/t/<setId>/<taskId>?site=<https base URL>&version=<N>
```

`site` is an HTTPS URL with no credentials, query or fragment, naming a public
host (loopback, private and link-local addresses are refused, since the
learner's machine makes the request); its path is the
base under which `sets/` lives, so a Pages project path works. A link opened
before CodeTrial has a configuration lands on Setup and returns to the
assignment once Setup saves. A link that does not open because CodeTrial is not
running is opened again once it runs; on another port, the learner changes the
port in the address. There is no "latest" lookup: a new version
comes with a new link and its PIN.

### Sign-in

GitHub sign-in uses the device flow: the page shows a short code and a link to
github.com, and the learner approves there. Only a public client ID is needed,
built into the release from the repository variable
`CODETRIAL_GITHUB_CLIENT_ID` and overridable with `GITHUB_CLIENT_ID`. A build
without either cannot sign a learner in, and the page says to set
`GITHUB_CLIENT_ID`. No client secret is stored on the learner's machine. The sign-in is kept across launches
so a learner signs in once, before class rather than during it: GitHub limits
how many device authorizations one application completes per hour, and a
course may use its own OAuth App with device flow enabled. Sign-in gives the
result a GitHub login and numeric ID; it controls nothing about who can
download a package.

### Unlock

On PIN entry the local server fetches the manifest and ciphertext over HTTPS
with bounded size and time and no redirects at all, checks the
manifest, the size and the hash, derives the key off the async workers, and
decrypts and validates everything. A download that does not match its hash is
`package_invalid`; a matching ciphertext whose tag fails is reported as a wrong
PIN, the likeliest cause. The PIN and derived key are dropped once the set is
decoded. CodeTrial never writes the decoded set anywhere; it lives in process
memory until exit, which is not a promise about what the operating system
pages to disk. A restart downloads the set and asks for the PIN again. No
failure falls back to a catalog problem, or to the default task.

### Rules and preparation

After unlock, before anything is timed, the learner prepares:

1. **Rules.** The page states every rule, the set's `lookAwaySeconds`, what
   ends an attempt, that a violation makes the attempt invalid with no grace,
   and what is not detected (a second monitor, a phone held at screen height).
   It says to switch off notifications, keep the machine plugged in, and use
   the workspace's built-in calculator rather than another app or tab. The
   learner ticks an acknowledgement, recorded with the time and the rules.
2. **Devices.** Microphone and camera permission is granted now, so no prompt
   can appear mid-attempt. The same tracks are kept for the whole attempt.
   The face model is downloaded as the page opens, with a progress bar, and
   started when the camera is allowed; the page says when it is ready, and
   Calibrate waits for it. A server that lacks the model's files is named in
   the message, with the script that fetches them, and an offline download is
   reported as that and not as a missing model.
3. **Calibration.** About twenty seconds with the face detector running: the
   learner looks at the screen, then types a given line into a text field (a
   calibration in which it was not typed is refused). The phase ends once the
   line is typed. This
   measures where their face sits, how their head is held while reading, and
   how far and how long it dips while typing, and sets the per-learner limits
   the look-away rule uses. A camera, light or framing that gives no stable
   face fails calibration with advice, and the learner retries. The resulting
   limits, not images, are recorded with the attempt.
4. **Connection.** The room and Jim are connected and acknowledged.

Only then is **Start** enabled. Start enters fullscreen and begins the clock
and the rules together. A failure at any step keeps the assignment and offers a
retry, and spends none of the attempt's time.

## Test rules

Task mode treats the announced rules as conditions of a valid attempt. This is
an exception to [integrity-evidence.md](integrity-evidence.md) and
[observable-delivery-policy.md](observable-delivery-policy.md) for task mode
only; interviews are unchanged.

| Rule            | The attempt ends when                                                                                                       |
| --------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Fullscreen      | The workspace leaves fullscreen, by any means, Escape included                                                              |
| Page visibility | The page becomes hidden: another tab, a minimized window, a locked screen                                                   |
| Look-away       | For longer than `lookAwaySeconds` without a break, no face is in frame or the head is tilted down past the calibrated limit |

Fullscreen and page visibility have no grace period. Page visibility is
`document.visibilityState`, not window focus: a window that loses focus while
staying visible does not end an attempt. The workspace requests a screen wake
lock for the attempt so an idle screen does not lock it; a refused wake lock is
stated, not hidden.

The look-away rule allows what normal work looks like: a glance at the
keyboard, a moment out of frame, both shorter than `lookAwaySeconds`. What it
is for is sustained attention elsewhere, such as reading a phone or tablet in
the lap. From the moment a continuous look-away passes half the threshold, the
page shows a countdown and plays a sound, so a learner who is not watching the
screen still learns they are about to end the attempt.

The camera rule reads the detector's face count, box and keypoints, and
estimates head pitch from where the nose sits between the eyes and the mouth
(or from the box when the keypoints are missing), relative to the learner's own
calibration. It does not read eye gaze, expression or anything else about a
face. The detector neither uploads nor records camera frames. An optional
local tab recording includes any camera preview visible in the tab, as the
recording notice explains. Only successful,
fresh samples count: a detector that stops answering, a camera track that ends
or mutes, or a sampling gap is a technical interruption, never a look-away.
More than one face in frame is not a rule.

A violation ends the attempt at once: monitoring stops, the work freezes at the
latest acknowledged revision, the room ends, and the attempt is invalid. A new
attempt is a new Start. An invalid attempt states facts, never intent: "left
fullscreen at 03:12; the attempt ended under the announced rules". It never
says or implies cheating.

## Outcomes

Every attempt ends in exactly one outcome. The first terminal event the runtime
accepts wins; later ones are ignored, and a frozen final revision never changes.

| Outcome     | Cause                                                             | Result                                                |
| ----------- | ----------------------------------------------------------------- | ----------------------------------------------------- |
| Completed   | Finish or timeout                                                 | Learning review, or feedback unavailable              |
| Invalid     | A test rule was broken                                            | Rule, time and measurement; no ratings; no model call |
| Interrupted | Camera or detector failure, page reload, browser crash, room loss | Cause and time; no ratings; no model call; may retry  |

A reloaded or crashed page cannot resume: the page held the room and the
attempt's monitoring. Per-tab session storage keeps only the room identifier,
the bounded result-polling deadline and whether Start was still unacknowledged,
keyed by assignment site, version and task;
no code, transcript or result is stored there. Reload restores result retrieval,
not a running attempt, and never extends its deadline. The surviving local server
finalizes such an attempt as interrupted after `pendingAdmissionSeconds`
without the page, or at the deadline if that comes first: an attempt nobody was
watching is never completed by its timeout. If the CodeTrial process itself exits, the attempt and its
result are lost, and the next launch says so. Violations after Finish, during
wrap-up, still end the attempt as invalid; the frozen code is kept in the
record.

Every outcome keeps the final acknowledged code with its revision ID, and the
result endpoint returns it to a reloaded page while the process lives: pending
while an attempt is still being finalized, the result once it exists, and an
explicit `result_unavailable` when there is none. The result screen offers an
explicit new-attempt action that clears only this tab's recovery identifier
and returns to unlocking; it never deletes the server's retained result.
A polling deadline ends waiting on pending delivery, not access to a retained
result. Reload checks the server again until it reports `result_unavailable`.

## Admission and errors

The page unlocks with `POST /api/task-sets/<setId>/unlock`, loads with
`GET /api/task-sets/<setId>/tasks/<taskId>` and is admitted with
`POST /api/task-sets/<setId>/tasks/<taskId>/attempts`, each naming the
assignment's site and version; none of them is the interview's `/api/token`.
Admission requires the decoded set, the acknowledgement, granted devices, a
completed calibration and an acknowledged connection; Start then requests
fullscreen synchronously from the click. A second Start while an attempt runs
is `active_task_session`. A request key deduplicates a retried Start: a replay
returns the same admission, and a key older than `pendingAdmissionSeconds` is
refused rather than starting a second attempt; a replay naming a different
site, set, version, task or language is `client_override`. Each assignment (site, set and
version) is unlocked on its own, so links for two versions of one set, or two
sites that happen to use the same set name, never replace each other. Room
metadata carries IDs and versions only. The clock starts exactly once, at
Start.

The connection made after calibration waits for Start for at most
`pendingAdmissionSeconds`. If it ends first, or the page loses the room before
Start, the page lets the room go and asks for calibration again, which connects
afresh under a new request key; the server frees the account within ten
seconds of a page leaving before Start, or of one never joining.

Errors contain exactly `code`, `retryable`, `message`; no secrets. An HTTP
status of `page` marks an error the page raises itself, before or without any
request, and `wire` one the room sends on the data channel.

| Code                      | HTTP | Retryable | Recovery action                    |
| ------------------------- | ---- | --------- | ---------------------------------- |
| `credentials_missing`     | 412  | false     | Add the Gemini key and restart     |
| `task_mode_off`           | 403  | false     | Restart with `--task-mode`         |
| `loopback_required`       | 403  | false     | Read the setup notes               |
| `authentication_required` | 401  | false     | Sign in                            |
| `csrf_rejected`           | 403  | false     | Reload                             |
| `site_invalid`            | 400  | false     | Check the assignment link          |
| `invalid_pin`             | 403  | true      | Retry PIN                          |
| `unlock_required`         | 403  | false     | Enter PIN                          |
| `package_unavailable`     | 503  | true      | Retry load                         |
| `package_invalid`         | 422  | false     | Ask the instructor                 |
| `set_closed`              | 410  | false     | Return to task list                |
| `task_not_found`          | 404  | false     | Return to task list                |
| `rules_unacknowledged`    | 403  | false     | Read and accept the rules          |
| `devices_required`        | page | true      | Allow microphone and camera        |
| `calibration_required`    | 403  | true      | Run calibration                    |
| `fullscreen_required`     | page | true      | Retry Start                        |
| `client_override`         | 400  | false     | Reload                             |
| `request_too_large`       | 413  | false     | Reload                             |
| `language_unsupported`    | 400  | false     | Choose one of the task's languages |
| `language_unavailable`    | 412  | false     | Restart with Compiler Explorer on  |
| `platform_unsupported`    | 400  | false     | Open a supported desktop browser   |
| `request_key_expired`     | 409  | false     | Start again                        |
| `setup_failed`            | 503  | true      | Retry Start                        |
| `setup_pending`           | 409  | true      | Retry                              |
| `admission_throttled`     | 429  | true      | Wait, then retry Start             |
| `active_task_session`     | 409  | false     | Finish in original open tab        |
| `result_unavailable`      | 404  | false     | Start a new attempt                |
| `action_rejected`         | wire | true      | Retry after acknowledged state     |

## Phase wire contract

Messages are version 1 closed JSON objects. IDs are opaque strings matching
`[A-Za-z0-9_-]{1,128}`, never instructions. Every server state is a full
snapshot with a monotonically increasing `seq`; clients ignore older and
duplicate snapshots.

`task_action` (client to runtime): `version`, `requestId`, `action` enum
`hint | discuss | finish`, `revisionId` (string or null), `targetId` (known
completion target ID or null). Hint and Discuss name the target they are about,
and null means the whole task. Discuss and Finish refer to an acknowledged
revision. Request IDs deduplicate effects.

`task_end` (client to runtime): `version`, `requestId`, `outcome` enum
`invalid | interrupted`, `cause` enum `fullscreen | visibility | look_away` for
invalid or `camera | detector | reload` for interrupted, and `durationMs`. The
runtime ends the attempt on the first one it accepts during work or wrap-up.

`task_connected`: `version` only; repeated until any `task_state` arrives, and
the runtime answers each one with a state snapshot even when nothing changed.
`task_start`: `version` only; sent once from the Start click, after fullscreen
is granted. The runtime starts the clock and the interviewer on the first one
it receives in the ready phase and ignores the rest.
`task_thinking`: `version`, `thinking` boolean; accepted during work or
wrap-up, and it suppresses nudges. `task_error`: `version`, `code`, `requestId`
(or null), `retryable`, `message`.

`task_revision`: `version`, `requestId`, `revisionId`, `code` (at most 32,000
UTF-8 bytes, a limit the page enforces before sending), `trigger` enum
`run | discuss | finish`. `task_run`: `version`, `requestId`, `runId`,
`revisionId`, `passed`, `total`, `diagnostics`; counts satisfy
0 <= passed <= total, and a zero total requires a diagnostic and means a runner
failure, not a failed case. Results are learner-reported practice evidence.

`task_state` (runtime to client): `version`, `seq`, `phase` enum
`ready | work | wrap-up | feedback`, `deadlineAt`, `serverNow`, `interviewer` enum
`available | degraded | unavailable`, `checks` (`id`, `state` enum
`open | covered | unresolved`, `support` enum
`none | conceptual_hint | targeted_followup`), `hintRungsUsed`, `hintRungsMax`,
`acknowledgedCaptureRequestId`, `finalRevisionId`, `captureOverflow`,
`activeTargetId`, and `outcome` (null until the attempt
ends, then `completed`, `invalid` or `interrupted` with its cause and time).
`serverNow` is the server time when the snapshot is built, formatted as RFC
3339 at whole-second resolution, like `deadlineAt`. Only after accepting a new
sequence number does the page update its server clock offset, and only toward
a later server time, so a snapshot delayed on the way cannot lengthen the time
remaining. The page keeps that offset against both the browser's monotonic
clock and its wall clock and counts down by whichever shows less time left, so
a different local clock does not move a deadline and a sleep, which stops the
monotonic clock, can only shorten the time left. A new room starts the offset
over. A
snapshot without `serverNow` is refused, so a page served from disk must match
the binary. This draft field keeps payload version 1.

`board_image` (client to runtime, a byte stream): the learner's whiteboard
drawing as a JPEG of at most the interview whiteboard's size, with the stroke
count the page counted as an attribute. It is the interview's example drawing,
sent once the drawing has been still for a moment, and the runtime reads it
off the stream with the interview's bounds on size, time, concurrent readers
and sender. A drawing is held for the attempt and shown to Jim only when Jim
calls `read_board`, which the model does when the learner refers to it; it is
never sent unprompted, never kept for the review or the export, and never
evidence for a check, a hint or a rating. Asked before any drawing exists,
`read_board` answers that nothing is drawn.

`task_review` (runtime to client): the result described under Capture and
Learning review, sent once when the attempt is finalized. A page that lost its
room retrieves the same object from `/api/task-reviews/{room}`.

Only the server advances phases or covers checks. Finish cancellation sends no
action and keeps work editable. Beyond the core questions, the runtime may
reserve at most `extraChecks` targeted follow-ups per session; each, and each
conceptual hint, has a server-issued support request ID that a model links only
to later evidence. Wrap-up begins on Finish or at the configured lead time and
asks at most the configured uncovered checks; Finish freezes one acknowledged
revision exactly once, and timeout freezes the latest acknowledged revision.
Uncovered checks are insufficient evidence, not zero scores.

## Capture and Learning review

Keep the initial, final and evidence-referenced revisions within the snapshot
ceiling. Run summaries stay bound to their original revision. Overflow is
explicit, never silently truncated. No client message may cover a check.

A result is a closed object with `assessmentMode: "task"`, `taskContract` and
`taskAssessment`. The server stamps `setId`, `setVersion`, `taskId`,
`sessionId`, `sessionOrdinal`, `rubricProfile`, `githubLogin`, `githubId`,
`language`, `rulesAcknowledgedAt`, `rules`, `calibration`, `outcome`,
`captureOverflow`,
`finalRevisionId` and `finalCode`. `sessionOrdinal` counts the account's
admitted attempts at the set; a Start that failed or was abandoned before it
was answered is not one. A completed attempt adds `dimensions`, `gaps`
and `nextActions`, or `feedbackUnavailable: true` when no review could be
generated. An invalid or interrupted attempt adds its cause, time and
measurement, and has no ratings.

Dimensions are exactly `reasoningParticipation`, `implementationOwnership`,
`testingAndDiagnosis` and `revisionAndImprovement`, each with exactly:

- `rating`: integer 0..3 or null. Zero requires an observed unresolved
  opportunity; 1 emerging; 2 demonstrated with support; 3 independently.
- `reason`: nonblank narrative, at most 1024 UTF-8 bytes.
- `insufficientReason`: null for numeric ratings; for null ratings, one of
  `missing_turns | garbled_turns | timeout | telemetry_gap | capture_overflow`.
- `support`: `none | conceptual_hint | targeted_followup`.
- `evidence`: at most eight closed `{kind, id}` objects with kind
  `revision | run | turn`, each resolving to captured session data. A numeric
  rating cites at least one turn the recognizer returned as English; 2 and 3
  are judged against the support each cited turn was recorded with.

A captured turn written mostly in a non-Latin script cannot support a check,
and the review model reads the interview's "not recognized as English" marker
in its place. Gaps and next actions get the interview report's rules for
advice: they never coach a learner to speak so a recognizer understands them.

No aggregate score, hiring field, fenced code or implementation reproduction is
allowed; a narrative sharing twelve consecutive code tokens with the reference
is refused. Outages and recognition gaps are conditions, never penalties.
Bounded report repair exhausts to feedback unavailable, never to a partial or
leaked review. The export is the result above, final code included, and names
itself as locally generated learning feedback.

The learner is shown no hiring outcome, verdict or recommendation, in the
page or in the review, and keeps the review to read later: it stays in the
page after the attempt and the export carries it.

## Jim during work

Jim guides the learner through REACTO in order: repeat the task, walk an
example with an edge case, state the algorithm and its cost, code it, test by
predicting before running, then optimize. He names the step, moves on only
when the learner has done the current one, and invites back a skipped step
with a question instead of doing it for them.

The learner talks to Jim throughout. Jim may be asked to restate or explain
the task and to clarify it, and answers from the brief, the contract and the
clarifications in the public projection alone, in plain words and small
pieces; where the task says nothing he says so and asks which part is unclear.
He never invents a requirement and never confirms, corrects or completes the
learner's answer. A hint is a different thing: the platform serves one rung at
a time on request, and explaining the task is not one.

Jim also looks at the learner's code, so a reminder is a matter of the
platform's timing and not the model's. After `nudgeIdleSeconds` without
activity, and no oftener than every `nudgeIntervalSeconds`, the platform sends
Jim one nudge; it is held while the learner speaks or has ticked Thinking
quietly. If the learner has changed the code since the starter and since the
last such look, the nudge asks Jim to read the editor and raise at most one
reminder or question: where to look and what kind of problem, never the
corrected line, code or the answer. The attempt has at most `codeReviewNudges`
of these. Every other nudge only invites the learner to explain their
reasoning, and silence is not an assessment.

## Workspace

The editor paints the interview editor's syntax colours, line numbers, indent
guides and bracket match behind its textarea, in the language the learner chose.
Besides the editor, the workspace provides a calculator, so arithmetic never
needs another tab or app. It evaluates one expression with `+ - * / // %`,
`^` for powers, `pi`, `e`, and the functions `sin cos tan asin acos atan
atan2 sinh cosh tanh exp ln log log2 sqrt cbrt pow hypot min max abs floor
ceil round deg rad`. A selector sets whether trigonometric functions take and
return radians or degrees, and in degrees the multiples of 90 are exact. A
result shows fifteen significant digits, except an integer up to 2^53, which
is shown whole.

The calculator's `//` rounds down and `%` takes the sign of the divisor, so
`-7 // 3` is -3, `-7 % 3` is 2, and `a == (a // b) * b + a % b` holds for
integers of every sign while the arithmetic stays exact. An exact multiple
gives 0 and `//` never gives -0, so `-6 % -3` and `0 // -3` are 0. An infinite
divisor is the exception: `%` leaves a finite dividend unchanged and `//` gives
0, so `-1 % 10^400` is -1 and `-1 // 10^400` is 0. A fractional operand can
miss the identity, because `//` rounds the quotient before flooring it, so
`1 // 0.1` is 10. A dividend of the other sign that is tiny beside the divisor
rounds to the divisor itself, so `-1e-20 % 3` is 3, as with Python floats.

A button above the editor gives it the whole page, and hides the brief and the
public cases, until it is pressed again; the code is the same either way. The
workspace keeps the rules, the countdown, connection status and help inside the
fullscreen view. Credential repair, sign-in and device selection happen only
before Start. CodeTrial stops monitoring before it leaves fullscreen itself at
the end of an attempt.

A task that names the `cube` explainer shows the cube under its contract,
open, with the brief. It is drawn from one model, so every view agrees: the
3D view, the unfolded net and the position code are the same cube, and a turn
of any face of it, a notation such as `[R, D] B2`, or a code typed from the
public cases changes all three. The model is checked against the solver's own
turn tables. Turning is the learner's own; nothing the cube does is sent to
the runtime, Jim or the review.

A whiteboard below the editor is the interview's example drawing: pens, an
eraser, undo and clear. What is drawn on it leaves the browser only as the
`board_image` stream described under the wire protocol, and the interviewer
sees it only when the learner refers to it.

## Runtime and delivery bounds

Editor drafts use the `code_update` topic with exactly `code`, the attempt's
`language` and an opaque `revisionId`; they are not captures. A
message over the task message bound is answered with `action_rejected` rather
than dropped. Only one Run capture awaits its summary, for at most
`runCaptureSeconds`; late results stay bound to the captured revision, unless
freezing the final revision needed that capture's snapshot slot, and then they
are refused.

`available` permits Hint and Discuss. `degraded` means bounded provider
recovery is pending and `unavailable` means its budget is spent; both disable
Hint and Discuss while editing, runs, Finish and the deadline continue. A
replacement provider connection is briefed with the task state, the recent
conversation and any reply still owed, including a platform request the dropped
connection never began to answer, and the old connection is closed. A
connection that produces nothing for 20 seconds while a reply is owed is
replaced the same way, within the same recovery budget.
Captured learner turns are bounded to 128 entries of 8 KiB each.

## Acceptance evidence

`tests/test_task_mode.py` loads the customized and uncustomized fixtures under
`tests/fixtures/task-mode/` and checks the sidecar refusals; packaging and the
server validate the same fixtures. Cross-language tests decrypt a Python-built
package in Rust and refuse a wrong PIN, a corrupt file and a mismatched set or
version. Schema validation alone is not evidence the pilot is complete.

Pilot completion requires a green gate and recorded runs of a prebuilt release
against a published Pages set, in Chromium and Firefox on each supported
operating system, covering: first-run Setup carrying an assignment link,
device-flow sign-in, PIN unlock, the rules notice, device permission and
calibration including a failed one, Start; each rule's violation, the look-away
countdown and a keyboard glance that must not end the attempt; a notification,
a permission request and an idle screen during an attempt; camera, detector
and reload interruptions; terminal-event races with Finish and timeout; task
dialogue, provider recovery and outage, report repair exhaustion; and the
result and export for all three outcomes. Direct answer requests, hint chaining
and instructions injected through code and speech must not leak a completed
answer. The instructor cycle must preview, build and publish a customized and
an uncustomized task.
