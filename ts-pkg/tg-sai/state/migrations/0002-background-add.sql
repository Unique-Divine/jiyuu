-- Optional members of a ready chat, added later by the worker one at a time
-- with pacing and a daily cap. One row per chat and user.
CREATE TABLE background_add (
  request_id TEXT NOT NULL,
  chat_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  role TEXT NOT NULL,
  not_before_ms INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL CHECK (
    state IN ('pending', 'added', 'invited', 'failed')
  ),
  last_error TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  PRIMARY KEY (chat_id, user_id)
) STRICT;
