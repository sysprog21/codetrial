//! Strict selection changes admission, never the interviewer or its assessment.
//! The receipt remembers only this tab's previous assignment. It grants no
//! room access and is not proof that the browser kept its newest receipt.

use axum::http::StatusCode;
use axum::response::Response;
use ring::rand::SecureRandom;
use serde_json::{Value, json};

use crate::agent::{PROBLEMS, Problem, find_problem};
use crate::token::{sign_hs256, verify_hs256};

use super::json_response;

const PURPOSE: &str = "strict-selection-v1";
const DIFFICULTIES: &[&str] = &["Easy", "Medium", "Hard"];

pub(super) struct Assignment {
    pub page: &'static str,
    pub receipt: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SelectionError {
    Mode,
    Problem,
    Difficulty,
    Receipt,
    NoAlternative,
    Random,
}

impl SelectionError {
    pub(super) fn response(self) -> Response {
        let (status, code, error) = match self {
            Self::Mode => (
                StatusCode::BAD_REQUEST,
                "selection_mode_invalid",
                "Choose Practice or Strict Interview.",
            ),
            Self::Problem => (
                StatusCode::BAD_REQUEST,
                "strict_problem_forbidden",
                "Strict Interview assigns its problem on the server.",
            ),
            Self::Difficulty => (
                StatusCode::BAD_REQUEST,
                "strict_difficulty_invalid",
                "Choose one or more distinct difficulties: Easy, Medium or Hard.",
            ),
            Self::Receipt => (
                StatusCode::BAD_REQUEST,
                "strict_assignment_invalid",
                "The previous Strict assignment could not be verified. Return to the lobby to start a new selection sequence.",
            ),
            Self::NoAlternative => (
                StatusCode::CONFLICT,
                "strict_no_alternative",
                "No different problem is available at the selected difficulties. Return to the lobby to change the selection.",
            ),
            Self::Random => (
                StatusCode::SERVICE_UNAVAILABLE,
                "strict_selection_unavailable",
                "A problem could not be assigned. Try again later.",
            ),
        };
        json_response(status, json!({ "code": code, "error": error }))
    }
}

/// Missing means Practice for old pages, bookmarks and API clients.
pub(super) fn assign(
    request: &Value,
    owner: i64,
    secret: &str,
) -> Result<Option<Assignment>, SelectionError> {
    assign_with_entropy(request, owner, secret, |bytes| {
        ring::rand::SystemRandom::new()
            .fill(bytes)
            .map_err(|_| SelectionError::Random)
    })
}

fn assign_with_entropy(
    request: &Value,
    owner: i64,
    secret: &str,
    fill: impl FnMut(&mut [u8]) -> Result<(), SelectionError>,
) -> Result<Option<Assignment>, SelectionError> {
    match request.get("selectionMode") {
        None => return Ok(None),
        Some(Value::String(mode)) if mode == "practice" => return Ok(None),
        Some(Value::String(mode)) if mode == "strict" => {}
        _ => return Err(SelectionError::Mode),
    }
    if request.get("problemId").is_some() {
        return Err(SelectionError::Problem);
    }

    // Old Strict links name no difficulty. Explicit filters must be valid in
    // full: dropping a misspelling or an empty list would broaden the draw.
    let difficulties = match request.get("difficulties") {
        None => DIFFICULTIES.to_vec(),
        Some(Value::Array(values)) if !values.is_empty() && values.len() <= DIFFICULTIES.len() => {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|level| DIFFICULTIES.contains(level))
                        .ok_or(SelectionError::Difficulty)
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        _ => return Err(SelectionError::Difficulty),
    };
    if difficulties
        .iter()
        .enumerate()
        .any(|(index, level)| difficulties[..index].contains(level))
    {
        return Err(SelectionError::Difficulty);
    }
    let previous = match request.get("previousAssignment") {
        None => None,
        Some(Value::String(receipt)) => Some(read_receipt(receipt, owner, secret)?),
        _ => return Err(SelectionError::Receipt),
    };
    let problem = choose(PROBLEMS, &difficulties, previous, |count| {
        random_index(count, fill)
    })?;
    let page = problem.variant().page;
    Ok(Some(Assignment {
        page,
        receipt: receipt_for(page, owner, secret),
    }))
}

fn receipt_for(page: &str, owner: i64, secret: &str) -> String {
    let payload = format!("{PURPOSE}:{owner}:{page}");
    format!("{payload}.{}", sign_hs256(secret, &payload))
}

/// Bound parsing and authentication work separately from whether a receipt
/// authenticates: an oversized forgery must not reach the HMAC check.
fn receipt_parts(receipt: &str) -> Result<(&str, &str), SelectionError> {
    if receipt.len() > 512 {
        return Err(SelectionError::Receipt);
    }
    receipt.rsplit_once('.').ok_or(SelectionError::Receipt)
}

fn read_receipt<'a>(receipt: &'a str, owner: i64, secret: &str) -> Result<&'a str, SelectionError> {
    let (payload, signature) = receipt_parts(receipt)?;
    if !verify_hs256(secret, payload, signature) {
        return Err(SelectionError::Receipt);
    }
    let prefix = format!("{PURPOSE}:{owner}:");
    let page = payload
        .strip_prefix(&prefix)
        .ok_or(SelectionError::Receipt)?;

    // Do not resolve a retired page to the default exercise or accept a
    // published-id alias as though it were the page the server signed.
    if !find_problem(page).is_some_and(|problem| problem.variant().page == page) {
        return Err(SelectionError::Receipt);
    }
    Ok(page)
}

fn choose<'a>(
    problems: &'a [Problem],
    difficulties: &[&str],
    previous: Option<&str>,
    index: impl FnOnce(usize) -> Result<usize, SelectionError>,
) -> Result<&'a Problem, SelectionError> {
    let choices: Vec<_> = problems
        .iter()
        .filter(|problem| difficulties.contains(&problem.difficulty))
        .filter(|problem| Some(problem.variant().page) != previous)
        .collect();
    if choices.is_empty() {
        return Err(SelectionError::NoAlternative);
    }
    Ok(choices[index(choices.len())?])
}

/// The caller has already refused an empty pool. Inject only the entropy
/// source so tests can reach the rejection boundary without relying on luck.
fn random_index(
    count: usize,
    mut fill: impl FnMut(&mut [u8]) -> Result<(), SelectionError>,
) -> Result<usize, SelectionError> {
    let count = count as u64;

    // Reject the partial bucket rather than biasing the last few exercises. An
    // unavailable entropy source refuses admission, never chooses a default.
    let limit = u64::MAX - u64::MAX % count;
    loop {
        let mut bytes = [0; 8];
        fill(&mut bytes)?;
        let value = u64::from_le_bytes(bytes);
        if value < limit {
            return Ok((value % count) as usize);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/web/selection.rs"]
mod tests;
