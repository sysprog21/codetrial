//! Google Drive delivery: the staged object into a Shared Drive file the
//! candidate can read for a day.
//!
//! CodeTrial never holds the media. Egress writes it straight to the staging
//! bucket, and this module moves it from there to Drive without the bytes ever
//! landing on this machine's disk: they are read in ranges and written to a
//! resumable session in the same loop.
//!
//! Every call carries `supportsAllDrives=true`. Without it the API pretends a
//! Shared Drive file does not exist, which surfaces as a 404 on a file that was
//! just created.
//!
//! What is not here, deliberately: retries. One attempt lives here and the
//! schedule lives in the delivery queue, because a retry that a worker cannot
//! see is a retry nobody can bound.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::recording::{BoxFuture, DeliveryProvider};

/// Chunks are a multiple of 256 KiB, which the resumable protocol requires, and
/// large enough that a forty-minute recording is a few dozen requests rather
/// than a few thousand. Each one is held in memory once, so this is also the
/// memory this delivery costs.
pub const UPLOAD_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// Statuses worth trying the same chunk again for. Everything else is an
/// answer: a 401 needs a new token, a 403 needs a permission an operator has to
/// grant, a 404 means the file or the session is gone.
pub const RETRYABLE_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];

/// How many failed chunk writes one upload may absorb before it gives up and
/// lets the queue's own schedule take over.
///
/// A budget for the whole transfer rather than a count per chunk. Per chunk is
/// the shape that looks right and is unbounded: a session that keeps reporting
/// the same offset hands back a chunk to re-send, and a counter that starts
/// again with each one never runs out.
const UPLOAD_RETRY_BUDGET: u32 = 8;

/// How long one transfer may run before it gives the row back.
///
/// Bounded below the delivery queue's claim window, and that is the whole
/// reason for the number: a transfer that outlived its claim would be running
/// while another worker started a second one, and two resumable sessions
/// against the same recording is exactly the duplicate the claim exists to
/// prevent. Half an hour is more than any recording this pipeline produces
/// needs.
pub const TRANSFER_DEADLINE_SECONDS: i64 = 1800;

/// Longer than the shared client's thirty seconds: a chunk is eight megabytes,
/// and a link slow enough to need more than that is still a link worth
/// finishing on.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

/// A token is good for an hour; this asks for a new one a minute early rather
/// than discovering the expiry inside a 401 halfway through an upload.
const TOKEN_MARGIN_SECONDS: i64 = 60;

const SCOPES: &str =
    "https://www.googleapis.com/auth/drive https://www.googleapis.com/auth/devstorage.read_write";

/// The Drive property that says which recording a file is. It is the duplicate
/// search key, so a transfer resumed after a restart finds its own file instead
/// of creating a second one.
pub const RECORDING_ID_PROPERTY: &str = "codetrial_recording_id";

pub struct GoogleDelivery {
    http: reqwest::Client,
    credentials: ServiceAccount,
    bucket: String,
    drive_id: String,
    token: Mutex<Option<CachedToken>>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct ServiceAccount {
    client_email: String,
    private_key: Vec<u8>,
    token_uri: String,
}

struct CachedToken {
    value: String,
    expires_at: i64,
}

/// Whether a service account has the two fields this binary needs.
///
/// Shape only, and deliberately: this runs at configuration time, where the
/// answer is "an operator pasted the wrong thing". Whether the key itself can
/// be signed with is [`GoogleDelivery::new`]'s question, and startup asks it
/// before serving.
pub fn readable_service_account(service_account_json: &str) -> Result<(), String> {
    let credentials: Value = serde_json::from_str(service_account_json)
        .map_err(|error| format!("the service account is not JSON: {error}"))?;
    if credentials
        .get("client_email")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("the service account has no client_email".to_string());
    }
    if credentials
        .get("private_key")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("the service account has no private_key".to_string());
    }
    Ok(())
}

