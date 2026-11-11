# Teach a change by asking

Two surfaces ask the person questions: the [contributor
explanation](workbench.md) and the [guided review](interactive.md#guided-review).
This page owns what they share: how a question is asked and followed up, and
what a session never becomes. The method follows
[socratic-tutor](https://github.com/janestreet/agent_skills): teach by asking
questions that make the person predict, explain and defend, and explain
directly only after they have tried.

## What a question is

- **Grounded.** It names a path and quotes a line of the pinned diff, or of the
  contract it asks about, so the person can open what is being asked about.
- **Answered first.** The agent has read the answer at the head before asking,
  and has run any test or command the question depends on. A question whose
  answer the agent has not read is an unchecked guess.
- **Aimed at reasoning.** Prefer a question that makes the person predict an
  outcome, explain why, find the flaw in a plausible claim, or apply the idea to
  a new case, over one a sentence from the diff answers.
- **One, then stop.** The message ends on the question: no hint, option, sample
  answer or teaching after it. A compound question hides a checklist and is
  split.
- **Fresh.** A later session on the same branch asks about something else: a
  consequential change in a file no earlier session asked about or, once every
  one has been asked, another angle on the least recent. The sessions of the
  contributor explanation are remembered by `check-interactive-review.py asked`;
  a guided session reads its own ledger's `# Understanding` map.
- **Not a leak.** Nothing the person has not reached is said before they
  answer, a reference answer included. Two things are never held back: a
  leaked credential, candidate material or any P0 is said the moment it is
  seen ([SKILL.md](SKILL.md)), and every finding not yet reached is handed over
  when the session stops or ends.

## What follows an answer

- A wrong or partial answer is not correct. Ask a narrower question that
  exposes the contradiction and names a line they can open. After 2 misses on
  one idea, explain in a few sentences and re-test it from another angle.
- A right answer may be pressed once ("are you sure? what about X?") with an X
  that is a real case at the head, never an invented doubt. Do not flatter.
- A fact the person cannot derive from the diff, a contract or an architecture,
  is told in a few sentences with its source, not asked.
- The agent can be the one who is wrong. When an answer contradicts a claim of
  the agent's, reread the head before defending it. A defect the person states
  is a lead: verify it under [step 4](SKILL.md#4-verify) before it is a
  finding. Credit only what they expressed, and never answer for them.
- "just tell me" answers the current question and nothing more, and earns
  nothing.

## What a session never becomes

- **A grade.** No surface here assesses the person: the contributor's
  explanation is assessed only by the contributor
  ([the contract](../../../docs/large-change-explanation.md)). A guided
  review's map records whether answers matched the head, and its answers and
  map stay in `$SCRATCH`: not in a PR body, a comment, the severity counts or
  an experience report, which says only that a session ran. Neither counts
  toward the explanation, a push, a post or a merge. The explanation's own
  record is kept as [workbench.md](workbench.md#when-the-contributor-confirms)
  says.
- **Started by a guess.** A session starts on an explicit request. When it is
  unclear whether the user wants a quiz or an answer, ask one short question.
