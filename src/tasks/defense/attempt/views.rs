//! What an attempt shows of itself: the facts a result keeps and the view the
//! panel draws.

use serde_json::{Value, json};

use super::Attempt;
use crate::tasks::defense::examiner_brief;
use crate::tasks::defense::hints::Hints;

impl Attempt {
    /// What the attempt recorded, item by item, without marks.
    pub(in crate::tasks::defense) fn facts(&self) -> Value {
        let plan = &self.plan;
        let items: Vec<Value> = plan
            .lines
            .iter()
            .map(|item| {
                let runs: Vec<Value> = self
                    .runs
                    .iter()
                    .filter(|run| run.item == item.id)
                    .map(|run| {
                        // The test output is the page's; the result has no room
                        // for it.
                        let mut run = json!(run);
                        if let Some(object) = run.as_object_mut() {
                            object.remove("summary");
                        }
                        run
                    })
                    .collect();
                // A prediction no run consumed is still the learner's record.
                json!({"id": item.id, "file": item.file, "line": item.line, "quote": item.quote,
                    "exercise": item.exercise, "change": item.change, "changedLine": item.changed_line,
                    "runs": runs, "pendingPrediction": self.prediction(&item.id),
                    "begunAt": self.begun_at(&item.id)})
            })
            .collect();
        let brief = self.bug_brief();

        // The result the student keeps names the attempt's bug only by its
        // place in the package's list, which the instructors resolve.
        json!({
            "stamp": plan.stamp,
            "items": items,
            "bug": {"id": "bug", "task": plan.bug.task, "variant": plan.bug.variant,
                "bugIndex": plan.bug_index, "begunAt": self.begun_at("bug"), "modelsBuilt": brief.built, "buildError": brief.build_error,
                "simulations": self.sims.len(),
                "claim": self.claim.as_ref().map(|c| &c.stated),
                // What the marks read; `marked` takes them out of the result.
                "checks": self.claim.as_ref().map(|c| json!({"cycleClose": c.cycle_close,
                    "exerciseMatches": c.exercise_matches}))},
            "scored": !plan.stamp.auditor,
        })
    }

    /// What the panel is shown: the items with the specs each may run, the
    /// predictions waiting for a run, every run, the bug item without its
    /// mutant
    /// or rule, the simulations, and the claim as stated. Never the claim's
    /// checks: the first divergence and whether the cycle or the exercise is
    /// right would tell the learner the answer.
    pub(in crate::tasks::defense) fn panel_view(&self, hints: &Hints, items_open: bool) -> Value {
        let plan = &self.plan;
        let lines: Vec<Value> = plan
            .lines
            .iter()
            .map(|item| {
                json!({"id": item.id, "file": item.file, "line": item.line, "quote": item.quote,
                    "exercise": item.exercise, "near": item.near, "nearFirst": item.near_first,
                    "change": item.change, "deletes": item.changed_line.is_none(),
                    "specs": self.specs_for(&item.id).unwrap_or_default(),
                    "prediction": self.prediction(&item.id),
                    "begun": self.begun_at(&item.id).is_some()})
            })
            .collect();

        // The last simulation is the one the panel draws; the others are
        // counted.
        json!({"stamp": {"studentId": plan.stamp.student_id, "head": plan.stamp.head,
                "auditor": plan.stamp.auditor},
            "lines": lines, "runs": self.runs, "bug": self.bug_brief(),
            "bugBegun": self.begun_at("bug").is_some(),
            "itemsOpen": items_open,
            "sims": self.sims.last().into_iter().collect::<Vec<_>>(),
            "simulations": self.sims.len(),
            "claim": self.claim.as_ref().map(|c| &c.stated),
            "hints": {"allowed": hints.allowed, "given": hints.given}})
    }

    /// The items as Jim is given them; see `examiner_brief`.
    pub fn examiner_brief(&self) -> String {
        examiner_brief(&self.plan)
    }

    /// The ids of the line items, in the order they were drawn.
    pub fn item_ids(&self) -> Vec<String> {
        self.plan.lines.iter().map(|l| l.id.clone()).collect()
    }
}
