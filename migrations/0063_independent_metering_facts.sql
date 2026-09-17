-- Freeze legacy claims before taking their financial snapshot.
LOCK TABLE request_logs, request_log_ingest IN ACCESS EXCLUSIVE MODE;

CREATE TABLE request_metering_facts (
    id uuid PRIMARY KEY,
    started_at timestamptz NOT NULL,
    completed_at timestamptz NOT NULL CHECK (completed_at >= started_at),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    api_key_id uuid NOT NULL REFERENCES api_keys(id) ON DELETE RESTRICT,
    request_source text NOT NULL CHECK (request_source IN ('client','scheduled_test')),
    api_format api_format NOT NULL,
    api_operation text NOT NULL,
    request_protocol text NOT NULL CHECK (request_protocol IN ('non_stream','sse','websocket')),
    client_model varchar(300) NOT NULL,
    upstream_model varchar(300),
    model_rule_id uuid REFERENCES model_rules(id) ON DELETE RESTRICT,
    channel_group_id uuid REFERENCES channel_groups(id) ON DELETE RESTRICT,
    channel_id uuid REFERENCES channels(id) ON DELETE RESTRICT,
    model_id uuid REFERENCES models(id) ON DELETE RESTRICT,
    outcome text NOT NULL CHECK (outcome IN ('succeeded','failed','rejected','cancelled')),
    input_tokens bigint CHECK (input_tokens >= 0),
    cached_input_tokens bigint CHECK (cached_input_tokens >= 0 AND cached_input_tokens <= input_tokens),
    cache_write_tokens bigint CHECK (cache_write_tokens >= 0 AND cache_write_tokens <= input_tokens),
    output_tokens bigint CHECK (output_tokens >= 0),
    reasoning_tokens bigint CHECK (reasoning_tokens >= 0 AND reasoning_tokens <= output_tokens),
    currency char(3) CHECK (currency = 'USD'),
    price_unit_tokens bigint CHECK (price_unit_tokens > 0),
    price_effective_at timestamptz,
    input_unit_price numeric(24,12) CHECK (input_unit_price >= 0),
    cached_input_unit_price numeric(24,12) CHECK (cached_input_unit_price >= 0),
    cache_write_unit_price numeric(24,12) CHECK (cache_write_unit_price >= 0),
    output_unit_price numeric(24,12) CHECK (output_unit_price >= 0),
    cost_amount numeric(24,8) CHECK (cost_amount >= 0),
    peak_pricing boolean NOT NULL,
    amount_state text GENERATED ALWAYS AS (
        CASE
            WHEN outcome IN ('failed','cancelled') THEN 'zero_by_policy'
            WHEN cost_amount IS NULL AND (outcome = 'rejected' OR api_operation = 'standalone_web_search')
                THEN 'not_applicable'
            WHEN cost_amount IS NULL THEN 'unknown'
            WHEN model_id IS NOT NULL AND currency IS NOT NULL THEN 'priced'
            ELSE 'invalid'
        END
    ) STORED NOT NULL,
    CONSTRAINT request_metering_prices_check CHECK (
        (currency IS NULL AND price_unit_tokens IS NULL AND price_effective_at IS NULL
            AND input_unit_price IS NULL AND cached_input_unit_price IS NULL
            AND cache_write_unit_price IS NULL AND output_unit_price IS NULL)
        OR
        (currency IS NOT NULL AND price_unit_tokens IS NOT NULL AND price_effective_at IS NOT NULL
            AND input_unit_price IS NOT NULL AND cached_input_unit_price IS NOT NULL
            AND cache_write_unit_price IS NOT NULL AND output_unit_price IS NOT NULL)
    ),
    CONSTRAINT request_metering_zero_policy_check CHECK (
        outcome NOT IN ('failed','cancelled') OR (cost_amount IS NOT NULL AND cost_amount = 0)
    ),
    CHECK (cached_input_tokens IS NULL OR input_tokens IS NOT NULL),
    CHECK (cache_write_tokens IS NULL OR input_tokens IS NOT NULL),
    CHECK (reasoning_tokens IS NULL OR output_tokens IS NOT NULL),
    CONSTRAINT request_metering_operation_format_check CHECK (
        (api_format='open_ai_chat_completions' AND api_operation='chat_completions')
        OR (api_format='open_ai_responses' AND api_operation IN ('responses','standalone_web_search'))
        OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit'))
    )
);

INSERT INTO request_metering_facts (
    id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
    request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,
    model_id,outcome,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,
    reasoning_tokens,currency,price_unit_tokens,price_effective_at,input_unit_price,
    cached_input_unit_price,cache_write_unit_price,output_unit_price,cost_amount,peak_pricing
)
SELECT id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
       request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,
       model_id,outcome,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,
       reasoning_tokens,currency,price_unit_tokens,price_effective_at,input_unit_price,
       cached_input_unit_price,cache_write_unit_price,output_unit_price,cost_amount,peak_pricing
FROM request_logs;

CREATE TABLE request_settlements (
    request_id uuid PRIMARY KEY REFERENCES request_metering_facts(id) ON DELETE RESTRICT,
    cost_amount numeric(24,8) NOT NULL CHECK (cost_amount >= 0),
    currency char(3),
    settled_at timestamptz NOT NULL DEFAULT now(),
    policy_version smallint NOT NULL DEFAULT 1 CHECK (policy_version = 1)
);

