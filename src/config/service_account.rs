//! The shape of the Google service account an operator pastes into the
//! configuration, checked where the value is read so that a wrong paste is
//! reported with the other invalid entries, and again by the delivery that
//! signs with it.

use serde_json::Value;

/// Whether a service account has the two fields this binary needs.
///
/// Shape only, and deliberately: this runs at configuration time, where the
/// answer is "an operator pasted the wrong thing". Whether the key itself can
/// be signed with is
/// [`GoogleDelivery::new`](crate::delivery::GoogleDelivery::new)'s question,
/// and startup asks it before serving.
pub fn readable_service_account(service_account_json: &str) -> Result<(), String> {
    service_account_fields(&parse_service_account(service_account_json)?).map(|_| ())
}

/// The fields of a service account this binary reads, checked once for both
/// the configuration-time question and startup.
pub(crate) struct ServiceAccountFields<'a> {
    pub(crate) client_email: &'a str,
    pub(crate) private_key: &'a str,
    pub(crate) token_uri: &'a str,
}

pub(crate) fn service_account_fields(
    credentials: &Value,
) -> Result<ServiceAccountFields<'_>, String> {
    let required = |field: &str| {
        credentials
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("the service account has no {field}"))
    };
    Ok(ServiceAccountFields {
        client_email: required("client_email")?,
        private_key: required("private_key")?,
        token_uri: credentials
            .get("token_uri")
            .and_then(Value::as_str)
            .unwrap_or("https://oauth2.googleapis.com/token"),
    })
}

pub(crate) fn parse_service_account(service_account_json: &str) -> Result<Value, String> {
    serde_json::from_str(service_account_json)
        .map_err(|error| format!("the service account is not JSON: {error}"))
}
