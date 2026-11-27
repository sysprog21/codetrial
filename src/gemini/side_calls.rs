//! The side calls made over HTTP rather than the live socket: the interim
//! review and the phase judgment, and the one attempt on the report keys they
//! and the peer observer share.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// The pause-time note-taker. One attempt, no repair loop, no retry.
///
/// Everything `generate_report_with_keys` spends its budget defending is
/// absent here on purpose. There is no schema to violate, because the answer
/// is lines of prose; there is nobody waiting on it, because the interview is
/// still running; and there is nothing lost when it fails, because the
/// transcript this was reading still reaches the final reviewer whole. A retry
/// would only take a second pause to re-read a stretch the next call sees
/// anyway.
pub(crate) async fn generate_interim_review_with_keys(
    keys: &GeminiKeys,
    model: &str,
    prompt: &str,
    scope: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    generate_interim_review_at(keys, &gemini_generate_content_url(model), prompt, scope).await
}

pub(super) async fn generate_interim_review_at(
    keys: &GeminiKeys,
    url: &str,
    prompt: &str,
    scope: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let request = content_request(
        &crate::agent::interim_system_instruction(),
        prompt,
        interim_generation_config(),
    );
    side_call_once(keys, url, &request, "interim", scope, true).await
}

/// The phase judge: one attempt, like the interim review and for the same
/// reasons. A call that fails ticks nothing, and the next candidate turn asks
/// again over the same transcript tail.
pub(crate) async fn generate_phase_judgment_with_keys(
    keys: &GeminiKeys,
    model: &str,
    prompt: &str,
    scope: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    generate_phase_judgment_at(keys, &gemini_generate_content_url(model), prompt, scope).await
}

pub(super) async fn generate_phase_judgment_at(
    keys: &GeminiKeys,
    url: &str,
    prompt: &str,
    scope: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let request = content_request(
        &crate::agent::phase_judge_system_instruction(),
        prompt,
        phase_judge_generation_config(),
    );

    // A rate limit is not reported against the key. Cooling it would take the
    // key off the report surface for every room in this process, and the judge,
    // which may run dozens of times an interview, would then be what left a
    // final report without a key. The next judgment simply asks again.
    side_call_once(keys, url, &request, "phase", scope, false).await
}

/// Deterministic, and JSON: the answer is read by the server, not a person.
fn phase_judge_generation_config() -> Value {
    json!({
        "responseMimeType": "application/json",
        "maxOutputTokens": 1024,
        "thinkingConfig": { "thinkingBudget": 0 },
        "temperature": 0.0,
        "seed": GENERATION_SEED
    })
}

/// Plain text and a small ceiling, where the report asks for JSON against a
/// schema. The prompt caps the answer at four lines; this caps what an answer
/// that ignores that can cost. Thinking is off for the reason it is off on the
/// report: the budget it spends comes out of the same allowance as the output.
/// The temperature is below the report's because this is note-taking, not
/// writing.
pub(super) fn interim_generation_config() -> Value {
    json!({
        "responseMimeType": "text/plain",
        "maxOutputTokens": 512,
        "thinkingConfig": { "thinkingBudget": 0 },
        "temperature": 0.2,
        "seed": GENERATION_SEED
    })
}

/// One attempt at a side call on the report keys: the interim review, the
/// phase judge or the peer observer, which differ only in the request, the
/// label and whether a rate limit cools the key.
pub(super) async fn side_call_once(
    keys: &GeminiKeys,
    url: &str,
    request: &Value,
    surface: &str,
    scope: &str,
    cool_on_quota: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let api_key = keys.select_report()?;
    let result = generate_content_once(
        &api_key,
        url,
        request,
        INTERIM_ATTEMPT_TIMEOUT,
        &http_usage_label(surface, scope, 1, 0),
    )
    .await;
    if let Err(error) = &result
        && let Some(failure) = credential_failure(error.as_ref())
        && (cool_on_quota || failure != CredentialFailure::Quota)
    {
        keys.failed(&api_key, failure, ApiSurface::Report);
    }
    result
}
