-- Business guards are database constraints, not optional repository validation.
CREATE TRIGGER request_settlements_validate BEFORE INSERT ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_eligibility_check')
    WHERE NOT EXISTS (
        SELECT 1 FROM request_metering_facts f JOIN api_keys k ON k.id=f.api_key_id AND k.user_id=f.user_id
        WHERE f.id=NEW.request_id AND f.amount_state IN ('priced','zero_by_policy')
          AND f.cost_amount IS NEW.cost_amount AND f.currency IS NEW.currency
    );
END;
CREATE TRIGGER request_settlement_pending_update BEFORE UPDATE ON request_settlement_pending BEGIN
    SELECT RAISE(ABORT, 'request_settlement_pending_protect');
END;
CREATE TRIGGER request_settlement_pending_delete BEFORE DELETE ON request_settlement_pending
WHEN NOT EXISTS (SELECT 1 FROM request_settlements WHERE request_id=OLD.request_id) BEGIN
    SELECT RAISE(ABORT, 'request_settlement_pending_protect');
END;
CREATE TRIGGER request_log_ingest_ack_guard BEFORE DELETE ON request_log_ingest
WHEN OLD.metered_at IS NULL
  OR NOT EXISTS (SELECT 1 FROM request_metering_facts WHERE id=OLD.request_log_id)
  OR NOT EXISTS (SELECT 1 FROM request_logs WHERE id=OLD.request_log_id) BEGIN
    SELECT RAISE(ABORT, 'request_log_ingest_ack_guard');
END;

CREATE TRIGGER models_routed_identity BEFORE UPDATE OF source_model_id ON models
WHEN NEW.source_model_id IS NOT OLD.source_model_id
 AND EXISTS (SELECT 1 FROM model_routing_profiles WHERE model_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'models_routed_identity');
END;
CREATE TRIGGER models_tombstone BEFORE UPDATE ON models
WHEN OLD.deleted_at IS NOT NULL OR (
    NEW.deleted_at IS NOT NULL AND (
        EXISTS (SELECT 1 FROM channels WHERE test_pricing_model_id=NEW.id)
        OR EXISTS (SELECT 1 FROM model_routing_profiles p JOIN model_rules r
                   ON r.model_routing_profile_id=p.id WHERE p.model_id=NEW.id AND r.enabled)
    )
) BEGIN
    SELECT RAISE(ABORT, 'models_tombstone');
END;
CREATE TRIGGER channel_groups_tombstone BEFORE UPDATE ON channel_groups
WHEN OLD.deleted_at IS NOT NULL
 OR (NEW.deleted_at IS NOT NULL AND (
    NEW.connector_kind <> 'openai_compatible'
    OR EXISTS (SELECT 1 FROM channels WHERE channel_group_id=NEW.id AND deleted_at IS NULL)
 )) BEGIN
    SELECT RAISE(ABORT, 'channel_groups_tombstone');
END;
CREATE TRIGGER channels_tombstone_update BEFORE UPDATE ON channels
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'channels_tombstone');
END;

-- A deferred FK assertion preserves transaction-local routing drafts while rejecting invalid commits.
CREATE TABLE _gateway_true (value INTEGER PRIMARY KEY CHECK (value=1)) STRICT;
INSERT INTO _gateway_true VALUES (1);
CREATE TRIGGER gateway_true_no_update BEFORE UPDATE ON _gateway_true BEGIN
    SELECT RAISE(ABORT, 'routing_assertion_protected');
END;
CREATE TRIGGER gateway_true_no_delete BEFORE DELETE ON _gateway_true BEGIN
    SELECT RAISE(ABORT, 'routing_assertion_protected');
