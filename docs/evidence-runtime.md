# Evidence runtime

The evidence ledger is the deterministic, session-scoped record of observations
used by the interview runtime. Version 1 records an append-only sequence with a
browser source timestamp when supplied and a server receipt timestamp. The
receipt is read from the server's clock once per packet and is never taken from
the packet: `at` is the browser's claim, and the receipt is the one timestamp in
the ledger that is not. Its reduced code state holds a digest and an explicit
parser status, never editor text. Its test state holds claimed totals, deltas, a
regression flag and a bounded failure digest; browser-reported test results are
observations, not a grading verdict.

A run that reports no cases at all did not execute. It is recorded as an
attempt, with its diagnostic, and it does not move the deltas or the progress
flags: a runner that failed to start is not a regression, and the next run of
unchanged code after one is not progress. An edit followed by a run is one
edit/test cycle; further runs over an untouched buffer are retries and are not
counted again.

Code observations use Tree-sitter 0.25.10 with pinned C, C++, Java,
JavaScript and Python grammars. A successful parse classifies only stable
syntax facts, and those facts are the named structure, the identifiers and
every leaf token, compared in document order. The tokens and the order are both
load-bearing: operators are anonymous nodes in each of these grammars, so facts
drawn from named nodes alone cannot tell `a + b` from `a - b`, and facts
compared as sets cannot tell `f(a, b)` from `f(b, a)`. Unsupported languages or
unavailable parsers produce `parser_unavailable`, and an error tree produces
`syntax_invalid`. Both states intentionally make no structural claim, and no
text-diff fallback exists.

The control-flow and interface kinds a classification turns on are named in
full rather than matched as substrings, and they are named for all five grammars
rather than for one. Data structures are the exception, matched on words such as
`list` and `dict` with syntactic groupings and destructuring patterns excluded,
because the literal kinds are too many to enumerate. Every
call is spelled with the word "function" or "method" somewhere, so a substring
match reported calling a helper as changing the signature of the function it
was called from; every grammar has its own word for a loop, so a list built
from `for_statement` alone reported `for (const x of items)` and `for (int x :
a)` as unclassifiable. What is nested inside a closure is recorded as the
closure's, under a `closure.` prefix, because a comparator's parameter list is
spelled exactly like a signature's and counting the two as one kind reported
writing a comparator as changing a signature.

A loop whose binding changes shape, `for i in` becoming `for i, n in`, is a
control-flow edit even though no loop was added: the binding is compared by node
kind alone, so renaming the loop variable stays a rename. Kinds outside the
three categories are an expression edit however many of them moved.

The model is told less than the ledger records. A code change reaches the
projection as a coarse class, `formatting`, `comment`, `identifier` or `code`,
with no node facts. The finer class is a heuristic over grammar node kinds and
the ledger keeps it for replay, but the review gate turns only on whether an
edit was layout or a comment, and nothing yet shows the finer class helps the
interviewer rather than misleading it when it is wrong.

An edit that repairs a buffer that did not parse is a semantic change even when
there is nothing to diff it against. The pairwise analysis reports
`syntax_invalid` when either side fails to parse, so the repair is recovered
from the retained baseline where one exists, and otherwise recorded as the
parse of the buffer in hand: observed, semantic, and carrying no classification
that was never computed.

A language switch carries the new tab's buffer in the same packet, so there is
nothing to diff it against and it is observed on its own: the parse is recorded
with no classification. The short-lived baseline that recovers a completed edit
out of an invalid draft is tagged with the language it belongs to and survives a
trip to another tab.

Diagnostic categories are counted only when the runner named one from structured
execution state; free-form compiler prose is counted as `other` rather than
classified. Categories nothing observed are left out of the projection instead
of being written down as zero, which would be a claim no measurement supports.

Ledger entries are bounded and session-scoped. Raw code, raw runner output and
transcripts remain outside the ledger. They must not be exported as analytics
through this runtime.

The fixture in `tests/fixtures/evidence-ledger.json` is replayed against the
frozen ledger in `tests/golden/evidence-ledger.json`, which is what makes a
change to any of the above visible as a diff. Regenerate it with
`UPDATE_EVIDENCE_GOLDEN=1` after a deliberate change and read the result.

The reducer also measures raw packets received separately from accepted code
events, and measures what the session handed the models: bytes and a count for
the watch prompts, the conversational turns (greeting, cold restart, the reply
a data event asks for, and the wrap-up), the interim reviews, the final report
prompt, the `read_editor` responses and every other tool answer, refusals
included. Every model-bound text falls into exactly one of them. They are bytes
rather than tokens because the tokenizer belongs to the provider, they are
written to stderr once at the end of a session, and they are deliberately
absent from the projection: a model handed its own byte count is being told
something no interview should turn on. A bounded digest history makes an edit
that revisits an earlier state observable as an undo/redo cycle; it retains
hashes only, each a digest of the language and the buffer together, so the same
text in another tab is not a way back. Meaningful-change and test-progress
receipt timestamps, plus their latency, are measurements rather than claims
about candidate intent.
