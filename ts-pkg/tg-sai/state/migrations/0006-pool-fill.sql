-- Pool chats this host made for Deal Desk's deal chat pool (boku#77).
-- Deal Desk owns the pool; this is only the local recovery journal for one
-- fill: the chat, its stage, and the bot add, written before each call so a
-- crash or lost response never leads to a second create or add.
CREATE TABLE pool_fill (
  ref TEXT PRIMARY KEY CHECK (length(ref) = 8),
  account_id INTEGER NOT NULL,
  stage TEXT NOT NULL CHECK (
    stage IN (
      'creating', 'creation_unknown', 'created', 'bot_added', 'promoted',
      'photo_set', 'registered'
    )
  ),
  status TEXT NOT NULL CHECK (
    status IN ('active', 'failed', 'done', 'retired')
  ),
  chat_id INTEGER,
  bot_adds INTEGER NOT NULL DEFAULT 0,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  next_attempt_ms INTEGER NOT NULL,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX pool_fill_status ON pool_fill (account_id, status);

-- Earliest time the next pool chat may be created, per account.
CREATE TABLE pool_pace (
  account_id INTEGER PRIMARY KEY,
  next_fill_ms INTEGER NOT NULL
) STRICT;

-- Pool fills add the bot: member_add gains source 'pool'. SQLite cannot
-- alter a CHECK, so the table is rebuilt.
CREATE TABLE member_add_new (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL CHECK (source IN ('setup', 'background', 'pool')),
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
