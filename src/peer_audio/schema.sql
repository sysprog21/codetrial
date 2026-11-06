CREATE TABLE peer_audio_records (
 session_id TEXT PRIMARY KEY REFERENCES peer_sessions(id) ON DELETE CASCADE,
 status TEXT NOT NULL CHECK(status IN ('recording','ready','failed','deleted')),
 bytes INTEGER NOT NULL DEFAULT 0, duration_ms INTEGER NOT NULL DEFAULT 0,
 created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, purged_at INTEGER
);
CREATE INDEX peer_audio_expiry ON peer_audio_records(expires_at) WHERE purged_at IS NULL;
