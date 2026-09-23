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

Parsing reuses one parser per grammar and keeps the last buffer's tree, so an
update parses only the new buffer, incrementally from the previous tree edited to
match it. A test holds the incremental tree to the fresh one node by node,
because an edit that claims too little of the buffer leaves nodes at the wrong
offsets while still counting the same kinds. With the crate optimized an update
costs about 1 ms at fifty lines and 12 ms at five hundred, down from 3.6 ms and
39 ms, and it stays inline on the agent's task; a buffer past 64 KiB is recorded
as not parsed instead.

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

The model is told less than the ledger records. What a prompt carries is a few
lines of plain text rendered from the ledger's aggregates, and of those only the
lines the rest of that prompt does not already state: the code history with the
last edit as a coarse class (`formatting`, `comment`, `identifier` or `code`),
the test state and nonzero diagnostic categories, labeled as the browser's
unverified claims, hints, phase coverage, the time since the last program
change and the session state. A watch prompt carries the code and goes to a
session that holds the conversation, so it gets the tests, diagnostics, hints,
phases and session state; the interim review gets the code history, tests,
diagnostics, last change, hints and session state; the report, whose own
sections carry the last run, the hint counts, the framework evidence and the
transcript, gets the code history, the test history, the diagnostics and the
session state. No
entry, digest, timestamp or node fact reaches any of them. The ledger itself was
the prompt once, as JSON capped at 6,000 bytes; counted with Gemini's tokenizer
that was 1,000 to 2,700 tokens a prompt, more than half of them SHA-256 digests,
and a simulated forty-five line session spent 42,212 tokens on review evidence
where pasting the code had spent 6,776. The text view is bounded by construction
and costs tens of tokens. The finer class stays in the ledger for
replay: the review gate turns only on whether an edit was layout or a comment,
and nothing yet shows the finer class helps the interviewer rather than
misleading it when it is wrong.

A watch prompt sends only the lines that differ from the ones the last watch
prompt left the same Live session holding, and no evidence heading at all when
none do; a cold replacement session, which holds nothing, gets every line, and a
prompt the socket refused leaves the lines it carried to be sent again. The
simulated session above spent 7,551 tokens on reviews against `main`'s 6,776,
with the code in every review and one review more, which the semantic gate
fired.

A proactive review is armed by `substantive_revision`, the program changes that
were not renames alone, and only while the buffer parses: a review of a rename
or of a half-typed line is an "mm-hm" at best, and the next change that parses
arms it again. `semantic_revision` still counts every program change for the
evidence it reports. The code itself reaches a watch prompt fenced as the candidate's
untrusted text and numbered as `read_editor` numbers it: the whole buffer up to
eighty lines and 4,000 bytes, and past that the lines that changed with three lines of context, at most forty
lines of 160 characters. The change is measured from the code the model was
last shown, by any carrier: a watch prompt, a test reaction, which carries the
change as well, `read_editor`, a requested hint or a cold briefing. With no
change the prompt says the editor is unchanged rather than sending it again or
sending the model to read it, and the interviewer is told to call `read_editor`
only for code nothing has shown it. A watch evidence line that was shown and
has since gone is sent as its key with `none`.
Sending only the changed lines was tried first: in a small sample against the
Live model the interviewer read the editor on every review anyway, reaching
first audio in about 950 ms against 565 ms when the whole buffer came with the
prompt, and the read returned the whole buffer regardless. The code is source,
so it is taken from the runtime's buffer and never enters the ledger.

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
classified. Categories nothing observed are left out of the prompt instead
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
included; an HTTP prompt is counted with the system instruction it is sent
behind. Every model-bound text falls into exactly one of them. They are bytes
rather than tokens because the tokenizer belongs to the provider, they are
written to stderr once at the end of a session, and they are deliberately
absent from the prompt: a model handed its own byte count is being told
something no interview should turn on. Beside them, every model-bound text is
folded in order into one SHA-256, so two runs of a session sent the same prompts
exactly when that digest matches.

The interim review and the final report sample with a fixed seed. Measured on
the report model at its temperature, four calls with one prompt and the seed
returned one answer, and four without it returned four. The Live session is
left unseeded: identical words for every candidate is not a trade the interview
should make, and its audio cannot be replayed byte for byte either way. A bounded digest history makes an edit
that revisits an earlier state observable as an undo/redo cycle; it retains
hashes only, each a digest of the language and the buffer together, so the same
text in another tab is not a way back. Meaningful-change and test-progress
receipt timestamps, plus their latency, are measurements rather than claims
about candidate intent.
