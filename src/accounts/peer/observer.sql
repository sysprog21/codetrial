CREATE TABLE peer_observations (
 session_id TEXT PRIMARY KEY REFERENCES peer_sessions(id) ON DELETE CASCADE,
 status TEXT NOT NULL, error TEXT, advice_json TEXT NOT NULL DEFAULT '[]',
 created_at INTEGER NOT NULL, completed_at INTEGER,
 last_seen_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, transcript_withdrawn INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE peer_transcripts (
 session_id TEXT NOT NULL REFERENCES peer_sessions(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL, slot INTEGER NOT NULL CHECK(slot IN (0,1)), round_idx INTEGER,
 started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, text TEXT NOT NULL,
 PRIMARY KEY(session_id,seq)
);
