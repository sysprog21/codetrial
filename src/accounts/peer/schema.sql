CREATE TABLE peer_sessions (
 id TEXT PRIMARY KEY, creator_id INTEGER NOT NULL REFERENCES users(id),
 status TEXT NOT NULL, version INTEGER NOT NULL, settings_json TEXT NOT NULL,
 state_json TEXT NOT NULL, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
 provider_id TEXT, room_name TEXT, room_generation INTEGER NOT NULL DEFAULT 0,
 hard_deadline INTEGER, outcome TEXT, room_deleted_at INTEGER,
 delete_attempts INTEGER NOT NULL DEFAULT 0, next_delete_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE peer_members (
 session_id TEXT NOT NULL REFERENCES peer_sessions(id) ON DELETE CASCADE,
 slot INTEGER NOT NULL CHECK(slot IN (0,1)), user_id INTEGER NOT NULL REFERENCES users(id),
 participant_identity TEXT NOT NULL, entered_at INTEGER, last_seen_at INTEGER,
 PRIMARY KEY(session_id,slot), UNIQUE(session_id,user_id)
);
CREATE TABLE peer_invites (
 token_hash TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES peer_sessions(id) ON DELETE CASCADE,
 expires_at INTEGER NOT NULL, revoked_at INTEGER, consumed_by INTEGER REFERENCES users(id), consumed_at INTEGER
);
CREATE TABLE peer_active_users (
 user_id INTEGER PRIMARY KEY REFERENCES users(id), session_id TEXT NOT NULL REFERENCES peer_sessions(id) ON DELETE CASCADE
);
CREATE TABLE peer_rounds (
 session_id TEXT NOT NULL REFERENCES peer_sessions(id) ON DELETE CASCADE,
 idx INTEGER NOT NULL CHECK(idx IN (0,1)), interviewer_slot INTEGER NOT NULL,
 candidate_slot INTEGER NOT NULL, problem_id TEXT NOT NULL, status TEXT NOT NULL,
 version INTEGER NOT NULL, started_at INTEGER, deadline INTEGER, sealed_at INTEGER,
 writer_lease TEXT, writer_lease_until INTEGER, final_capture_incomplete INTEGER NOT NULL DEFAULT 0,
 state_json TEXT NOT NULL, PRIMARY KEY(session_id,idx)
);
CREATE TABLE peer_snapshots (
 session_id TEXT NOT NULL, round_idx INTEGER NOT NULL, revision INTEGER NOT NULL,
 language TEXT NOT NULL, code TEXT NOT NULL, test_summary_json TEXT NOT NULL, updated_at INTEGER NOT NULL,
 FOREIGN KEY(session_id,round_idx) REFERENCES peer_rounds(session_id,idx) ON DELETE CASCADE,
 PRIMARY KEY(session_id,round_idx)
);
CREATE TABLE peer_feedback (
 session_id TEXT NOT NULL, round_idx INTEGER NOT NULL, author_slot INTEGER NOT NULL,
 kind TEXT NOT NULL, rubric_version TEXT NOT NULL, payload_json TEXT NOT NULL, submitted_at INTEGER,
 FOREIGN KEY(session_id,round_idx) REFERENCES peer_rounds(session_id,idx) ON DELETE CASCADE,
 PRIMARY KEY(session_id,round_idx,author_slot)
);
CREATE TABLE peer_problem_exposures (
 user_id INTEGER NOT NULL REFERENCES users(id), problem_id TEXT NOT NULL,
 kind TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(user_id,problem_id,kind)
);
CREATE INDEX peer_sessions_cleanup ON peer_sessions(status,next_delete_at);
CREATE INDEX peer_members_user ON peer_members(user_id,session_id);
