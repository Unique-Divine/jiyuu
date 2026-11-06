-- One row per telemetry event: a Telegram call, an attempt, a flood wait, or
-- a run/worker boundary. Field names follow herdr's logging vocabulary.
CREATE TABLE event (
  event_id TEXT PRIMARY KEY NOT NULL,
  request_id TEXT,
  run_id TEXT NOT NULL,
  job_id TEXT,
  ts_ms INTEGER NOT NULL,
  duration_ms INTEGER,
  event TEXT NOT NULL,
  subsystem TEXT NOT NULL,
  command TEXT,
  account_id INTEGER,
  profile TEXT,
  method TEXT,
  attempt INTEGER,
  outcome TEXT NOT NULL
    CHECK (outcome IN ('started', 'ok', 'error', 'timeout', 'aborted')),
  error_code INTEGER,
  err TEXT,
  wait_s INTEGER,
  req TEXT CHECK (req IS NULL OR json_valid(req)),
  res TEXT CHECK (res IS NULL OR json_valid(res)),
  host TEXT,
  pid INTEGER,
  version TEXT
) STRICT;

CREATE INDEX event_account_method_event_ts
  ON event (account_id, method, event, ts_ms);
CREATE INDEX event_request_id ON event (request_id);
CREATE INDEX event_run_id ON event (run_id);
CREATE INDEX event_ts_ms ON event (ts_ms);
