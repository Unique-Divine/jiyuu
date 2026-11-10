-- An add call that timed out or got a Telegram server error may still have
-- invited the user, so it counts toward today's adds as 'uncertain'.
-- SQLite cannot alter a CHECK, so the table is rebuilt.
CREATE TABLE member_add_new (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL CHECK (source IN ('setup', 'background')),
  outcome TEXT NOT NULL CHECK (outcome IN ('added', 'refused', 'uncertain')),
  chat_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  at_ms INTEGER NOT NULL
) STRICT;

INSERT INTO member_add_new (id, source, outcome, chat_id, user_id, at_ms)
SELECT id, source, outcome, chat_id, user_id, at_ms FROM member_add;

DROP TABLE member_add;

ALTER TABLE member_add_new RENAME TO member_add;

CREATE INDEX member_add_at ON member_add (at_ms);