END;
CREATE VIEW _gateway_routing_shape AS
SELECT r.id AS rule_id,
    (NOT (r.enabled AND NOT EXISTS (
        SELECT 1 FROM model_rule_routing_tiers t WHERE t.model_rule_id=r.id
    )) AND NOT EXISTS (
        SELECT 1 FROM model_rule_routing_tiers t WHERE t.model_rule_id=r.id
        AND NOT EXISTS (SELECT 1 FROM model_rule_routing_candidates c
                        WHERE c.model_rule_id=t.model_rule_id AND c.priority=t.priority)
    )) AS valid
FROM model_rules r;
CREATE TABLE _gateway_routing_assertions (
    rule_id TEXT PRIMARY KEY REFERENCES model_rules(id) ON DELETE CASCADE ON UPDATE CASCADE,
    valid INTEGER NOT NULL CHECK (valid IN (0,1))
        REFERENCES _gateway_true(value) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TRIGGER routing_assertion_insert BEFORE INSERT ON _gateway_routing_assertions
WHEN NOT EXISTS (SELECT 1 FROM _gateway_routing_shape s WHERE s.rule_id=NEW.rule_id AND s.valid=NEW.valid) BEGIN
    SELECT RAISE(ABORT, 'routing_assertion_protected');
END;
CREATE TRIGGER routing_assertion_update BEFORE UPDATE ON _gateway_routing_assertions
WHEN (OLD.rule_id IS NOT NEW.rule_id AND EXISTS (SELECT 1 FROM model_rules WHERE id=OLD.rule_id))
 OR NOT EXISTS (SELECT 1 FROM _gateway_routing_shape s WHERE s.rule_id=NEW.rule_id AND s.valid=NEW.valid) BEGIN
    SELECT RAISE(ABORT, 'routing_assertion_protected');
END;
CREATE TRIGGER routing_assertion_delete BEFORE DELETE ON _gateway_routing_assertions
WHEN EXISTS (SELECT 1 FROM model_rules WHERE id=OLD.rule_id) BEGIN
    SELECT RAISE(ABORT, 'routing_assertion_protected');
END;

CREATE TRIGGER codex_create_images_group AFTER INSERT ON channel_groups
WHEN NEW.connector_kind='codex_oauth' AND NEW.api_format='open_ai_responses'
 AND NOT EXISTS (SELECT 1 FROM channel_groups WHERE connector_pool_id=NEW.connector_pool_id AND api_format='open_ai_images') BEGIN
    INSERT INTO channel_groups (id,name,api_format,connector_kind,connector_pool_id,enabled,sharing_only)
    VALUES (ag_md5_uuid('ai-gateway:codex-images-group:' || NEW.id),
            substr(NEW.name,1,55) || ' Images ' || NEW.id,
            'open_ai_images','codex_oauth',NEW.connector_pool_id,0,NEW.sharing_only);
END;
CREATE TRIGGER codex_create_projections AFTER INSERT ON codex_oauth_credentials BEGIN
    SELECT RAISE(ABORT, 'codex_images_group_missing') WHERE NOT EXISTS (
        SELECT 1 FROM channel_groups WHERE connector_pool_id=NEW.connector_pool_id AND api_format='open_ai_images'
    );
    INSERT INTO codex_oauth_credential_channels VALUES (NEW.channel_id,'open_ai_responses',NEW.channel_id);
    INSERT INTO channels (
        id,channel_group_id,api_format,name,base_url,enabled,billing_multiplier,proxy_id,
        override_document,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,
        upstream_auth_kind,available_models,auto_disable_allowed,supports_websocket
    ) SELECT ag_md5_uuid('ai-gateway:codex-images-channel:' || NEW.channel_id),
        g.id,'open_ai_images',c.name,c.base_url,1,c.billing_multiplier,c.proxy_id,
        '{}',c.connect_timeout_ms,c.response_header_timeout_ms,c.stream_idle_timeout_ms,
        'none','["gpt-image-2"]',0,0
      FROM channels c JOIN channel_groups g ON g.connector_pool_id=NEW.connector_pool_id
      AND g.api_format='open_ai_images' WHERE c.id=NEW.channel_id;
    INSERT INTO codex_oauth_credential_channels VALUES
        (NEW.channel_id,'open_ai_images',ag_md5_uuid('ai-gateway:codex-images-channel:' || NEW.channel_id));
END;
CREATE TRIGGER codex_sync_images AFTER UPDATE OF
name,base_url,billing_multiplier,proxy_id,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms ON channels
WHEN NEW.api_format='open_ai_responses' BEGIN
    UPDATE channels SET name=NEW.name,base_url=NEW.base_url,billing_multiplier=NEW.billing_multiplier,
        proxy_id=NEW.proxy_id,connect_timeout_ms=NEW.connect_timeout_ms,
        response_header_timeout_ms=NEW.response_header_timeout_ms,stream_idle_timeout_ms=NEW.stream_idle_timeout_ms,
        updated_at=ag_now()
    WHERE id IN (
        SELECT i.channel_id FROM codex_oauth_credential_channels r
        JOIN codex_oauth_credential_channels i ON i.credential_id=r.credential_id AND i.api_format='open_ai_images'
        JOIN codex_oauth_credentials c ON c.channel_id=r.credential_id AND c.deleted_at IS NULL
        WHERE r.channel_id=NEW.id AND r.api_format='open_ai_responses'
    );
END;
CREATE TRIGGER codex_tombstone_images AFTER UPDATE OF deleted_at ON codex_oauth_credentials
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL BEGIN
    UPDATE channels SET enabled=0,name='deleted-codex-images-' || id,proxy_id=NULL,updated_at=ag_now()
    WHERE id IN (SELECT channel_id FROM codex_oauth_credential_channels
                 WHERE credential_id=NEW.channel_id AND api_format='open_ai_images');
END;
CREATE TRIGGER codex_sharing_only_update AFTER UPDATE OF sharing_only ON channel_groups
WHEN NEW.connector_kind='codex_oauth' AND NEW.sharing_only IS NOT OLD.sharing_only BEGIN
    UPDATE channel_groups SET sharing_only=NEW.sharing_only,updated_at=ag_now()
    WHERE connector_pool_id=NEW.connector_pool_id AND id<>NEW.id AND sharing_only IS NOT NEW.sharing_only;
END;
CREATE TRIGGER codex_sharing_binding_guard BEFORE UPDATE ON codex_sharing_groups
WHEN NEW.id IS NOT OLD.id OR NEW.credential_id IS NOT OLD.credential_id
 OR NEW.provider_account_id IS NOT OLD.provider_account_id OR NEW.provider_user_id IS NOT OLD.provider_user_id
 OR json_array_length(NEW.seats)<json_array_length(OLD.seats) BEGIN
    SELECT RAISE(ABORT, 'codex_sharing_binding_guard');
END;
CREATE TRIGGER codex_sharing_identity_guard BEFORE UPDATE OF account_id,user_id,deleted_at ON codex_oauth_credentials
WHEN (NEW.deleted_at IS NOT NULL AND EXISTS (SELECT 1 FROM codex_sharing_groups WHERE credential_id=OLD.channel_id))
 OR EXISTS (SELECT 1 FROM codex_sharing_groups WHERE credential_id=OLD.channel_id
            AND (provider_account_id<>coalesce(NEW.account_id,'') OR provider_user_id IS NOT NEW.user_id)) BEGIN
    SELECT RAISE(ABORT, 'codex_sharing_identity_guard');
END;






-- Column-wise normalization is explicit in SQLite writes: bind resolved pool/operation and set updated_at=ag_now().

CREATE TRIGGER audit_logs_immutable_update BEFORE UPDATE ON audit_logs BEGIN
    SELECT RAISE(ABORT, 'audit_logs_immutable_update');
END;

CREATE TRIGGER audit_logs_immutable_delete BEFORE DELETE ON audit_logs BEGIN
    SELECT RAISE(ABORT, 'audit_logs_immutable_delete');
END;

CREATE TRIGGER request_metering_facts_immutable_update BEFORE UPDATE ON request_metering_facts BEGIN
    SELECT RAISE(ABORT, 'request_metering_facts_immutable_update');
END;

CREATE TRIGGER request_metering_facts_immutable_delete BEFORE DELETE ON request_metering_facts BEGIN
    SELECT RAISE(ABORT, 'request_metering_facts_immutable_delete');
END;

CREATE TRIGGER request_settlements_immutable_update BEFORE UPDATE ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_immutable_update');
END;

CREATE TRIGGER request_settlements_immutable_delete BEFORE DELETE ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_immutable_delete');
END;

CREATE TRIGGER request_logs_immutable_update BEFORE UPDATE ON request_logs BEGIN
    SELECT RAISE(ABORT, 'request_logs_immutable_update');
END;

CREATE TRIGGER users_no_hard_delete BEFORE DELETE ON users BEGIN
    SELECT RAISE(ABORT, 'users_no_hard_delete');
END;

CREATE TRIGGER user_groups_no_hard_delete BEFORE DELETE ON user_groups BEGIN
    SELECT RAISE(ABORT, 'user_groups_no_hard_delete');
END;

CREATE TRIGGER api_keys_no_hard_delete BEFORE DELETE ON api_keys BEGIN
    SELECT RAISE(ABORT, 'api_keys_no_hard_delete');
END;

CREATE TRIGGER channel_groups_no_hard_delete BEFORE DELETE ON channel_groups BEGIN
    SELECT RAISE(ABORT, 'channel_groups_no_hard_delete');
END;

CREATE TRIGGER channels_no_hard_delete BEFORE DELETE ON channels BEGIN
    SELECT RAISE(ABORT, 'channels_no_hard_delete');
END;

CREATE TRIGGER models_no_hard_delete BEFORE DELETE ON models BEGIN
    SELECT RAISE(ABORT, 'models_no_hard_delete');
END;

CREATE TRIGGER channels_active_group_insert BEFORE INSERT ON channels
WHEN (NEW.deleted_at IS NOT NULL AND (SELECT connector_kind FROM channel_groups WHERE id=NEW.channel_group_id)<>'openai_compatible') OR (NEW.deleted_at IS NULL AND (SELECT deleted_at FROM channel_groups WHERE id=NEW.channel_group_id) IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'channels_active_group_insert');
END;

CREATE TRIGGER channels_active_pricing_insert BEFORE INSERT ON channels
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'channels_active_pricing_insert');
END;

