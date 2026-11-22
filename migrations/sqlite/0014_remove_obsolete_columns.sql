ALTER TABLE api_keys DROP COLUMN last_used_at;
ALTER TABLE audit_logs DROP COLUMN source_ip_prefix;

-- The table-level CHECK on attempts requires a rebuild. Copy retained columns once
-- without changing protocol or operation values on historical rows.
CREATE TABLE request_logs_new (
    id TEXT NOT NULL CONSTRAINT request_logs_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    started_at TEXT NOT NULL CONSTRAINT request_logs_started_at_storage CHECK (started_at IS NULL OR (ag_time_valid(started_at))) CHECK (started_at IS NULL OR instr(started_at, char(0))=0),
    completed_at TEXT NOT NULL CONSTRAINT request_logs_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT request_logs_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    api_key_id TEXT NOT NULL CONSTRAINT request_logs_api_key_id_storage CHECK (api_key_id IS NULL OR (ag_uuid_valid(api_key_id))) CHECK (api_key_id IS NULL OR instr(api_key_id, char(0))=0),
    api_format TEXT COLLATE ag_api_format NOT NULL CONSTRAINT request_logs_api_format_storage CHECK (api_format IS NULL OR (api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images'))) CHECK (api_format IS NULL OR instr(api_format, char(0))=0),
    client_model TEXT NOT NULL CONSTRAINT request_logs_client_model_storage CHECK (client_model IS NULL OR (length(client_model) <= 300)) CHECK (client_model IS NULL OR instr(client_model, char(0))=0),
    upstream_model TEXT CONSTRAINT request_logs_upstream_model_storage CHECK (upstream_model IS NULL OR (length(upstream_model) <= 300)) CHECK (upstream_model IS NULL OR instr(upstream_model, char(0))=0),
    model_rule_id TEXT CONSTRAINT request_logs_model_rule_id_storage CHECK (model_rule_id IS NULL OR (ag_uuid_valid(model_rule_id))) CHECK (model_rule_id IS NULL OR instr(model_rule_id, char(0))=0),
    channel_group_id TEXT CONSTRAINT request_logs_channel_group_id_storage CHECK (channel_group_id IS NULL OR (ag_uuid_valid(channel_group_id))) CHECK (channel_group_id IS NULL OR instr(channel_group_id, char(0))=0),
    channel_id TEXT CONSTRAINT request_logs_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id))) CHECK (channel_id IS NULL OR instr(channel_id, char(0))=0),
    outcome TEXT NOT NULL CHECK (outcome IS NULL OR instr(outcome, char(0))=0),
    response_status_code INTEGER CONSTRAINT request_logs_response_status_code_storage CHECK (response_status_code IS NULL OR (response_status_code BETWEEN -32768 AND 32767)),
    ttft_ms INTEGER CONSTRAINT request_logs_ttft_ms_storage CHECK (ttft_ms IS NULL OR (ttft_ms BETWEEN -2147483648 AND 2147483647)),
    total_duration_ms INTEGER CONSTRAINT request_logs_total_duration_ms_storage CHECK (total_duration_ms IS NULL OR (total_duration_ms BETWEEN -2147483648 AND 2147483647)),
    output_tokens_per_second TEXT CONSTRAINT request_logs_output_tokens_per_second_storage CHECK (output_tokens_per_second IS NULL OR (ag_decimal_valid(output_tokens_per_second, 14, 4))) CHECK (output_tokens_per_second IS NULL OR instr(output_tokens_per_second, char(0))=0),
    input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_tokens INTEGER,
    output_tokens INTEGER,
    model_id TEXT CONSTRAINT request_logs_model_id_storage CHECK (model_id IS NULL OR (ag_uuid_valid(model_id))) CHECK (model_id IS NULL OR instr(model_id, char(0))=0),
    currency TEXT CONSTRAINT request_logs_currency_storage CHECK (currency IS NULL OR (length(currency) <= 3)) CHECK (currency IS NULL OR instr(currency, char(0))=0),
    price_unit_tokens INTEGER,
    price_effective_at TEXT CONSTRAINT request_logs_price_effective_at_storage CHECK (price_effective_at IS NULL OR (ag_time_valid(price_effective_at))) CHECK (price_effective_at IS NULL OR instr(price_effective_at, char(0))=0),
    input_unit_price TEXT CONSTRAINT request_logs_input_unit_price_storage CHECK (input_unit_price IS NULL OR (ag_decimal_valid(input_unit_price, 24, 12))) CHECK (input_unit_price IS NULL OR instr(input_unit_price, char(0))=0),
    cached_input_unit_price TEXT CONSTRAINT request_logs_cached_input_unit_price_storage CHECK (cached_input_unit_price IS NULL OR (ag_decimal_valid(cached_input_unit_price, 24, 12))) CHECK (cached_input_unit_price IS NULL OR instr(cached_input_unit_price, char(0))=0),
    cache_write_unit_price TEXT CONSTRAINT request_logs_cache_write_unit_price_storage CHECK (cache_write_unit_price IS NULL OR (ag_decimal_valid(cache_write_unit_price, 24, 12))) CHECK (cache_write_unit_price IS NULL OR instr(cache_write_unit_price, char(0))=0),
    output_unit_price TEXT CONSTRAINT request_logs_output_unit_price_storage CHECK (output_unit_price IS NULL OR (ag_decimal_valid(output_unit_price, 24, 12))) CHECK (output_unit_price IS NULL OR instr(output_unit_price, char(0))=0),
    cost_amount TEXT CONSTRAINT request_logs_cost_amount_storage CHECK (cost_amount IS NULL OR (ag_decimal_valid(cost_amount, 24, 8))) CHECK (cost_amount IS NULL OR instr(cost_amount, char(0))=0),
    error_code TEXT CONSTRAINT request_logs_error_code_storage CHECK (error_code IS NULL OR (length(error_code) <= 100)) CHECK (error_code IS NULL OR instr(error_code, char(0))=0),
    error_summary TEXT CONSTRAINT request_logs_error_summary_storage CHECK (error_summary IS NULL OR (length(error_summary) <= 16384)) CHECK (error_summary IS NULL OR instr(error_summary, char(0))=0),
    request_source TEXT DEFAULT ('client') NOT NULL CHECK (request_source IS NULL OR instr(request_source, char(0))=0),
    request_protocol TEXT DEFAULT ('non_stream') NOT NULL CHECK (request_protocol IS NULL OR instr(request_protocol, char(0))=0),
    reasoning_tokens INTEGER,
    reasoning_effort TEXT CONSTRAINT request_logs_reasoning_effort_storage CHECK (reasoning_effort IS NULL OR (length(reasoning_effort) <= 32)) CHECK (reasoning_effort IS NULL OR instr(reasoning_effort, char(0))=0),
    fast_mode INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_logs_fast_mode_storage CHECK (fast_mode IS NULL OR (fast_mode IN (0,1))),
    api_operation TEXT NOT NULL CHECK (api_operation IS NULL OR instr(api_operation, char(0))=0),
    peak_pricing INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_logs_peak_pricing_storage CHECK (peak_pricing IS NULL OR (peak_pricing IN (0,1))),
    CONSTRAINT request_logs_api_operation_format_check CHECK ((api_format='open_ai_chat_completions' AND api_operation IN ('chat_completion','chat_completions'))
                 OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search','standalone_web_search'))
                 OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit'))),
    CONSTRAINT request_logs_cache_write_tokens_check CHECK ((cache_write_tokens >= 0)),
    CONSTRAINT request_logs_cache_write_unit_price_check CHECK ((ag_decimal_cmp(cache_write_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_cached_input_tokens_check CHECK ((cached_input_tokens >= 0)),
    CONSTRAINT request_logs_cached_input_unit_price_check CHECK ((ag_decimal_cmp(cached_input_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_check CHECK ((completed_at >= started_at)),
    CONSTRAINT request_logs_check1 CHECK (((cached_input_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cached_input_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check2 CHECK (((cache_write_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cache_write_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check3 CHECK ((((currency IS NULL) AND (price_unit_tokens IS NULL) AND (price_effective_at IS NULL) AND (input_unit_price IS NULL) AND (cached_input_unit_price IS NULL) AND (cache_write_unit_price IS NULL) AND (output_unit_price IS NULL)) OR ((currency IS NOT NULL) AND (price_unit_tokens IS NOT NULL) AND (price_effective_at IS NOT NULL) AND (input_unit_price IS NOT NULL) AND (cached_input_unit_price IS NOT NULL) AND (cache_write_unit_price IS NOT NULL) AND (output_unit_price IS NOT NULL)))),
    CONSTRAINT request_logs_cost_amount_check CHECK ((ag_decimal_cmp(cost_amount, '0') >= 0)),
    CONSTRAINT request_logs_currency_usd_only CHECK (((currency IS NULL) OR (currency = 'USD'))),
    CONSTRAINT request_logs_failed_cancelled_zero_cost_check CHECK (((NOT ag_array_contains(json_array('failed', 'cancelled'), outcome)) OR ((cost_amount IS NOT NULL) AND (ag_decimal_cmp(cost_amount, '0') = 0)))),
    CONSTRAINT request_logs_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT request_logs_input_unit_price_check CHECK ((ag_decimal_cmp(input_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_outcome_check CHECK (ag_array_contains(json_array('succeeded', 'failed', 'rejected', 'cancelled'), outcome)),
    CONSTRAINT request_logs_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT request_logs_output_tokens_per_second_check CHECK ((ag_decimal_cmp(output_tokens_per_second, '0') >= 0)),
    CONSTRAINT request_logs_output_unit_price_check CHECK ((ag_decimal_cmp(output_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT request_logs_reasoning_tokens_check CHECK ((reasoning_tokens >= 0)),
    CONSTRAINT request_logs_reasoning_tokens_within_output CHECK (((reasoning_tokens IS NULL) OR ((output_tokens IS NOT NULL) AND (reasoning_tokens <= output_tokens)))),
    CONSTRAINT request_logs_request_protocol_check CHECK (ag_array_contains(json_array('non_stream', 'sse', 'websocket'), request_protocol)),
    CONSTRAINT request_logs_request_source_check CHECK (ag_array_contains(json_array('client', 'scheduled_test'), request_source)),
    CONSTRAINT request_logs_response_status_code_check CHECK (((response_status_code >= 100) AND (response_status_code <= 599))),
    CONSTRAINT request_logs_total_duration_ms_check CHECK ((total_duration_ms >= 0)),
    CONSTRAINT request_logs_ttft_ms_check CHECK ((ttft_ms >= 0)),
    CONSTRAINT request_logs_pkey PRIMARY KEY (id),
    CONSTRAINT request_logs_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES api_keys (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES group_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES channel_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_model_id_fkey FOREIGN KEY (model_id) REFERENCES models (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_model_rule_id_fkey FOREIGN KEY (model_rule_id) REFERENCES model_rule_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

INSERT INTO request_logs_new (rowid, id, started_at, completed_at, user_id, api_key_id, api_format,
    client_model, upstream_model, model_rule_id, channel_group_id, channel_id, outcome,
    response_status_code, ttft_ms, total_duration_ms, output_tokens_per_second, input_tokens,
    cached_input_tokens, cache_write_tokens, output_tokens, model_id, currency, price_unit_tokens,
    price_effective_at, input_unit_price, cached_input_unit_price, cache_write_unit_price,
    output_unit_price, cost_amount, error_code, error_summary, request_source, request_protocol,
    reasoning_tokens, reasoning_effort, fast_mode, api_operation, peak_pricing)
SELECT rowid, id, started_at, completed_at, user_id, api_key_id, api_format, client_model, upstream_model,
    model_rule_id, channel_group_id, channel_id, outcome, response_status_code, ttft_ms, total_duration_ms,
    output_tokens_per_second, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
    model_id, currency, price_unit_tokens, price_effective_at, input_unit_price, cached_input_unit_price,
    cache_write_unit_price, output_unit_price, cost_amount, error_code, error_summary, request_source,
    request_protocol, reasoning_tokens, reasoning_effort, fast_mode, api_operation, peak_pricing
FROM request_logs;

DROP TRIGGER request_log_ingest_ack_guard;
DROP TABLE request_logs;
ALTER TABLE request_logs_new RENAME TO request_logs;

CREATE INDEX request_logs_api_format_started_at_idx ON request_logs (api_format, started_at DESC, id DESC);
CREATE INDEX request_logs_api_key_id_started_at_idx ON request_logs (api_key_id, started_at DESC);
CREATE INDEX request_logs_channel_group_model_started_at_idx ON request_logs (channel_group_id, upstream_model, started_at DESC) WHERE (channel_group_id IS NOT NULL);
CREATE INDEX request_logs_channel_id_started_at_idx ON request_logs (channel_id, started_at DESC);
CREATE INDEX request_logs_channel_model_started_at_idx ON request_logs (channel_id, upstream_model, started_at DESC) WHERE (channel_id IS NOT NULL);
CREATE INDEX request_logs_client_model_started_at_idx ON request_logs (client_model, started_at DESC, id DESC);
CREATE INDEX request_logs_failed_started_at_idx ON request_logs (started_at DESC) WHERE (outcome = 'failed');
CREATE INDEX request_logs_outcome_started_at_idx ON request_logs (outcome, started_at DESC, id DESC);
CREATE INDEX request_logs_scheduled_test_started_at_idx ON request_logs (started_at DESC, id DESC) WHERE (request_source = 'scheduled_test');
CREATE INDEX request_logs_started_at_id_idx ON request_logs (started_at DESC, id DESC);
CREATE INDEX request_logs_upstream_model_started_at_idx ON request_logs (upstream_model, started_at DESC, id DESC);
CREATE INDEX request_logs_user_id_started_at_idx ON request_logs (user_id, started_at DESC);

CREATE TRIGGER request_logs_immutable_update BEFORE UPDATE ON request_logs BEGIN
    SELECT RAISE(ABORT, 'request_logs_immutable_update');
END;
CREATE TRIGGER request_log_ingest_ack_guard BEFORE DELETE ON request_log_ingest
WHEN OLD.metered_at IS NULL
  OR NOT EXISTS (SELECT 1 FROM request_metering_facts WHERE id=OLD.request_log_id)
  OR NOT EXISTS (SELECT 1 FROM request_logs WHERE id=OLD.request_log_id) BEGIN
    SELECT RAISE(ABORT, 'request_log_ingest_ack_guard');
END;
