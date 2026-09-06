//! Account storage and session lifecycle: the SQLite side of signing in, kept
//! apart from the HTTP handlers in [`crate::web`] that call it. Nothing here
//! knows about axum, so the schema and the queries can be read and tested
//! without a router in the way.
//!
//! What stays in this file is what every submodule needs: the configuration
//! types, the one connection they share, and the two helpers that decide
//! whether a GitHub login is one at all. The rest is grouped by what it is
//! about -- [`schema`] for the database's shape, and [`sessions`],
//! [`interviews`] and [`reports`] for the queries against it -- and re-exported
//! here, so a caller still writes `accounts::create_session` and does not have
//! to know which file it landed in.

pub mod interviews;
pub mod reports;
pub mod schema;
pub mod sessions;

pub use interviews::*;
pub use reports::*;
pub use schema::*;
pub use sessions::*;

use std::path::PathBuf;

use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use ring::rand::SecureRandom;

use rusqlite::OptionalExtension;

use serde_json::{Value, json};

use crate::current_epoch_seconds;

/// No `Debug`: `session_secret` signs the session cookie, so a copy in a log
/// is not a credential to steal but the ability to mint any user's session,
/// and `oauth` carries the client secret beneath it.
#[derive(Clone, PartialEq, Eq)]
pub struct GitHubLoginConfig {
    /// Absent unless an OAuth app is configured. The two credentials are only
    /// ever useful together, so they travel together and a handler that has
    /// this at all has both.
    pub oauth: Option<GitHubOauth>,
    pub session_secret: String,
    pub db_path: PathBuf,
    pub oauth_base_url: String,
    pub api_base_url: String,
}

/// Same reason. The pair is only ever useful together, so the secret is never
/// more than one field away from anything that prints the id.
#[derive(Clone, PartialEq, Eq)]
pub struct GitHubOauth {
    pub client_id: String,
    pub client_secret: String,
}

/// The row in `users`, keyed by the local id every other table references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedInUser {
    pub id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
    /// The primary verified address GitHub gave us at sign-in, and only that.
    /// `None` for every self-declared login, which is the whole point: a typed
    /// handle is a label, and a recording is delivered to a person.
    pub verified_email: Option<String>,
}

/// What GitHub told us. Its `github_id` is not the local `users.id`, which is
/// why this is a separate type rather than a reused [`SignedInUser`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubProfile {
    pub github_id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
    /// Present only for a real OAuth sign-in that returned a primary verified
    /// address. Nothing else in `/user/emails` is kept: the response lists
    /// every address a person has registered, and this pipeline needs one.
    pub verified_email: Option<String>,
}

/// The account database and the settings that reach it, resolved once at
/// startup. Both used to be rebuilt per request: `login_config` allocated five
/// strings to answer questions about immutable config, and every query opened a
/// fresh SQLite handle, paying a file open and a schema read before doing any
/// work. One connection also means `PRAGMA foreign_keys` is actually in force,
/// which per-request handles never had, so `ON DELETE CASCADE` now applies.
///
/// The ceiling that buys: one connection behind a mutex serializes every
/// account query, on the blocking pool rather than on an async worker. An
/// interview is a handful of queries at its edges, so the queue is never the
/// thing anyone waits for; a deployment running many concurrent interviews
/// would want a pool here, and would find out by measuring rather than by
/// reading this.
pub struct Accounts {
    config: GitHubLoginConfig,
    connection: Mutex<rusqlite::Connection>,
}

impl Accounts {
    pub fn open(config: GitHubLoginConfig) -> rusqlite::Result<Self> {
        let connection = rusqlite::Connection::open(&config.db_path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        Ok(Self {
            config,
            connection: Mutex::new(connection),
        })
    }

    pub fn config(&self) -> &GitHubLoginConfig {
        &self.config
    }

    /// Runs the migrations on the connection the server will keep using, so a
    /// router that was handed a database nobody initialized still works and no
    /// request pays for the schema check.
    pub fn initialize_schema(&self) -> rusqlite::Result<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        schema::initialize_connection(&mut connection)
    }

    /// Writes are rare and single-writer here, so one guarded connection beats
    /// a pool. Callers already run on the blocking pool, so holding the lock
    /// across the query parks no async worker.
    ///
    /// The ceiling, so the next person does not have to rediscover it: every
    /// query in the process queues behind every other. These are indexed
    /// point lookups on a small database, so they retire in tens of
    /// microseconds, and it stops being enough when concurrent interviews
    /// times queries per interview approaches that rate. A connection pool is
    /// the answer at that point, not a second mutex.
    pub(crate) fn with<T>(
        &self,
        task: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        task(&connection)
    }
}

pub fn normalized_github_login(value: &str) -> String {
    value.trim().trim_start_matches('@').to_ascii_lowercase()
}

/// Alphanumerics and dashes, alphanumeric at both ends, 39 bytes at most.
///
/// Deliberately looser than what GitHub accepts at signup today, which also
/// forbids consecutive dashes. That rule is not retroactive, and this gates a
/// handle a candidate types about themselves rather than a name this server
/// derives anything from: a legacy account turned away at the door costs an
/// interview, and letting an odd-looking one through costs nothing, because
/// the handle is a label and never a key. Callers that need the stricter form
/// add it themselves; [`crate::config`] does, for a name it puts in a filename.
pub fn valid_github_login(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// rusqlite is synchronous, so every account query runs on the blocking pool
/// instead of parking an async worker on disk I/O.
pub async fn blocking<T, F>(task: F) -> rusqlite::Result<T>
where
    F: FnOnce() -> rusqlite::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .unwrap_or_else(|error| {
            // Callers only need "the query did not happen", but the reason a
            // blocking task died is not a SQL problem and must not vanish into
            // one.
            eprintln!("account database task did not complete: {error}");
            Err(rusqlite::Error::InvalidQuery)
        })
}
