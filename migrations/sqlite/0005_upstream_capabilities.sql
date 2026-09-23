-- Startup transfers and validates legacy configuration, retargets history, and
-- retires legacy tables in the same transaction immediately after this DDL.

CREATE TABLE routing_groups (
    id TEXT NOT NULL CONSTRAINT routing_groups_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT routing_groups_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT routing_groups_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    sharing_only INTEGER DEFAULT 0 NOT NULL CONSTRAINT routing_groups_sharing_only_storage CHECK (sharing_only IS NULL OR sharing_only IN (0,1)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT routing_groups_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT routing_groups_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT routing_groups_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT routing_groups_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT routing_groups_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT routing_groups_pkey PRIMARY KEY (id)
) STRICT;

CREATE UNIQUE INDEX routing_groups_active_name_idx
    ON routing_groups (name)
    WHERE deleted_at IS NULL;

CREATE INDEX routing_groups_deleted_at_idx
    ON routing_groups (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER routing_groups_timestamp BEFORE UPDATE ON routing_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'routing_groups_timestamp');
END;

ALTER TABLE connector_pools
    ADD COLUMN routing_group_id TEXT CHECK (routing_group_id IS NULL OR ag_uuid_valid(routing_group_id))
        REFERENCES routing_groups(id) ON DELETE RESTRICT;
CREATE UNIQUE INDEX connector_pools_routing_group_idx ON connector_pools(routing_group_id);

CREATE TABLE upstream_accesses (
    id TEXT NOT NULL CONSTRAINT upstream_accesses_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT upstream_accesses_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    connector_kind TEXT NOT NULL CONSTRAINT upstream_accesses_connector_kind_storage CHECK (connector_kind IS NULL OR instr(connector_kind, char(0))=0),
    base_url TEXT NOT NULL CONSTRAINT upstream_accesses_base_url_storage CHECK (base_url IS NULL OR instr(base_url, char(0))=0),
    proxy_id TEXT CONSTRAINT upstream_accesses_proxy_id_storage CHECK (proxy_id IS NULL OR (ag_uuid_valid(proxy_id) AND instr(proxy_id, char(0))=0)),
    connect_timeout_ms INTEGER CONSTRAINT upstream_accesses_connect_timeout_ms_storage CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms BETWEEN -2147483648 AND 2147483647),
    response_header_timeout_ms INTEGER CONSTRAINT upstream_accesses_response_header_timeout_ms_storage CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms BETWEEN -2147483648 AND 2147483647),
    stream_idle_timeout_ms INTEGER CONSTRAINT upstream_accesses_stream_idle_timeout_ms_storage CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms BETWEEN -2147483648 AND 2147483647),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT upstream_accesses_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT upstream_accesses_revision_storage CHECK (revision IS NULL OR (ag_uuid_valid(revision) AND instr(revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_accesses_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_accesses_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT upstream_accesses_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT upstream_accesses_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_accesses_connector_kind_check
        CHECK (ag_array_contains(json_array('openai_compatible', 'codex_oauth'), connector_kind)),
    CONSTRAINT upstream_accesses_base_url_check
        CHECK (ag_regex('^https?://', base_url)),
    CONSTRAINT upstream_accesses_connect_timeout_check
        CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms > 0),
    CONSTRAINT upstream_accesses_response_header_timeout_check
        CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms > 0),
    CONSTRAINT upstream_accesses_stream_idle_timeout_check
        CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms > 0),
    CONSTRAINT upstream_accesses_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT upstream_accesses_pkey PRIMARY KEY (id),
    CONSTRAINT upstream_accesses_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES proxies (id) ON DELETE RESTRICT
) STRICT;

-- Access names are inherited from legacy channels and may repeat across
-- routing groups, so the active lookup index is intentionally not unique.
CREATE INDEX upstream_accesses_active_name_idx
    ON upstream_accesses (name)
    WHERE deleted_at IS NULL;

