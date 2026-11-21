-- The hosted service owns workflow state. This local outbox holds the
-- last result until its cloud receipt is acknowledged.
CREATE TABLE managed_deal_chat_pool_receipt (
  account_id INTEGER PRIMARY KEY,
  result_json TEXT NOT NULL CHECK (json_valid(result_json))
) STRICT;
