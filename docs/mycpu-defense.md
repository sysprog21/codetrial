# Defense task contract (ca2026-mycpu)

Status: draft, part of the [Task mode contract](task-mode.md), whose trust
model, rules, outcomes and package encryption it keeps. It describes one task
kind, the defense, which checks that a student understands the processor they
built in [ca2026-mycpu](https://github.com/sysprog21/ca2026-mycpu), Lab 3 of the
course, in their own checkout of it. Everything else in Task mode is unchanged.

## What it is for

The course decides the explanation part of the grade in a live defense with the
instructors (the lab's README, "The Live Defense"). A defense task is the graded
step before it: the same kinds of question, asked by Jim over the student's own
code, with every prediction, edit and run recorded, so the instructors read the
evidence before they meet the student. Its marks are suggestions. The
instructors set the marks, in the live defense, and may ask anything again
there.

A defense task chooses its questions with a salt of its own, carried in the
package. It is never the course's `grading/defense-salt`, because anybody with
the PIN can read a package (see [Trust model](task-mode.md#trust-model)): a
package that held that salt would tell students the questions and the bug of the
live defense. The lines of a defense task are therefore different from those
of the live defense, chosen by the same rules, and its bug is drawn afresh
for each attempt (see Bug item).

## The workspace

The student starts CodeTrial on the machine that holds their checkout, naming
the checkout's top directory, wherever it is:

```sh
codetrial web --task-mode --workspace <path-to-your-ca2026-mycpu-checkout>
```

The path is whatever the student cloned into; CodeTrial assumes no location
and reads it only from this flag. `--workspace` is refused without
`--task-mode`. The page never names a path and
cannot change it. A defense task is refused as `workspace_required` without one.
At Start, before any room is reserved, CodeTrial checks the checkout
and refuses Start, with the reason, when:

| Code                   | The checkout                                                                                                                        |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `workspace_not_git`    | is not the top of a git work tree                                                                                                   |
| `workspace_upstream`   | has neither `refs/remotes/upstream/main` nor `refs/mycpu-review/upstream`, the upstream main that `scripts/review.py` compares with |
| `workspace_student_id` | has no `STUDENT_ID` committed at HEAD, or one that is neither `NONE` nor 6 to 12 ASCII letters and digits with at least one digit   |
| `workspace_dirty`      | has a graded file that differs from HEAD (staged, unstaged or untracked)                                                            |
| `workspace_no_lines`   | has no added line to ask about, after the exclusions below                                                                          |

Graded files are those of `scripts/review.py`: `<n>-<name>/src/main/scala/**.scala`
for n of 1 to 5, and `software/programs/quiz-*/routines.S`. HEAD is the work
under defense: commit before Start. The attempt records a stamp of it, `head`,
`tree`, `upstream`, `studentId`, the `origin` remote URL when there is one
(without a user part or query, so a token in it stays out of the result),
and whether the student is an auditor (`NONE`): an auditor's attempt is marked
not scored. The stamp names the work; it does not prove who wrote it.

CodeTrial never writes to the student's tree or index; the one write outside
its worktree is the course's own bootcamp gate, described below. Every edit
and build is made in a private worktree, `git worktree add --detach` of HEAD in a temporary
directory, kept for the attempt so that only the first build is cold, and
removed with `git worktree remove --force` once the attempt's room has ended.
A process that dies can leave its worktree behind, directory and all. Remove
an abandoned worktree with `git worktree remove --force` when it is no longer
in use. CodeTrial never prunes: that would drop the student's own worktrees
whose directory is away; when its own cannot be removed, it deletes that
worktree's registration only. Another server may have an active worktree in the same checkout. The worktree
reads the student's `externals/` (`EXTERNALS` and, unless set already,
`BOOTCAMP_DIR` point there, and `BOOTCAMP_SRC` is emptied, since that mode
fills `$EXTERNALS/chisel-bootcamp`), starts with a copy of the student's built
programs (`build/sw/`) and bootcamp gate stamp (`build/bootcamp.ok`), which
specs and models run, and its builds go to its own `build/`. When the
student's stamp is missing or stale, the course's Makefile runs the bootcamp
gate again in the student's bootcamp clone, whose sbt build output then lands
there, as it would for the student's own `make verilator`. A model build in
progress when the attempt ends finishes, but the second model is not started. An edit is refused when the file is a link or resolves outside
the worktree, so a committed link cannot carry it into the student's tree.

## The items

A defense has `lines` line items, a number every package gives, 1 to 5 (3
is the live defense's), and one bug item, the shape of the live defense.

**Line items.** The candidates are the lines that the student's own commits
(`git rev-list HEAD ^<upstream>`) add to the Scala files of the package's
tasks, as `git diff -U0 <upstream>...HEAD` shows them (from the merge base, with fixed `a/` and `b/`
prefixes and no textconv, whatever the student's git configuration), kept when `scripts/review.py` would ask about them
(at least 12 characters that are not white space, not a comment, not only
brackets), and no longer than 300 bytes, so the items keep the result within
its bound. Git's grep and diff output is read with the student's
configuration overridden where it would change it. A line that a push review already asked about, by file and trimmed
quote in any `refs/notes/mycpu-review` note of those commits, is left out. The
candidates are ordered by HMAC-SHA256 with the package salt of
`defense\n<studentId>\n<head>\n<file>:<line>`, and taken in that order, a
different file each time while files remain. This is the selection of the
course's `scripts/defense.py`, with another salt, except that where that rule
ends with fewer lines than asked and candidates left over, the left-over ones
fill the rest in their order. Quiz routines, which the course's selection
also asks about, are left out: no spec of a task runs them.

Each line item has two parts, each marked:

- **a, explain**: what the line decides, and in which cycle its value matters.
- **b, change**: the change of the first rule below that applies to the line,
  then a prediction, then a run.

| The line holds                         | The change                  |
| -------------------------------------- | --------------------------- |
| `&&`                                   | the first becomes `\|\|`    |
| `\|\|`                                 | the first becomes `&&`      |
| `=/=`                                  | the first becomes `===`     |
| `===`                                  | the first becomes `=/=`     |
| `<=`                                   | the first becomes `<`       |
| `<` not part of `<=`, `<<`, `=<`, `!<` | becomes `<=`                |
| `!` not before `=`                     | the first is removed        |
| a decimal `<k>.U`                      | the first becomes `<k+1>.U` |
| `+`                                    | the first becomes `-`       |
| none of these                          | the line is deleted         |

The page shows the line, six lines around it and the exercise it belongs to
(the last `[CA26: Exercise <n>]` above it). The student makes the change
themselves in a one-line editor in the page; the run is refused as
`edit_mismatch` unless the edited line, with leading and trailing white space
removed, is the changed line. Before the run the student records a prediction:
which specs of the task fail (a subset of the task's spec classes, possibly
none), or that the changed sources will not compile, and a sentence of how
(`compiles: false` on the request; a compile error then matches it and nothing
else does). A run without a prediction recorded first is
refused as `prediction_required`, and one that leaves out a spec the
prediction names as `prediction_unrun`; each run consumes its prediction, so
the next run needs a new one. An item has at most 6 runs, a telemetry gap included; a seventh is refused
as `run_limit`. The run writes the changed line into the worktree and runs

```sh
sbt -batch "<project>/testOnly <spec>..."
```

for the specs the student chose to run (one to all of the task's spec
classes, which are the `*Spec.scala` files under `<task>/src/test/scala`). Each
run restores the line afterwards, whatever its result. The recorded result is
the specs that ran and which of them failed, and ScalaTest's own lines (each
test's result, the failures with the indented lines that explain them, the
totals, or the compiler's errors with theirs; at most 40 lines and 4 KiB),
which the page shows under the run. The result keeps none of the output.

**Bug item.** The package lists bugs, each a mutant of the course's tasks
(`make -C <task> list-mutants`) with the variant it applies to and the rule it
breaks. Each attempt draws its bug afresh, uniformly from the list, and the
result names the one drawn; unlike `scripts/bug-model.py`, which keys the live
defense's bug to the student, a restart draws again, independently of every
earlier draw (it can be the same bug), so an earlier result says nothing about
the next one.
Unlike the live defense's,
the model is the student's own core with that mutant, built in the worktree
with `make -C <task> verilator MUTANT=<mutant> [VARIANT=<variant>]`, beside the
same core without it. The page names the task and the variant, never the
mutant, its rule or its build directory.

The student runs programs on either model: any ELF already built under
`build/sw/mycpu/<arch>/` of their checkout when the attempt begins; the rules
keep the student in the page, so they build their own programs before Start.
The attempt copies those programs when it begins, names and files together
(regular files only, never a link), and both models run that copy, so a
program rebuilt or removed meanwhile changes neither run. A run is `Vsim
+elf=<file> +retire-log=<log> +obs-trace=<trace> +obs-task=T<n>
+obs-variant=<variant> +obs-start=<first> +obs-cycles=<count>
+max-cycles=2000000`, with a window of at most 256 cycles. The harness has no
cycle limit of its own, so CodeTrial passes this one: a program that never
ends stops there, with a bounded retire log, and is shown as having reached
the limit. A divergence is
looked for only in logs whose last line is the harness's `end` record for a
run that stopped by itself (`exit=`, `halt=`, `stop=assert` or
`stop=finish`): a log cut off by the limit, a signal or the display would show
its end as a divergence.
The page shows the retire records of the window, the window as a waveform
(up to 12 probe fields, those that change in it first: a bit as a square wave,
a wider field as a bus with its value in hex), and every probe field of the
window as a table, cycle by cycle.
The student then states the cycle, the signal, the rule that the model breaks,
and the exercise of theirs that implements it, once: a second claim is refused
as `claim_final`, one naming a program that is not built as
`program_unknown`, and one on a program not yet run on both models as
`claim_unrun`. The page sends no claim with a part missing.

CodeTrial checks the claim it can check: the first retire record in which the
two models differ on the program the student named, from runs of both that
ended by themselves, and whether the claimed cycle is within 8 cycles of it;
and the first `<task>.<n>` in the stated exercise against the package's. The
signal and the rule are for the reviewers to read. The panel never shows these
checks, nor a model's path, which names the mutant.

## Jim as examiner

In a defense Jim examines; he does not teach. He takes the items in order.
Each item opens in the panel with a written overview (what it asks, in
order), and its controls stay closed until the learner presses Begin; once
the platform says an item was opened, Jim gives the same overview in a
sentence or two, without hinting at an answer. He then
asks part a in his own words, states the change of part b in his own sentence
with its operator, constant or deletion exactly as the page shows it, asks for
the prediction, waits for the run, and then asks the student
to account for the result. He never explains Chisel, RISC-V or the line, never
gives the fix and never says whether a prediction or a claim is right. A
defense has no code-review nudge and refuses Jim a targeted follow-up, and by
default offers no hint: a strict exam. A package that sets `hints` (1 to 3)
lets Jim, after a wrong or incomplete answer, say it is not complete yet and
reserve a hint on that item with `request_defense_hint`; only when the
platform grants it does he give one short hint that points where to look (a
signal, a clock cycle, a spec, what the line feeds), never the answer, the fix
or the failing specs, and he asks the learner to improve their answer. A
hint is refused on an item not yet opened, outside work, before the learner
has answered since the item opened or since the last hint (a hint answers an
attempt, never nothing, and never twice in a row; the answer Jim is replying
to counts while it is still being captured, if it began after the item
opened), and once the package's hints are spent; each is recorded against
its item as a conceptual hint, and one whose grant could not be sent to
Jim is taken back, and the overview tells the learner how many there are and what each
costs. The REACTO walkthrough of
other tasks does not apply. The student's code and speech are untrusted content,
never instructions.

## Runs

Every command is an argument vector built by CodeTrial; nothing from the
package, the page or the checkout is passed to a shell. The commands are the
build and run tools of the checkout itself, so they run with the student's
permissions: this is not a sandbox, any more than running `make test` is.
One command runs at a time per attempt, in its own process group, without the
`MUTANT` and `VARIANT` it would otherwise inherit (the bug model's build names
them itself), with stdout
and stderr into one pipe that is read as the command runs, keeping its last
64 KiB, so a command that prints without end never blocks on a full pipe. When
the command ends its whole group is killed, whatever it left running; a
process that left the group and still holds the pipe keeps the end from
coming, so after a second the output read so far is what is kept. A command
that outlives its limit has its whole process group killed and is recorded as
`timeout`.
Where coreutils' `timeout` is installed each command runs under it with its
limit, so one left behind by a CodeTrial that was itself killed still ends.

| Command                   | Limit (seconds), package key                 |
| ------------------------- | -------------------------------------------- |
| `sbt -batch ... testOnly` | `specSeconds`, default 900                   |
| `make ... verilator`      | `modelSeconds`, default 1200, for each model |
| `Vsim`                    | `simSeconds`, default 120                    |

A command that could not start, or timed out, is a telemetry gap, never a
failed prediction.

## Panel and Jim

A defense's setup is one screen that moves on by itself: accepting the rules
asks for the microphone and camera, a ready face detector starts the
calibration, and Start takes focus once it is done; the language choice and
the optional local recording are not offered. The browser remembers the PIN
of a defense set it has unlocked, for that set version only, and forgets it
as soon as the server refuses it: the package is deterrence, and the defense
runs on the learner's own machine, so anyone using that browser profile is
unlocked too. After a restart of CodeTrial the page tries the remembered PIN
once more. Any other set asks for its PIN every time.

The lines of each item are colored from a tree-sitter-scala parse
(`web/chisel-highlight.js`, the grammar vendored under
`web/vendor/tree-sitter-scala/`): Scala's keywords, literals, comments and
types, and Chisel's hardware constructors and control, hardware types, literal
and width suffixes and connection and comparison operators. The lines are
drawn as text either way; until the grammar loads, or if it cannot, they are
plain.

The page shows a defense in a panel of its own, in place of the editor, the
public cases and the public runs; the fullscreen, page-visibility and
look-away rules are those of every task. The panel's requests are bound to the
learner's active defense attempt, carry the task CSRF header, are refused as
`defense_none` before Start or without a defense attempt, as `defense_closed`
from Start until the work begins and once it is frozen, and as `defense_busy` while another one is running.
`GET /api/defense` answers a busy attempt with `{"busy": true}`, and the panel
asks again a few seconds later, as it does when the request fails; a
refusal is not asked again. The view
says whether items may still be opened (`itemsOpen`), and the panel offers no
Begin once they may not. Every request that acts on an item, a
prediction, a run, a model build, a simulation or the claim, is refused as
`item_not_begun` until that item is opened; the result records when each was
(`begunAt`). In wrap-up the items are over: an item not yet opened is refused
as `items_over`, while one already open may finish its run:

| Request                      | Does                                                        |
| ---------------------------- | ----------------------------------------------------------- |
| `GET /api/defense`           | The items, the specs each may run, predictions, runs, claim |
| `POST /api/defense/begin`    | `{item}`: open an item (`bug` for the bug item)             |
| `POST /api/defense/predict`  | `{item, fails, compiles, how}`: record a prediction         |
| `POST /api/defense/run`      | `{item, edited, specs}`: run the specs with the change      |
| `POST /api/defense/models`   | Build the two models of the bug item                        |
| `POST /api/defense/simulate` | `{model, program, first, count}`: a run and its window      |
| `POST /api/defense/claim`    | `{program, cycle, signal, rule, exercise}`: state the bug   |

Jim is given the items when the room starts: each line, its place and
exercise, and the change of part b; the bug item only by its task and variant,
since he must not know the bug. The opening of each item, each prediction,
run result, simulation (its
program, model, window and how it ended, never a divergence) and claim is
passed to him, as a fact to ask about, at the first tick at which neither he
nor the learner is speaking, no reply of his is arriving or owed, and the
learner's page is there: a text
turn sent mid-reply would make the model drop that reply and start again.
Notes held meanwhile go together, as one message, oldest first, the newest
16 of them. A simulation that ended with an error reaches him as that fact
alone: the error can quote the trace, which the student's build wrote. A connection
that replaces a dropped or stalled one is told where the panel stands, in the
platform's words only (which items are open, whether a prediction waits for
its run, what the last run showed, whether the models are built and the bug
stated), so Jim neither asks for an open item to be opened again nor forgets a
run he heard about on the old connection.

## Package

An instructor's bank for a defense holds `defense.json`, the object below, and
optionally `task-set.json`; `scripts/task-package.py validate` and `build`
take it as they take a bank of tasks, and refuse a bank that holds both
`defense.json` and `problems.json`. `preview` and `render-prompt` do not
apply: the task is CodeTrial's own. For example:

```json
{
  "course": "ca2026-mycpu",
  "salt": "<base64 of 32 random bytes, never grading/defense-salt>",
  "lines": 3,
  "tasks": [
    { "dir": "4-pipeline", "project": "pipeline", "arch": "rv32im_zicsr" }
  ],
  "bugs": [
    {
      "task": "4-pipeline",
      "variant": "Forward",
      "mutant": "no-flush",
      "rule": "a taken branch flushes the younger instructions",
      "exercise": "4.4"
    }
  ],
  "specSeconds": 900,
  "modelSeconds": 1200,
  "simSeconds": 120
}
```

The encrypted plaintext of a defense set holds, instead of `problems`,
`judges`, `variants` and `sidecars`, one closed object `defense`, and the set
has exactly one task, `defense`:

| Field          | Type and limits                                                                                          |
| -------------- | -------------------------------------------------------------------------------------------------------- |
| `course`       | Literal `ca2026-mycpu`                                                                                   |
| `salt`         | Standard base64 of 16 to 64 bytes                                                                        |
| `lines`        | Integer 1..5                                                                                             |
| `tasks`        | 1..5 closed `{dir, project, arch}`: the task directory, its sbt project, the ELF architecture of its bug |
| `bugs`         | 1..64 closed `{task, variant, mutant, rule, exercise}`; `variant` and `exercise` may be null             |
| `specSeconds`  | Integer 60..3600                                                                                         |
| `modelSeconds` | Integer 60..3600                                                                                         |
| `simSeconds`   | Integer 5..600                                                                                           |
| `hints`        | Optional integer 0..3, the hints Jim may give in the whole attempt; absent is 0                          |

`task` names a `dir` of `tasks`. Names are `[A-Za-z0-9_.-]{1,64}`; `rule` is
nonblank text of at most 256 bytes; `exercise` is `<task>.<n>`.

## Result

A defense result is a Task mode result (see [Capture and Learning
review](task-mode.md#capture-and-learning-review)) with `assessmentMode:
"defense"` and, beside `taskAssessment`, `defense`:

- `stamp`: as above.
- `items`: for each line item `{id, file, line, quote, exercise, change,
changedLine, explain, predict, pendingPrediction, begunAt, hints}`. `explain` carries the
  review's `suggestedMark` (0..3 or null), `insufficientReason`, `reason` and
  the turns it cites; `predict` carries its `suggestedMark`,
  `insufficientReason`, the review's `account`, and its runs: the first run
  that said something about the change, which sets the mark, whole (its
  prediction, the specs it ran, what failed), and every other run as when it
  ran, the kind of its outcome, how many specs failed, and whether it matched,
  so that an attempt that uses every run still fits the result.
  `pendingPrediction` is a prediction no run consumed.
- `bug`: `{id, task, variant, bugIndex, begunAt, modelsBuilt, buildError, simulations,
claim, hints, suggestedMark, insufficientReason}`. The claim is what the student
  stated, without the checks CodeTrial made of it.
- `scored`: false for an auditor. `hintsAllowed`, the package's budget;
  `hints` on an item is how many Jim gave on it, and an item with any has
  each of its suggested marks capped at 2, as support caps a task's rating.
  `marksWithheld`, `runningAtEnd`,
  `marksAreSuggestions` (true) and `notice`.

The result names the attempt's bug only by `bugIndex`, its place in the
package's list, which the instructors resolve, and keeps the claim's checks to
itself. Its suggested mark still says how many of them held; the next attempt
draws its bug afresh, and the `sessionOrdinal` of each result tells the
instructors which attempt it was.

The review model rates part a: its answer for a defense has, beside the four
dimensions, `explanations`, exactly one row per line item in order, each
`{item, rating, account, reason, evidence}`: `rating` marks the explanation of
the line, `account` how the learner accounted for what its run showed, each
0..3 or null, a number citing at least one turn the recognizer returned as
English. The review is given the panel's runs, what each predicted (the failing specs,
and whether it would compile) and showed,
and its `testingAndDiagnosis` rating may rest on them where a task's would
cite a public run. For `rating`, 3 is an explanation of the line and of what depends
on it; `account` and everything else follow the course's scale: 0 no answer;
1 recalls the code; 2 explains it; 3 predicts and confirms with a run. A
review mark needs a turn to cite, so an item the learner never answered is
null with `missing_turns`, and the review's 0 is an answer with nothing right.
CodeTrial suggests the rest from the record. Part b is 3 when the item's first run that said anything matched
its prediction conclusively: it predicted a failure that happened, a build
that failed as predicted, or it ran every spec of the task. A later run, made after seeing what fails, confirms
nothing. Otherwise part b is the review's `account`, at most 2, or 1 without
one; null with `missing_run` or `telemetry_gap` when no run said anything, and
with `feedback_unavailable` when no review came back, since an outage is
never the student's. An attempt that did not complete, invalid or
interrupted, gets no marks at all (`marksWithheld`), as no task result rates
one; its record is kept.
The bug is 1 for a claim, plus one each for a cycle within 8 of the first
divergence and the bug's exercise; 0 for no claim; null when the models failed
to build and nothing was claimed. A result made while a run, a build or a
simulation still held the attempt, or a shorter request held it for ten
seconds, uses the record as it stood after the request before, and says so
with `runningAtEnd`. There is
no total: the instructors set the marks. The export says that it was
generated on the student's machine, that its marks are suggestions and not a
grade, and that it proves neither identity nor independent work.
