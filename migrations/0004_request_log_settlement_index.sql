-- Durable settlement recovery scans only logs whose immutable cost facts are
-- complete enough to acquire `billed_at`. Currency and API-key ownership are
-- checked again by the transactional claim.
CREATE INDEX request_logs_unbilled_settlement_idx
    ON request_logs (completed_at, id)
    WHERE billed_at IS NULL
      AND cost_amount IS NOT NULL
      AND currency IS NOT NULL;
