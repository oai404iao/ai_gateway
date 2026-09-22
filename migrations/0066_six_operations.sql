-- Startup prepares the fixed-provenance plan before this migration and validates
-- the complete runtime snapshot afterwards, in the same locked transaction.
CREATE TEMP TABLE _operation_old_capabilities ON COMMIT DROP AS SELECT * FROM channel_capabilities;
CREATE TEMP TABLE _operation_old_rules ON COMMIT DROP AS SELECT * FROM model_operation_rules;
CREATE TEMP TABLE _operation_old_tiers ON COMMIT DROP AS SELECT * FROM model_capability_tiers;

-- Retained tombstones may reference deleted pricing models; the split must not
-- reinterpret those unchanged historical references as new assignments.
ALTER TABLE channel_capabilities DISABLE TRIGGER channel_capabilities_enforce_active_test_pricing_model;
ALTER TABLE model_operation_rules DISABLE TRIGGER model_operation_rules_enforce_active_pricing_model;
ALTER TABLE channel_capabilities DISABLE TRIGGER channel_capabilities_preserve_tombstones;
ALTER TABLE channel_capabilities DISABLE TRIGGER channel_capabilities_set_updated_at;
ALTER TABLE upstream_accesses DISABLE TRIGGER upstream_accesses_preserve_tombstones;
ALTER TABLE upstream_accesses DISABLE TRIGGER upstream_accesses_set_updated_at;
ALTER TABLE model_operation_rules DISABLE TRIGGER model_operation_rules_set_updated_at;

ALTER TABLE model_capability_candidates DROP CONSTRAINT model_capability_candidates_tier_fk;
ALTER TABLE model_capability_candidates DROP CONSTRAINT model_capability_candidates_capability_fk;
ALTER TABLE model_capability_tiers DROP CONSTRAINT model_capability_tiers_rule_fk;
ALTER TABLE upstream_accesses DROP CONSTRAINT upstream_accesses_connector_kind_check;
ALTER TABLE upstream_accesses ADD CONSTRAINT upstream_accesses_connector_kind_check
    CHECK (connector_kind IN ('general','codex')) NOT VALID;
UPDATE upstream_accesses SET connector_kind=CASE connector_kind
    WHEN 'openai_compatible' THEN 'general' WHEN 'codex_oauth' THEN 'codex' ELSE connector_kind END;
ALTER TABLE upstream_accesses VALIDATE CONSTRAINT upstream_accesses_connector_kind_check;
ALTER TABLE connector_pools DROP CONSTRAINT connector_pools_connector_kind_check;
UPDATE connector_pools SET connector_kind='codex' WHERE connector_kind='codex_oauth';
ALTER TABLE connector_pools ADD CONSTRAINT connector_pools_connector_kind_check CHECK (connector_kind='codex');

ALTER TABLE channel_capabilities
    DROP CONSTRAINT channel_capabilities_operation_check,
    DROP CONSTRAINT channel_capabilities_transports_check,
    DROP CONSTRAINT channel_capabilities_request_compression_operation_check,
    DROP CONSTRAINT channel_capabilities_probe_check,
    DROP COLUMN transports;
ALTER TABLE model_operation_rules DROP CONSTRAINT model_operation_rules_operation_check;

UPDATE channel_capabilities c SET operation=p->>'operation',
    request_compression=CASE WHEN p->>'operation'='responses-ws' THEN 'default' ELSE c.request_compression END,
    test_model=CASE WHEN p->>'operation'='responses-ws' THEN NULL ELSE c.test_model END,
    test_pricing_model_id=CASE WHEN p->>'operation'='responses-ws' THEN NULL ELSE c.test_pricing_model_id END
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'capabilities') p
WHERE c.id=(p->>'id')::uuid;
INSERT INTO channel_capabilities
SELECT (jsonb_populate_record(NULL::channel_capabilities,
    to_jsonb(source) || p ||
    jsonb_build_object('request_compression','default','test_model',NULL,'test_pricing_model_id',NULL))).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'capabilities') p
JOIN _operation_old_capabilities source ON source.id=(p->>'source_id')::uuid
WHERE p->>'id' <> p->>'source_id';

UPDATE model_operation_rules r SET operation=p->>'operation'
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'rules') p
WHERE r.id=(p->>'id')::uuid;
INSERT INTO model_operation_rules
SELECT (jsonb_populate_record(NULL::model_operation_rules,to_jsonb(source)||p)).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'rules') p
JOIN _operation_old_rules source ON source.id=(p->>'source_id')::uuid
WHERE p->>'id' <> p->>'source_id';

DELETE FROM model_capability_candidates;
DELETE FROM model_capability_tiers;
INSERT INTO model_capability_tiers
SELECT (jsonb_populate_record(NULL::model_capability_tiers,to_jsonb(source)||p)).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'tiers') p
JOIN _operation_old_tiers source ON source.id=(p->>'source_id')::uuid;
INSERT INTO model_capability_candidates
SELECT (jsonb_populate_record(NULL::model_capability_candidates,p)).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'candidates') p;

