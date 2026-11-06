-- General app log rows share the event table. A log row has a level, a short
-- constant msg written by tg-sai code (never user data), and attrs summarized
-- like req/res. Telegram call rows leave all three NULL.
ALTER TABLE event ADD COLUMN level TEXT
  CHECK (level IS NULL OR level IN ('debug', 'info', 'warn', 'error'));
ALTER TABLE event ADD COLUMN msg TEXT;
ALTER TABLE event ADD COLUMN attrs TEXT
  CHECK (attrs IS NULL OR json_valid(attrs));
-- Lets `tg-sai events logs` scan only log rows, newest first.
CREATE INDEX event_level_ts_ms ON event (level, ts_ms) WHERE level IS NOT NULL;
