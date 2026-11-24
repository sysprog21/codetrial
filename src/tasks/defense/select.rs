//! Which lines a defense asks about: the selection of the course's
//! `scripts/review.py` and `scripts/defense.py`, keyed with the package's
//! salt.

use std::collections::BTreeSet;

use ring::hmac;

/// A line the student's commits add to a graded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub file: String,
    /// One-based, in the file as HEAD has it.
    pub line: usize,
    pub text: String,
}

/// Whether the course grades `path`: a task's Scala sources, or a quiz
/// routine.
pub fn graded(path: &str) -> bool {
    let scala = path.split_once('/').is_some_and(|(task, rest)| {
        task_dir(task)
            && rest
                .strip_prefix("src/main/scala/")
                .is_some_and(|file| file.len() > 6 && file.ends_with(".scala"))
    });
    let quiz = path
        .strip_prefix("software/programs/quiz-")
        .and_then(|rest| rest.strip_suffix("/routines.S"))
        .is_some_and(|name| !name.is_empty() && !name.contains('/'));
    scala || quiz
}

/// `<1..5>-<name>`, a task directory.
pub(super) fn task_dir(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 2 && (b'1'..=b'5').contains(&bytes[0]) && bytes[1] == b'-' && !name.contains('/')
}

/// `<task>.<n>`: an exercise as the course numbers it.
pub(super) fn exercise_id(text: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    text.split_once('.')
        .is_some_and(|(task, item)| digits(task) && digits(item))
}

/// The longest line an item asks about.
pub(super) const MAX_LINE_BYTES: usize = 300;

/// The lines `git diff -U0` adds, by file and new line number.
pub fn added_lines(diff: &str) -> Vec<Added> {
    let mut out = Vec::new();
    let mut file: Option<&str> = None;
    let mut line = 0;

    // Between `diff --git` and the first hunk: an added line that itself starts
    // with `++ ` reads as `+++ ` too, and is no file header.
    let mut header = false;
    for text in diff.split('\n') {
        if text.starts_with("diff --git ") {
            header = true;
            file = None;
        } else if let Some(path) = text.strip_prefix("+++ ").filter(|_| header) {
            file = path.strip_prefix("b/");
        } else if let Some(hunk) = text.strip_prefix("@@") {
            header = false;
            line = hunk
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok())
                .unwrap_or(0);
        } else if let (Some(added), Some(path)) = (text.strip_prefix('+'), file) {
            out.push(Added {
                file: path.to_owned(),
                line,
                text: added.to_owned(),
            });
            line += 1;
        }
    }
    out
}

/// Whether a line says enough to ask about: twelve characters that are not
/// white space, not a comment, and more than brackets; and no more than
/// `MAX_LINE_BYTES`, so that the items stay within the result's bound.
pub fn worth_asking(text: &str) -> bool {
    let trimmed = text.trim();
    let solid = trimmed.chars().filter(|c| !c.is_whitespace()).count();
    if solid < 12
        || trimmed.len() > MAX_LINE_BYTES
        || ["//", "#", "/*", "*"]
            .iter()
            .any(|p| trimmed.starts_with(p))
    {
        return false;
    }
    let identifier = trimmed.chars().any(|c| c.is_ascii_alphabetic() || c == '_');
    let brackets = trimmed.chars().all(|c| "{}()[];,".contains(c));
    identifier && !brackets
}

/// `count` candidates in the order of their key, a different file each time
/// while some file has not been taken yet.
pub fn choose(
    salt: &[u8],
    student: &str,
    head: &str,
    candidates: &[Added],
    count: usize,
) -> Vec<Added> {
    let ordered = keyed_order(salt, student, head, candidates);
    let files: BTreeSet<&str> = candidates.iter().map(|c| c.file.as_str()).collect();
    let mut used = BTreeSet::new();
    let mut chosen: Vec<Added> = Vec::new();
    for candidate in &ordered {
        if chosen.len() == count {
            break;
        }
        if !used.contains(candidate.file.as_str()) || files.len() <= used.len() {
            used.insert(candidate.file.as_str());
            chosen.push(candidate.clone());
        }
    }

    // The course's rule passes over a second line of a file while other files
    // remain, and can end short of `count` with candidates left: those fill the
    // rest, in their order. Where it does not end short, it is unchanged.
    for candidate in ordered {
        if chosen.len() == count {
            break;
        }
        if !chosen.contains(&candidate) {
            chosen.push(candidate);
        }
    }
    chosen
}

/// The candidates in the order of their key: HMAC-SHA256 with the salt,
/// compared as bytes, which orders them as their hex does.
fn keyed_order(salt: &[u8], student: &str, head: &str, candidates: &[Added]) -> Vec<Added> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, salt);
    let mut keyed: Vec<(hmac::Tag, &Added)> = candidates
        .iter()
        .map(|c| {
            let message = format!("defense\n{student}\n{head}\n{}:{}", c.file, c.line);
            (hmac::sign(&key, message.as_bytes()), c)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
    keyed.into_iter().map(|(_, c)| c.clone()).collect()
}

/// The lines around `line` (one-based) and the exercise it belongs to: the
/// last `[CA26: Exercise <n>]` on or above it.
pub fn context(source: &str, line: usize, around: usize) -> (Vec<String>, Option<String>) {
    let lines: Vec<&str> = source.split('\n').collect();
    let low = line.saturating_sub(1).saturating_sub(around);
    let high = lines.len().min(line + around);
    let near = lines
        .get(low..high)
        .unwrap_or_default()
        .iter()
        .map(|l| (*l).to_owned())
        .collect();
    let exercise = lines
        .iter()
        .take(line)
        .filter_map(|l| exercise_of(l))
        .next_back();
    (near, exercise)
}

fn exercise_of(line: &str) -> Option<String> {
    let rest = &line[line.find("[CA26: Exercise ")? + 16..];
    let number = &rest[..rest.find(']')?];
    exercise_id(number).then(|| number.to_owned())
}