CREATE TRIGGER profiles_active_pricing_insert BEFORE INSERT ON model_routing_profiles
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'profiles_active_pricing_insert');
END;

CREATE TRIGGER rules_active_pricing_insert BEFORE INSERT ON model_rules
WHEN EXISTS (SELECT 1 FROM models m JOIN model_routing_profiles p ON p.model_id=m.id WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'rules_active_pricing_insert');
END;

CREATE TRIGGER channels_active_group_update BEFORE UPDATE ON channels
WHEN (NEW.deleted_at IS NOT NULL AND (SELECT connector_kind FROM channel_groups WHERE id=NEW.channel_group_id)<>'openai_compatible') OR (NEW.deleted_at IS NULL AND (SELECT deleted_at FROM channel_groups WHERE id=NEW.channel_group_id) IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'channels_active_group_update');
END;

CREATE TRIGGER channels_active_pricing_update BEFORE UPDATE OF test_pricing_model_id ON channels
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'channels_active_pricing_update');
END;

CREATE TRIGGER profiles_active_pricing_update BEFORE UPDATE OF model_id ON model_routing_profiles
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'profiles_active_pricing_update');
END;

CREATE TRIGGER rules_active_pricing_update BEFORE UPDATE ON model_rules
WHEN EXISTS (SELECT 1 FROM models m JOIN model_routing_profiles p ON p.model_id=m.id WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'rules_active_pricing_update');
END;