DELETE FROM api_key_capability_grants;
INSERT INTO api_key_capability_grants
SELECT (jsonb_populate_record(NULL::api_key_capability_grants,p)).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'api_key_grants') p;
DELETE FROM api_key_policy_capability_grants;
INSERT INTO api_key_policy_capability_grants
SELECT (jsonb_populate_record(NULL::api_key_policy_capability_grants,p)).*
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'policy_grants') p;

INSERT INTO channel_identity_registry
    (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id)
SELECT (p->>'id')::uuid,source.label,source.created_at,source.canonical_channel_id,
    source.codex_credential_id,(p->>'id')::uuid
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'capabilities') p
JOIN channel_identity_registry source ON source.id=(p->>'source_id')::uuid
WHERE p->>'id' <> p->>'source_id';
INSERT INTO model_rule_identity_registry (id,label,created_at,canonical_rule_id)
SELECT (p->>'id')::uuid,source.label,source.created_at,(p->>'id')::uuid
FROM _operation_upgrade_plan, jsonb_array_elements(plan->'rules') p
JOIN model_rule_identity_registry source ON source.id=(p->>'source_id')::uuid
WHERE p->>'id' <> p->>'source_id';

ALTER TABLE channel_capabilities
    ADD CONSTRAINT channel_capabilities_operation_check CHECK
        (operation IN ('chat_completion','responses','responses-ws','web_search','images_edit','images_generation')),
    ADD CONSTRAINT channel_capabilities_request_compression_operation_check
        CHECK (request_compression='default' OR operation='responses'),
    ADD CONSTRAINT channel_capabilities_probe_check
        CHECK (test_model IS NULL OR operation IN ('chat_completion','responses'));
ALTER TABLE model_operation_rules ADD CONSTRAINT model_operation_rules_operation_check CHECK
    (operation IN ('chat_completion','responses','responses-ws','web_search','images_edit','images_generation'));
ALTER TABLE model_capability_tiers ADD CONSTRAINT model_capability_tiers_rule_fk
    FOREIGN KEY (rule_id,operation) REFERENCES model_operation_rules(id,operation) ON DELETE CASCADE;
ALTER TABLE model_capability_candidates
    ADD CONSTRAINT model_capability_candidates_tier_fk
        FOREIGN KEY (tier_id,operation) REFERENCES model_capability_tiers(id,operation) ON DELETE CASCADE,
    ADD CONSTRAINT model_capability_candidates_capability_fk
        FOREIGN KEY (capability_id,operation) REFERENCES channel_capabilities(id,operation) ON DELETE RESTRICT;

-- Immutable financial rows and existing query history keep their original wire
-- encoding. Readers normalize it; new writes use only canonical operation names.
ALTER TABLE request_logs DROP CONSTRAINT request_logs_api_operation_format_check;
ALTER TABLE request_logs ADD CONSTRAINT request_logs_api_operation_format_check CHECK (
    (api_format='open_ai_chat_completions' AND api_operation IN ('chat_completion','chat_completions'))
    OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search','standalone_web_search'))
    OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit')));
ALTER TABLE request_metering_facts DROP CONSTRAINT request_metering_operation_format_check;
ALTER TABLE request_metering_facts ADD CONSTRAINT request_metering_operation_format_check CHECK (
    (api_format='open_ai_chat_completions' AND api_operation IN ('chat_completion','chat_completions'))
    OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search','standalone_web_search'))
    OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit')));

ALTER TABLE request_metering_facts ALTER COLUMN amount_state SET EXPRESSION AS (
    CASE
        WHEN outcome IN ('failed','cancelled') THEN 'zero_by_policy'
        WHEN cost_amount IS NULL AND (outcome='rejected' OR api_operation IN ('standalone_web_search','web_search')) THEN 'not_applicable'
        WHEN cost_amount IS NULL THEN 'unknown'
        WHEN model_id IS NOT NULL AND currency IS NOT NULL THEN 'priced'
        ELSE 'invalid'
    END
);

ALTER TABLE channel_capabilities ENABLE TRIGGER channel_capabilities_enforce_active_test_pricing_model;
ALTER TABLE model_operation_rules ENABLE TRIGGER model_operation_rules_enforce_active_pricing_model;
ALTER TABLE channel_capabilities ENABLE TRIGGER channel_capabilities_preserve_tombstones;
ALTER TABLE channel_capabilities ENABLE TRIGGER channel_capabilities_set_updated_at;
ALTER TABLE upstream_accesses ENABLE TRIGGER upstream_accesses_preserve_tombstones;
ALTER TABLE upstream_accesses ENABLE TRIGGER upstream_accesses_set_updated_at;
ALTER TABLE model_operation_rules ENABLE TRIGGER model_operation_rules_set_updated_at;
