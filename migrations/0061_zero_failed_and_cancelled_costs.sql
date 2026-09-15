-- Failed and cancelled requests are free under the current billing policy.
-- Reverse charges that were already settled before normalizing history.
CREATE TEMP TABLE request_log_non_success_refunds ON COMMIT DROP AS
SELECT
    user_id,
    api_key_id,
    sum(cost_amount) AS amount
FROM request_logs
WHERE outcome IN ('failed', 'cancelled')
  AND billed_at IS NOT NULL
  AND cost_amount > 0
GROUP BY user_id, api_key_id;

DO $$
DECLARE
    invalid_api_key_id uuid;
BEGIN
    SELECT refund.api_key_id
    INTO invalid_api_key_id
    FROM request_log_non_success_refunds AS refund
    LEFT JOIN api_keys AS api_key
      ON api_key.id = refund.api_key_id
     AND api_key.user_id = refund.user_id
    WHERE api_key.id IS NULL
    ORDER BY refund.api_key_id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'failed-request refund has no matching API Key owner for %s',
                invalid_api_key_id
            ),
            HINT = 'Reconcile request-log ownership before retrying migration 0061.';
    END IF;

    SELECT api_key.id
    INTO invalid_api_key_id
    FROM api_keys AS api_key
    JOIN request_log_non_success_refunds AS refund
      ON refund.api_key_id = api_key.id
     AND refund.user_id = api_key.user_id
    WHERE api_key.quota_used_amount < refund.amount
    ORDER BY api_key.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'failed-request refund exceeds the accumulated quota for API Key %s',
                invalid_api_key_id
            ),
            HINT = 'Reconcile the API Key quota before retrying migration 0061.';
    END IF;
END;
$$;

UPDATE users AS account
SET balance_amount = account.balance_amount + refund.amount
FROM (
    SELECT user_id, sum(amount) AS amount
    FROM request_log_non_success_refunds
    GROUP BY user_id
) AS refund
WHERE account.id = refund.user_id;

UPDATE api_keys AS api_key
SET quota_used_amount = api_key.quota_used_amount - refund.amount
FROM request_log_non_success_refunds AS refund
WHERE api_key.id = refund.api_key_id
  AND api_key.user_id = refund.user_id;

ALTER TABLE request_logs DISABLE TRIGGER request_logs_prevent_mutation;
UPDATE request_logs
SET cost_amount = 0
WHERE outcome IN ('failed', 'cancelled')
  AND cost_amount IS DISTINCT FROM 0;
ALTER TABLE request_logs ENABLE TRIGGER request_logs_prevent_mutation;

ALTER TABLE request_logs
    DROP CONSTRAINT request_logs_check4,
    ADD CONSTRAINT request_logs_failed_cancelled_zero_cost_check
        CHECK (
            outcome NOT IN ('failed', 'cancelled')
            OR (cost_amount IS NOT NULL AND cost_amount = 0)
        ),
    ADD CONSTRAINT request_logs_billed_state_check
        CHECK (
            billed_at IS NULL
            OR (
                outcome IN ('failed', 'cancelled')
                AND cost_amount = 0
            )
            OR (
                cost_amount IS NOT NULL
                AND model_id IS NOT NULL
                AND currency IS NOT NULL
                AND price_unit_tokens IS NOT NULL
                AND price_effective_at IS NOT NULL
                AND input_unit_price IS NOT NULL
                AND cached_input_unit_price IS NOT NULL
                AND cache_write_unit_price IS NOT NULL
                AND output_unit_price IS NOT NULL
            )
        );
