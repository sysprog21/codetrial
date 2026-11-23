//! The `defense` section of a result: every item with its evidence and a
//! suggested mark on the course's scale (0 no answer, 1 recalls the code,
//! 2 explains it, 3 predicts and confirms with a run). The instructors set
//! the marks; these are what the evidence supports.
//!
//! In two steps: `facts` reads the attempt, and `marked` adds the marks from
//! those facts and the review's `explanations`, so the facts kept after each
//! request can stand in for an attempt a run still holds when it ends.

use serde_json::{Value, json};

use super::attempt::Attempt;

/// What the attempt recorded, item by item, without marks.
pub fn facts(attempt: &Attempt) -> Value {
    let plan = attempt.plan();
    let items: Vec<Value> = plan
        .lines
        .iter()
        .map(|item| {
            let runs: Vec<Value> = attempt
                .runs()
                .iter()
                .filter(|run| run.item == item.id)
                .map(|run| {
                    // The test output is the page's; the result has no room for
                    // it.
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
                "runs": runs, "pendingPrediction": attempt.prediction(&item.id),
                "begunAt": attempt.begun_at(&item.id)})
        })
        .collect();
    let brief = attempt.bug_brief();

    // The result the student keeps names the attempt's bug only by its place in
    // the package's list, which the instructors resolve.
    json!({
        "stamp": plan.stamp,
        "items": items,
        "bug": {"id": "bug", "task": plan.bug.task, "variant": plan.bug.variant,
            "bugIndex": plan.bug_index, "begunAt": attempt.begun_at("bug"), "modelsBuilt": brief.built, "buildError": brief.build_error,
            "simulations": attempt.sims().len(),
            "claim": attempt.claim().map(|c| &c.stated),
            // What the marks read; `marked` takes them out of the result.
            "checks": attempt.claim().map(|c| json!({"cycleClose": c.cycle_close,
                "exerciseMatches": c.exercise_matches}))},
        "scored": !plan.stamp.auditor,
    })
}

/// Whether any run in `facts` said something about its change.
pub fn ran(facts: &Value) -> bool {
    facts["items"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item["runs"].as_array().into_iter().flatten())
        .any(|run| !run["matched"].is_null())
}

/// Each line item's runs as the review is told them: what was predicted,
/// what the run showed, and whether they agreed.
pub fn run_summary(facts: &Value) -> Value {
    let items = facts["items"].as_array().into_iter().flatten();
    Value::Array(
        items
            .map(|item| {
                let runs: Vec<Value> = item["runs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|run| {
                        // The outcome's kind and failing specs: a gap's reason
                        // can be the student's own git or build output.
                        let mut outcome = json!({"kind": run["outcome"]["kind"]});
                        if !run["outcome"]["failed"].is_null() {
                            outcome["failed"] = run["outcome"]["failed"].clone();
                        }
                        json!({"predicted": run["prediction"]["fails"],
                            "predictedCompiles": run["prediction"]["compiles"],
                            "outcome": outcome, "matched": run["matched"]})
                    })
                    .collect();
                json!({"item": item["id"], "runs": runs})
            })
            .collect(),
    )
}

/// The row of the review's `explanations` for `item`.
fn explanation<'a>(explanations: &'a Value, item: &str) -> Option<&'a Value> {
    explanations
        .as_array()?
        .iter()
        .find(|row| row["item"] == item)
}

/// Part b: 3 when the first run that said something about the change
/// confirmed, conclusively, the prediction it started from: a later run, once
/// the learner has seen what fails, confirms nothing. Otherwise the review's
/// mark of the learner's account of the run, at most 2, or 1 without one;
/// null when the review itself was unavailable, which is never the learner's.
/// Null with a reason when no run said anything.
pub(super) fn change_mark(runs: &[Value], account: &Value, reviewed: bool) -> (Value, Value) {
    let Some(first) = runs.iter().find(|run| !run["matched"].is_null()) else {
        let reason = if runs.is_empty() {
            "missing_run"
        } else {
            "telemetry_gap"
        };
        return (Value::Null, json!(reason));
    };
    // Conclusive is a match that shows something.
    if first["conclusive"] == true {
        return (json!(3), Value::Null);
    }
    if !reviewed {
        return (Value::Null, json!("feedback_unavailable"));
    }
    (
        json!(account.as_u64().map_or(1, |rating| rating.min(2))),
        Value::Null,
    )
}

/// The bug: 1 for a claim, plus one each for a cycle within the slack of the
/// first divergence and for the bug's exercise; 0 for no claim; null when the
/// models failed to build and nothing was claimed.
pub(super) fn bug_mark(bug: &Value) -> (Value, Value) {
    let claim = &bug["checks"];
    if claim.is_null() {
        if bug["modelsBuilt"] != true && !bug["buildError"].is_null() {
            return (Value::Null, json!("telemetry_gap"));
        }
        return (json!(0), Value::Null);
    }
    let hits = [&claim["cycleClose"], &claim["exerciseMatches"]]
        .into_iter()
        .filter(|hit| **hit == true)
        .count();
    (json!(1 + hits), Value::Null)
}

