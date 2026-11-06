-- A privacy refusal no longer makes a per-user invite link: the receipt's
-- shared background_queue link covers the member. Such rows end 'privacy'.
-- SQLite cannot alter a CHECK, so the table is rebuilt; 'invited' rows,
-- the old privacy outcome, become 'privacy'.
CREATE TABLE background_add_new (
  request_id TEXT NOT NULL,
  chat_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  role TEXT NOT NULL,
  not_before_ms INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL CHECK (
    state IN ('pending', 'added', 'privacy', 'failed')
  ),
  last_error TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  PRIMARY KEY (chat_id, user_id)
) STRICT;

INSERT INTO background_add_new (
  request_id, chat_id, user_id, role, not_before_ms, attempts, state,
  last_error, created_ms, updated_ms
)
SELECT
  request_id, chat_id, user_id, role, not_before_ms, attempts,
  CASE state WHEN 'invited' THEN 'privacy' ELSE state END,
  last_error, created_ms, updated_ms
FROM background_add;

DROP TABLE background_add;

ALTER TABLE background_add_new RENAME TO background_add;
