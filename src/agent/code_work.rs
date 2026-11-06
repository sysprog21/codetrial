//! What the candidate has written and run: whether the code is past the
//! starter, which test run covers which version of it, and how a test is
//! recorded.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// How much the candidate must have added to the template before the editor
/// counts as holding their code: a short expression, not a keystroke. Counted
/// in non-whitespace characters, in order; see `changed_characters`. The same
/// bound, added or removed, is how far the editor may move from a tested
/// snapshot before that run stops covering it, so tuning one tunes both.
pub(super) const MIN_WRITTEN_CHARS: usize = 5;

/// Cells in the table `changed_characters` fills, a few milliseconds of work.
pub(super) const MAX_WRITTEN_TABLE: usize = 4_000_000;

/// Whether the editor holds code the candidate wrote.
///
/// The interviewer's say-so is not enough for the phases that are about code.
/// A session once ticked Coding with nothing typed at all, because the model
/// recorded it from what the candidate said they would write.
pub fn code_written(state: &RuntimeState) -> bool {
    written_in(state, &state.language, &state.code)
}

/// Whether the editor holds only the starter, give or take layout, or nothing
/// at all, as when the candidate cleared it: the states in which they have
/// certainly written nothing, where `code_written` being false may also mean
/// too little to count or a change too long to compare.
pub fn code_untouched(state: &RuntimeState) -> bool {
    content_chars(&state.code).next().is_none()
        || content_chars(&state.code).eq(content_chars(starter(state, &state.language)))
}

/// The starter the page served for `language`, or nothing for one it did not.
pub(super) fn starter<'a>(state: &'a RuntimeState, language: &str) -> &'a str {
    state
        .code_templates
        .get(language)
        .map_or("", String::as_str)
}

/// Whether `code` holds code the candidate wrote over the starter of
/// `language`.
pub(crate) fn written_in(state: &RuntimeState, language: &str, code: &str) -> bool {
    let template = starter(state, language);

    // Too long to compare is not written: the shortcut below already credits
    // anything that grew by the threshold, which is all a length can prove.
    written_beyond(template, code).unwrap_or(false)
}

/// Whether `code` adds at least `MIN_WRITTEN_CHARS` of content to `before`, or
/// `None` when the two differ over a stretch too long to compare. Each caller
/// decides which way an unknown answer fails, because the two readings of
/// "written" fail in opposite directions.
fn written_beyond(before: &str, code: &str) -> Option<bool> {
    let before = content_chars(before).collect::<Vec<_>>();
    let code = content_chars(code).collect::<Vec<_>>();

    // A common subsequence is never longer than the starter, so code that
    // outgrows it by the threshold has written that much whatever it kept, and
    // a large paste needs no table.
    if code.len() >= before.len() + MIN_WRITTEN_CHARS {
        return Some(true);
    }
    changed_characters(&before, &code).map(|(added, _)| added >= MIN_WRITTEN_CHARS)
}

/// Whether the runner reported itself unavailable for the language on screen.
pub(super) fn runner_unavailable_on_screen(state: &RuntimeState) -> bool {
    state.runner_unavailable.as_deref() == Some(state.language.as_str())
}

/// What Test can be recorded from right now, decided once for the evidence
/// gate, the test reactions and the wrap-up refusal, so none of them can tell
/// the interviewer to record something another refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestSource {
    /// A run of the code on screen: recorded as `test_event`. It wins over an
    /// outage reported after it, because the run happened.
    Run,
    /// No such run, and the runner is reported missing for this language:
    /// the candidate's hand trace, recorded as `candidate_speech`.
    Trace,
    /// Neither: the candidate has to click Run.
    Neither,
    /// A whiteboard, where nothing runs at all: the cases the candidate names
    /// against the drawing are the testing, recorded as `candidate_speech` or
    /// `board_snapshot`. Not `Trace`, which is an outage in an interview that
    /// had a runner, and whose every sentence names one.
    Board,
}

pub(crate) fn test_source(state: &RuntimeState) -> TestSource {
    if state.interview_mode.is_whiteboard() {
        TestSource::Board
    } else if state.code_execution_disabled {
        TestSource::Trace
    } else if tested_code_is_current(state) {
        TestSource::Run
    } else if runner_unavailable_on_screen(state) {
        TestSource::Trace
    } else {
        TestSource::Neither
    }
}

