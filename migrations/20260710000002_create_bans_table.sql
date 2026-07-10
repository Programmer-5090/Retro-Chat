CREATE TABLE IF NOT EXISTS bans (
    username TEXT PRIMARY KEY,
    banned_by TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