CREATE TRIGGER model_rule_routing_tiers_no_move BEFORE UPDATE ON model_rule_routing_tiers
WHEN OLD.model_rule_id IS NOT NEW.model_rule_id BEGIN
    SELECT RAISE(ABORT, 'model_rule_routing_tiers_no_move');
END;

CREATE TRIGGER model_rule_routing_candidates_no_move BEFORE UPDATE ON model_rule_routing_candidates
WHEN OLD.model_rule_id IS NOT NEW.model_rule_id BEGIN
    SELECT RAISE(ABORT, 'model_rule_routing_candidates_no_move');
END;

CREATE TRIGGER model_rules_shape_insert AFTER INSERT ON model_rules BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
    UPDATE model_routing_profiles SET updated_at=ag_now() WHERE id=NEW.model_routing_profile_id;
END;

CREATE TRIGGER model_rules_shape_update AFTER UPDATE ON model_rules BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
    UPDATE model_routing_profiles SET updated_at=ag_now() WHERE id=NEW.model_routing_profile_id;
END;

CREATE TRIGGER model_rules_shape_delete AFTER DELETE ON model_rules BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=OLD.id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
    UPDATE model_routing_profiles SET updated_at=ag_now() WHERE id=OLD.model_routing_profile_id;
END;

CREATE TRIGGER model_rule_routing_tiers_shape_insert AFTER INSERT ON model_rule_routing_tiers BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER model_rule_routing_tiers_shape_update AFTER UPDATE ON model_rule_routing_tiers BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER model_rule_routing_tiers_shape_delete AFTER DELETE ON model_rule_routing_tiers BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=OLD.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER model_rule_routing_candidates_shape_insert AFTER INSERT ON model_rule_routing_candidates BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER model_rule_routing_candidates_shape_update AFTER UPDATE ON model_rule_routing_candidates BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=NEW.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER model_rule_routing_candidates_shape_delete AFTER DELETE ON model_rule_routing_candidates BEGIN
    INSERT INTO _gateway_routing_assertions (rule_id,valid)
    SELECT rule_id,valid FROM _gateway_routing_shape WHERE rule_id=OLD.model_rule_id
    ON CONFLICT(rule_id) DO UPDATE SET valid=excluded.valid;