CREATE INDEX upstream_accesses_deleted_at_idx
    ON upstream_accesses (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER upstream_accesses_timestamp BEFORE UPDATE ON upstream_accesses
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_timestamp');
END;

CREATE TABLE upstream_channels (
    id TEXT NOT NULL CONSTRAINT upstream_channels_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    group_id TEXT NOT NULL CONSTRAINT upstream_channels_group_id_storage CHECK (group_id IS NULL OR (ag_uuid_valid(group_id) AND instr(group_id, char(0))=0)),
    access_id TEXT NOT NULL CONSTRAINT upstream_channels_access_id_storage CHECK (access_id IS NULL OR (ag_uuid_valid(access_id) AND instr(access_id, char(0))=0)),
    credential_id TEXT CONSTRAINT upstream_channels_credential_id_storage CHECK (credential_id IS NULL OR (ag_uuid_valid(credential_id) AND instr(credential_id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT upstream_channels_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT upstream_channels_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    binding_revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT upstream_channels_binding_revision_storage CHECK (binding_revision IS NULL OR (ag_uuid_valid(binding_revision) AND instr(binding_revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_channels_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_channels_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT upstream_channels_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT upstream_channels_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_channels_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT upstream_channels_pkey PRIMARY KEY (id),
    CONSTRAINT upstream_channels_group_id_fkey FOREIGN KEY (group_id) REFERENCES routing_groups (id) ON DELETE RESTRICT,
    CONSTRAINT upstream_channels_access_id_fkey FOREIGN KEY (access_id) REFERENCES upstream_accesses (id) ON DELETE RESTRICT,
    CONSTRAINT upstream_channels_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES upstream_credentials (id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX upstream_channels_active_group_name_idx
    ON upstream_channels (group_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX upstream_channels_access_id_idx
    ON upstream_channels (access_id);

CREATE INDEX upstream_channels_credential_id_idx
    ON upstream_channels (credential_id)
    WHERE credential_id IS NOT NULL;

CREATE INDEX upstream_channels_deleted_at_idx
    ON upstream_channels (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER upstream_channels_timestamp BEFORE UPDATE ON upstream_channels
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_timestamp');
END;

CREATE TABLE channel_capabilities (
    id TEXT NOT NULL CONSTRAINT channel_capabilities_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    channel_id TEXT NOT NULL CONSTRAINT channel_capabilities_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id) AND instr(channel_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT channel_capabilities_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    transports TEXT NOT NULL CONSTRAINT channel_capabilities_transports_storage CHECK (transports IS NULL OR (ag_array_valid(transports, 'text') AND instr(transports, char(0))=0)),
    enabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    available_models TEXT DEFAULT '[]' NOT NULL CONSTRAINT channel_capabilities_available_models_storage CHECK (available_models IS NULL OR (ag_array_valid(available_models, 'text') AND instr(available_models, char(0))=0)),
    request_compression TEXT DEFAULT 'default' NOT NULL CONSTRAINT channel_capabilities_request_compression_storage CHECK (request_compression IS NULL OR instr(request_compression, char(0))=0),
    test_model TEXT CONSTRAINT channel_capabilities_test_model_storage CHECK (test_model IS NULL OR (length(test_model) <= 300 AND instr(test_model, char(0))=0)),
    test_pricing_model_id TEXT CONSTRAINT channel_capabilities_test_pricing_model_id_storage CHECK (test_pricing_model_id IS NULL OR (ag_uuid_valid(test_pricing_model_id) AND instr(test_pricing_model_id, char(0))=0)),
    auto_disabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_auto_disabled_storage CHECK (auto_disabled IS NULL OR auto_disabled IN (0,1)),
    auto_disable_reason TEXT CONSTRAINT channel_capabilities_auto_disable_reason_storage CHECK (auto_disable_reason IS NULL OR (length(auto_disable_reason) <= 500 AND instr(auto_disable_reason, char(0))=0)),
    auto_disable_at TEXT CONSTRAINT channel_capabilities_auto_disable_at_storage CHECK (auto_disable_at IS NULL OR (ag_time_valid(auto_disable_at) AND instr(auto_disable_at, char(0))=0)),
    auto_disable_allowed INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_auto_disable_allowed_storage CHECK (auto_disable_allowed IS NULL OR auto_disable_allowed IN (0,1)),
    status_statistics_enabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_status_statistics_enabled_storage CHECK (status_statistics_enabled IS NULL OR status_statistics_enabled IN (0,1)),
    config_template_id TEXT CONSTRAINT channel_capabilities_config_template_id_storage CHECK (config_template_id IS NULL OR (ag_uuid_valid(config_template_id) AND instr(config_template_id, char(0))=0)),
    override_document TEXT DEFAULT '{}' NOT NULL CONSTRAINT channel_capabilities_override_document_storage CHECK (override_document IS NULL OR (ag_json_valid(override_document) AND instr(override_document, char(0))=0)),
    billing_multiplier TEXT DEFAULT '1' NOT NULL CONSTRAINT channel_capabilities_billing_multiplier_storage CHECK (billing_multiplier IS NULL OR (ag_decimal_valid(billing_multiplier, 24, 12) AND instr(billing_multiplier, char(0))=0)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT channel_capabilities_revision_storage CHECK (revision IS NULL OR (ag_uuid_valid(revision) AND instr(revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_capabilities_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_capabilities_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT channel_capabilities_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT channel_capabilities_channel_operation_key
        UNIQUE (channel_id, operation),
    CONSTRAINT channel_capabilities_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT channel_capabilities_operation_check
        CHECK (ag_array_contains(json_array(
            'chat_completions',
            'responses',
            'standalone_web_search',
            'images_generation',
            'images_edit'
        ), operation)),
    CONSTRAINT channel_capabilities_transports_check
        CHECK (
            json_array_length(transports) > 0
            AND ag_array_position(transports, NULL) IS NULL
            AND ag_array_subset(transports, json_array('http_json', 'http_sse', 'websocket', 'multipart'))
        ),
    CONSTRAINT channel_capabilities_available_models_check
        CHECK (ag_array_position(available_models, NULL) IS NULL),
    CONSTRAINT channel_capabilities_request_compression_check
        CHECK (ag_array_contains(json_array('default', 'zstd'), request_compression)),
    CONSTRAINT channel_capabilities_request_compression_operation_check
        CHECK (
            request_compression = 'default'
            OR (
                operation = 'responses'
                AND (ag_array_contains(transports, 'http_json') OR ag_array_contains(transports, 'http_sse'))
            )
        ),
    CONSTRAINT channel_capabilities_override_document_check
        CHECK (json_type(override_document) = 'object'),
    CONSTRAINT channel_capabilities_billing_multiplier_check
        CHECK (ag_decimal_cmp(billing_multiplier, '0') >= 0),
    CONSTRAINT channel_capabilities_test_model_pair_check
        CHECK ((test_model IS NULL) = (test_pricing_model_id IS NULL)),
    CONSTRAINT channel_capabilities_test_model_catalogue_check
        CHECK (test_model IS NULL OR ag_array_contains(available_models, test_model)),
    CONSTRAINT channel_capabilities_probe_check
        CHECK (
            test_model IS NULL
            OR (
                ag_array_contains(json_array('chat_completions', 'responses'), operation)
                AND ag_array_contains(transports, 'http_json')
            )
        ),
    CONSTRAINT channel_capabilities_auto_disable_check
        CHECK (auto_disabled OR (auto_disable_reason IS NULL AND auto_disable_at IS NULL)),
    CONSTRAINT channel_capabilities_deleted_state_check
        CHECK (deleted_at IS NULL OR (NOT enabled AND NOT auto_disabled)),
    CONSTRAINT channel_capabilities_pkey PRIMARY KEY (id),
    CONSTRAINT channel_capabilities_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    CONSTRAINT channel_capabilities_test_pricing_model_id_fkey FOREIGN KEY (test_pricing_model_id) REFERENCES models (id) ON DELETE RESTRICT,
    CONSTRAINT channel_capabilities_config_template_id_fkey FOREIGN KEY (config_template_id) REFERENCES config_templates (id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX channel_capabilities_deleted_at_idx
    ON channel_capabilities (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER channel_capabilities_timestamp BEFORE UPDATE ON channel_capabilities
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_timestamp');
END;

-- Canonical configuration rows are never physically deleted, and once a row is
-- tombstoned it can no longer be modified. Soft deletion is a one-way
-- transition; deleted_at is not kept in reverse sync with enabled.
CREATE TRIGGER routing_groups_preserve_tombstones_update BEFORE UPDATE ON routing_groups
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'routing_groups_preserve_tombstones');
END;
CREATE TRIGGER routing_groups_preserve_tombstones_delete BEFORE DELETE ON routing_groups BEGIN
    SELECT RAISE(ABORT, 'routing_groups_preserve_tombstones');
END;
CREATE TRIGGER upstream_accesses_preserve_tombstones_update BEFORE UPDATE ON upstream_accesses
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_preserve_tombstones');
END;
CREATE TRIGGER upstream_accesses_preserve_tombstones_delete BEFORE DELETE ON upstream_accesses BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_preserve_tombstones');
END;
CREATE TRIGGER upstream_channels_preserve_tombstones_update BEFORE UPDATE ON upstream_channels
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_preserve_tombstones');
END;
CREATE TRIGGER upstream_channels_preserve_tombstones_delete BEFORE DELETE ON upstream_channels BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_preserve_tombstones');
END;
CREATE TRIGGER channel_capabilities_preserve_tombstones_update BEFORE UPDATE ON channel_capabilities
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_preserve_tombstones');
END;
CREATE TRIGGER channel_capabilities_preserve_tombstones_delete BEFORE DELETE ON channel_capabilities BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_preserve_tombstones');
END;

CREATE TABLE model_operation_rules (
    id TEXT NOT NULL CONSTRAINT model_operation_rules_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    model_routing_profile_id TEXT NOT NULL CONSTRAINT model_operation_rules_model_routing_profile_id_storage CHECK (model_routing_profile_id IS NULL OR (ag_uuid_valid(model_routing_profile_id) AND instr(model_routing_profile_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_operation_rules_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT model_operation_rules_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_operation_rules_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_operation_rules_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    CONSTRAINT model_operation_rules_profile_operation_key
        UNIQUE (model_routing_profile_id, operation),
    CONSTRAINT model_operation_rules_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_operation_rules_operation_check
        CHECK (ag_array_contains(json_array(
            'chat_completions',
            'responses',
            'standalone_web_search',
            'images_generation',
            'images_edit'
        ), operation)),
    CONSTRAINT model_operation_rules_pkey PRIMARY KEY (id),
    CONSTRAINT model_operation_rules_model_routing_profile_id_fkey FOREIGN KEY (model_routing_profile_id) REFERENCES model_routing_profiles (id) ON DELETE CASCADE
) STRICT;

CREATE TRIGGER model_operation_rules_timestamp BEFORE UPDATE ON model_operation_rules
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'model_operation_rules_timestamp');
END;

CREATE TABLE model_capability_tiers (
    id TEXT NOT NULL CONSTRAINT model_capability_tiers_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    rule_id TEXT NOT NULL CONSTRAINT model_capability_tiers_rule_id_storage CHECK (rule_id IS NULL OR (ag_uuid_valid(rule_id) AND instr(rule_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_capability_tiers_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    priority INTEGER NOT NULL CONSTRAINT model_capability_tiers_priority_storage CHECK (priority IS NULL OR priority BETWEEN -2147483648 AND 2147483647),
    strategy TEXT NOT NULL CONSTRAINT model_capability_tiers_strategy_storage CHECK (strategy IS NULL OR instr(strategy, char(0))=0),
    CONSTRAINT model_capability_tiers_rule_priority_key
        UNIQUE (rule_id, priority),
    CONSTRAINT model_capability_tiers_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_capability_tiers_priority_check
        CHECK (priority >= 0),
    CONSTRAINT model_capability_tiers_strategy_check
        CHECK (ag_array_contains(json_array('weighted_random', 'weighted_round_robin'), strategy)),
    CONSTRAINT model_capability_tiers_pkey PRIMARY KEY (id),
    CONSTRAINT model_capability_tiers_rule_fk
        FOREIGN KEY (rule_id, operation)
        REFERENCES model_operation_rules (id, operation) ON DELETE CASCADE
) STRICT;

CREATE TABLE model_capability_candidates (
    tier_id TEXT NOT NULL CONSTRAINT model_capability_candidates_tier_id_storage CHECK (tier_id IS NULL OR (ag_uuid_valid(tier_id) AND instr(tier_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_capability_candidates_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    capability_id TEXT NOT NULL CONSTRAINT model_capability_candidates_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    upstream_model TEXT NOT NULL CONSTRAINT model_capability_candidates_upstream_model_storage CHECK (upstream_model IS NULL OR (length(upstream_model) <= 300 AND instr(upstream_model, char(0))=0)),
    weight INTEGER NOT NULL CONSTRAINT model_capability_candidates_weight_storage CHECK (weight IS NULL OR weight BETWEEN -2147483648 AND 2147483647),
    CONSTRAINT model_capability_candidates_pkey PRIMARY KEY (tier_id, capability_id, upstream_model),
    CONSTRAINT model_capability_candidates_tier_fk
        FOREIGN KEY (tier_id, operation)
        REFERENCES model_capability_tiers (id, operation) ON DELETE CASCADE,
    CONSTRAINT model_capability_candidates_capability_fk
        FOREIGN KEY (capability_id, operation)
        REFERENCES channel_capabilities (id, operation) ON DELETE RESTRICT,
    CONSTRAINT model_capability_candidates_upstream_model_check
        CHECK (trim(upstream_model) <> ''),
    CONSTRAINT model_capability_candidates_weight_check
        CHECK (weight > 0)
) STRICT;

CREATE INDEX model_capability_candidates_capability_id_idx
    ON model_capability_candidates (capability_id);

CREATE TABLE api_key_capability_grants (
    api_key_id TEXT NOT NULL CONSTRAINT api_key_capability_grants_api_key_id_storage CHECK (api_key_id IS NULL OR (ag_uuid_valid(api_key_id) AND instr(api_key_id, char(0))=0)),
    capability_id TEXT NOT NULL CONSTRAINT api_key_capability_grants_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    origin_kind TEXT NOT NULL CONSTRAINT api_key_capability_grants_origin_kind_storage CHECK (origin_kind IS NULL OR instr(origin_kind, char(0))=0),
    origin_id TEXT NOT NULL CONSTRAINT api_key_capability_grants_origin_id_storage CHECK (origin_id IS NULL OR (ag_uuid_valid(origin_id) AND instr(origin_id, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_key_capability_grants_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    CONSTRAINT api_key_capability_grants_pkey PRIMARY KEY (api_key_id, capability_id, origin_kind, origin_id),
    CONSTRAINT api_key_capability_grants_origin_kind_check
        CHECK (ag_array_contains(json_array('group', 'channel', 'capability'), origin_kind)),
    CONSTRAINT api_key_capability_grants_capability_origin_check
        CHECK (origin_kind <> 'capability' OR origin_id = capability_id),
    CONSTRAINT api_key_capability_grants_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES api_keys (id) ON DELETE CASCADE,
    CONSTRAINT api_key_capability_grants_capability_id_fkey FOREIGN KEY (capability_id) REFERENCES channel_capabilities (id) ON DELETE CASCADE
) STRICT;

CREATE INDEX api_key_capability_grants_capability_id_idx
    ON api_key_capability_grants (capability_id);

CREATE TABLE api_key_policy_capability_grants (
    policy_id TEXT NOT NULL CONSTRAINT api_key_policy_capability_grants_policy_id_storage CHECK (policy_id IS NULL OR (ag_uuid_valid(policy_id) AND instr(policy_id, char(0))=0)),
    capability_id TEXT NOT NULL CONSTRAINT api_key_policy_capability_grants_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    origin_kind TEXT NOT NULL CONSTRAINT api_key_policy_capability_grants_origin_kind_storage CHECK (origin_kind IS NULL OR instr(origin_kind, char(0))=0),
    origin_id TEXT NOT NULL CONSTRAINT api_key_policy_capability_grants_origin_id_storage CHECK (origin_id IS NULL OR (ag_uuid_valid(origin_id) AND instr(origin_id, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_key_policy_capability_grants_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    CONSTRAINT api_key_policy_capability_grants_pkey PRIMARY KEY (policy_id, capability_id, origin_kind, origin_id),
    CONSTRAINT api_key_policy_capability_grants_origin_kind_check
        CHECK (ag_array_contains(json_array('group', 'channel', 'capability'), origin_kind)),
    CONSTRAINT api_key_policy_capability_grants_capability_origin_check
        CHECK (origin_kind <> 'capability' OR origin_id = capability_id),
    CONSTRAINT api_key_policy_capability_grants_policy_id_fkey FOREIGN KEY (policy_id) REFERENCES api_key_policies (id) ON DELETE CASCADE,
    CONSTRAINT api_key_policy_capability_grants_capability_id_fkey FOREIGN KEY (capability_id) REFERENCES channel_capabilities (id) ON DELETE CASCADE
) STRICT;

CREATE INDEX api_key_policy_capability_grants_capability_id_idx
    ON api_key_policy_capability_grants (capability_id);

-- Read-only historical identities keep original UUIDs and labels resolvable
-- after the legacy configuration tables are retired. Registry ids stay the
-- original dispatch identities (legacy physical channel UUIDs or replacement
-- capability UUIDs) and canonical linkage is nullable so legacy channels with
-- ambiguous replacements stay unlinked. Codex credential UUIDs remain the
-- stable legacy channel UUIDs used for quota aggregation, so no separate
-- credential registry is created.
CREATE TABLE group_identity_registry (
    id TEXT NOT NULL CONSTRAINT group_identity_registry_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    label TEXT NOT NULL CONSTRAINT group_identity_registry_label_storage CHECK (label IS NULL OR (length(label) <= 100 AND instr(label, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT group_identity_registry_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    canonical_group_id TEXT CONSTRAINT group_identity_registry_canonical_group_id_storage CHECK (canonical_group_id IS NULL OR (ag_uuid_valid(canonical_group_id) AND instr(canonical_group_id, char(0))=0)),
    CONSTRAINT group_identity_registry_label_check
        CHECK (length(trim(label)) BETWEEN 1 AND 100),
    CONSTRAINT group_identity_registry_pkey PRIMARY KEY (id),
    CONSTRAINT group_identity_registry_canonical_group_id_fkey FOREIGN KEY (canonical_group_id) REFERENCES routing_groups (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE channel_identity_registry (
    id TEXT NOT NULL CONSTRAINT channel_identity_registry_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    label TEXT NOT NULL CONSTRAINT channel_identity_registry_label_storage CHECK (label IS NULL OR (length(label) <= 100 AND instr(label, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_identity_registry_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    canonical_channel_id TEXT CONSTRAINT channel_identity_registry_canonical_channel_id_storage CHECK (canonical_channel_id IS NULL OR (ag_uuid_valid(canonical_channel_id) AND instr(canonical_channel_id, char(0))=0)),
    codex_credential_id TEXT CONSTRAINT channel_identity_registry_codex_credential_id_storage CHECK (codex_credential_id IS NULL OR (ag_uuid_valid(codex_credential_id) AND instr(codex_credential_id, char(0))=0)),
    capability_id TEXT CONSTRAINT channel_identity_registry_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    CONSTRAINT channel_identity_registry_label_check
        CHECK (length(trim(label)) BETWEEN 1 AND 100),
    CONSTRAINT channel_identity_registry_pkey PRIMARY KEY (id),
    CONSTRAINT channel_identity_registry_canonical_channel_id_fkey FOREIGN KEY (canonical_channel_id) REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    CONSTRAINT channel_identity_registry_codex_credential_id_fkey FOREIGN KEY (codex_credential_id) REFERENCES upstream_credentials (id) ON DELETE RESTRICT,
    CONSTRAINT channel_identity_registry_capability_id_fkey FOREIGN KEY (capability_id) REFERENCES channel_capabilities (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE model_rule_identity_registry (
    id TEXT NOT NULL PRIMARY KEY CHECK (ag_uuid_valid(id) AND instr(id, char(0))=0),
    label TEXT NOT NULL CHECK (length(trim(label)) > 0 AND instr(label, char(0))=0),
    created_at TEXT NOT NULL CHECK (ag_time_valid(created_at) AND instr(created_at, char(0))=0),
    canonical_rule_id TEXT REFERENCES model_operation_rules (id) ON DELETE RESTRICT
        CHECK (canonical_rule_id IS NULL OR (ag_uuid_valid(canonical_rule_id) AND instr(canonical_rule_id, char(0))=0))
) STRICT;

CREATE TRIGGER model_rule_identity_registry_immutable_update BEFORE UPDATE ON model_rule_identity_registry BEGIN
    SELECT RAISE(ABORT, 'model_rule_identity_registry_immutable');
END;

CREATE TRIGGER model_rule_identity_registry_immutable_delete BEFORE DELETE ON model_rule_identity_registry BEGIN
    SELECT RAISE(ABORT, 'model_rule_identity_registry_immutable');
END;

CREATE TRIGGER group_identity_registry_immutable_update BEFORE UPDATE ON group_identity_registry BEGIN
    SELECT RAISE(ABORT, 'group_identity_registry_immutable');
END;

CREATE TRIGGER group_identity_registry_immutable_delete BEFORE DELETE ON group_identity_registry BEGIN
    SELECT RAISE(ABORT, 'group_identity_registry_immutable');
END;

CREATE TRIGGER channel_identity_registry_immutable_update BEFORE UPDATE ON channel_identity_registry BEGIN
    SELECT RAISE(ABORT, 'channel_identity_registry_immutable');
END;

CREATE TRIGGER channel_identity_registry_immutable_delete BEFORE DELETE ON channel_identity_registry BEGIN
    SELECT RAISE(ABORT, 'channel_identity_registry_immutable');
END;
