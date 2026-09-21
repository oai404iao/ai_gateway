-- Canonical joint capability-cutover schema. Not yet a registered migration;
-- this file only creates the new source-of-truth tables. Legacy configuration
-- transfer, external-key migration, and cutover happen outside this DDL.

CREATE TABLE routing_groups (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    sharing_only boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT routing_groups_name_check
        CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    CONSTRAINT routing_groups_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled)
);

CREATE UNIQUE INDEX routing_groups_active_name_idx
    ON routing_groups (name)
    WHERE deleted_at IS NULL;

CREATE INDEX routing_groups_deleted_at_idx
    ON routing_groups (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER routing_groups_set_updated_at
BEFORE UPDATE ON routing_groups
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE upstream_accesses (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL,
    connector_kind text NOT NULL,
    base_url text NOT NULL,
    proxy_id uuid REFERENCES proxies (id) ON DELETE RESTRICT,
    connect_timeout_ms integer,
    response_header_timeout_ms integer,
    stream_idle_timeout_ms integer,
    enabled boolean NOT NULL DEFAULT true,
    revision uuid NOT NULL DEFAULT gen_random_uuid(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT upstream_accesses_name_check
        CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_accesses_connector_kind_check
        CHECK (connector_kind IN ('openai_compatible', 'codex_oauth')),
    CONSTRAINT upstream_accesses_base_url_check
        CHECK (base_url ~* '^https?://'),
    CONSTRAINT upstream_accesses_connect_timeout_check
        CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms > 0),
    CONSTRAINT upstream_accesses_response_header_timeout_check
        CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms > 0),
    CONSTRAINT upstream_accesses_stream_idle_timeout_check
        CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms > 0),
    CONSTRAINT upstream_accesses_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled)
);

-- Access names are inherited from legacy channels and may repeat across
-- routing groups, so the active lookup index is intentionally not unique.
CREATE INDEX upstream_accesses_active_name_idx
    ON upstream_accesses (name)
    WHERE deleted_at IS NULL;

