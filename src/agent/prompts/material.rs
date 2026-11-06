//! How candidate material reaches the model: numbering, excerpts, test-run
//! rendering, untrusted fences and the read-tool and hint texts.

use crate::agent::{
    MAX_CANDIDATE_CASES, MAX_TEST_FAILURES, python_truthy, truthy_string, value_string,
};

pub(super) fn numbered_list(items: &[&str]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| format!("  {}. {item}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) const NOTHING_RECORDED: &str = "(nothing recorded yet)";
pub(super) const EMPTY_EDITOR: &str = "(the editor was left empty)";
pub(super) const NO_SPEECH: &str = "(no speech was captured)";

/// Untrusted text with any line that could pass for a fence marker quoted, so
/// a comment reading `END UNTRUSTED EDITOR` cannot close its block early and
/// put what follows it outside the fence. The numbered views get this from
/// their line numbers; this is for the paths that quote text as written.
/// Case and leading whitespace are ignored, since neither stops a model from
/// reading the line as the marker.
pub(super) fn quote_fence_markers(text: &str) -> std::borrow::Cow<'_, str> {
    fn marker(line: &str) -> bool {
        let line = line.trim_start().as_bytes();
        ["BEGIN UNTRUSTED", "END UNTRUSTED"].iter().any(|marker| {
            line.get(..marker.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(marker.as_bytes()))
        })
    }

    // A line is whatever a model reads as one, so a carriage return or a
    // Unicode separator starts one as surely as a line feed does.
    let breaks = crate::agent::value::breaks_a_prompt_line;
    if !text.split(breaks).any(marker) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut quoted = String::with_capacity(text.len() + 16);
    for line in text.split_inclusive(breaks) {
        if marker(line) {
            quoted.push_str(QUOTED_MARKER_PREFIX);
        }
        quoted.push_str(line);
    }
    std::borrow::Cow::Owned(quoted)
}

/// What `quote_fence_markers` puts in front of a line that reads as a marker.
pub(super) const QUOTED_MARKER_PREFIX: &str = "(quoted) ";

/// What the transcript blocks a side call or the report reads say about who
/// is speaking.
pub(super) const TRANSCRIPT_SPEAKERS: &str = " (Interviewer = the AI, Candidate = the human)";

/// An untrusted block: `body` between `BEGIN UNTRUSTED {label}{detail}` and
/// `END UNTRUSTED {label}`, with any line of it that reads as a marker quoted.
/// Every fence is built here, so whether one can be closed early does not rest
/// on its caller having numbered, flattened or quoted the text first.
/// `detail` is what the opening line adds, such as the language, and is empty
/// or starts with a space.
pub(crate) fn untrusted_block(label: &str, detail: &str, body: &str) -> String {
    let body = quote_fence_markers(body);
    format!("BEGIN UNTRUSTED {label}{detail}\n{body}\nEND UNTRUSTED {label}")
}

/// The editor and the transcript, fenced as conversation data, the way both
/// side calls read them. A whiteboard stretch gets the caller's `no_editor`
/// note where the editor would be, since an empty editor block reads as no
/// code written.
pub(super) fn untrusted_blocks(
    no_editor: Option<&str>,
    language: &str,
    code: &str,
    transcript: &str,
) -> String {
    let work = if let Some(note) = no_editor {
        note.to_string()
    } else {
        let code = if code.is_empty() { EMPTY_EDITOR } else { code };
        untrusted_block("EDITOR", &format!(" ({language})"), code)
    };
    let transcript = if transcript.is_empty() {
        NO_SPEECH
    } else {
        transcript
    };
    format!(
        "{work}\n{}",
        untrusted_block("TRANSCRIPT", TRANSCRIPT_SPEAKERS, transcript)
    )
}

/// The lines worth numbering: the buffer without the blank lines an editor
/// leaves at its end, which were numbered and paid for on every snapshot.
fn content_lines(code: &str) -> Vec<&str> {
    let mut lines = code.lines().collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// One numbered line, `12| code`. Unpadded: right-aligning the numbers cost a
/// space or two on every line of every snapshot and told the model nothing.
pub(super) fn numbered_line(number: usize, line: &str) -> String {
    format!("{number}| {line}")
}

/// The most `numbered` writes. The editor arrives unbounded, and what this
/// returns stays in the Live session for the rest of the interview, so a paste
/// of a large file is held to about eight thousand tokens rather than all of
/// it. Far above any solution to an interview problem, and far above the watch
/// excerpt, whose promise is that `read_editor` shows what it leaves out.
pub const MAX_NUMBERED_BYTES: usize = 32_000;
/// Characters kept of one line in `numbered`, for the same reason
/// `MAX_EXCERPT_LINE_CHARS` exists, and well above it for the same promise.
const MAX_NUMBERED_LINE_CHARS: usize = 1_000;

pub fn numbered(code: &str) -> String {
    numbered_from(code, 1)
}

/// `numbered` from line `from` on, which is how `read_editor` pages through a
/// buffer past `MAX_NUMBERED_BYTES`: the note that ends a cut page names the
/// line to ask for next, so nothing the excerpts leave out is out of reach.
pub fn numbered_from(code: &str, from: usize) -> String {
    numbered_from_with_limit(code, from, MAX_NUMBERED_BYTES)
}

pub(crate) fn numbered_from_with_limit(code: &str, from: usize, max_bytes: usize) -> String {
    let lines = content_lines(code);
    if lines.is_empty() {
        return "(the editor is currently empty)".to_string();
    }
    let from = from.max(1);
    if from > lines.len() {
        return format!("(the editor has {} lines)", lines.len());
    }
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate().skip(from - 1) {
        let rendered = numbered_bounded_line(index + 1, line);

        // `MAX_NUMBERED_BYTES` bounds the numbered lines. The pointer at
        // `read_editor` below is added once the loop stops, so a view that had
        // to stop is that string longer: the cap is on what is quoted, as in
        // `code_head`.
        if !out.is_empty() && out.len() + rendered.len() + 1 > max_bytes {
            out.push_str(&format!(
                "\n... {} more lines; call `read_editor` with fromLine {} for them",
                lines.len() - index,
                index + 1
            ));
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&rendered);
    }
    out
}

pub(crate) fn numbered_bounded_line(number: usize, line: &str) -> String {
    let kept = line
        .chars()
        .take(MAX_NUMBERED_LINE_CHARS)
        .collect::<String>();
    let cut = if kept.len() < line.len() { " ..." } else { "" };
    numbered_line(number, &format!("{kept}{cut}"))
}

/// Lines of context kept either side of a changed region.
const EXCERPT_CONTEXT_LINES: usize = 3;
/// The most lines a changed-region excerpt shows. A paste over the template
/// changes every line, and the model reads the rest through `read_editor` when
/// it needs it.
const MAX_EXCERPT_LINES: usize = 40;
/// Characters kept of one line, so one enormous line cannot stand in for the
/// whole budget the line cap exists to hold.
pub const MAX_EXCERPT_LINE_CHARS: usize = 160;
/// A buffer this short is shown whole rather than as its changed lines.
const MAX_WHOLE_BUFFER_LINES: usize = 80;
const MAX_WHOLE_BUFFER_BYTES: usize = 4_000;

/// The code a watch prompt shows, numbered as `read_editor` numbers it and
/// fenced as the candidate's untrusted text: the whole buffer while it is
/// short,
/// and past that the lines that changed since `previous` with a few lines of
/// context. `None` when no line changed.
///
/// A watch prompt used to name the edit and nothing else, and told the model to
/// call `read_editor` before saying anything about it: a tool round trip on
/// every review, carrying the whole buffer anyway. Showing only the changed
/// lines was tried first and did not remove the trip: against the Live model
/// the interviewer still read the editor on every sampled review before judging
/// a change it could see only part of, about 950 ms to first audio against 565
/// ms when the whole buffer came with the prompt, and the read carried the
/// whole buffer in any case. So a short buffer goes whole, which costs the
/// tokens the read would have and saves the trip; the changed region is kept
/// for a buffer too long to send, found by the lines both buffers share at the
/// start and at the end, and cut at the line cap.
pub fn changed_excerpt(language: &str, previous: &str, current: &str) -> Option<String> {
    let before = previous.lines().collect::<Vec<_>>();
    let after = current.lines().collect::<Vec<_>>();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let changed_end = after.len() - suffix;
    if prefix == changed_end && before.len() == after.len() {
        return None;
    }

    // Measured and shown without the blank lines at the end, which a change can
    // still be among: removing them is a change, and the excerpt of it is the
    // lines above.
    let total = content_lines(current).len();
    let whole = total <= MAX_WHOLE_BUFFER_LINES && current.len() <= MAX_WHOLE_BUFFER_BYTES;

    // A deletion leaves no changed line in the new buffer, so the context
    // either side of where it was is all there is to show.
    let (first, last) = if whole {
        (0, total)
    } else {
        let last = (changed_end + EXCERPT_CONTEXT_LINES).min(total);
        (prefix.saturating_sub(EXCERPT_CONTEXT_LINES).min(last), last)
    };
    let cap = if whole { total } else { MAX_EXCERPT_LINES };
    let shown = (last - first).min(cap);
    let mut body = after[first..first + shown]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            // Cut only in an excerpt. A whole buffer is under the byte cap
            // already, and a line cut there made "all N lines" untrue.
            if whole {
                return numbered_line(first + offset + 1, line);
            }
            let kept = line
                .chars()
                .take(MAX_EXCERPT_LINE_CHARS)
                .collect::<String>();
            let cut = if kept.len() < line.len() { " ..." } else { "" };
            numbered_line(first + offset + 1, &format!("{kept}{cut}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if shown < last - first {
        body.push_str(&format!(
            "\n... {} more lines; call `read_editor` for them",
            last - first - shown
        ));
    }
    if body.is_empty() {
        body.push_str("(the editor is currently empty)");
    }

    // The range shown, not the range the change spans: a region past the line
    // cap used to be labelled with lines the excerpt never reached.
    //
    // Nothing shown takes the whole-buffer wording too. A buffer over the byte
    // cap whose lines are all blank measures zero content lines, and the
    // numbered form then reads "lines 1-0 of 0": a range that runs backwards,
    // over the body's own "the editor is currently empty".
    let scope = if whole || shown == 0 {
        format!("all {total} lines")
    } else {
        format!(
            "lines {}-{} of {total}, around the change since the last review",
            first + 1,
            first + shown,
        )
    };
    Some(untrusted_block(
        "EDITOR",
        &format!(" ({language}, {scope})"),
        &body,
    ))
}

pub fn format_test_run(run: Option<&serde_json::Value>, total_runs: u32) -> String {
    render_test_run(run, total_runs, MAX_TEST_FAILURES)
}

/// The run as a live reaction reads it: one failing case and a count of the
/// rest. The reaction asks the candidate to pick one failure and reason about
/// it, so the others were only material for the interviewer to say too much
/// with; `read_editor` and the report still read every one.
pub fn format_test_run_for_reaction(run: &serde_json::Value, total_runs: u32) -> String {
    render_test_run(Some(run), total_runs, 1)
}

fn render_test_run(
    run: Option<&serde_json::Value>,
    total_runs: u32,
    max_failures: usize,
) -> String {
    let Some(run) = run else {
        return "No test run was recorded; tests may not have been attempted or may not have been available for the selected language/problem yet.".to_string();
    };
    if !python_truthy(run) {
        return "No test run was recorded; tests may not have been attempted or may not have been available for the selected language/problem yet.".to_string();
    }
    if let Some(setup_error) = truthy_string(run.get("setupError")) {
        return format!(
            "Latest test run (run #{total_runs}) could not execute the code: {setup_error}"
        );
    }

    let language = value_string(run.get("language")).unwrap_or_else(|| "?".to_string());
    let passed = run
        .get("passed")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let total = run
        .get("total")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let mut lines = vec![format!(
        "Latest test run (run #{total_runs}, {language}): {passed}/{total} cases passed."
    )];

    if let Some(failures) = run.get("failures").and_then(serde_json::Value::as_array) {
        let failures = failures
            .iter()
            .filter_map(serde_json::Value::as_object)
            .take(MAX_TEST_FAILURES)
            .collect::<Vec<_>>();

        // Counted from the reported totals where they say more failed than the
        // browser listed: it sends four at most, and the rest are still
        // failures the interviewer should know exist.
        let failing = usize::try_from(total.saturating_sub(passed))
            .unwrap_or(0)
            .max(failures.len());
        let listed = failures.len().min(max_failures);
        let unlisted = failing - listed;
        for failure in failures.into_iter().take(max_failures) {
            let label = value_string(failure.get("label")).unwrap_or_else(|| "?".to_string());
            let input = optional_input(failure);
            if let Some(error) = truthy_string(failure.get("error")) {
                lines.push(format!("- FAILED {label}{input}: raised {error}"));
            } else {
                let expected =
                    value_string(failure.get("expected")).unwrap_or_else(|| "None".to_string());
                let got = value_string(failure.get("got")).unwrap_or_else(|| "None".to_string());
                lines.push(format!(
                    "- FAILED {label}{input}: expected {expected}, got {got}"
                ));
            }
        }
        if unlisted > 0 {
            lines.push(format!(
                "- {unlisted} {}failing {} not listed",
                if listed > 0 { "more " } else { "" },
                if unlisted == 1 { "case" } else { "cases" }
            ));
        }
    }
    if let Some(cases) = run
        .get("candidateCases")
        .and_then(serde_json::Value::as_array)
    {
        for case in cases
            .iter()
            .filter_map(serde_json::Value::as_object)
            .take(MAX_CANDIDATE_CASES)
        {
            let label = value_string(case.get("label")).unwrap_or_else(|| "?".to_string());
            let input = optional_input(case);
            if let Some(error) = truthy_string(case.get("error")) {
                lines.push(format!("- CANDIDATE CASE {label}{input}: raised {error}"));
            } else {
                let got = value_string(case.get("got")).unwrap_or_else(|| "None".to_string());

                // The candidate's own expectation, where they wrote one:
                // without it a case that contradicts them reads the same as one
                // that only printed output.
                let expected = value_string(case.get("expected").filter(|value| !value.is_null()))
                    .map(|expected| format!(", candidate expected {expected}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "- CANDIDATE CASE {label}{input}: got {got}{expected}"
                ));
            }
        }
    }

    lines.join("\n")
}

fn optional_input(case: &serde_json::Map<String, serde_json::Value>) -> String {
    // The sanitizer writes the key on every case, so a run recorded before the
    // browser sent one carries a null here. Omit that rather than claiming the
    // case ran "with input None".
    value_string(case.get("input").filter(|value| !value.is_null()))
        .map(|input| format!(" with input {input}"))
        .unwrap_or_default()
}

/// What `read_editor` answers, and what a requested hint carries after its
/// clue: the editor and the latest run fenced as the candidate's text, and the
/// platform's timer outside both, last. The reading the instructions tell the
/// model to trust is the last sentence; fenced, the answer ended on the fence
/// marker instead, and unfenced the candidate's code sat in a tool answer the
/// model otherwise takes as the platform's word.
pub fn read_editor_text(
    language: &str,
    code: &str,
    from_line: usize,
    last_test_run: Option<&serde_json::Value>,
    test_runs: u32,
    minutes_left: i64,
) -> String {
    read_editor_text_from_numbered(
        language,
        &numbered_from(code, from_line),
        last_test_run,
        test_runs,
        minutes_left,
    )
}

pub(crate) fn read_editor_text_from_numbered(
    language: &str,
    numbered_code: &str,
    last_test_run: Option<&serde_json::Value>,
    test_runs: u32,
    minutes_left: i64,
) -> String {
    format!(
        "{}\n{}\n{}",
        untrusted_block("EDITOR", &format!(" ({language})"), numbered_code),
        untrusted_block("TEST RUN", "", &format_test_run(last_test_run, test_runs)),
        crate::agent::timer_line(minutes_left)
    )
}

/// What `read_board` answers with.
///
/// The image itself cannot travel this way: a tool response is JSON, so the
/// room loop re-sends the image on the mode's image input path.
/// The counts are here because they are the part of a board a model cannot
/// read off the picture: how much of it is new since the last one it was sent,
/// and how long ago the candidate drew it.
pub fn read_board_text(
    strokes: u32,
    snapshots: u32,
    drawn_seconds_ago: Option<u64>,
    minutes_left: i64,
) -> String {
    let board = if snapshots == 0 {
        "The candidate has not drawn anything yet, so there is no board to look at.".to_string()
    } else {
        let when = match drawn_seconds_ago {
            Some(seconds) => format!("about {seconds} seconds ago"),
            None => "a moment ago".to_string(),
        };
        format!(
            "The candidate's board has been put in front of you again as an image: {strokes} strokes, as the candidate last left it {when}. Look at that image rather than at what you remember of the board."
        )
    };
    format!("{board}\n\n{}", crate::agent::timer_line(minutes_left))
}

pub fn log_hint_text(hints_used: u32) -> String {
    format!("Recorded. Total hints so far: {hints_used}.")
}

pub fn hint_rung_text(hints_used: u32, rung: usize, clue: &str) -> String {
    format!(
        "{} Hint rung {rung}, the only clue to give now: {clue} Say it as one question or nudge in your own words, fitted to their current code, and stop for their response. Name no technique, data structure, or step this clue does not already name.",
        log_hint_text(hints_used)
    )
}

/// No clue at all while the key step is held. Pointing the model back at the
/// earlier rung "from a different angle" was an invitation to improvise, and a
/// live run answered it with the key step: "if we sort the adjustments...".
pub fn hint_rung_withheld_text(hints_used: u32) -> String {
    format!(
        "Not counted as a hint; total hints so far: {hints_used}. The next rung names the key step and stays withheld until the candidate has put an approach of their own into words or code. Give no clue this turn: in one short sentence, ask what they would try first, even a slow version, and wait. Do not restate an earlier clue, and name no technique, data structure, ordering, or step."
    )
}

pub fn hint_ladder_used_text(hints_used: u32) -> String {
    format!(
        "{} Every rung is used. Keep helping them reason from their own code without revealing another step.",
        log_hint_text(hints_used)
    )
}
