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

## What enforces it

The report prompt states the boundary, and the server independently scans every
provider-authored narrative field before accepting a report. A prohibited claim
gets a path-specific semantic validation error, and the provider has one bounded
repair opportunity. If it remains, the report is incomplete and carries no score
or verdict.

Technical uses such as a confidence interval, or an evidence record's explicit
confidence value, are not presentation judgments and remain allowed.

## Adding a delivery signal later

Any future delivery signal requires an explicit opt-in separate from interview
recording consent, independent validation for reliability and subgroup effects,
clear `observed` versus `inferred` provenance, purpose-limited retention and
withdrawal, accessible non-participation, and a new prompt, rubric, and report
contract. It cannot silently reuse camera, microphone, avatar, or integrity
telemetry.
