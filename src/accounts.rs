//! SQLite account storage, independent of HTTP handlers.
//! Re-exports keep callers independent of the session, interview, and report
//! modules.

pub mod interviews;
pub(crate) mod peer;
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

/// No Debug: signing secrets and OAuth credentials must stay out of logs.
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

/// No Debug: printing the client id must not expose its adjacent secret.
#[derive(Clone, PartialEq, Eq)]
pub struct GitHubOauth {
    pub client_id: String,
    pub client_secret: String,
}

/// The row in `users`, keyed by the local id every other table references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedInUser {
    pub id: i64,
    pub identity_verified: bool,
    pub login: String,
    pub avatar_url: Option<String>,
    /// The primary verified address GitHub gave us at sign-in, and only that.
    /// `None` for every self-declared login, which is the whole point: a typed
    /// handle is a label, and a recording is delivered to a person.
    pub verified_email: Option<String>,
    /// Stable account identity stored in `users.github_id`.
    /// Positive values are GitHub account IDs. Negative values are fresh
    /// random identities for self-declared accounts. Unlike `users.id`, this
    /// value is not recycled when a throwaway account is deleted.
    pub account_identity: i64,
    /// GitHub's numeric account ID, for a login GitHub vouched for; `None` for
    /// a self-declared one.
    pub github_id: Option<u64>,
}

/// Login data: positive ids come from OAuth; negative ids are self-declared.
/// `github_id` is distinct from the local `users.id`.
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

/// One connection per server, with foreign keys enabled on every query.
/// Queries run on the blocking pool and serialize through this connection.
pub struct Accounts {
    config: GitHubLoginConfig,

    // A poisoned lock is recovered: a `transaction` that panics rolls back as
    // it unwinds. A `with` closure that panics between two plain statements
    // would not, so a write of more than one statement belongs in
    // `transaction`.
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

    /// Serializes this process's queries; callers use the blocking pool.
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

    /// Reserves the writer before reads that control a write, across server
    /// processes.
    pub(crate) fn transaction<T>(
        &self,
        task: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        self.with(|connection| {
            let transaction = rusqlite::Transaction::new_unchecked(
                connection,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let result = task(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
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
