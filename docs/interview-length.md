# Interview length

What the lobby offers, what the endpoints enforce, and how recording lowers
both. One rule matters more than the rest: the length offered and the length
enforced come from one function, so they cannot drift apart.

The lobby offers 30, 45, and 60 minutes, and the candidate picks one before
starting. `CODETRIAL_DURATION_MIN` is only the length preselected for a
candidate who does not choose; the endpoint accepts 10 to 90 either way, so a
deployment that lowers the default has not lowered the ceiling.

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

Startup refuses a cap below `CODETRIAL_DURATION_MIN`, so the length this
deployment preselects is always one it can record; the pairing is a
configuration error rather than a quietly shortened interview. Should a cap ever
sit below every length the row offers, the lobby leaves the row alone rather
than rendering a page with nothing to press.
