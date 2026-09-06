# Integrity evidence

Integrity features produce evidence for a person to review. Nothing here scores
a candidate, and nothing here is a claim that anybody cheated. That rule is
what shapes everything below: what the replay page shows, what it says beside
it, and what CodeTrial declines to look for at all.

Integrity features produce evidence for a person to review. Nothing here scores
a candidate, and nothing here is a claim that anybody cheated.

## Response windows

The replay page lists a *response window* for each turn the interviewer took:
the time between the interviewer finishing that turn and the interviewer
speaking again, taken from the `avatar` state rows the browser recorded. A
window no candidate transcript turn was attributed to says so; the turn itself
is read in the transcript beside the timeline, not on the window.

Attribution is recorded rather than inferred. The browser numbers each window
as it opens, stamps that number on the row opening it, and stamps the number
that was current when a transcript stream started on the turn that stream
produced. A turn therefore lands on the question it began answering, even
though the row carrying it is written when the stream finishes, which is
routinely after the interviewer has moved on.

No window opens before the interviewer has been heard speaking, and none opens
on a `listening` that did not follow one. The browser records a state on first
sight of the interviewer, before there is a question to leave a gap after, and
a window opened there would report the greeting as elapsed response time.

What a long window can mean is bounded by the interviewer itself, and this is
the first thing to know about the number. `timing_decision` in `src/agent.rs`
nudges a candidate who has been silent for 25 seconds and waits 30 between
nudges, so the interruption lands in that range; the nudge makes the
interviewer speak, which closes the window. Neither talking nor typing counts
as silence there, so a window runs long only while the candidate is working: a
long window means they were busy, which is the innocent reading. A candidate
silently reading an answer off a second screen is interrupted within half a
minute and produces a run of short windows holding no transcript, and that run
is the shape worth opening, not the length of any one of them. In the
behavioral round, where the editor is disabled, the nudge fires more reliably
still.

These things belong beside every one of those numbers, and the page says them
where it shows them, alongside the bound above:

- It is measured from the clock of the browser that recorded the interview. That
  is the candidate's own clock, the server never trusts it for ordering, and it
  can jump in either direction: a window can read longer than it lasted as
  easily as it can read as unmeasurable.
- It includes the time CodeTrial took to prepare the reply, not only the time
  the candidate took to give one. The interviewer publishes two states,
  `listening` and `speaking`, so a window closes when the interviewer starts
  talking, which is after the model round trip rather than after the candidate
  stops. That round trip is not the same on every turn — the server logs it per
  turn precisely because it varies — so it is not a constant a reader can
  subtract, and no window is a candidate's response time on its own.
- A window opens whenever the interviewer stops speaking, which is often not the
  end of a question. The interviewer's own silence nudge, proactive review, test
  reaction, time warning, round transition and wrap-up all end in speech, and so
  does a candidate interrupting; a window also follows a moment when the
  interviewer published no state at all, and one where it dropped out and came
  back, and one where the candidate paused the interview. That last one is the
  only cause the replay can name, because the browser records a `paused` event
  for it, and a window overlapping one says so on itself.
- A window with no duration is one the recording ended inside, or one where that
  clock ran backwards between the two rows.
- Everything above describes a browser reporting honestly. The replay is
  produced by the candidate's own page, so a modified one can stamp a turn with
  another window's number and make a silent window read as answered. Nothing
  here detects that, and nothing about the earlier scheme did either: it placed
  a turn by row order, which the same page chose. It is disclosed rather than
  defended because the reviewer's evidence is the recording, and the panel is a
  place to look in it rather than a thing to read instead of it.
- A recording made before the browser stamped those numbers cannot place its
  turns, and says so on each window rather than reporting a question nobody
  answered. The distinction is the whole of it: "no candidate transcript
  recorded" is a claim about the candidate, and it is the only one this panel
  makes, so it is never made on a recording that cannot support it. Past the
  first window there is no way to place an unstamped turn without guessing at
  timing, and guessing is what the earlier version of this did.
- A lost batch is invisible, and it distorts windows five ways. The replay feed
  splices a batch of up to 32 consecutive events off its queue before sending,
  and only a `429` puts it back, so any other failure — the network, an
  oversized batch, a proxy, a `5xx` — drops it. A lost closing row merges two
  windows and doubles the duration; a lost `listening` row omits a question
  entirely; a lost `transcript` row makes an answered question look unanswered;
  a lost `resumed` row would mark the rest of the interview as paused, except
  that the interviewer speaking proves it is not, which bounds that one to the
  window it happened in; and a lost `ended` row brings back the phantom window
  the interviewer's goodbye would otherwise open, on an interview where nothing
  else went wrong. All five are disclosed rather than corrected, because the
  sequence numbers are allocated server-side and a batch that never arrived
  leaves no gap to notice. Each loss costs the windows it touches and no more:
  a window opens on the `avatar` row before it rather than on anything
  cumulative, so the rest of the interview still renders.

A window is a place in the recording to go and watch. It is not a finding: a
candidate who is thinking is slow too, and the number cannot tell the two
apart. It also cannot separate the silence before an answer from the answer
itself, because a transcript turn is recorded when it ends.

## What CodeTrial does not look for

CodeTrial does not attempt to detect interview-assistant overlays, and this is
a decision rather than a gap. The shipped tools exclude themselves from screen
capture at the operating-system level — `WDA_EXCLUDEFROMCAPTURE` on Windows and
the equivalent content-protection flag elsewhere — and at least one turns it on
at launch before any window is shown. There is nothing for a browser or a
recording to see, so a capture that shows nothing is evidence of nothing, not
evidence of absence. Reading gaze direction is refused for the same kind of
reason and one more: those tools tell their users to place the overlay directly
below the webcam, so the reading angle is designed around, and CodeTrial does
not infer intent from a candidate's face at all. What the camera is for here is
presence, which `web/face-presence.js` reports as a count, and reading a
direction off it would be a different kind of claim about a person than
anything this product makes.

What survives both is timing, which is what the response window renders, and a
person watching the replay.
