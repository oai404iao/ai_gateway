-- A route's price source and its forwarded upstream model are one entity.
-- Legacy rows may have pointed at a priced model while forwarding a different
-- `upstream_model`. Preserve each legacy forwarding identity by creating a
-- matching model row when it does not already exist, then make rules point to
-- that row before removing the redundant column.

WITH missing_upstream_models AS (
    SELECT DISTINCT ON (rule.upstream_model)
        rule.upstream_model AS source_model_id,
        model.display_name,
        model.provider_name,
        model.enabled,
        model.currency,
        model.price_unit_tokens,
        model.input_unit_price,
        model.cached_input_unit_price,
        model.cache_write_unit_price,
        model.output_unit_price,
        model.price_effective_at,
        model.source_payload,
        model.last_synced_at
    FROM model_rules AS rule
    JOIN models AS model ON model.id = rule.model_id
    WHERE NOT EXISTS (
        SELECT 1
        FROM models AS existing
        WHERE existing.source_model_id = rule.upstream_model
    )
    ORDER BY rule.upstream_model, rule.id
)
INSERT INTO models (
    id,
    source_model_id,
    display_name,
    provider_name,
    enabled,
    currency,
    price_unit_tokens,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    price_effective_at,
    source_payload,
    last_synced_at
)
SELECT
    (
        substr(md5('ai-gateway-upstream-model:' || source_model_id), 1, 8)
        || '-'
        || substr(md5('ai-gateway-upstream-model:' || source_model_id), 9, 4)
        || '-'
        || substr(md5('ai-gateway-upstream-model:' || source_model_id), 13, 4)
        || '-'
        || substr(md5('ai-gateway-upstream-model:' || source_model_id), 17, 4)
        || '-'
        || substr(md5('ai-gateway-upstream-model:' || source_model_id), 21, 12)
    )::uuid,
    source_model_id,
    display_name,
    provider_name,
    enabled,
    currency,
    price_unit_tokens,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    price_effective_at,
    source_payload,
    last_synced_at
FROM missing_upstream_models
ON CONFLICT (source_model_id) DO NOTHING;

UPDATE model_rules AS rule
SET model_id = model.id
FROM models AS model
WHERE model.source_model_id = rule.upstream_model
  AND rule.model_id <> model.id;

ALTER TABLE model_rules RENAME COLUMN model_id TO upstream_model_id;
ALTER TABLE model_rules DROP COLUMN upstream_model;

DO $$
BEGIN
    ALTER TABLE model_rules
        RENAME CONSTRAINT model_rules_model_id_fkey TO model_rules_upstream_model_id_fkey;
EXCEPTION
    WHEN undefined_object THEN NULL;
END;
$$;

CREATE INDEX request_logs_started_at_id_idx
    ON request_logs (started_at DESC, id DESC);
CREATE INDEX request_logs_api_format_started_at_idx
    ON request_logs (api_format, started_at DESC, id DESC);
CREATE INDEX request_logs_outcome_started_at_idx
    ON request_logs (outcome, started_at DESC, id DESC);
CREATE INDEX request_logs_client_model_started_at_idx
    ON request_logs (client_model, started_at DESC, id DESC);
CREATE INDEX request_logs_upstream_model_started_at_idx
    ON request_logs (upstream_model, started_at DESC, id DESC);
