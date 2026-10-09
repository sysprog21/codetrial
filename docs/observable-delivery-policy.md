# Observable delivery policy

What a CodeTrial report may say about a candidate, what it may not, and what
enforces the line.

## What is assessed

Engineering content present in candidate speech, code, and test reasoning.

Not assessed: accent or dialect, typing speed, filler words or disfluencies,
posture, eye contact, facial expression, body language, voice tone,
attractiveness, presentation-derived nervousness or confidence, and personality
or psychological traits. Camera and audio presence and integrity events describe
session conditions, not candidate performance.

Speech-recognition failures are session conditions too. Unexpected language
switches, garbled speech and contextually unrelated transcripts require
clarification, not an inference about the candidate. Uncertain turns and
unsupported notes must not earn or lose assessment credit or justify a verdict;
interviewer agreement does not establish that the candidate answered correctly.
Use clear candidate clarification or independent engineering evidence, and state
when reliable communication evidence is insufficient. The gap itself is neither
clear nor unclear communication, so it cannot by itself decide the verdict.

The interim notes and the report never read a candidate turn written mostly
in a non-Latin script: the English interview makes such a turn the
recognizer's output, and they read a fixed marker saying it was not recognized
as English in its place. A turn of only one or two such letters, a symbol the
question is about, is kept. The live interviewer, the replay and the stored
transcript keep the recognizer's text, and because the interviewer heard that
turn, the server refuses evidence it records from the candidate's speech until
the candidate's latest turn is one the report can read. Recognition errors that come back in
Latin letters, into Spanish or into unrelated English, are not marked, and
there the live, interim and report prompts, with the scan below, are what
enforces this policy. The recognition hints sent at live setup shape only the
transcript that notes, reports and recovery read; the interviewer hears the
audio itself. None of this is a guarantee that provider-generated transcripts
are accurate.

## What enforces it

The report prompt states the boundary, and the server independently scans every
provider-authored narrative field before accepting a report. A prohibited claim
gets a path-specific semantic validation error naming the matched phrases,
grouped by policy rule, and the provider has up to two semantic repairs within
the shared five-call report budget. The repair prompt bounds the number and
length of errors it carries: an error names as many phrases as fit and counts
the rest, and every failing path gets one error before any path gets a second.
When the repairs run out, the latest response that becomes a valid report
once its prohibited self-review checks and follow-up assessments are dropped
and its prohibited success criteria replaced is kept, even if a later response
broke something else: a list left empty gets one fixed, neutral check, a
criterion is replaced with fixed text that judges no one, and a follow-up keeps
whether it was raised without an assessment. Without such a response, a claim that remains
after the last repair leaves the report incomplete, with no score or verdict,
and the candidate may request the one regeneration that
[provider cost and degradation](provider-cost-and-degradation.md) describes.

The same scan refuses a report that names the natural language a transcript
came out in, such as "a response in Mandarin", or judges English proficiency,
and an improvement or plan item that asks the candidate to speak English,
audibly or more clearly. The language is the recognizer's output, not the
candidate's, and a candidate switching programming languages remains a coding
event the report may name. A neutral mention of transcription passes, because
refusing it through every repair would leave the report incomplete.

Technical uses such as a confidence interval, or an evidence record's explicit
confidence value, are not presentation judgments and remain allowed.

## Adding a delivery signal later

Any future delivery signal requires an explicit opt-in separate from interview
recording consent, independent validation for reliability and subgroup effects,
clear `observed` versus `inferred` provenance, purpose-limited retention and
withdrawal, accessible non-participation, and a new prompt, rubric, and report
contract. It cannot silently reuse camera, microphone, avatar, or integrity
telemetry.