END;

CREATE TRIGGER credential_pool_insert BEFORE INSERT ON codex_oauth_credentials
WHEN NEW.connector_pool_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM channel_groups WHERE id=NEW.channel_group_id AND connector_kind='codex_oauth' AND api_format='open_ai_responses' AND connector_pool_id=NEW.connector_pool_id) BEGIN
    SELECT RAISE(ABORT, 'credential_pool_insert');
END;

CREATE TRIGGER credential_pool_update BEFORE UPDATE OF channel_group_id,connector_pool_id ON codex_oauth_credentials
WHEN NEW.connector_pool_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM channel_groups WHERE id=NEW.channel_group_id AND connector_kind='codex_oauth' AND api_format='open_ai_responses' AND connector_pool_id=NEW.connector_pool_id) BEGIN
    SELECT RAISE(ABORT, 'credential_pool_update');
END;

CREATE TRIGGER channel_group_pool_kind BEFORE INSERT ON channel_groups
WHEN NEW.connector_kind='codex_oauth' AND NEW.connector_pool_id IS NOT NULL AND (SELECT connector_kind FROM connector_pools WHERE id=NEW.connector_pool_id) IS NOT NEW.connector_kind BEGIN
    SELECT RAISE(ABORT, 'channel_group_pool_kind');
END;

CREATE TRIGGER channel_groups_sharing_insert BEFORE INSERT ON channel_groups
WHEN NEW.connector_kind='codex_oauth' AND EXISTS (SELECT 1 FROM channel_groups WHERE connector_pool_id=NEW.connector_pool_id AND sharing_only IS NOT NEW.sharing_only) BEGIN
    SELECT RAISE(ABORT, 'channel_groups_sharing_insert');
