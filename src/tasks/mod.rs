//! Validated, session-owned programming exercises and their task-mode
//! contracts.

use std::sync::LazyLock;

use serde::Serialize;
use serde_json::Value;

pub mod access;
pub mod defense;
pub mod exercise;
pub mod package;
pub mod report;
pub mod session;
mod validation;

pub use exercise::{Exercise, TaskRecord};

/// Bytes of learner code one revision may carry.
/// `codeBytes` in task-defaults.json, as a constant the message bound below
/// it can be computed from; a test holds the two, and the page's copy, equal.
pub const MAX_CODE_BYTES: usize = 32000;

/// The server-owned task phase. Setup states before `Work` are the page's;
/// the interviewer's availability is an overlay, not a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Ready,
    Work,
    WrapUp,
    Feedback,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Work => "work",
            Self::WrapUp => "wrap-up",
            Self::Feedback => "feedback",
        }
    }

    /// Whether the learner can still change the work.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Work | Self::WrapUp)
    }
}

/// Every language a task may be worked in: the ones the catalog ships
/// starters for and the task page can run.
pub const LANGUAGES: [&str; 5] = ["python", "javascript", "c", "cpp", "java"];

/// What a task may ask the page to show beside its brief. Each is built into
/// the page; `scripts/problem_bank/tasks.py` keeps the same list.
pub const EXPLAINERS: [&str; 1] = ["cube"];

/// The languages the page compiles on Compiler Explorer rather than running
/// in the browser. A server with it turned off cannot run them.
pub const COMPILED_LANGUAGES: [&str; 3] = ["c", "cpp", "java"];

/// Whether this server's page can run `language`: C, C++ and Java compile on
/// Compiler Explorer, which a server may turn off.
pub fn runnable(language: &str, compiler_explorer: bool) -> bool {
    LANGUAGES.contains(&language) && (compiler_explorer || !COMPILED_LANGUAGES.contains(&language))
}

static DEFAULTS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../problem-bank/task-defaults.json"))
        .expect("checked task defaults")
});

pub fn default_number(key: &str) -> u64 {
    DEFAULTS[key].as_u64().expect("numeric task default")
}

pub fn default_value(key: &str) -> Value {
    DEFAULTS[key].clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError(pub String);

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TaskError {}

pub(crate) fn invalid(message: impl Into<String>) -> TaskError {
    TaskError(message.into())
}

pub(crate) fn identifier(value: &str) -> bool {
    record_id(value) && value.as_bytes()[0].is_ascii_lowercase()
}

/// A task id: a problem-bank record id, which unlike `identifier` may start
/// with a digit (`3sum`) but not with a dash.
pub(crate) fn record_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn bounded_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}

pub(crate) fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
#[path = "../../tests/unit/tasks/support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "../../tests/unit/tasks/rubik.rs"]
mod rubik;