/// The facts with the suggested marks, part a from the review's
/// `explanations`, null where no review came back. An attempt that did not
/// complete gets no marks at all, as no task result rates one.
/// `running_at_end` says a run still held the attempt when it ended, so the
/// facts are those kept after the request before it.
pub fn marked(
    mut facts: Value,
    explanations: &Value,
    hints: &Hints,
    running_at_end: bool,
    completed: bool,
) -> Value {
    let reviewed = explanations.is_array();

    // A hint is support: the item it was given on can be marked 2 at most, as a
    // task's dimension supported by a hint is.
    let hinted = |id: &str| hints.on(id);
    let cap = |mark: Value, given: u64| {
        if given == 0 {
            mark
        } else {
            mark.as_u64().map_or(mark, |mark| json!(mark.min(2)))
        }
    };
    if let Some(items) = facts["items"].as_array_mut() {
        for item in items {
            let id = item["id"].as_str().unwrap_or_default().to_owned();
            let row = explanation(explanations, &id);
            let rating = row.map_or(Value::Null, |row| row["rating"].clone());
            let account = row.map_or(Value::Null, |row| row["account"].clone());
            let runs = item["runs"].as_array().cloned().unwrap_or_default();
            let (change, change_reason) = if completed {
                change_mark(&runs, &account, reviewed)
            } else {
                (Value::Null, Value::Null)
            };
            let missing = match (completed, reviewed, rating.is_null()) {
                (false, _, _) | (true, true, false) => Value::Null,
                (true, false, _) => json!("feedback_unavailable"),
                (true, true, true) => json!("missing_turns"),
            };
            let given = hinted(&id);
            item["hints"] = json!(given);
            let change = cap(change, given);
            item["explain"] = json!({
                "suggestedMark": if completed { cap(rating, given) } else { Value::Null },
                "reason": row.map_or(Value::Null, |row| row["reason"].clone()),
                "evidence": row.map_or(json!([]), |row| row["evidence"].clone()),
                "insufficientReason": missing,
            });
            item["predict"] = json!({"suggestedMark": change, "insufficientReason": change_reason,
                "account": account, "runs": kept_runs(&runs)});
            if let Some(object) = item.as_object_mut() {
                object.remove("runs");
            }
        }
    }
    let (bug, bug_reason) = if completed {
        bug_mark(&facts["bug"])
    } else {
        (Value::Null, Value::Null)
    };
    let bug_hints = hinted("bug");
    facts["bug"]["hints"] = json!(bug_hints);
    facts["bug"]["suggestedMark"] = cap(bug, bug_hints);
    facts["hintsAllowed"] = json!(hints.allowed);
    facts["bug"]["insufficientReason"] = bug_reason;
    if let Some(bug) = facts["bug"].as_object_mut() {
        bug.remove("checks");
    }
    facts["marksWithheld"] = json!(!completed);
    facts["runningAtEnd"] = json!(running_at_end);
    facts["marksAreSuggestions"] = json!(true);
    facts["notice"] = json!(
        "Generated on the student's machine. The marks are suggestions for the instructors, not a grade, and the record proves neither identity nor independent work."
    );
    facts
}

/// An item's runs as the result keeps them. The first run that said something
/// about the change sets part b's mark and is kept whole; every other run
/// keeps when it ran, what it showed and whether it matched, so that an
/// attempt that uses all its runs still fits the result.
fn kept_runs(runs: &[Value]) -> Vec<Value> {
    let telling = runs.iter().position(|run| !run["matched"].is_null());
    runs.iter()
        .enumerate()
        .map(|(at, run)| {
            if Some(at) == telling {
                return run.clone();
            }
            let outcome = &run["outcome"];
            json!({"startedAt": run["startedAt"], "endedAt": run["endedAt"],
                "outcome": {"kind": outcome["kind"],
                    "failedCount": outcome["failed"].as_array().map(Vec::len)},
                "matched": run["matched"], "conclusive": run["conclusive"]})
        })
        .collect()
}

/// The hints an attempt allows, and those Jim gave, by item.
#[derive(Debug, Default, Clone)]
pub struct Hints {
    pub given: std::collections::BTreeMap<String, u64>,
    pub allowed: u64,
}

impl Hints {
    /// The hints given on `item`.
    pub fn on(&self, item: &str) -> u64 {
        self.given.get(item).copied().unwrap_or(0)
    }
}

/// The `defense` section of the result of `attempt`.
pub fn record(attempt: &Attempt, explanations: &Value, hints: &Hints, completed: bool) -> Value {
    marked(facts(attempt), explanations, hints, false, completed)
}