END;

CREATE TRIGGER api_key_policies_timestamp BEFORE UPDATE ON api_key_policies
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'api_key_policies_timestamp');
END;

CREATE TRIGGER api_keys_timestamp BEFORE UPDATE ON api_keys
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'api_keys_timestamp');
END;

CREATE TRIGGER channel_groups_timestamp BEFORE UPDATE ON channel_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'channel_groups_timestamp');
END;

CREATE TRIGGER channels_timestamp BEFORE UPDATE ON channels
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'channels_timestamp');
END;

CREATE TRIGGER codex_oauth_credentials_timestamp BEFORE UPDATE ON codex_oauth_credentials
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'codex_oauth_credentials_timestamp');
END;

CREATE TRIGGER codex_quota_window_periods_timestamp BEFORE UPDATE ON codex_quota_window_periods
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'codex_quota_window_periods_timestamp');
END;

CREATE TRIGGER codex_sharing_groups_timestamp BEFORE UPDATE ON codex_sharing_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'codex_sharing_groups_timestamp');
END;

CREATE TRIGGER config_templates_timestamp BEFORE UPDATE ON config_templates
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'config_templates_timestamp');
END;

CREATE TRIGGER model_routing_profiles_timestamp BEFORE UPDATE ON model_routing_profiles
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'model_routing_profiles_timestamp');
END;

CREATE TRIGGER model_rules_timestamp BEFORE UPDATE ON model_rules
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'model_rules_timestamp');
END;

CREATE TRIGGER models_timestamp BEFORE UPDATE ON models
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'models_timestamp');
END;

CREATE TRIGGER proxies_timestamp BEFORE UPDATE ON proxies
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'proxies_timestamp');
END;

CREATE TRIGGER registration_invitation_codes_timestamp BEFORE UPDATE ON registration_invitation_codes
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'registration_invitation_codes_timestamp');
END;

CREATE TRIGGER system_settings_timestamp BEFORE UPDATE ON system_settings
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'system_settings_timestamp');
END;

CREATE TRIGGER user_groups_timestamp BEFORE UPDATE ON user_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'user_groups_timestamp');
END;

CREATE TRIGGER users_timestamp BEFORE UPDATE ON users
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'users_timestamp');
END;

