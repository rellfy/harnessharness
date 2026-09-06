CREATE TABLE message (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL
);

CREATE INDEX message_session_id ON message (session_id, id);
