use serde_json::{Value, json};
pub(super) fn present(problem_id: &str, guide: bool) -> Option<Value> {
    let p = crate::agent::find_problem(problem_id)?;
    let v = crate::agent::variant_for(p.id)?;
    Some(if guide {
        json!({"contract":v.contract,"constraints":v.constraints,"clarifications":v.clarifications,"followUps":v.follow_ups,"hints":v.hints,"optimal":p.optimal,"pitfalls":p.pitfalls})
    } else {
        json!({"page":v.page,"title":v.title})
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/guide.rs"]
mod tests;