impl GoogleDelivery {
    /// Built from the service account JSON an operator configured.
    ///
    /// The key is parsed here rather than at first use: a malformed credential
    /// should stop a deployment at startup, not halfway through the first
    /// interview it was supposed to deliver.
    pub fn new(
        service_account_json: &str,
        bucket: &str,
        drive_id: &str,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self, String> {
        let credentials: Value = serde_json::from_str(service_account_json)
            .map_err(|error| format!("the service account is not JSON: {error}"))?;
        let client_email = credentials
            .get("client_email")
            .and_then(Value::as_str)
            .ok_or("the service account has no client_email")?
            .to_string();
        let private_key = credentials
            .get("private_key")
            .and_then(Value::as_str)
            .ok_or("the service account has no private_key")?;
        let token_uri = credentials
            .get("token_uri")
            .and_then(Value::as_str)
            .unwrap_or("https://oauth2.googleapis.com/token")
            .to_string();
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| format!("the delivery client could not be built: {error}"))?,
            credentials: ServiceAccount {
                client_email,
                private_key: rsa_key(private_key)?,
                token_uri,
            },
            bucket: bucket.to_string(),
            drive_id: drive_id.to_string(),
            token: Mutex::new(None),
            now,
        })
    }

    /// A bearer token, minted or remembered.
    ///
    /// The lock is held across the mint, which serializes the first call of a
    /// burst rather than letting every one of them mint its own. Minting is a
    /// round trip to Google and a signature; doing it once is worth the wait.
    async fn access_token(&self) -> Result<String, String> {
        let now = (self.now)();
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref()
            && token.expires_at - TOKEN_MARGIN_SECONDS > now
        {
            return Ok(token.value.clone());
        }

        let assertion = self.assertion(now)?;
        let response = self
            .http
            .post(&self.credentials.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await
            .map_err(|error| format!("the token request failed: {error}"))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("the token response was not JSON: {error}"))?;
        if !status.is_success() {
            // The error code, not the description: Google's description
            // sometimes quotes the assertion back, and this string reaches an
            // audit line.
            let code = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(format!("the token request was refused: {status} {code}"));
        }
        let value = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or("the token response carried no access_token")?
            .to_string();
        let lifetime = body
            .get("expires_in")
            .and_then(Value::as_i64)
            .unwrap_or(3600);
        *cached = Some(CachedToken {
            value: value.clone(),
            expires_at: now + lifetime,
        });
        Ok(value)
    }

    /// Forget the cached token.
    ///
    /// Called on a `401`, which is the one answer that says the token is wrong
    /// whatever the local clock thinks. Without this every attempt in the
    /// queue's schedule reuses the same rejected credential and the recording
    /// runs out of attempts against a token that was never going to work.
    async fn invalidate(&self) {
        *self.token.lock().await = None;
    }

    /// The signed claim that buys a token.
    fn assertion(&self, now: i64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "typ": "JWT"}).to_string());
        let claims = URL_SAFE_NO_PAD.encode(
            json!({
                "iss": self.credentials.client_email,
                "scope": SCOPES,
                "aud": self.credentials.token_uri,
                "iat": now,

                // A minute short of the hour Google allows. Exactly an hour is
                // within the rule until this machine's clock is a second ahead
                // of theirs, and then every delivery fails on an assertion that
                // looks fine here.
                "exp": now + 3540,
            })
            .to_string(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = sign_rs256(&self.credentials.private_key, signing_input.as_bytes())?;
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    /// The file this recording already has in the Shared Drive, if any.
    ///
    /// Searched by app property rather than by name, because a name is a thing
    /// a person can change and this is the key the resume depends on.
    async fn existing_file(
        &self,
        recording_id: &str,
        token: &str,
    ) -> Result<Option<String>, String> {
        // Spelled the way Drive's own documentation spells it, spaces included.
        // This is a query language on somebody else's server and the failure
        // mode for getting it wrong is a 400 on every delivery.
        let query = format!(
            "appProperties has {{ key='{RECORDING_ID_PROPERTY}' and value='{recording_id}' }} and trashed = false"
        );
        let response = self
            .http
            .get("https://www.googleapis.com/drive/v3/files")
            .bearer_auth(token)
            .query(&[
                ("q", query.as_str()),
                ("corpora", "drive"),
                ("driveId", self.drive_id.as_str()),
                ("includeItemsFromAllDrives", "true"),
                ("supportsAllDrives", "true"),
                ("fields", "files(id)"),
            ])
            .send()
            .await
            .map_err(|error| format!("the duplicate search failed: {error}"))?;
        if response.status().as_u16() == 401 {
            self.invalidate().await;
        }
        let body = read_json(response, "the duplicate search").await?;
        Ok(body
            .get("files")
            .and_then(Value::as_array)
            .and_then(|files| files.first())
            .and_then(|file| file.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// The staged object's size, which the resumable protocol needs up front.
    async fn object_size(&self, gcs_object: &str, token: &str) -> Result<u64, String> {
        let response = self
            .http
            .get(format!(
                "https://storage.googleapis.com/storage/v1/b/{}/o/{}",
                self.bucket,
                encode_path(gcs_object)
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| format!("the staged object could not be read: {error}"))?;
        if response.status().as_u16() == 401 {
            self.invalidate().await;
        }
        let body = read_json(response, "the staged object").await?;
        body.get("size")
            .and_then(|size| {
                size.as_str()
                    .and_then(|size| size.parse::<u64>().ok())
                    .or_else(|| size.as_u64())
            })
            .filter(|size| *size > 0)
            .ok_or_else(|| "the staged object has no bytes".to_string())
    }

    /// One range of the staged object.
    async fn object_range(
        &self,
        gcs_object: &str,
        token: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, String> {
        let response = self
            .http
            .get(format!(
                "https://storage.googleapis.com/storage/v1/b/{}/o/{}?alt=media",
                self.bucket,
                encode_path(gcs_object)
            ))
            .bearer_auth(token)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .await
            .map_err(|error| format!("a range of the staged object failed: {error}"))?;
        if response.status().as_u16() == 401 {
            self.invalidate().await;
        }
        if !response.status().is_success() {
            return Err(format!(
                "a range of the staged object was refused: {}",
                response.status()
            ));
        }

        // `206`, and the length it promises, before the body is read. A proxy
        // that ignores `Range` answers `200` with the whole object, and
        // buffering that is however many gigabytes the recording is rather than
        // the one chunk this delivery is supposed to cost. A chunked answer
        // with no length at all is refused for the same reason: an unbounded
        // read is not one to take on trust from a hop this pipeline does not
        // control.
        let expected = end - start + 1;
        if response.status().as_u16() != 206 {
            return Err(format!(
                "a range of the staged object came back whole: {}",
                response.status()
            ));
        }
        if response.content_length() != Some(expected) {
            return Err(format!(
                "a range of the staged object promised the wrong size: asked for {expected}"
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("a range of the staged object was truncated: {error}"))?;
        if bytes.len() as u64 != expected {
            return Err(format!(
                "a range of the staged object was the wrong size: asked for {expected}, got {}",
                bytes.len()
            ));
        }
        Ok(bytes.to_vec())
    }

    /// Open a resumable session and return the URI the chunks go to.
    ///
    /// A session that is never finished creates no file, which is why an
    /// attempt that dies mid-upload leaves nothing behind for the duplicate
    /// search to find and nothing in the Drive for retention to chase.
    async fn upload_session(
        &self,
        filename: &str,
        recording_id: &str,
        token: &str,
    ) -> Result<String, String> {
        let response = self
            .http
            .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&supportsAllDrives=true")
            .bearer_auth(token)
            .json(&json!({
                "name": filename,
                "parents": [self.drive_id],
                "mimeType": "video/mp4",
                "appProperties": { RECORDING_ID_PROPERTY: recording_id },
            }))
            .send()
            .await
            .map_err(|error| format!("the upload session could not be opened: {error}"))?;
        if response.status().as_u16() == 401 {
            self.invalidate().await;
        }
        if !response.status().is_success() {
            return Err(format!(
                "the upload session was refused: {}",
                response.status()
            ));
        }
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| "the upload session carried no Location".to_string())
    }

    /// Where a session got to, in bytes, after a chunk failed.
    ///
    /// `308` with a `Range` of `bytes=0-n` means the server holds `n + 1`
    /// bytes; `308` with no `Range` means it holds none. Anything else means
    /// the session is finished or gone, and the caller has to decide which.
    async fn session_progress(&self, session: &str, total: u64) -> Result<Option<u64>, String> {
        let response = self
            .http
            .put(session)
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .header(reqwest::header::CONTENT_RANGE, format!("bytes */{total}"))
            .send()
            .await
            .map_err(|error| format!("the upload session could not be queried: {error}"))?;
        if response.status().as_u16() != 308 {
            return Ok(None);
        }
        stored_bytes(&response, total).map(Some)
    }
}

/// The PEM body, decoded and checked.
///
/// Google issues an unencrypted PKCS#8 RSA key, which is what `ring` wants, so
/// this strips the armour and then asks `ring` whether what came out is really
/// a key. Checked here rather than at first use, because a credential this
/// broken should stop a deployment rather than the first interview it was
/// supposed to deliver.
fn rsa_key(pem: &str) -> Result<Vec<u8>, String> {
    let der = pkcs8_der(pem)?;
    ring::signature::RsaKeyPair::from_pkcs8(&der)
        .map_err(|error| format!("the private key could not be read: {error}"))?;
    Ok(der)
}

fn pkcs8_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(|line| line.chars().filter(|character| !character.is_whitespace()))
        .collect();
    STANDARD
        .decode(body)
        .map_err(|error| format!("the private key is not base64: {error}"))
}

/// RSASSA-PKCS1-v1_5 over SHA-256, which is what `RS256` means.
///
/// `ring` rather than a JWT library: it is already in this tree behind rustls,
/// and what is needed here is one signature over bytes this module assembled
/// itself.
fn sign_rs256(pkcs8: &[u8], message: &[u8]) -> Result<Vec<u8>, String> {
    let key = ring::signature::RsaKeyPair::from_pkcs8(pkcs8)
        .map_err(|error| format!("the private key could not be read: {error}"))?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        message,
        &mut signature,
    )
    .map_err(|error| format!("the assertion could not be signed: {error}"))?;
    Ok(signature)
}

/// Percent-encoding for one path segment.
///
/// A GCS object name contains slashes, and the JSON API takes it as a single
/// path segment, so those slashes have to arrive as `%2F` or the request names
/// a different object.
fn encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn read_json(response: reqwest::Response, what: &str) -> Result<Value, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{what} was refused: {status}"));
    }
    response
        .json()
        .await
        .map_err(|error| format!("{what} was not JSON: {error}"))
}

/// Epoch seconds as RFC 3339 in UTC, which is the only shape Drive accepts for
/// `expirationTime`.
///
/// Written out rather than pulled in: this is the one date this binary
/// formats, and the civil-from-days arithmetic is shorter than the argument for
/// a date crate.
pub fn rfc3339(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let seconds = epoch_seconds.rem_euclid(86_400);

    // Howard Hinnant's civil_from_days, with the era shifted so that day zero
    // is 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

impl DeliveryProvider for GoogleDelivery {
    fn transfer<'a>(
        &'a self,
        gcs_object: &'a str,
        filename: &'a str,
        recording_id: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let token = self.access_token().await?;

            // Before anything is uploaded. A restart mid-transfer must find its
            // own file rather than make a second one, and the row's
            // `drive_file_id` cannot answer for an upload that finished and
            // then failed to be written down.
            if let Some(existing) = self.existing_file(recording_id, &token).await? {
                return Ok(existing);
            }

            let total = self.object_size(gcs_object, &token).await?;
            let session = self.upload_session(filename, recording_id, &token).await?;

            let mut sent = 0u64;
            let mut budget = UPLOAD_RETRY_BUDGET;
            let deadline = (self.now)() + TRANSFER_DEADLINE_SECONDS;
            while sent < total {
                if (self.now)() >= deadline {
                    // Given up rather than run on. The session dies with this
                    // attempt and creates no file, so the queue's next attempt
                    // starts clean instead of racing this one.
                    return Err("the upload outran its deadline".to_string());
                }
                let end = (sent + UPLOAD_CHUNK_BYTES).min(total) - 1;
                let chunk = self.object_range(gcs_object, &token, sent, end).await?;
                let last = sent + chunk.len() as u64 - 1;

                let response = self
                    .http
                    .put(&session)
                    .header(
                        reqwest::header::CONTENT_RANGE,
                        format!("bytes {sent}-{last}/{total}"),
                    )
                    .body(chunk)
                    .send()
                    .await;

                // Matched once, so the response stays owned by the branch that
                // reads it. Deriving a status number first and then unwrapping
                // the response again needs a sentinel for the no-answer case,
                // and that sentinel has to be excluded by hand from every
                // comparison below it.
                if let Ok(response) = response {
                    let status = response.status().as_u16();
                    if status == 200 || status == 201 {
                        let body: Value = response
                            .json()
                            .await
                            .map_err(|error| format!("the upload answer was not JSON: {error}"))?;
                        return body
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| "the upload answer carried no file id".to_string());
                    }
                    if status == 308 {
                        // What the server says it holds, not what this call
                        // sent. A `308` acknowledging part of a chunk is
                        // allowed by the protocol, and assuming the whole chunk
                        // landed would skip those bytes and finish a corrupt
                        // file.
                        let stored = stored_bytes(&response, total)?;
                        if stored > sent {
                            sent = stored;
                            continue;
                        }

                        // No progress at all. Spend a unit of the budget rather
                        // than re-sending the same bytes forever.
                        if budget == 0 {
                            return Err("the upload made no progress".to_string());
                        }
                        budget -= 1;
                        continue;
                    }
                    if !RETRYABLE_STATUSES.contains(&status) {
                        return Err(format!("a chunk was refused: {status}"));
                    }
                }

                // Retryable, or no answer at all: a dropped connection is the
                // case the resumable protocol exists for, and it is not an
                // answer, so it goes through the same budget as one. Ask the
                // session what it actually holds rather than assuming, and
                // spend a unit of the budget whether or not that moves `sent`:
                // a session stuck at one offset is exactly the case a per-chunk
                // counter never escapes.
                if budget == 0 {
                    return Err("the upload ran out of retries".to_string());
                }
                budget -= 1;
                match self.session_progress(&session, total).await? {
                    Some(received) => sent = received,

                    // Not a 308, so the session is finished or gone. The
                    // duplicate search on the next attempt is what finds out
                    // which.
                    None => return Err("the upload session is no longer open".to_string()),
                }
            }
            Err("the upload finished without an answer".to_string())
        })
    }

    fn share<'a>(
        &'a self,
        drive_file_id: &'a str,
        recipient_email: &'a str,
        expires_at: i64,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let token = self.access_token().await?;
            let response = self
                .http
                .post(format!(
                    "https://www.googleapis.com/drive/v3/files/{drive_file_id}/permissions?supportsAllDrives=true&sendNotificationEmail=false"
                ))
                .bearer_auth(&token)
                .json(&json!({
                    "role": "reader",
                    "type": "user",
                    "emailAddress": recipient_email,
                    "expirationTime": rfc3339(expires_at),
                }))
                .send()
                .await
                .map_err(|error| format!("the share failed: {error}"))?;
            if response.status().as_u16() == 401 {
                self.invalidate().await;
            }
            let body = read_json(response, "the share").await?;
            body.get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "the share answer carried no permission id".to_string())
        })
    }

    fn revoke<'a>(
        &'a self,
        drive_file_id: &'a str,
        permission_id: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let token = self.access_token().await?;
            let response = self
                .http
                .delete(format!(
                    "https://www.googleapis.com/drive/v3/files/{drive_file_id}/permissions/{permission_id}?supportsAllDrives=true"
                ))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|error| format!("the revoke failed: {error}"))?;
            if response.status().as_u16() == 401 {
                self.invalidate().await;
            }
            gone_or_ok(response.status().as_u16(), "the revoke")
        })
    }

    fn delete_file<'a>(&'a self, drive_file_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let token = self.access_token().await?;
            let response = self
                .http
                .delete(format!(
                    "https://www.googleapis.com/drive/v3/files/{drive_file_id}?supportsAllDrives=true"
                ))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|error| format!("the file deletion failed: {error}"))?;
            if response.status().as_u16() == 401 {
                self.invalidate().await;
            }
            gone_or_ok(response.status().as_u16(), "the file deletion")
        })
    }

    fn delete_object<'a>(&'a self, gcs_object: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let token = self.access_token().await?;
            let response = self
                .http
                .delete(format!(
                    "https://storage.googleapis.com/storage/v1/b/{}/o/{}",
                    self.bucket,
                    encode_path(gcs_object)
                ))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|error| format!("the staged object deletion failed: {error}"))?;
            if response.status().as_u16() == 401 {
                self.invalidate().await;
            }
            gone_or_ok(response.status().as_u16(), "the staged object deletion")
        })
    }
}

