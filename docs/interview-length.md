# Interview length

What the lobby offers, what the endpoints enforce, and how recording lowers
both. One rule matters more than the rest: the length offered and the length
enforced come from one function, so they cannot drift apart.

The lobby offers 30, 45, and 60 minutes, and the candidate picks one before
starting. `CODETRIAL_DURATION_MIN` is the length preselected for a candidate who
does not choose; the endpoint accepts 10 to 90 either way, so a deployment that
lowers the default has not lowered the ceiling.

`/api/session` reports the default as `defaultDurationMin`, clamped into 10 to
90; where recording is on, startup refuses a cap below that clamped length, as
the last paragraph says. The lobby preselects it for Medium and Hard, and
for Easy at most 30. A default that is not one of the three buttons selects the
nearest enabled one, and a length the candidate chose is never replaced by the
default. The Easy cap and the snapping are the lobby's suggestion only.
`/api/token` does not look at the problem: a request that names no length, such
as a link that skips the lobby, is granted `defaultDurationMin` itself, not
snapped, whatever the difficulty. A `duration` in the interview URL that is not
a positive number (`0`, `-5`, `30abc`, `abc` or empty) names no length, so the
interview page sends none and gets the same default.

Recording lowers it. `CODETRIAL_RECORDING_MAX_MINUTES` bounds a single
recording, and the sweeper fails one still running five minutes past it, so an
interview longer than the cap loses exactly the stretch past it and the artifact
is failed rather than delivered. Where recording is on, the longest interview
this deployment will start is that cap, clamped into the 10–90 the endpoint
offers; where it is off, the ceiling is 90.

The lobby is told the ceiling rather than left to discover it. `/api/session`
reports `maxDurationMin`, and the duration row disables the lengths above it
with the reason beside them — a length this deployment cannot record is never
offered and then quietly shortened. `/api/token` clamps to the same ceiling, so
a request that skips the lobby is bounded too, and both endpoints read it from
one function so the length that is offered and the length that is enforced
cannot drift apart.

Where recording is on, startup refuses a cap below `CODETRIAL_DURATION_MIN`,
held to 10 to 90 like the default itself, so the length this deployment
preselects is always one it can record; the pairing is a configuration error
rather than a quietly shortened interview. Should a cap ever sit below every
length the row offers, the lobby leaves the row alone rather than rendering a
page with nothing to press.
