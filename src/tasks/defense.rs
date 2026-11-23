//! The defense task (docs/mycpu-defense.md): questions about the student's
//! own ca2026-mycpu work, asked over their checkout, with every prediction,
//! edit and run recorded for the instructors to read before the live defense.

use base64::Engine;
use serde::{Deserialize, Serialize};

mod change;
mod checkout;
mod runner;
mod select;

pub use change::{Change, change_of};
pub use checkout::{Checkout, Restore, Stamp, Worktree, safe_name, valid_student_id};
pub use runner::{Ended, Ran, TAIL_BYTES, run};
pub use select::{Added, added_lines, choose, context, graded, worth_asking};

use super::{TaskError, bounded_text, invalid};

/// The one task id of a defense set.
pub const TASK_ID: &str = "defense";

/// The `defense` object of a defense set's package.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub course: String,
    pub salt: String,
    pub lines: usize,
    pub tasks: Vec<CourseTask>,
    pub bugs: Vec<Bug>,
    pub spec_seconds: u64,
    pub model_seconds: u64,
    pub sim_seconds: u64,
    /// The hints Jim may give in the whole attempt, 0 for a strict exam.
    #[serde(default)]
    pub hints: u64,
}

/// The most hints a defense may allow.
pub const MAX_HINTS: u64 = 3;

/// A task directory of the course, the sbt project that builds it and the
/// architecture its programs are built for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CourseTask {
    pub dir: String,
    pub project: String,
    pub arch: String,
}

/// A mutant of one of the course's tasks, and the rule it breaks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bug {
    pub task: String,
    pub variant: Option<String>,
    pub mutant: String,
    pub rule: String,
    pub exercise: Option<String>,
}

impl Config {
    /// The package's `defense` object, validated.
    pub fn parse(value: &serde_json::Value) -> Result<Self, TaskError> {
        // Every key of a bug is required, the null ones included, as the
        // packaging tool requires them: serde alone would let a missing
        // `Option` through.
        let mut bugs = value["bugs"].as_array().into_iter().flatten();
        if bugs.any(|bug| {
            ["variant", "exercise"]
                .iter()
                .any(|key| bug.get(key).is_none())
        }) {
            return Err(invalid("defense: every bug names its variant and exercise"));
        }
        let config: Self =
            serde_json::from_value(value.clone()).map_err(|e| invalid(format!("defense: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), TaskError> {
        if self.course != "ca2026-mycpu" {
            return Err(invalid("defense: course must be ca2026-mycpu"));
        }
        if !(16..=64).contains(&self.salt_bytes()?.len()) {
            return Err(invalid("defense: salt must be 16 to 64 bytes"));
        }
        if !(1..=5).contains(&self.lines) {
            return Err(invalid("defense: lines must be 1..5"));
        }
        if self.hints > MAX_HINTS {
            return Err(invalid("defense: hints must be 0..3"));
        }
        if !(1..=5).contains(&self.tasks.len()) || !(1..=64).contains(&self.bugs.len()) {
            return Err(invalid("defense: 1..5 tasks and 1..64 bugs"));
        }
        let seconds = [
            (self.spec_seconds, 60..=3600),
            (self.model_seconds, 60..=3600),
            (self.sim_seconds, 5..=600),
        ];
        if seconds.iter().any(|(value, range)| !range.contains(value)) {
            return Err(invalid("defense: a time limit is out of range"));
        }
        for task in &self.tasks {
            if !select::task_dir(&task.dir)
                || !name(&task.dir)
                || !name(&task.project)
                || !name(&task.arch)
            {
                return Err(invalid(format!("defense: task {} is not valid", task.dir)));
            }
        }
        for bug in &self.bugs {
            let known = self.tasks.iter().any(|t| t.dir == bug.task);
            let exercise = bug.exercise.as_deref().is_none_or(select::exercise_id);
            if !known
                || !name(&bug.mutant)
                || bug.variant.as_deref().is_some_and(|v| !name(v))
                || !bounded_text(&bug.rule, 256)
                || !exercise
            {
                return Err(invalid(format!("defense: bug {} is not valid", bug.mutant)));
            }
        }
        Ok(())
    }

    pub fn salt_bytes(&self) -> Result<Vec<u8>, TaskError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.salt)
            .map_err(|_| invalid("defense: salt is not standard base64"))
    }

    /// The course task a graded file belongs to.
    pub fn task_of(&self, file: &str) -> Option<&CourseTask> {
        let dir = file.split('/').next()?;
        self.tasks.iter().find(|t| t.dir == dir)
    }
}

/// `[A-Za-z0-9_.-]{1,64}`.
fn name(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// One line item: the line, where it is, and the change part b asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineItem {
    pub id: String,
    pub file: String,
    pub line: usize,
    pub quote: String,
    pub exercise: Option<String>,
    pub near: Vec<String>,
    /// The first line of `near`, one-based.
    pub near_first: usize,
    pub change: String,
    /// The line after the change; `None` when the change deletes it.
    pub changed_line: Option<String>,
}

/// What this student's defense asks: the lines, and the bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub stamp: Stamp,
    pub lines: Vec<LineItem>,
    pub bug: Bug,
    /// The bug's place in the package's list.
    pub bug_index: usize,
}