/// Whether Test can be recorded now: it has no row yet, a received run still
/// covers the code on screen, and that code is the candidate's. The one
/// definition both the platform's own recording and the test reaction read.
///
/// Cheapest first, since a code update asks after every keystroke burst until
/// Test is recorded: the row check, then the run's language and code, and only
/// then whether the candidate wrote anything, which compares against the
/// starter.
pub(crate) fn test_recordable(state: &RuntimeState) -> bool {
    // `test_source` answers Board for a whiteboard before it reads any run, so
    // a page that sends editor packets there anyway records no execution: the
    // interview has no test runner, as the evidence gate also holds.
    !phases_evidenced(state, &[FrameworkPhase::Test])
        && test_source(state) == TestSource::Run
        && code_written(state)
}

/// What a test reaction should say about recording Test after a run, given
/// whether that run earned execution credit. RunAgain is only for a run that
/// earned credit the candidate has already typed past. A run that earned
/// nothing is answered by `uncredited_test_results_reaction` instead, so it is
/// Settled here whatever an earlier run left recordable.
pub(crate) fn test_record_after_run(state: &RuntimeState, credited: bool) -> TestRecord {
    if !credited {
        TestRecord::Settled
    } else if test_recordable(state) {
        TestRecord::Record
    } else if !phases_evidenced(state, &[FrameworkPhase::Test]) && code_written(state) {
        TestRecord::RunAgain
    } else {
        TestRecord::Settled
    }
}

/// Whether the editor still holds the code the latest executed run covered:
/// the same language, with less than a short expression added or removed
/// since. Fixing a failing case in place keeps the run; writing the solution
/// after running a stub does not, and neither does deleting a guard the run
/// exercised, which changes what runs without typing anything. A change too
/// long to compare is not current: a same-length rewrite of a long solution
/// would otherwise count as the code that ran.
pub(crate) fn tested_code_is_current(state: &RuntimeState) -> bool {
    state
        .tested_code
        .as_ref()
        .is_some_and(|tested| covers(tested, &state.language, &state.code))
}

/// Outcome claims require identical source and language. The Test gate keeps
/// credit through small edits, but even whitespace or comments can affect
/// literals, indentation, token boundaries or compiler directives. Comparing
/// bytes avoids claiming semantic equivalence without a language parser.
pub(crate) fn tested_code_is_exact(state: &RuntimeState) -> bool {
    state
        .tested_code
        .as_ref()
        .is_some_and(|tested| tested.is_current(state))
}

/// Displayed-report outcomes belong to exact submitted source, independently
/// of the older run whose Test credit can survive edits or later outages.
pub(crate) fn latest_test_report_is_exact(state: &RuntimeState) -> bool {
    state.last_test_run.is_some()
        && state
            .last_test_run_code
            .as_ref()
            .is_some_and(|tested| tested.is_current(state))
}

/// Trusted source provenance, outside candidate-controlled report text. No
/// notice is needed when the exact source is known or no editor outcome
/// applies.
pub(crate) fn latest_test_report_provenance_note(state: &RuntimeState) -> Option<&'static str> {
    if state.interview_mode.is_whiteboard()
        || state.code_execution_disabled
        || state.last_test_run.is_none()
        || latest_test_report_is_exact(state)
    {
        None
    } else if state.last_test_run_code.is_some() {
        Some(
            "The displayed browser report belongs to different source or language. Its outcome does not establish whether the current editor code passes or fails.",
        )
    } else {
        Some(
            "The displayed browser report has no recorded source provenance. Its outcome does not establish whether the current editor code passes or fails.",
        )
    }
}

/// What changed since the model last saw the editor, with the execution status
/// that keeps the update from reading as a new run. One wording for every
/// route that delivers an update, so the routes cannot drift apart.
pub(crate) fn editor_update_body(state: &RuntimeState) -> Option<String> {
    changed_excerpt(&state.language, &state.code_shown, &state.code)
        .map(|excerpt| format!("{excerpt}\n{}", editor_update_execution_status(state)))
}

/// Editor delivery carries no new execution and must not promote retained
/// Test credit into an outcome for code that has since changed.
pub(crate) fn editor_update_execution_status(state: &RuntimeState) -> &'static str {
    if state.code_execution_disabled {
        "Execution is disabled. Verify with hand traces, not a new test run."
    } else if latest_test_report_is_exact(state) {
        "The latest browser report belongs to this exact source and language; use only its recorded outcome. This update is not a new run."
    } else {
        "This update ran no tests. Do not assume earlier failures or passes describe this code."
    }
}

/// Whether `code` in `language` is still the code `tested` holds, by the Test
/// gate's rule: the same language, with less than a short expression added or
/// removed. The one definition every comparison of two runs, or of a run and
/// the editor, goes through.
pub(crate) fn covers(tested: &TestedCode, language: &str, code: &str) -> bool {
    tested.language == language
        && (tested.code == code || edited_within(language, &tested.code, code))
}

