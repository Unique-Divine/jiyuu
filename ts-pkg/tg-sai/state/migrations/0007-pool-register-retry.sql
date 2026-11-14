-- When a pool fill's current run of Deal Desk registration retries began,
-- so a registration that keeps failing raises an alert after a day.
ALTER TABLE pool_fill ADD COLUMN register_retry_since_ms INTEGER;