const AROUND: usize = 6;

/// The plan of the defense of the checkout's HEAD.
pub fn plan(config: &Config, checkout: &Checkout) -> Result<Plan, &'static str> {
    let stamp = checkout.stamp()?;
    let salt = config.salt_bytes().map_err(|_| "package_invalid")?;
    let candidates: Vec<Added> = checkout
        .candidates(&stamp)?
        .into_iter()
        // A line of a task the package names: its specs are what part b runs. A
        // quiz routine has none, so it is not asked about here.
        .filter(|c| config.task_of(&c.file).is_some())
        .collect();
    let chosen = choose(
        &salt,
        &stamp.student_id,
        &stamp.head,
        &candidates,
        config.lines,
    );
    if chosen.is_empty() {
        return Err("workspace_no_lines");
    }
    let lines = chosen
        .into_iter()
        .enumerate()
        .map(|(index, added)| {
            let source = checkout
                .source(&stamp.head, &added.file)
                .unwrap_or_default();
            let (near, exercise) = context(&source, added.line, AROUND);
            let change = change_of(&added.text);
            LineItem {
                id: format!("q{}", index + 1),
                near_first: added.line.saturating_sub(AROUND).max(1),
                quote: added.text.trim().to_owned(),
                changed_line: change.changed.map(|line| line.trim().to_owned()),
                change: change.what,
                file: added.file,
                line: added.line,
                exercise,
                near,
            }
        })
        .collect();
    let bug_index = drawn_bug(config.bugs.len())?;
    let bug = config.bugs[bug_index].clone();
    Ok(Plan {
        stamp,
        lines,
        bug,
        bug_index,
    })
}

/// The bug of this attempt, drawn uniformly from the package's list,
/// independently of every earlier draw (it can be the same bug), so an
/// earlier result says nothing about the next one. The result names the one
/// drawn.
fn drawn_bug(count: usize) -> Result<usize, &'static str> {
    let mut bytes = [0_u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| "setup_failed")?;
    let count = u64::try_from(count.max(1)).unwrap_or(u64::MAX);
    Ok(usize::try_from(u64::from_be_bytes(bytes) % count).unwrap_or(0))
}

/// Whether the student's edited line is the change asked for: equal once
/// leading and trailing white space is removed. A deletion is an empty line.
pub fn edit_matches(item: &LineItem, edited: &str) -> bool {
    match &item.changed_line {
        Some(changed) => edited.trim() == changed,
        None => edited.trim().is_empty(),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense_checkout.rs"]
mod checkout_tests;

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense_runner.rs"]
mod runner_tests;

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense_change.rs"]
mod change_tests;
