//! The bounded report-key transport shared by interim, phase and peer calls.
use super::{
    ApiSurface, CredentialFailure, GeminiKeys, INTERIM_ATTEMPT_TIMEOUT, credential_failure,
    generate_content_once, http_usage_label,
};
use serde_json::Value;

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
