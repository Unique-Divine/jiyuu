-- One active Telegram flood hold per account and method group. Writers
-- upsert with MAX(until_ms), so a shorter wait never shortens a hold.
CREATE TABLE flood_hold (
  account_id TEXT NOT NULL,
  grp TEXT NOT NULL,
  until_ms INTEGER NOT NULL,
  method TEXT NOT NULL,
  wait_s INTEGER,
  source TEXT NOT NULL CHECK (
    source IN ('flood_wait', 'flood_premium_wait', 'peer_flood', 'manual')
  ),
  request_id TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  PRIMARY KEY (account_id, grp)
) STRICT;
