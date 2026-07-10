CREATE TABLE IF NOT EXISTS rooms (
    id SERIAL PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS room_members (
    room_id INTEGER NOT NULL REFERENCES rooms(id),
    username TEXT NOT NULL,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, username)
);

ALTER TABLE messages ADD COLUMN IF NOT EXISTS room_id INTEGER REFERENCES rooms(id);
CREATE INDEX IF NOT EXISTS idx_messages_room_id ON messages (room_id, created_at DESC);
