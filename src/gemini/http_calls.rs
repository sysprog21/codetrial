//! The transport under the side calls made over HTTP rather than the live
//! socket: the request envelope, one attempt, the usage label and the failure
//! handling every such call shares.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// The seed both HTTP calls sample with. Measured against the report model at
/// its report temperature, four calls with one prompt and this seed returned
/// one answer four times, and four without it returned four. The same prompt
/// then gets the same notes and the same report, which is what lets a replayed
/// session be compared with the one it replays; the Live session is left
/// unseeded, since a voice that answers every candidate in identical words is
/// not a trade the interview should make, and its audio cannot be replayed
/// byte for byte whatever the seed.
pub(crate) const GENERATION_SEED: i64 = 71;

/// The seed a regeneration samples with when the first generation had an
/// answer refused, whether it then ran out of repairs, lost its repair call or
/// missed the deadline. With `GENERATION_SEED` the frozen prompt answers what
/// it answered the first time, and the same repairs refuse it again, so the
/// one regeneration would be spent on a known result. Fixed rather than drawn,
/// so a replay that fails the same way regenerates the same way. A generation
/// that never had an answer refused keeps `GENERATION_SEED`, and the report it
/// gets is the one that answer would have been.
pub(crate) const REGENERATION_SEED: i64 = 72;

/// The `generateContent` envelope: the constant instruction first, then the
/// one prompt part, and whatever the caller wants generated from it. The two
/// callers differ only in the instruction and the config, and the envelope is
/// the wire contract, which is not a thing to assert in two places.
pub(super) fn content_request(system: &str, prompt: &str, generation_config: Value) -> Value {
    json!({
        "systemInstruction": { "parts": [ { "text": system } ] },
        "contents": [ { "parts": [ { "text": prompt } ] } ],
        "generationConfig": generation_config
    })
}

pub(super) fn http_usage_label(surface: &str, room: &str, call: usize, retry: u32) -> String {
    format!("{surface} room={room} call={call} retry={retry}")
}

pub(super) fn http_failure_diagnostic(
    error: &(dyn std::error::Error + 'static),
    stage: &str,
    elapsed: Duration,
    timeout: Duration,
) -> Value {
    let request = error.downcast_ref::<reqwest::Error>();
    let mut source = error.source();
    let mut depth = 0;
    let mut io_kind = None;
    while let Some(cause) = source.filter(|_| depth < 8) {
        depth += 1;
        if let Some(io) = cause.downcast_ref::<io::Error>() {
            io_kind = Some(format!("{:?}", io.kind()));
        }
        source = cause.source();
    }
    json!({
        "stage": stage,
        "elapsed_ms": crate::agent::saturating_ms(elapsed),
        "timeout_ms": crate::agent::saturating_ms(timeout),
        "timeout": request.is_some_and(reqwest::Error::is_timeout),
        "connect": request.is_some_and(reqwest::Error::is_connect),
        "request": request.is_some_and(reqwest::Error::is_request),
        "body": request.is_some_and(reqwest::Error::is_body),
        "decode": request.is_some_and(reqwest::Error::is_decode),
        "builder": request.is_some_and(reqwest::Error::is_builder),
        "redirect": request.is_some_and(reqwest::Error::is_redirect),
        "status": error.downcast_ref::<ApiFailure>().map(|failure| failure.status)
            .or_else(|| request.and_then(reqwest::Error::status).map(|status| status.as_u16())),
        "source_depth": depth,
        "io_kind": io_kind,
    })
}

fn observe_http_failure(
    error: &(dyn std::error::Error + 'static),
    stage: &str,
    started: std::time::Instant,
    timeout: Duration,
) {
    // Error strings may contain URLs or credentials. Only typed classifications
    // and bounded timing cross this log boundary; the original error is
    // returned.
    eprintln!(
        "codetrial http_failure {}",
        http_failure_diagnostic(error, stage, started.elapsed(), timeout)
    );
}

/// One `generateContent` call, with no opinion about retries.
///
/// The report transport makes the same two calls itself, so an attempt observer
/// can see the decoded response before its text is read. Both share
/// `post_generate_content` and `response_text`: written twice, an auth-header
/// change or a different reading of a text-less response lands in one of them.
pub(super) async fn generate_content_once(
    api_key: &str,
    url: &str,
    request: &Value,
    timeout: Duration,
    what: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let response = post_generate_content(api_key, url, request, timeout, what).await?;
    response_text(&response, what)
}

pub(super) fn response_text(
    response: &Value,
    what: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    gemini_text(response).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Gemini {what} response had no text"),
        )
        .into()
    })
}

/// The most of an answer read before it is refused. A report is cut at 256 KiB
/// of text later; this is the envelope around it (candidates, usage, thinking
/// metadata), with room to spare, and exists so that an endpoint streaming
/// without end cannot grow the buffer until the request timeout.
const MAX_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The stage a failed read of an answer's body is logged under: the size cap is
/// the one error this module makes itself, a body that would not decode is the
/// `reqwest` decode error, and anything else is the read itself failing, a
/// reset or a timeout, which says nothing about the body.
pub(super) fn body_stage(error: &(dyn std::error::Error + 'static)) -> &'static str {
    if error.is::<io::Error>() {
        "body_limit"
    } else if error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_decode)
    {
        "decode"
    } else {
        "body_read"
    }
}

/// The answer's JSON, read a chunk at a time so the size is checked before it
/// is held, not after.
async fn read_json_capped(
    mut response: reqwest::Response,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BODY_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("response exceeds {MAX_RESPONSE_BODY_BYTES} bytes"),
            )
            .into());
        }
        body.extend_from_slice(&chunk);
    }

    // Decoded by reqwest from the bytes already held, so a malformed answer
    // still surfaces as the decode failure the diagnostics classify.
    Ok(reqwest::Response::from(axum::http::Response::new(body))
        .json::<Value>()
        .await?)
}

/// The transport half of `generate_content_once`, for a caller that reads
/// something other than text out of the answer.
pub(super) async fn post_generate_content(
    api_key: &str,
    url: &str,
    request: &Value,
    timeout: Duration,
    what: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let started = std::time::Instant::now();
    let response = crate::http_client()
        .post(url)
        .header("x-goog-api-key", api_key)
        .timeout(timeout)
        .json(request)
        .send()
        .await
        .inspect_err(|error| observe_http_failure(error, "send", started, timeout))?;
    let status = response.status();
    if !status.is_success() {
        let body = read_json_capped(response).await.unwrap_or(Value::Null);
        let failure = ApiFailure::from_response(status.as_u16(), &body);
        observe_http_failure(&failure, "http_status", started, timeout);
        return Err(failure.into());
    }
    let response = read_json_capped(response).await.inspect_err(|error| {
        observe_http_failure(error.as_ref(), body_stage(error.as_ref()), started, timeout);
    })?;
    let usage = response
        .get("usageMetadata")
        .map(TokenUsage::from_metadata)
        .unwrap_or_default();
    eprintln!("gemini {what} usage {}", usage.log_fields());
    Ok(response)
}

#[cfg(test)]
#[path = "../../tests/unit/gemini/http_stage.rs"]
mod stage_tests;