CREATE INDEX upstream_accesses_deleted_at_idx
    ON upstream_accesses (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER upstream_accesses_set_updated_at
BEFORE UPDATE ON upstream_accesses
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE upstream_channels (
    id uuid PRIMARY KEY,
    group_id uuid NOT NULL REFERENCES routing_groups (id) ON DELETE RESTRICT,
    access_id uuid NOT NULL REFERENCES upstream_accesses (id) ON DELETE RESTRICT,
    credential_id uuid REFERENCES upstream_credentials (id) ON DELETE RESTRICT,
    name varchar(100) NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    binding_revision uuid NOT NULL DEFAULT gen_random_uuid(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT upstream_channels_name_check
        CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_channels_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled)
);

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

CREATE TRIGGER upstream_channels_set_updated_at
BEFORE UPDATE ON upstream_channels
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE channel_capabilities (
    id uuid PRIMARY KEY,
    channel_id uuid NOT NULL REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    operation text NOT NULL,
    transports text[] NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    available_models text[] NOT NULL DEFAULT '{}',
    request_compression text NOT NULL DEFAULT 'default',
    test_model varchar(300),
    test_pricing_model_id uuid REFERENCES models (id) ON DELETE RESTRICT,
    auto_disabled boolean NOT NULL DEFAULT false,
    auto_disable_reason varchar(500),
    auto_disable_at timestamptz,
    auto_disable_allowed boolean NOT NULL DEFAULT false,
    status_statistics_enabled boolean NOT NULL DEFAULT false,
    config_template_id uuid REFERENCES config_templates (id) ON DELETE RESTRICT,
    override_document jsonb NOT NULL DEFAULT '{}'::jsonb,
    billing_multiplier numeric(24, 12) NOT NULL DEFAULT 1,
    revision uuid NOT NULL DEFAULT gen_random_uuid(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT channel_capabilities_channel_operation_key
        UNIQUE (channel_id, operation),
    CONSTRAINT channel_capabilities_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT channel_capabilities_operation_check
        CHECK (operation IN (
            'chat_completions',
            'responses',
            'standalone_web_search',
            'images_generation',
            'images_edit'
        )),
    CONSTRAINT channel_capabilities_transports_check
        CHECK (
            cardinality(transports) > 0
            AND array_position(transports, NULL::text) IS NULL
            AND transports <@ ARRAY['http_json', 'http_sse', 'websocket', 'multipart']::text[]
        ),
    CONSTRAINT channel_capabilities_available_models_check
        CHECK (array_position(available_models, NULL::text) IS NULL),
    CONSTRAINT channel_capabilities_request_compression_check
        CHECK (request_compression IN ('default', 'zstd')),
    CONSTRAINT channel_capabilities_request_compression_operation_check
        CHECK (
            request_compression = 'default'
            OR (
                operation = 'responses'
                AND ('http_json' = ANY (transports) OR 'http_sse' = ANY (transports))
            )
        ),
    CONSTRAINT channel_capabilities_override_document_check
        CHECK (jsonb_typeof(override_document) = 'object'),
    CONSTRAINT channel_capabilities_billing_multiplier_check
        CHECK (billing_multiplier >= 0),
    CONSTRAINT channel_capabilities_test_model_pair_check
        CHECK ((test_model IS NULL) = (test_pricing_model_id IS NULL)),
    CONSTRAINT channel_capabilities_test_model_catalogue_check
        CHECK (test_model IS NULL OR test_model = ANY (available_models)),
    CONSTRAINT channel_capabilities_probe_check
        CHECK (
            test_model IS NULL
            OR (
                operation IN ('chat_completions', 'responses')
                AND 'http_json' = ANY (transports)
            )
        ),
    CONSTRAINT channel_capabilities_auto_disable_check
        CHECK (auto_disabled OR (auto_disable_reason IS NULL AND auto_disable_at IS NULL)),
    CONSTRAINT channel_capabilities_deleted_state_check
        CHECK (deleted_at IS NULL OR (NOT enabled AND NOT auto_disabled))
);

CREATE INDEX channel_capabilities_deleted_at_idx
    ON channel_capabilities (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE TRIGGER channel_capabilities_set_updated_at
BEFORE UPDATE ON channel_capabilities
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Canonical configuration rows are never physically deleted, and once a row is
-- tombstoned it can no longer be modified. Soft deletion is a one-way
-- transition; deleted_at is not kept in reverse sync with enabled.
CREATE FUNCTION preserve_canonical_config_tombstones()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format('%s rows must be retained as soft-delete tombstones', TG_TABLE_NAME);
    END IF;
    IF OLD.deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format('%s tombstones are immutable', TG_TABLE_NAME);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER routing_groups_preserve_tombstones
BEFORE UPDATE OR DELETE ON routing_groups
FOR EACH ROW EXECUTE FUNCTION preserve_canonical_config_tombstones();

CREATE TRIGGER upstream_accesses_preserve_tombstones
BEFORE UPDATE OR DELETE ON upstream_accesses
FOR EACH ROW EXECUTE FUNCTION preserve_canonical_config_tombstones();

CREATE TRIGGER upstream_channels_preserve_tombstones
BEFORE UPDATE OR DELETE ON upstream_channels
FOR EACH ROW EXECUTE FUNCTION preserve_canonical_config_tombstones();

CREATE TRIGGER channel_capabilities_preserve_tombstones
BEFORE UPDATE OR DELETE ON channel_capabilities
FOR EACH ROW EXECUTE FUNCTION preserve_canonical_config_tombstones();

CREATE TABLE model_operation_rules (
    id uuid PRIMARY KEY,
    model_routing_profile_id uuid NOT NULL
        REFERENCES model_routing_profiles (id) ON DELETE CASCADE,
    operation text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT model_operation_rules_profile_operation_key
        UNIQUE (model_routing_profile_id, operation),
    CONSTRAINT model_operation_rules_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_operation_rules_operation_check
        CHECK (operation IN (
            'chat_completions',
            'responses',
            'standalone_web_search',
            'images_generation',
            'images_edit'
        ))
);

CREATE TRIGGER model_operation_rules_set_updated_at
BEFORE UPDATE ON model_operation_rules
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE model_capability_tiers (
    id uuid PRIMARY KEY,
    rule_id uuid NOT NULL,
    operation text NOT NULL,
    priority integer NOT NULL,
    strategy text NOT NULL,
    CONSTRAINT model_capability_tiers_rule_priority_key
        UNIQUE (rule_id, priority),
    CONSTRAINT model_capability_tiers_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_capability_tiers_priority_check
        CHECK (priority >= 0),
    CONSTRAINT model_capability_tiers_strategy_check
        CHECK (strategy IN ('weighted_random', 'weighted_round_robin')),
    CONSTRAINT model_capability_tiers_rule_fk
        FOREIGN KEY (rule_id, operation)
        REFERENCES model_operation_rules (id, operation) ON DELETE CASCADE
);

CREATE TABLE model_capability_candidates (
    tier_id uuid NOT NULL,
    operation text NOT NULL,
    capability_id uuid NOT NULL,
    upstream_model varchar(300) NOT NULL,
    weight integer NOT NULL,
    PRIMARY KEY (tier_id, capability_id, upstream_model),
    CONSTRAINT model_capability_candidates_tier_fk
        FOREIGN KEY (tier_id, operation)
        REFERENCES model_capability_tiers (id, operation) ON DELETE CASCADE,
    CONSTRAINT model_capability_candidates_capability_fk
        FOREIGN KEY (capability_id, operation)
        REFERENCES channel_capabilities (id, operation) ON DELETE RESTRICT,
    CONSTRAINT model_capability_candidates_upstream_model_check
        CHECK (btrim(upstream_model) <> ''),
    CONSTRAINT model_capability_candidates_weight_check
        CHECK (weight > 0)
);

CREATE INDEX model_capability_candidates_capability_id_idx
    ON model_capability_candidates (capability_id);

CREATE TABLE api_key_capability_grants (
    api_key_id uuid NOT NULL REFERENCES api_keys (id) ON DELETE CASCADE,
    capability_id uuid NOT NULL REFERENCES channel_capabilities (id) ON DELETE CASCADE,
    origin_kind text NOT NULL,
    origin_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (api_key_id, capability_id, origin_kind, origin_id),
    CONSTRAINT api_key_capability_grants_origin_kind_check
        CHECK (origin_kind IN ('group', 'channel', 'capability')),
    CONSTRAINT api_key_capability_grants_capability_origin_check
        CHECK (origin_kind <> 'capability' OR origin_id = capability_id)
);

CREATE INDEX api_key_capability_grants_capability_id_idx
    ON api_key_capability_grants (capability_id);

CREATE TABLE api_key_policy_capability_grants (
    policy_id uuid NOT NULL REFERENCES api_key_policies (id) ON DELETE CASCADE,
    capability_id uuid NOT NULL REFERENCES channel_capabilities (id) ON DELETE CASCADE,
    origin_kind text NOT NULL,
    origin_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (policy_id, capability_id, origin_kind, origin_id),
    CONSTRAINT api_key_policy_capability_grants_origin_kind_check
        CHECK (origin_kind IN ('group', 'channel', 'capability')),
    CONSTRAINT api_key_policy_capability_grants_capability_origin_check
        CHECK (origin_kind <> 'capability' OR origin_id = capability_id)
);

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
    id uuid PRIMARY KEY,
    label varchar(100) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    canonical_group_id uuid REFERENCES routing_groups (id) ON DELETE RESTRICT,
    CONSTRAINT group_identity_registry_label_check
        CHECK (length(btrim(label)) BETWEEN 1 AND 100)
);

CREATE TABLE channel_identity_registry (
    id uuid PRIMARY KEY,
    label varchar(100) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    canonical_channel_id uuid REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    codex_credential_id uuid REFERENCES upstream_credentials (id) ON DELETE RESTRICT,
    capability_id uuid REFERENCES channel_capabilities (id) ON DELETE RESTRICT,
    CONSTRAINT channel_identity_registry_label_check
        CHECK (length(btrim(label)) BETWEEN 1 AND 100)
);

CREATE TABLE model_rule_identity_registry (
    id uuid PRIMARY KEY,
    label text NOT NULL CHECK (length(btrim(label)) > 0),
    created_at timestamptz NOT NULL,
    canonical_rule_id uuid REFERENCES model_operation_rules (id) ON DELETE RESTRICT
);

CREATE FUNCTION preserve_identity_registry_history()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = format('%s rows are immutable historical identities', TG_TABLE_NAME);
END;
$$;

CREATE TRIGGER group_identity_registry_immutable
BEFORE UPDATE OR DELETE ON group_identity_registry
FOR EACH ROW EXECUTE FUNCTION preserve_identity_registry_history();

CREATE TRIGGER channel_identity_registry_immutable
BEFORE UPDATE OR DELETE ON channel_identity_registry
FOR EACH ROW EXECUTE FUNCTION preserve_identity_registry_history();

CREATE TRIGGER model_rule_identity_registry_immutable
BEFORE UPDATE OR DELETE ON model_rule_identity_registry
FOR EACH ROW EXECUTE FUNCTION preserve_identity_registry_history();
