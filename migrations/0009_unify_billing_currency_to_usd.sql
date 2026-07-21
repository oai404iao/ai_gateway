-- Billing is USD-only. This migration deliberately refuses to relabel
-- historical non-USD amounts because the gateway does not perform currency
-- conversion.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM users WHERE currency <> 'USD')
       OR EXISTS (SELECT 1 FROM models WHERE currency <> 'USD')
       OR EXISTS (
            SELECT 1
            FROM request_logs
            WHERE currency IS NOT NULL
              AND currency <> 'USD'
       )
    THEN
        RAISE EXCEPTION
            'USD-only migration found non-USD data; convert it outside ai-gateway before retrying';
    END IF;
END;
$$;

-- Account balances no longer carry a per-user currency setting.
ALTER TABLE users DROP COLUMN currency;

-- Price snapshots retain an explicit currency only as immutable billing
-- metadata. Both model prices and request-log snapshots are constrained to
-- the system-wide USD settlement currency.
ALTER TABLE models
    ALTER COLUMN currency SET DEFAULT 'USD',
    ADD CONSTRAINT models_currency_usd_only CHECK (currency = 'USD');

ALTER TABLE request_logs
    ADD CONSTRAINT request_logs_currency_usd_only
        CHECK (currency IS NULL OR currency = 'USD');

DROP INDEX request_logs_unbilled_settlement_idx;
CREATE INDEX request_logs_unbilled_settlement_idx
    ON request_logs (completed_at, id)
    WHERE billed_at IS NULL
      AND cost_amount IS NOT NULL;