CREATE TRIGGER channels_channel_group_id_api_format_fkey_insert BEFORE INSERT ON channels
WHEN (NEW.channel_group_id IS NOT NULL AND ag_uuid_valid(NEW.channel_group_id)) AND (NEW.api_format IS NOT NULL AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND NOT EXISTS (SELECT 1 FROM channel_groups p WHERE p.id=NEW.channel_group_id AND p.api_format=NEW.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_channel_group_id_api_format_fkey');
END;

CREATE TRIGGER channels_channel_group_id_api_format_fkey_update BEFORE UPDATE ON channels
WHEN (NEW.channel_group_id IS NOT NULL AND ag_uuid_valid(NEW.channel_group_id)) AND (NEW.api_format IS NOT NULL AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND NOT EXISTS (SELECT 1 FROM channel_groups p WHERE p.id=NEW.channel_group_id AND p.api_format=NEW.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_channel_group_id_api_format_fkey');
END;

CREATE TRIGGER channels_channel_group_id_api_format_fkey_parent_update BEFORE UPDATE ON channel_groups
WHEN (ag_uuid_valid(NEW.id) AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND (OLD.id IS NOT NEW.id OR OLD.api_format IS NOT NEW.api_format) AND EXISTS (SELECT 1 FROM channels c WHERE c.channel_group_id=OLD.id AND c.api_format=OLD.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_channel_group_id_api_format_fkey');
END;

CREATE TRIGGER channels_proxy_id_fkey_insert BEFORE INSERT ON channels
WHEN (NEW.proxy_id IS NOT NULL AND ag_uuid_valid(NEW.proxy_id)) AND NOT EXISTS (SELECT 1 FROM proxies p WHERE p.id=NEW.proxy_id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_proxy_id_fkey');
END;

CREATE TRIGGER channels_proxy_id_fkey_update BEFORE UPDATE ON channels
WHEN (NEW.proxy_id IS NOT NULL AND ag_uuid_valid(NEW.proxy_id)) AND NOT EXISTS (SELECT 1 FROM proxies p WHERE p.id=NEW.proxy_id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_proxy_id_fkey');
END;

CREATE TRIGGER channels_proxy_id_fkey_parent_update BEFORE UPDATE ON proxies
WHEN (ag_uuid_valid(NEW.id)) AND (OLD.id IS NOT NEW.id) AND EXISTS (SELECT 1 FROM channels c WHERE c.proxy_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_proxy_id_fkey');
END;

CREATE TRIGGER channels_proxy_id_fkey_parent_delete BEFORE DELETE ON proxies
WHEN EXISTS (SELECT 1 FROM channels c WHERE c.proxy_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_proxy_id_fkey');
END;

CREATE TRIGGER channels_config_template_id_fkey_insert BEFORE INSERT ON channels
WHEN (NEW.config_template_id IS NOT NULL AND ag_uuid_valid(NEW.config_template_id)) AND NOT EXISTS (SELECT 1 FROM config_templates p WHERE p.id=NEW.config_template_id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_config_template_id_fkey');
END;

CREATE TRIGGER channels_config_template_id_fkey_update BEFORE UPDATE ON channels
WHEN (NEW.config_template_id IS NOT NULL AND ag_uuid_valid(NEW.config_template_id)) AND NOT EXISTS (SELECT 1 FROM config_templates p WHERE p.id=NEW.config_template_id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_config_template_id_fkey');
END;

CREATE TRIGGER channels_config_template_id_fkey_parent_update BEFORE UPDATE ON config_templates
WHEN (ag_uuid_valid(NEW.id)) AND (OLD.id IS NOT NEW.id) AND EXISTS (SELECT 1 FROM channels c WHERE c.config_template_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_config_template_id_fkey');
END;

CREATE TRIGGER channels_config_template_id_fkey_parent_delete BEFORE DELETE ON config_templates
WHEN EXISTS (SELECT 1 FROM channels c WHERE c.config_template_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:channels_config_template_id_fkey');
END;

CREATE TRIGGER model_rule_tiers_rule_format_fk_insert BEFORE INSERT ON model_rule_routing_tiers
WHEN (NEW.model_rule_id IS NOT NULL AND ag_uuid_valid(NEW.model_rule_id)) AND (NEW.api_format IS NOT NULL AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND NOT EXISTS (SELECT 1 FROM model_rules p WHERE p.id=NEW.model_rule_id AND p.api_format=NEW.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:model_rule_tiers_rule_format_fk');
END;

CREATE TRIGGER model_rule_tiers_rule_format_fk_update BEFORE UPDATE ON model_rule_routing_tiers
WHEN (NEW.model_rule_id IS NOT NULL AND ag_uuid_valid(NEW.model_rule_id)) AND (NEW.api_format IS NOT NULL AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND NOT EXISTS (SELECT 1 FROM model_rules p WHERE p.id=NEW.model_rule_id AND p.api_format=NEW.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:model_rule_tiers_rule_format_fk');
END;

CREATE TRIGGER model_rule_tiers_rule_format_fk_parent_update BEFORE UPDATE ON model_rules
WHEN (ag_uuid_valid(NEW.id) AND NEW.api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images')) AND (OLD.id IS NOT NEW.id OR OLD.api_format IS NOT NEW.api_format) AND EXISTS (SELECT 1 FROM model_rule_routing_tiers c WHERE c.model_rule_id=OLD.id AND c.api_format=OLD.api_format) BEGIN
    SELECT RAISE(ABORT, 'routing_dependency:model_rule_tiers_rule_format_fk');
END;