/// How many bytes a resumable session says it holds.
///
/// `Range: bytes=0-n` means `n + 1` bytes; no `Range` at all means none, which
/// is what a session that has received nothing answers.
fn stored_bytes(response: &reqwest::Response, total: u64) -> Result<u64, String> {
    let Some(range) = response
        .headers()
        .get(reqwest::header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(0);
    };

    // `bytes=0-n`, exactly. Anything else is a header this code cannot act on,
    // and guessing at it moves the offset past bytes that were never stored.
    let unreadable = || format!("the upload session reported an unreadable range: {range}");
    let end = range
        .strip_prefix("bytes=0-")
        .ok_or_else(unreadable)?
        .trim()
        .parse::<u64>()
        .map_err(|_| unreadable())?;
    let stored = end.checked_add(1).ok_or_else(unreadable)?;
    if stored > total {
        return Err(format!(
            "the upload session claims more bytes than the object has: {stored} of {total}"
        ));
    }
    Ok(stored)
}

/// A deletion that finds nothing has done its job.
///
/// Retention runs more than once against the same recording, and the second run
/// must not report a failure because the first one succeeded.
fn gone_or_ok(status: u16, what: &str) -> Result<(), String> {
    if (200..300).contains(&status) || status == 404 {
        return Ok(());
    }
    Err(format!("{what} was refused: {status}"))
}

#[cfg(test)]
#[path = "../tests/unit/delivery.rs"]
mod tests;
