-- Member adds the custody account made: setup jobs and background adds.
-- `addsToday` and the background daily cap count rows in the current UTC
-- day. A privacy refusal or flood invites nobody and gets no row.
CREATE TABLE member_add (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL CHECK (source IN ('setup', 'background')),
  outcome TEXT NOT NULL CHECK (outcome IN ('added', 'refused')),
  chat_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX member_add_at ON member_add (at_ms);

-- Background adds already finished keep counting toward today's cap.
INSERT INTO member_add (source, outcome, chat_id, user_id, at_ms)
SELECT
  'background',
  CASE state WHEN 'added' THEN 'added' ELSE 'refused' END,
  chat_id, user_id, updated_ms
FROM background_add
WHERE state IN ('added', 'failed');
