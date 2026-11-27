//! What the attempt's record answers: the questions the review's validation,
//! its prompt and the room's wrap-up ask of a session. They read two views
//! the session lends, so none of them reaches into its revisions, runs, turns
//! and checks, or into its task.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Check, CheckState, Outcome, Revision, RunMessage, Support, TaskSession, Turns};
use crate::tasks::defense::Slot;
use crate::tasks::exercise::Exercise;

/// The attempt's captured record, borrowed.
pub struct Evidence<'a> {
    checks: &'a BTreeMap<String, Check>,
    revisions: &'a BTreeMap<String, Revision>,
    runs: &'a BTreeMap<String, RunMessage>,
    turns: &'a Turns,
    final_revision_id: Option<&'a str>,
    outcome: Option<Outcome>,
    capture_overflow: bool,
}

/// The task the attempt pins, borrowed.
pub struct TaskFacts<'a> {
    exercise: &'a Exercise,
    defense: Option<&'a Slot>,
    language: &'a str,
}

impl TaskSession {
    pub fn evidence(&self) -> Evidence<'_> {
        Evidence {
            checks: &self.checks,
            revisions: &self.revisions,
            runs: &self.runs,
            turns: &self.turns,
            final_revision_id: self.final_revision_id.as_deref(),
            outcome: self.outcome,
            capture_overflow: self.capture_overflow,
        }
    }

    pub fn facts(&self) -> TaskFacts<'_> {
        TaskFacts {
            exercise: &self.task.exercise,
            defense: self.task.preparation.defense.as_ref(),
            language: self.task.language(),
        }
    }
}

impl<'a> Evidence<'a> {
    /// Whether a review's evidence reference names something this attempt
    /// recorded: a revision, a run on one, or a captured turn.
    pub fn resolves(&self, kind: Option<&str>, id: &str) -> bool {
        match kind {
            Some("revision") => self.revisions.contains_key(id),
            Some("run") => self
                .runs
                .get(id)
                .is_some_and(|run| self.revisions.contains_key(&run.revision_id)),
            Some("turn") => self.turns.contains_key(id),
            _ => false,
        }
    }

    /// What the learner said in the captured turn `id`.
    pub fn turn_text(&self, id: &str) -> Option<&'a str> {
        self.turns.get(id)
    }

    /// How many cases the run `id` ran, when there was such a run.
    pub fn run_total(&self, id: &str) -> Option<u32> {
        self.runs.get(id).map(|run| run.total)
    }

    /// Whether a check left unresolved kept one of `turn_ids`.
    pub fn unresolved_check_kept_any(&self, turn_ids: &[&str]) -> bool {
        self.checks.values().any(|check| {
            check.state == CheckState::Unresolved
                && turn_ids
                    .iter()
                    .any(|id| check.turn_ids.iter().any(|turn| turn == id))
        })
    }

    /// The support the captured turn `id` was recorded with, on every check
    /// that kept it.
    pub fn turn_supports(&self, id: &str) -> Vec<Support> {
        self.checks
            .values()
            .filter_map(|check| check.turn_support.get(id).copied())
            .collect()
    }

    /// The captured turns in the order they were spoken, with their ids.
    pub fn turn_entries(&self) -> Vec<(&'a str, &'a str)> {
        self.turns.iter().collect()
    }

    /// Whether the attempt ended on a revision.
    pub fn has_final_revision(&self) -> bool {
        self.final_revision_id.is_some()
    }

    /// How the attempt ended, once it has.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// How an attempt ended, as every result records it.
    pub fn ending(&self) -> Value {
        json!({"outcome": self.outcome, "captureOverflow": self.capture_overflow,
            "finalRevisionId": self.final_revision_id,
            "finalCode": self.final_revision_id
                .and_then(|id| self.revisions.get(id))
                .map(|revision| revision.code.as_str())})
    }
}

impl<'a> TaskFacts<'a> {
    /// The defense this attempt is, if it is one.
    pub fn defense(&self) -> Option<&'a Slot> {
        self.defense
    }

    /// The question a check asks the learner.
    pub fn check_question(&self, id: &str) -> Option<&'a str> {
        self.exercise.check_question(id)
    }

    /// The reference solution's text, for the review's overlap rule.
    pub fn review_reference(&self) -> String {
        self.exercise.review_reference()
    }

    /// The hints the task declares.
    pub fn hints(&self) -> &'a [String] {
        self.exercise.hints()
    }

    /// The task's requirements as the live interviewer reads them, in the
    /// attempt's language.
    pub fn live_projection(&self) -> Value {
        self.exercise.live_projection(self.language)
    }
}