CREATE FUNCTION validate_request_settlement() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    fact request_metering_facts%ROWTYPE;
BEGIN
    SELECT * INTO fact FROM request_metering_facts WHERE id = NEW.request_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'settlement fact is missing' USING ERRCODE = '23503';
    END IF;
    IF fact.amount_state NOT IN ('priced','zero_by_policy')
        OR NEW.cost_amount IS DISTINCT FROM fact.cost_amount
        OR NEW.currency IS DISTINCT FROM fact.currency
        OR NOT EXISTS (
            SELECT 1 FROM api_keys WHERE id = fact.api_key_id AND user_id = fact.user_id
        ) THEN
        RAISE EXCEPTION 'settlement facts are ineligible'
            USING ERRCODE = '23514', CONSTRAINT = 'request_settlements_eligibility_check';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER request_settlements_validate
BEFORE INSERT ON request_settlements
FOR EACH ROW EXECUTE FUNCTION validate_request_settlement();

INSERT INTO request_settlements (request_id,cost_amount,currency,settled_at)
SELECT id,cost_amount,currency,billed_at FROM request_logs WHERE billed_at IS NOT NULL;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM request_logs AS log
        LEFT JOIN request_metering_facts AS fact ON fact.id = log.id
        LEFT JOIN request_settlements AS receipt ON receipt.request_id = log.id
        WHERE fact.id IS NULL
            OR (to_jsonb(fact)-'amount_state') IS DISTINCT FROM (
                to_jsonb(jsonb_populate_record(NULL::request_metering_facts,to_jsonb(log)))-'amount_state'
            )
            OR receipt.settled_at IS DISTINCT FROM log.billed_at
            OR (log.billed_at IS NOT NULL AND receipt.cost_amount IS DISTINCT FROM log.cost_amount)
    ) THEN
        RAISE EXCEPTION 'metering backfill validation failed';
    END IF;
END;
$$;

CREATE FUNCTION protect_request_financial_facts() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'request financial facts and settlement receipts are immutable';
END;
$$;

CREATE TRIGGER request_metering_facts_immutable
BEFORE UPDATE OR DELETE ON request_metering_facts
FOR EACH ROW EXECUTE FUNCTION protect_request_financial_facts();

CREATE TRIGGER request_settlements_immutable
BEFORE UPDATE OR DELETE ON request_settlements
FOR EACH ROW EXECUTE FUNCTION protect_request_financial_facts();

CREATE TABLE request_settlement_pending (
    request_id uuid PRIMARY KEY REFERENCES request_metering_facts(id) ON DELETE RESTRICT,
    completed_at timestamptz NOT NULL
) WITH (autovacuum_vacuum_scale_factor=0.02, autovacuum_analyze_scale_factor=0.01);
INSERT INTO request_settlement_pending(request_id,completed_at)
SELECT fact.id,fact.completed_at FROM request_metering_facts AS fact
WHERE fact.amount_state IN ('priced','zero_by_policy')
    AND NOT EXISTS (SELECT 1 FROM request_settlements WHERE request_id=fact.id);
CREATE INDEX request_settlement_pending_order_idx ON request_settlement_pending(completed_at,request_id);

CREATE FUNCTION protect_settlement_pending() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' OR NOT EXISTS (
        SELECT 1 FROM request_settlements WHERE request_id=OLD.request_id
    ) THEN
        RAISE EXCEPTION 'unsettled financial work cannot be removed or changed';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER request_settlement_pending_protect
BEFORE UPDATE OR DELETE ON request_settlement_pending
FOR EACH ROW EXECUTE FUNCTION protect_settlement_pending();

CREATE INDEX request_metering_reconciliation_idx ON request_metering_facts(amount_state,id)
WHERE amount_state IN ('unknown','invalid');
CREATE INDEX request_metering_user_time_idx ON request_metering_facts(user_id,started_at);
CREATE INDEX request_metering_key_time_idx ON request_metering_facts(api_key_id,started_at);
CREATE INDEX request_metering_channel_time_idx ON request_metering_facts(channel_id,started_at);
CREATE INDEX request_metering_time_idx ON request_metering_facts(started_at);

ALTER TABLE request_log_ingest
    ADD COLUMN metered_at timestamptz,
    ADD COLUMN metering_attempt_count integer NOT NULL DEFAULT 0,
    ADD COLUMN metering_next_attempt_at timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN metering_last_error_code text;
CREATE INDEX request_log_ingest_metering_idx ON request_log_ingest(sequence)
WHERE metered_at IS NULL;
CREATE INDEX request_log_ingest_metering_retry_idx ON request_log_ingest(metering_next_attempt_at,sequence)
WHERE metered_at IS NULL AND metering_attempt_count > 0;
CREATE INDEX request_log_ingest_projection_idx ON request_log_ingest(sequence)
WHERE metered_at IS NOT NULL;

CREATE FUNCTION protect_unfinished_request_ingress() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.metered_at IS NULL
        OR NOT EXISTS (SELECT 1 FROM request_metering_facts WHERE id=OLD.request_log_id)
        OR NOT EXISTS (SELECT 1 FROM request_logs WHERE id=OLD.request_log_id) THEN
        RAISE EXCEPTION 'request ingress cannot be acknowledged before facts and projection'
            USING ERRCODE = '23514';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER request_log_ingest_ack_guard
BEFORE DELETE ON request_log_ingest
FOR EACH ROW EXECUTE FUNCTION protect_unfinished_request_ingress();

ALTER TABLE request_logs DROP CONSTRAINT request_logs_billed_state_check;
ALTER TABLE request_logs DROP COLUMN billed_at;

CREATE OR REPLACE FUNCTION prevent_log_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'audit_logs' THEN
        RAISE EXCEPTION 'audit_logs are append-only';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'request_logs are immutable projections';
END;
$$;