/// Whether fewer than `MIN_WRITTEN_CHARS` were added and fewer removed, from
/// one table: the common subsequence is the same either way round. Comments
/// are left out, so noting the complexity after a run, or deleting the
/// starter's prompt line, does not void a run of code that did not change.
pub(super) fn edited_within(language: &str, before: &str, code: &str) -> bool {
    let before = uncommented_chars(language, before);
    let code = uncommented_chars(language, code);
    changed_characters(&before, &code)
        .is_some_and(|(added, removed)| added < MIN_WRITTEN_CHARS && removed < MIN_WRITTEN_CHARS)
}

/// The content characters of `code` with its comments left out.
pub(super) fn uncommented_chars(language: &str, code: &str) -> Vec<char> {
    split_comments(language, code).0
}

/// `code` lexed into its content characters and its comments' text, one
/// comment to a line. Comments are `#` to the end of the line in Python, `//`
/// to the end of the line and `/* */` in the other tabs. A quote opens a string
/// until the same quote closes it, past a backslash escape, so a `#` or `//`
/// inside a literal is code; in Python a tripled quote opens a string only the
/// same three close, so a lone quote inside a docstring does not end it. An
/// unterminated string runs to the end and keeps everything after it counted,
/// which errs toward asking for a rerun; an unterminated block comment hides
/// the rest, and what it hid is counted as removed.
pub(super) fn split_comments(language: &str, code: &str) -> (Vec<char>, String) {
    let hash_comments = language == "python";
    let mut kept = Vec::with_capacity(code.len());
    let mut comments = String::new();
    let mut chars = code.chars().peekable();
    // The quote that opened the current string, and whether it was tripled.
    let mut quote: Option<(char, bool)> = None;
    let tripled = |chars: &std::iter::Peekable<std::str::Chars<'_>>, quote: char| {
        let mut ahead = chars.clone();
        ahead.next() == Some(quote) && ahead.next() == Some(quote)
    };
    while let Some(character) = chars.next() {
        if let Some((open, triple)) = quote {
            kept.push(character);
            if character == '\\' {
                kept.extend(chars.next());
            } else if character == open && (!triple || tripled(&chars, open)) {
                if triple {
                    kept.extend(chars.by_ref().take(2));
                }
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => {
                let triple = hash_comments && tripled(&chars, character);
                kept.push(character);
                if triple {
                    kept.extend(chars.by_ref().take(2));
                }
                quote = Some((character, triple));
            }
            '#' if hash_comments => {
                comments.extend(std::iter::from_fn(|| chars.next_if(|&next| next != '\n')));
                comments.push('\n');
            }
            '/' if !hash_comments && chars.peek() == Some(&'/') => {
                chars.next();
                comments.extend(std::iter::from_fn(|| chars.next_if(|&next| next != '\n')));
                comments.push('\n');
            }
            '/' if !hash_comments && chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.next_if_eq(&'/').is_some() {
                        break;
                    }
                    comments.push(next);
                }
                comments.push('\n');
            }
            _ => kept.push(character),
        }
    }
    kept.retain(|character| !character.is_whitespace());
    (kept, comments)
}

/// Changed by enough that an analysis of `before` may not describe `code`:
/// another language, more than 80 characters of code added and removed
/// together, comments aside, or a change too long to compare. Counting what
/// changed rather than the net size is what catches a new algorithm of about
/// the same length, which an unchanged size would pass as the same solution.
pub(crate) fn rewritten(before: &TestedCode, language: &str, code: &str) -> bool {
    if before.language != language {
        return true;
    }
    let old = uncommented_chars(language, &before.code);
    let new = uncommented_chars(language, code);
    changed_characters(&old, &new).is_none_or(|(added, removed)| added + removed > 80)
}

/// Records which code a complexity analysis just given describes. One given
/// over a run of the code on screen describes that code. One given ahead of
/// any such run may describe a draft, and finishing the same algorithm is not
/// a rewrite, so the first credited run after it is what it describes.
pub(super) fn anchor_analysis(state: &mut RuntimeState) {
    state.analysis = Some(if tested_code_is_current(state) {
        Analysis::Of(TestedCode {
            language: state.language.clone(),
            code: state.code.clone(),
        })
    } else {
        Analysis::AwaitingRun
    });
}

/// A test run that ran something: not a runner setup error, and not the 0/0
/// record a packet with no counts is sanitized into.
pub(crate) fn real_test_run(run: &serde_json::Value) -> bool {
    !evidence::run_failed_to_start(run)
        && run
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|total| total > 0)
}
