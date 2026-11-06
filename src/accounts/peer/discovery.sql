CREATE TABLE peer_availability (
 availability_id TEXT PRIMARY KEY,
 user_id INTEGER NOT NULL REFERENCES users(id),
 status TEXT NOT NULL CHECK(status IN ('provisioning','available','paired','expired')),
 settings_json TEXT NOT NULL,
 provider_id TEXT NOT NULL,
 room_name TEXT NOT NULL UNIQUE,
 participant_identity TEXT NOT NULL,
 created_at INTEGER NOT NULL,
 expires_at INTEGER NOT NULL,
 session_id TEXT REFERENCES peer_sessions(id),
 room_deleted_at INTEGER,
 delete_attempts INTEGER NOT NULL DEFAULT 0,
 next_delete_at INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX peer_availability_one_active_user
 ON peer_availability(user_id) WHERE status IN ('provisioning','available');
CREATE INDEX peer_availability_expiry ON peer_availability(status,expires_at);
CREATE TABLE peer_pair_requests (
 request_id TEXT PRIMARY KEY,
 availability_id TEXT NOT NULL REFERENCES peer_availability(availability_id),
 from_user_id INTEGER NOT NULL REFERENCES users(id),
 to_user_id INTEGER NOT NULL REFERENCES users(id),
 status TEXT NOT NULL CHECK(status IN ('pending','paired','declined','expired')),
 settings_json TEXT NOT NULL,
 from_confirmed_at INTEGER,
 to_confirmed_at INTEGER,
 created_at INTEGER NOT NULL,
 expires_at INTEGER NOT NULL,
 session_id TEXT REFERENCES peer_sessions(id)
);
CREATE INDEX peer_pair_requests_members ON peer_pair_requests(from_user_id,to_user_id,status);
CREATE INDEX peer_pair_requests_to ON peer_pair_requests(to_user_id,status,created_at);
CREATE UNIQUE INDEX peer_pair_requests_one_outgoing
 ON peer_pair_requests(from_user_id) WHERE status='pending';
CREATE TABLE peer_profiles (
 user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
 nickname TEXT NOT NULL,
 updated_at INTEGER NOT NULL
);
