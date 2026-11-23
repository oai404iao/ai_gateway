CREATE TYPE public.api_format AS ENUM (
    'open_ai_chat_completions',
    'open_ai_responses',
    'open_ai_images'
);

CREATE FUNCTION public.enforce_active_model_routing_profile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    priced_model_deleted_at timestamptz;
BEGIN
    SELECT model.deleted_at
    INTO priced_model_deleted_at
    FROM model_routing_profiles AS profile
    JOIN models AS model ON model.id = profile.model_id
    WHERE profile.id = NEW.model_routing_profile_id
    FOR KEY SHARE OF profile, model;

    IF FOUND AND priced_model_deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'protocol rules cannot be changed under a deleted pricing model';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.enforce_active_profile_model() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    priced_model_deleted_at timestamptz;
BEGIN
    SELECT deleted_at
    INTO priced_model_deleted_at
    FROM models
    WHERE id = NEW.model_id
    FOR KEY SHARE;

    IF FOUND AND priced_model_deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'routing profiles cannot reference a deleted pricing model';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.enforce_active_test_pricing_model() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    pricing_model_deleted_at timestamptz;
BEGIN
    IF NEW.test_pricing_model_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT deleted_at
    INTO pricing_model_deleted_at
    FROM models
    WHERE id = NEW.test_pricing_model_id
    FOR KEY SHARE;

    IF FOUND AND pricing_model_deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'scheduled tests cannot reference a deleted pricing model';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.enforce_model_tombstone_lifecycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='model tombstones are immutable';
    END IF;
    IF NEW.deleted_at IS NOT NULL AND (
        EXISTS (SELECT 1 FROM channel_capabilities
                WHERE test_pricing_model_id=NEW.id AND deleted_at IS NULL)
        OR EXISTS (SELECT 1 FROM model_routing_profiles p
                   JOIN model_operation_rules r ON r.model_routing_profile_id=p.id
                   WHERE p.model_id=NEW.id AND r.enabled)
    ) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='model has active configuration references';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.guard_upstream_credential_connector() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.connector_kind<>OLD.connector_kind THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='credential connector is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.guard_upstream_credential_lifecycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='upstream_credentials_lifecycle';
    END IF;
    IF OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind OR
       (NEW.deleted_at IS NOT NULL AND EXISTS (
            SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
       )) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='upstream_credentials_lifecycle';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.invalidate_codex_credential_token() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.access_token IS DISTINCT FROM NEW.access_token AND NEW.deleted_at IS NULL THEN
        UPDATE upstream_credentials SET revision=gen_random_uuid() WHERE id=NEW.channel_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.preserve_canonical_config_tombstones() RETURNS trigger
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

CREATE FUNCTION public.preserve_identity_registry_history() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = format('%s rows are immutable historical identities', TG_TABLE_NAME);
END;
$$;

CREATE FUNCTION public.prevent_identity_tombstone_hard_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = format('%s rows must be retained as soft-delete tombstones', TG_TABLE_NAME);
END;
$$;

CREATE FUNCTION public.prevent_log_mutation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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

CREATE FUNCTION public.prevent_model_tombstone_hard_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = 'model rows must be retained as soft-delete tombstones';
END;
$$;

CREATE FUNCTION public.prevent_routed_model_identity_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.source_model_id <> OLD.source_model_id
       AND EXISTS (
           SELECT 1
           FROM model_routing_profiles
           WHERE model_id = OLD.id
       )
    THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'priced model %s is attached to a model rule and its client model ID is immutable',
                OLD.id
            );
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.protect_request_financial_facts() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'request financial facts and settlement receipts are immutable';
END;
$$;

CREATE FUNCTION public.protect_settlement_pending() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' OR NOT EXISTS (
        SELECT 1 FROM request_settlements WHERE request_id=OLD.request_id
    ) THEN
        RAISE EXCEPTION 'unsettled financial work cannot be removed or changed';
    END IF;
    RETURN OLD;
END;
$$;

CREATE FUNCTION public.protect_unfinished_request_ingress() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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

CREATE FUNCTION public.set_updated_at() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

CREATE TABLE public.api_key_channel_grants (
    api_key_id uuid NOT NULL,
    channel_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.api_key_policies (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    allowed_group_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    allowed_channel_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    CONSTRAINT api_key_policies_allowed_channel_ids_no_nulls CHECK ((array_position(allowed_channel_ids, NULL::uuid) IS NULL)),
    CONSTRAINT api_key_policies_allowed_group_ids_no_nulls CHECK ((array_position(allowed_group_ids, NULL::uuid) IS NULL))
);

CREATE TABLE public.api_key_policy_channel_grants (
    policy_id uuid NOT NULL,
    channel_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.api_keys (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    name character varying(100) NOT NULL,
    secret_value text NOT NULL,
    status text NOT NULL,
    expires_at timestamp with time zone,
    permissions text[] NOT NULL,
    allowed_group_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    requests_per_minute integer,
    max_concurrent_requests integer,
    quota_limit_amount numeric(24,8),
    quota_used_amount numeric(24,8) DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    allowed_channel_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    is_system boolean DEFAULT false NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    CONSTRAINT api_keys_allowed_channel_ids_no_nulls CHECK ((array_position(allowed_channel_ids, NULL::uuid) IS NULL)),
    CONSTRAINT api_keys_allowed_group_ids_no_nulls CHECK ((array_position(allowed_group_ids, NULL::uuid) IS NULL)),
    CONSTRAINT api_keys_check CHECK (((expires_at IS NULL) OR (expires_at > created_at))),
    CONSTRAINT api_keys_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT api_keys_deleted_state_check CHECK (((deleted_at IS NULL) OR (status = 'revoked'::text))),
    CONSTRAINT api_keys_max_concurrent_requests_check CHECK (((max_concurrent_requests IS NULL) OR (max_concurrent_requests > 0))),
    CONSTRAINT api_keys_permissions_check CHECK ((permissions <@ ARRAY['proxy'::text, 'models.read'::text])),
    CONSTRAINT api_keys_permissions_check1 CHECK ((array_position(permissions, NULL::text) IS NULL)),
    CONSTRAINT api_keys_permissions_nonempty CHECK ((cardinality(permissions) > 0)),
    CONSTRAINT api_keys_quota_limit_amount_check CHECK (((quota_limit_amount IS NULL) OR (quota_limit_amount >= (0)::numeric))),
    CONSTRAINT api_keys_quota_used_amount_check CHECK ((quota_used_amount >= (0)::numeric)),
    CONSTRAINT api_keys_requests_per_minute_check CHECK (((requests_per_minute IS NULL) OR (requests_per_minute > 0))),
    CONSTRAINT api_keys_status_check CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text, 'revoked'::text, 'expired'::text])))
);

CREATE TABLE public.audit_logs (
    id uuid NOT NULL,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    actor_user_id uuid,
    actor_type text NOT NULL,
    action character varying(100) NOT NULL,
    object_type text NOT NULL,
    object_id uuid NOT NULL,
    before_redacted jsonb,
    after_redacted jsonb,
    correlation_id character varying(100),
    reason character varying(500),
    actor_role text,
    CONSTRAINT audit_logs_actor_role_check CHECK (((actor_role IS NULL) OR (actor_role = ANY (ARRAY['user'::text, 'admin'::text])))),
    CONSTRAINT audit_logs_actor_type_check CHECK ((actor_type = ANY (ARRAY['user'::text, 'system'::text]))),
    CONSTRAINT audit_logs_after_redacted_check CHECK (((after_redacted IS NULL) OR (jsonb_typeof(after_redacted) = 'object'::text))),
    CONSTRAINT audit_logs_before_redacted_check CHECK (((before_redacted IS NULL) OR (jsonb_typeof(before_redacted) = 'object'::text)))
);

CREATE TABLE public.channel_capabilities (
    id uuid NOT NULL,
    channel_id uuid NOT NULL,
    operation text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    available_models text[] DEFAULT '{}'::text[] NOT NULL,
    request_compression text DEFAULT 'default'::text NOT NULL,
    test_model character varying(300),
    test_pricing_model_id uuid,
    auto_disabled boolean DEFAULT false NOT NULL,
    auto_disable_reason character varying(500),
    auto_disable_at timestamp with time zone,
    auto_disable_allowed boolean DEFAULT false NOT NULL,
    status_statistics_enabled boolean DEFAULT false NOT NULL,
    config_template_id uuid,
    override_document jsonb DEFAULT '{}'::jsonb NOT NULL,
    billing_multiplier numeric(24,12) DEFAULT 1 NOT NULL,
    revision uuid DEFAULT gen_random_uuid() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT channel_capabilities_auto_disable_check CHECK ((auto_disabled OR ((auto_disable_reason IS NULL) AND (auto_disable_at IS NULL)))),
    CONSTRAINT channel_capabilities_available_models_check CHECK ((array_position(available_models, NULL::text) IS NULL)),
    CONSTRAINT channel_capabilities_billing_multiplier_check CHECK ((billing_multiplier >= (0)::numeric)),
    CONSTRAINT channel_capabilities_deleted_state_check CHECK (((deleted_at IS NULL) OR ((NOT enabled) AND (NOT auto_disabled)))),
    CONSTRAINT channel_capabilities_operation_check CHECK ((operation = ANY (ARRAY['chat_completion'::text, 'responses'::text, 'responses-ws'::text, 'web_search'::text, 'images_edit'::text, 'images_generation'::text]))),
    CONSTRAINT channel_capabilities_override_document_check CHECK ((jsonb_typeof(override_document) = 'object'::text)),
    CONSTRAINT channel_capabilities_probe_check CHECK (((test_model IS NULL) OR (operation = ANY (ARRAY['chat_completion'::text, 'responses'::text])))),
    CONSTRAINT channel_capabilities_request_compression_check CHECK ((request_compression = ANY (ARRAY['default'::text, 'zstd'::text]))),
    CONSTRAINT channel_capabilities_request_compression_operation_check CHECK (((request_compression = 'default'::text) OR (operation = 'responses'::text))),
    CONSTRAINT channel_capabilities_test_model_catalogue_check CHECK (((test_model IS NULL) OR ((test_model)::text = ANY (available_models)))),
    CONSTRAINT channel_capabilities_test_model_pair_check CHECK (((test_model IS NULL) = (test_pricing_model_id IS NULL)))
);

CREATE TABLE public.channel_identity_registry (
    id uuid NOT NULL,
    label character varying(100) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    canonical_channel_id uuid,
    capability_id uuid,
    CONSTRAINT channel_identity_registry_label_check CHECK (((length(btrim((label)::text)) >= 1) AND (length(btrim((label)::text)) <= 100)))
);

CREATE TABLE public.codex_oauth_credentials (
    channel_id uuid NOT NULL,
    label character varying(100) NOT NULL,
    email character varying(320),
    account_id character varying(300),
    plan_type character varying(100),
    is_fedramp boolean DEFAULT false NOT NULL,
    id_token text NOT NULL,
    access_token text NOT NULL,
    refresh_token text NOT NULL,
    access_token_expires_at timestamp with time zone,
    last_refreshed_at timestamp with time zone NOT NULL,
    refresh_generation bigint DEFAULT 0 NOT NULL,
    reauth_required boolean DEFAULT false NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    quota_threshold_percent smallint DEFAULT 95 NOT NULL,
    runtime_status text DEFAULT 'active'::text NOT NULL,
    quota_allowed boolean,
    quota_limit_reached boolean,
    primary_used_percent integer,
    primary_window_seconds integer,
    primary_reset_at timestamp with time zone,
    secondary_used_percent integer,
    secondary_window_seconds integer,
    secondary_reset_at timestamp with time zone,
    quota_checked_at timestamp with time zone,
    last_error_code character varying(100),
    last_error_summary character varying(1000),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id character varying(300),
    deleted_at timestamp with time zone,
    quota_reset_credits_available bigint,
    proxy_id uuid,
    available_models jsonb DEFAULT '[]'::jsonb NOT NULL,
    CONSTRAINT codex_oauth_credentials_access_token_check CHECK ((length(access_token) > 0)),
    CONSTRAINT codex_oauth_credentials_account_id_check CHECK ((btrim((account_id)::text) <> ''::text)),
    CONSTRAINT codex_oauth_credentials_account_or_user_check CHECK (((account_id IS NOT NULL) OR (user_id IS NOT NULL))),
    CONSTRAINT codex_oauth_credentials_available_models_check CHECK ((jsonb_typeof(available_models) = 'array'::text)),
    CONSTRAINT codex_oauth_credentials_check CHECK (((NOT reauth_required) OR (runtime_status = ANY (ARRAY['unavailable'::text, 'disabled'::text])))),
    CONSTRAINT codex_oauth_credentials_id_token_check CHECK ((length(id_token) > 0)),
    CONSTRAINT codex_oauth_credentials_label_check CHECK ((btrim((label)::text) <> ''::text)),
    CONSTRAINT codex_oauth_credentials_primary_used_percent_check CHECK (((primary_used_percent >= 0) AND (primary_used_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_primary_window_seconds_check CHECK ((primary_window_seconds > 0)),
    CONSTRAINT codex_oauth_credentials_quota_threshold_percent_check CHECK (((quota_threshold_percent >= 1) AND (quota_threshold_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_refresh_generation_check CHECK ((refresh_generation >= 0)),
    CONSTRAINT codex_oauth_credentials_refresh_token_check CHECK ((length(refresh_token) > 0)),
    CONSTRAINT codex_oauth_credentials_reset_credits_check CHECK (((quota_reset_credits_available IS NULL) OR (quota_reset_credits_available >= 0))),
    CONSTRAINT codex_oauth_credentials_runtime_status_check CHECK ((runtime_status = ANY (ARRAY['active'::text, 'draining'::text, 'unavailable'::text, 'disabled'::text]))),
    CONSTRAINT codex_oauth_credentials_secondary_used_percent_check CHECK (((secondary_used_percent >= 0) AND (secondary_used_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_secondary_window_seconds_check CHECK ((secondary_window_seconds > 0)),
    CONSTRAINT codex_oauth_credentials_user_id_check CHECK (((user_id IS NULL) OR (btrim((user_id)::text) <> ''::text)))
);

CREATE TABLE public.codex_oauth_flows (
    id uuid NOT NULL,
    actor_user_id uuid NOT NULL,
    label character varying(100) NOT NULL,
    proxy_id uuid,
    quota_threshold_percent smallint NOT NULL,
    state_hash bytea NOT NULL,
    code_verifier text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT codex_oauth_flows_code_verifier_check CHECK (((length(code_verifier) >= 43) AND (length(code_verifier) <= 128))),
    CONSTRAINT codex_oauth_flows_label_check CHECK ((btrim((label)::text) <> ''::text)),
    CONSTRAINT codex_oauth_flows_quota_threshold_percent_check CHECK (((quota_threshold_percent >= 1) AND (quota_threshold_percent <= 100))),
    CONSTRAINT codex_oauth_flows_state_hash_check CHECK ((octet_length(state_hash) = 32))
);

CREATE TABLE public.codex_quota_reset_events (
    id uuid NOT NULL,
    credential_id uuid NOT NULL,
    actor_user_id uuid NOT NULL,
    requested_at timestamp with time zone NOT NULL,
    outcome text NOT NULL,
    windows_reset integer NOT NULL,
    primary_applied_at timestamp with time zone,
    secondary_applied_at timestamp with time zone,
    correlation_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT codex_quota_reset_events_outcome_check CHECK ((outcome = ANY (ARRAY['reset'::text, 'nothing_to_reset'::text, 'no_credit'::text, 'already_redeemed'::text]))),
    CONSTRAINT codex_quota_reset_events_windows_reset_check CHECK (((windows_reset >= 0) AND (windows_reset <= 2)))
);

CREATE TABLE public.codex_quota_window_periods (
    id uuid NOT NULL,
    credential_id uuid NOT NULL,
    window_kind text NOT NULL,
    window_seconds integer NOT NULL,
    started_at timestamp with time zone NOT NULL,
    scheduled_reset_at timestamp with time zone NOT NULL,
    ended_at timestamp with time zone,
    reset_reason text,
    initial_used_percent integer NOT NULL,
    last_used_percent integer NOT NULL,
    first_observed_at timestamp with time zone NOT NULL,
    last_observed_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT codex_quota_window_periods_check CHECK ((scheduled_reset_at > started_at)),
    CONSTRAINT codex_quota_window_periods_check1 CHECK (((ended_at IS NULL) OR (ended_at >= started_at))),
    CONSTRAINT codex_quota_window_periods_check2 CHECK ((((ended_at IS NULL) AND (reset_reason IS NULL)) OR ((ended_at IS NOT NULL) AND (reset_reason IS NOT NULL)))),
    CONSTRAINT codex_quota_window_periods_check3 CHECK ((last_observed_at >= first_observed_at)),
    CONSTRAINT codex_quota_window_periods_initial_used_percent_check CHECK (((initial_used_percent >= 0) AND (initial_used_percent <= 100))),
    CONSTRAINT codex_quota_window_periods_last_used_percent_check CHECK (((last_used_percent >= 0) AND (last_used_percent <= 100))),
    CONSTRAINT codex_quota_window_periods_reset_reason_check CHECK ((reset_reason = ANY (ARRAY['natural'::text, 'manual'::text, 'openai_official'::text]))),
    CONSTRAINT codex_quota_window_periods_window_kind_check CHECK ((window_kind = ANY (ARRAY['primary'::text, 'secondary'::text]))),
    CONSTRAINT codex_quota_window_periods_window_seconds_check CHECK ((window_seconds > 0))
);

CREATE TABLE public.config_templates (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    description text,
    document jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT config_templates_document_check CHECK ((jsonb_typeof(document) = 'object'::text))
);

CREATE TABLE public.group_identity_registry (
    id uuid NOT NULL,
    label character varying(100) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    canonical_group_id uuid,
    CONSTRAINT group_identity_registry_label_check CHECK (((length(btrim((label)::text)) >= 1) AND (length(btrim((label)::text)) <= 100)))
);

CREATE TABLE public.model_capability_candidates (
    tier_id uuid NOT NULL,
    operation text NOT NULL,
    capability_id uuid NOT NULL,
    upstream_model character varying(300) NOT NULL,
    weight integer NOT NULL,
    CONSTRAINT model_capability_candidates_upstream_model_check CHECK ((btrim((upstream_model)::text) <> ''::text)),
    CONSTRAINT model_capability_candidates_weight_check CHECK ((weight > 0))
);

CREATE TABLE public.model_capability_tiers (
    id uuid NOT NULL,
    rule_id uuid NOT NULL,
    operation text NOT NULL,
    priority integer NOT NULL,
    strategy text NOT NULL,
    CONSTRAINT model_capability_tiers_priority_check CHECK ((priority >= 0)),
    CONSTRAINT model_capability_tiers_strategy_check CHECK ((strategy = ANY (ARRAY['weighted_random'::text, 'weighted_round_robin'::text])))
);

CREATE TABLE public.model_operation_rules (
    id uuid NOT NULL,
    model_routing_profile_id uuid NOT NULL,
    operation text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT model_operation_rules_operation_check CHECK ((operation = ANY (ARRAY['chat_completion'::text, 'responses'::text, 'responses-ws'::text, 'web_search'::text, 'images_edit'::text, 'images_generation'::text])))
);

CREATE TABLE public.model_routing_profiles (
    id uuid NOT NULL,
    model_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.model_rule_identity_registry (
    id uuid NOT NULL,
    label text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    canonical_rule_id uuid,
    CONSTRAINT model_rule_identity_registry_label_check CHECK ((length(btrim(label)) > 0))
);

CREATE TABLE public.models (
    id uuid NOT NULL,
    source_model_id character varying(300) NOT NULL,
    display_name character varying(300) NOT NULL,
    provider_name character varying(200),
    enabled boolean DEFAULT true NOT NULL,
    currency character(3) DEFAULT 'USD'::bpchar NOT NULL,
    price_unit_tokens bigint NOT NULL,
    input_unit_price numeric(24,12) NOT NULL,
    cached_input_unit_price numeric(24,12) NOT NULL,
    cache_write_unit_price numeric(24,12) NOT NULL,
    output_unit_price numeric(24,12) NOT NULL,
    price_effective_at timestamp with time zone NOT NULL,
    source_payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    last_synced_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    advanced_billing jsonb DEFAULT '{"long_context_tiers": [], "request_multipliers": []}'::jsonb NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    CONSTRAINT models_advanced_billing_object CHECK ((jsonb_typeof(advanced_billing) = 'object'::text)),
    CONSTRAINT models_cache_write_unit_price_check CHECK ((cache_write_unit_price >= (0)::numeric)),
    CONSTRAINT models_cached_input_unit_price_check CHECK ((cached_input_unit_price >= (0)::numeric)),
    CONSTRAINT models_currency_usd_only CHECK ((currency = 'USD'::bpchar)),
    CONSTRAINT models_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT models_deleted_state_check CHECK (((deleted_at IS NULL) OR (NOT enabled))),
    CONSTRAINT models_input_unit_price_check CHECK ((input_unit_price >= (0)::numeric)),
    CONSTRAINT models_output_unit_price_check CHECK ((output_unit_price >= (0)::numeric)),
    CONSTRAINT models_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT models_source_payload_check CHECK ((jsonb_typeof(source_payload) = 'object'::text))
);

CREATE TABLE public.plugin_artifacts (
    plugin_id text NOT NULL,
    digest text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT plugin_artifacts_digest_check CHECK ((length(digest) = 64)),
    CONSTRAINT plugin_artifacts_manifest_check CHECK ((jsonb_typeof(manifest) = 'object'::text)),
    CONSTRAINT plugin_artifacts_plugin_id_check CHECK ((plugin_id <> 'general'::text))
);

CREATE TABLE public.plugin_install_jobs (
    id uuid NOT NULL,
    actor_user_id uuid NOT NULL,
    operation text NOT NULL,
    status text NOT NULL,
    plugin_id text,
    artifact_digest text,
    error_code text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT plugin_install_jobs_check CHECK (((operation <> 'install'::text) OR (status <> 'succeeded'::text) OR ((plugin_id IS NOT NULL) AND (artifact_digest IS NOT NULL)))),
    CONSTRAINT plugin_install_jobs_operation_check CHECK ((operation = ANY (ARRAY['install'::text, 'discover'::text]))),
    CONSTRAINT plugin_install_jobs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'succeeded'::text, 'failed'::text])))
);

CREATE TABLE public.plugin_settings (
    plugin_id text NOT NULL,
    schema_version integer NOT NULL,
    settings_value jsonb NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT plugin_settings_plugin_id_check CHECK ((plugin_id <> 'general'::text)),
    CONSTRAINT plugin_settings_revision_check CHECK ((revision > 0)),
    CONSTRAINT plugin_settings_schema_version_check CHECK ((schema_version > 0)),
    CONSTRAINT plugin_settings_settings_value_check CHECK ((jsonb_typeof(settings_value) = 'object'::text))
);

CREATE TABLE public.plugin_states (
    plugin_id text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    artifact_digest text,
    revision bigint DEFAULT 1 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT plugin_states_check CHECK (((NOT enabled) OR (artifact_digest IS NOT NULL))),
    CONSTRAINT plugin_states_plugin_id_check CHECK ((plugin_id <> 'general'::text)),
    CONSTRAINT plugin_states_revision_check CHECK ((revision > 0))
);

CREATE TABLE public.proxies (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    proxy_url text NOT NULL,
    username text,
    password text,
    no_proxy_hosts text[] DEFAULT '{}'::text[] NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT proxies_proxy_url_check CHECK ((proxy_url ~* '^(https?|socks4a?|socks5h?)://'::text))
);

CREATE TABLE public.registration_invitation_codes (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    code_hash bytea NOT NULL,
    max_uses bigint,
    used_count bigint DEFAULT 0 NOT NULL,
    expires_at timestamp with time zone,
    enabled boolean DEFAULT true NOT NULL,
    user_group_id uuid NOT NULL,
    initial_balance_amount numeric(24,8) DEFAULT 0 NOT NULL,
    created_by uuid NOT NULL,
    last_used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT registration_invitation_codes_check CHECK (((max_uses IS NULL) OR (used_count <= max_uses))),
    CONSTRAINT registration_invitation_codes_initial_balance_amount_check CHECK ((initial_balance_amount >= (0)::numeric)),
    CONSTRAINT registration_invitation_codes_max_uses_check CHECK (((max_uses IS NULL) OR (max_uses > 0))),
    CONSTRAINT registration_invitation_codes_used_count_check CHECK ((used_count >= 0))
);

CREATE TABLE public.request_metering_facts (
    id uuid NOT NULL,
    started_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone NOT NULL,
    user_id uuid NOT NULL,
    api_key_id uuid NOT NULL,
    request_source text NOT NULL,
    api_format public.api_format NOT NULL,
    api_operation text NOT NULL,
    request_protocol text NOT NULL,
    client_model character varying(300) NOT NULL,
    upstream_model character varying(300),
    model_rule_id uuid,
    channel_group_id uuid,
    channel_id uuid,
    model_id uuid,
    outcome text NOT NULL,
    input_tokens bigint,
    cached_input_tokens bigint,
    cache_write_tokens bigint,
    output_tokens bigint,
    reasoning_tokens bigint,
    currency character(3),
    price_unit_tokens bigint,
    price_effective_at timestamp with time zone,
    input_unit_price numeric(24,12),
    cached_input_unit_price numeric(24,12),
    cache_write_unit_price numeric(24,12),
    output_unit_price numeric(24,12),
    cost_amount numeric(24,8),
    peak_pricing boolean NOT NULL,
    amount_state text GENERATED ALWAYS AS (
CASE
    WHEN (outcome = ANY (ARRAY['failed'::text, 'cancelled'::text])) THEN 'zero_by_policy'::text
    WHEN ((cost_amount IS NULL) AND ((outcome = 'rejected'::text) OR (api_operation = 'web_search'::text))) THEN 'not_applicable'::text
    WHEN (cost_amount IS NULL) THEN 'unknown'::text
    WHEN ((model_id IS NOT NULL) AND (currency IS NOT NULL)) THEN 'priced'::text
    ELSE 'invalid'::text
END) STORED NOT NULL,
    upstream_credential_id uuid,
    CONSTRAINT request_metering_facts_cache_write_unit_price_check CHECK ((cache_write_unit_price >= (0)::numeric)),
    CONSTRAINT request_metering_facts_cached_input_unit_price_check CHECK ((cached_input_unit_price >= (0)::numeric)),
    CONSTRAINT request_metering_facts_check CHECK ((completed_at >= started_at)),
    CONSTRAINT request_metering_facts_check1 CHECK (((cached_input_tokens >= 0) AND (cached_input_tokens <= input_tokens))),
    CONSTRAINT request_metering_facts_check2 CHECK (((cache_write_tokens >= 0) AND (cache_write_tokens <= input_tokens))),
    CONSTRAINT request_metering_facts_check3 CHECK (((reasoning_tokens >= 0) AND (reasoning_tokens <= output_tokens))),
    CONSTRAINT request_metering_facts_check4 CHECK (((cached_input_tokens IS NULL) OR (input_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_check5 CHECK (((cache_write_tokens IS NULL) OR (input_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_check6 CHECK (((reasoning_tokens IS NULL) OR (output_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_cost_amount_check CHECK ((cost_amount >= (0)::numeric)),
    CONSTRAINT request_metering_facts_currency_check CHECK ((currency = 'USD'::bpchar)),
    CONSTRAINT request_metering_facts_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT request_metering_facts_input_unit_price_check CHECK ((input_unit_price >= (0)::numeric)),
    CONSTRAINT request_metering_facts_outcome_check CHECK ((outcome = ANY (ARRAY['succeeded'::text, 'failed'::text, 'rejected'::text, 'cancelled'::text]))),
    CONSTRAINT request_metering_facts_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT request_metering_facts_output_unit_price_check CHECK ((output_unit_price >= (0)::numeric)),
    CONSTRAINT request_metering_facts_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT request_metering_facts_request_protocol_check CHECK ((request_protocol = ANY (ARRAY['non_stream'::text, 'sse'::text, 'websocket'::text]))),
    CONSTRAINT request_metering_facts_request_source_check CHECK ((request_source = ANY (ARRAY['client'::text, 'scheduled_test'::text]))),
    CONSTRAINT request_metering_operation_format_check CHECK ((((api_format = 'open_ai_chat_completions'::public.api_format) AND (api_operation = 'chat_completion'::text)) OR ((api_format = 'open_ai_responses'::public.api_format) AND (api_operation = ANY (ARRAY['responses'::text, 'responses-ws'::text, 'web_search'::text]))) OR ((api_format = 'open_ai_images'::public.api_format) AND (api_operation = ANY (ARRAY['images_generation'::text, 'images_edit'::text]))))),
    CONSTRAINT request_metering_prices_check CHECK ((((currency IS NULL) AND (price_unit_tokens IS NULL) AND (price_effective_at IS NULL) AND (input_unit_price IS NULL) AND (cached_input_unit_price IS NULL) AND (cache_write_unit_price IS NULL) AND (output_unit_price IS NULL)) OR ((currency IS NOT NULL) AND (price_unit_tokens IS NOT NULL) AND (price_effective_at IS NOT NULL) AND (input_unit_price IS NOT NULL) AND (cached_input_unit_price IS NOT NULL) AND (cache_write_unit_price IS NOT NULL) AND (output_unit_price IS NOT NULL)))),
    CONSTRAINT request_metering_zero_policy_check CHECK (((outcome <> ALL (ARRAY['failed'::text, 'cancelled'::text])) OR ((cost_amount IS NOT NULL) AND (cost_amount = (0)::numeric))))
);

CREATE TABLE public.request_log_ingest (
    sequence bigint NOT NULL,
    request_log_id uuid NOT NULL,
    schema_version smallint NOT NULL,
    payload bytea NOT NULL,
    staged_at timestamp with time zone DEFAULT now() NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error_code character varying(100),
    metered_at timestamp with time zone,
    metering_attempt_count integer DEFAULT 0 NOT NULL,
    metering_next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    metering_last_error_code text,
    CONSTRAINT request_log_ingest_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT request_log_ingest_schema_version_check CHECK ((schema_version > 0))
)
WITH (autovacuum_vacuum_threshold='1000', autovacuum_vacuum_scale_factor='0.01', autovacuum_analyze_threshold='1000', autovacuum_analyze_scale_factor='0.02', autovacuum_vacuum_insert_threshold='1000', autovacuum_vacuum_insert_scale_factor='0.01');

ALTER TABLE public.request_log_ingest ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.request_log_ingest_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE public.request_logs (
    id uuid NOT NULL,
    started_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone NOT NULL,
    user_id uuid NOT NULL,
    api_key_id uuid NOT NULL,
    api_format public.api_format NOT NULL,
    client_model character varying(300) NOT NULL,
    upstream_model character varying(300),
    model_rule_id uuid,
    channel_group_id uuid,
    channel_id uuid,
    outcome text NOT NULL,
    response_status_code smallint,
    ttft_ms integer,
    total_duration_ms integer,
    output_tokens_per_second numeric(14,4),
    input_tokens bigint,
    cached_input_tokens bigint,
    cache_write_tokens bigint,
    output_tokens bigint,
    model_id uuid,
    currency character(3),
    price_unit_tokens bigint,
    price_effective_at timestamp with time zone,
    input_unit_price numeric(24,12),
    cached_input_unit_price numeric(24,12),
    cache_write_unit_price numeric(24,12),
    output_unit_price numeric(24,12),
    cost_amount numeric(24,8),
    error_code character varying(100),
    error_summary character varying(16384),
    request_source text DEFAULT 'client'::text NOT NULL,
    request_protocol text DEFAULT 'non_stream'::text NOT NULL,
    reasoning_tokens bigint,
    reasoning_effort character varying(32),
    fast_mode boolean DEFAULT false NOT NULL,
    api_operation text NOT NULL,
    peak_pricing boolean DEFAULT false NOT NULL,
    CONSTRAINT request_logs_api_operation_format_check CHECK ((((api_format = 'open_ai_chat_completions'::public.api_format) AND (api_operation = 'chat_completion'::text)) OR ((api_format = 'open_ai_responses'::public.api_format) AND (api_operation = ANY (ARRAY['responses'::text, 'responses-ws'::text, 'web_search'::text]))) OR ((api_format = 'open_ai_images'::public.api_format) AND (api_operation = ANY (ARRAY['images_generation'::text, 'images_edit'::text]))))),
    CONSTRAINT request_logs_cache_write_tokens_check CHECK ((cache_write_tokens >= 0)),
    CONSTRAINT request_logs_cache_write_unit_price_check CHECK ((cache_write_unit_price >= (0)::numeric)),
    CONSTRAINT request_logs_cached_input_tokens_check CHECK ((cached_input_tokens >= 0)),
    CONSTRAINT request_logs_cached_input_unit_price_check CHECK ((cached_input_unit_price >= (0)::numeric)),
    CONSTRAINT request_logs_check CHECK ((completed_at >= started_at)),
    CONSTRAINT request_logs_check1 CHECK (((cached_input_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cached_input_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check2 CHECK (((cache_write_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cache_write_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check3 CHECK ((((currency IS NULL) AND (price_unit_tokens IS NULL) AND (price_effective_at IS NULL) AND (input_unit_price IS NULL) AND (cached_input_unit_price IS NULL) AND (cache_write_unit_price IS NULL) AND (output_unit_price IS NULL)) OR ((currency IS NOT NULL) AND (price_unit_tokens IS NOT NULL) AND (price_effective_at IS NOT NULL) AND (input_unit_price IS NOT NULL) AND (cached_input_unit_price IS NOT NULL) AND (cache_write_unit_price IS NOT NULL) AND (output_unit_price IS NOT NULL)))),
    CONSTRAINT request_logs_cost_amount_check CHECK ((cost_amount >= (0)::numeric)),
    CONSTRAINT request_logs_currency_usd_only CHECK (((currency IS NULL) OR (currency = 'USD'::bpchar))),
    CONSTRAINT request_logs_failed_cancelled_zero_cost_check CHECK (((outcome <> ALL (ARRAY['failed'::text, 'cancelled'::text])) OR ((cost_amount IS NOT NULL) AND (cost_amount = (0)::numeric)))),
    CONSTRAINT request_logs_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT request_logs_input_unit_price_check CHECK ((input_unit_price >= (0)::numeric)),
    CONSTRAINT request_logs_outcome_check CHECK ((outcome = ANY (ARRAY['succeeded'::text, 'failed'::text, 'rejected'::text, 'cancelled'::text]))),
    CONSTRAINT request_logs_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT request_logs_output_tokens_per_second_check CHECK ((output_tokens_per_second >= (0)::numeric)),
    CONSTRAINT request_logs_output_unit_price_check CHECK ((output_unit_price >= (0)::numeric)),
    CONSTRAINT request_logs_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT request_logs_reasoning_tokens_check CHECK ((reasoning_tokens >= 0)),
    CONSTRAINT request_logs_reasoning_tokens_within_output CHECK (((reasoning_tokens IS NULL) OR ((output_tokens IS NOT NULL) AND (reasoning_tokens <= output_tokens)))),
    CONSTRAINT request_logs_request_protocol_check CHECK ((request_protocol = ANY (ARRAY['non_stream'::text, 'sse'::text, 'websocket'::text]))),
    CONSTRAINT request_logs_request_source_check CHECK ((request_source = ANY (ARRAY['client'::text, 'scheduled_test'::text]))),
    CONSTRAINT request_logs_response_status_code_check CHECK (((response_status_code >= 100) AND (response_status_code <= 599))),
    CONSTRAINT request_logs_total_duration_ms_check CHECK ((total_duration_ms >= 0)),
    CONSTRAINT request_logs_ttft_ms_check CHECK ((ttft_ms >= 0))
)
WITH (autovacuum_vacuum_threshold='1000', autovacuum_vacuum_scale_factor='0.02', autovacuum_analyze_threshold='1000', autovacuum_analyze_scale_factor='0.02');

CREATE TABLE public.request_settlement_pending (
    request_id uuid NOT NULL,
    completed_at timestamp with time zone NOT NULL
)
WITH (autovacuum_vacuum_scale_factor='0.02', autovacuum_analyze_scale_factor='0.01');

CREATE TABLE public.request_settlements (
    request_id uuid NOT NULL,
    cost_amount numeric(24,8) NOT NULL,
    currency character(3),
    settled_at timestamp with time zone DEFAULT now() NOT NULL,
    policy_version smallint DEFAULT 1 NOT NULL,
    CONSTRAINT request_settlements_cost_amount_check CHECK ((cost_amount >= (0)::numeric)),
    CONSTRAINT request_settlements_policy_version_check CHECK ((policy_version = 1))
);

CREATE TABLE public.routing_groups (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT routing_groups_deleted_state_check CHECK (((deleted_at IS NULL) OR (NOT enabled))),
    CONSTRAINT routing_groups_name_check CHECK (((length(btrim((name)::text)) >= 1) AND (length(btrim((name)::text)) <= 100)))
);

CREATE TABLE public.spend_leaderboard_entries (
    period text NOT NULL,
    period_start date NOT NULL,
    user_id uuid NOT NULL,
    rank bigint NOT NULL,
    request_count bigint NOT NULL,
    priced_request_count bigint NOT NULL,
    total_tokens bigint NOT NULL,
    cost_amount numeric(24,8) NOT NULL,
    CONSTRAINT spend_leaderboard_entries_check CHECK (((priced_request_count >= 0) AND (priced_request_count <= request_count))),
    CONSTRAINT spend_leaderboard_entries_cost_amount_check CHECK ((cost_amount >= (0)::numeric)),
    CONSTRAINT spend_leaderboard_entries_rank_check CHECK ((rank > 0)),
    CONSTRAINT spend_leaderboard_entries_request_count_check CHECK ((request_count >= 0)),
    CONSTRAINT spend_leaderboard_entries_total_tokens_check CHECK ((total_tokens >= 0))
);

CREATE TABLE public.spend_leaderboard_periods (
    period text NOT NULL,
    period_start date NOT NULL,
    period_end date NOT NULL,
    refreshed_at timestamp with time zone NOT NULL,
    total_cost_amount numeric(24,8) NOT NULL,
    CONSTRAINT spend_leaderboard_periods_check CHECK ((period_end > period_start)),
    CONSTRAINT spend_leaderboard_periods_period_check CHECK ((period = ANY (ARRAY['day'::text, 'week'::text, 'month'::text]))),
    CONSTRAINT spend_leaderboard_periods_total_cost_amount_check CHECK ((total_cost_amount >= (0)::numeric))
);

CREATE TABLE public.system_settings (
    setting_key character varying(100) NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT system_settings_value_check CHECK ((jsonb_typeof(value) = 'object'::text))
);

CREATE TABLE public.upstream_accesses (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    connector_kind text NOT NULL,
    base_url text NOT NULL,
    proxy_id uuid,
    connect_timeout_ms integer,
    response_header_timeout_ms integer,
    stream_idle_timeout_ms integer,
    enabled boolean DEFAULT true NOT NULL,
    revision uuid DEFAULT gen_random_uuid() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT upstream_accesses_base_url_check CHECK ((base_url ~* '^https?://'::text)),
    CONSTRAINT upstream_accesses_connect_timeout_check CHECK (((connect_timeout_ms IS NULL) OR (connect_timeout_ms > 0))),
    CONSTRAINT upstream_accesses_connector_kind_check CHECK ((connector_kind ~ '^[a-z][a-z0-9_-]{0,63}$'::text)),
    CONSTRAINT upstream_accesses_deleted_state_check CHECK (((deleted_at IS NULL) OR (NOT enabled))),
    CONSTRAINT upstream_accesses_name_check CHECK (((length(btrim((name)::text)) >= 1) AND (length(btrim((name)::text)) <= 100))),
    CONSTRAINT upstream_accesses_response_header_timeout_check CHECK (((response_header_timeout_ms IS NULL) OR (response_header_timeout_ms > 0))),
    CONSTRAINT upstream_accesses_stream_idle_timeout_check CHECK (((stream_idle_timeout_ms IS NULL) OR (stream_idle_timeout_ms > 0)))
);

CREATE TABLE public.upstream_channels (
    id uuid NOT NULL,
    group_id uuid NOT NULL,
    access_id uuid NOT NULL,
    credential_id uuid,
    name character varying(100) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    binding_revision uuid DEFAULT gen_random_uuid() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT upstream_channels_deleted_state_check CHECK (((deleted_at IS NULL) OR (NOT enabled))),
    CONSTRAINT upstream_channels_name_check CHECK (((length(btrim((name)::text)) >= 1) AND (length(btrim((name)::text)) <= 100)))
);

CREATE TABLE public.upstream_credentials (
    id uuid NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    header_name text,
    secret text,
    allowed_base_urls jsonb DEFAULT '[]'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision uuid DEFAULT gen_random_uuid() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    connector_kind text DEFAULT 'general'::text NOT NULL,
    CONSTRAINT upstream_credentials_auth_check CHECK ((((deleted_at IS NOT NULL) AND (secret IS NULL) AND (NOT enabled)) OR ((deleted_at IS NULL) AND (jsonb_array_length(allowed_base_urls) > 0) AND (((kind = 'codex_oauth'::text) AND (secret IS NULL) AND (header_name IS NULL)) OR ((kind = 'bearer'::text) AND (secret IS NOT NULL) AND (length(btrim(secret)) > 0) AND (header_name IS NULL)) OR ((kind = 'header'::text) AND (secret IS NOT NULL) AND (length(btrim(secret)) > 0) AND (header_name IS NOT NULL)))))),
    CONSTRAINT upstream_credentials_connector_kind_check CHECK (((connector_kind ~ '^[a-z][a-z0-9_-]{0,63}$'::text) AND ((kind = 'codex_oauth'::text) = (connector_kind = 'codex'::text)))),
    CONSTRAINT upstream_credentials_kind_check CHECK ((kind = ANY (ARRAY['bearer'::text, 'header'::text, 'codex_oauth'::text]))),
    CONSTRAINT upstream_credentials_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 100))),
    CONSTRAINT upstream_credentials_scope_check CHECK ((jsonb_typeof(allowed_base_urls) = 'array'::text))
);

CREATE TABLE public.user_group_codex_quota_visibility (
    user_group_id uuid NOT NULL,
    channel_group_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.user_groups (
    id uuid NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(500),
    default_api_key_policy_id uuid,
    system_role text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    filter_fast_mode boolean DEFAULT false NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    CONSTRAINT user_groups_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT user_groups_system_role_check CHECK (((system_role IS NULL) OR (system_role = ANY (ARRAY['user'::text, 'admin'::text]))))
);

CREATE TABLE public.user_invitations (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    invited_by uuid NOT NULL,
    token_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    accepted_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT user_invitations_check CHECK ((expires_at > created_at))
);

CREATE TABLE public.user_sessions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    refresh_token_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    rotated_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    user_agent character varying(512),
    purpose text DEFAULT 'normal'::text NOT NULL,
    CONSTRAINT user_sessions_check CHECK ((expires_at > created_at)),
    CONSTRAINT user_sessions_purpose_check CHECK ((purpose = ANY (ARRAY['normal'::text, 'password_change'::text])))
);

CREATE TABLE public.users (
    id uuid NOT NULL,
    display_name character varying(200) CONSTRAINT users_name_not_null NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    balance_amount numeric(24,8) DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    email character varying(320),
    role text DEFAULT 'user'::text NOT NULL,
    password_hash text,
    auth_version bigint DEFAULT 1 NOT NULL,
    password_changed_at timestamp with time zone,
    default_api_key_policy_id uuid,
    is_system boolean DEFAULT false NOT NULL,
    user_group_id uuid DEFAULT '00000000-0000-0000-0000-000000000101'::uuid NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    websocket_enabled boolean DEFAULT false NOT NULL,
    password_change_required boolean DEFAULT false NOT NULL,
    temporary_password_issued_at timestamp with time zone,
    temporary_password_expires_at timestamp with time zone,
    CONSTRAINT users_auth_version_check CHECK ((auth_version > 0)),
    CONSTRAINT users_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT users_role_check CHECK ((role = ANY (ARRAY['user'::text, 'admin'::text]))),
    CONSTRAINT users_status_check CHECK ((status = ANY (ARRAY['invited'::text, 'active'::text, 'suspended'::text, 'disabled'::text]))),
    CONSTRAINT users_temporary_password_state_check CHECK (((password_change_required AND (password_hash IS NOT NULL) AND (temporary_password_issued_at IS NOT NULL) AND (temporary_password_expires_at IS NOT NULL) AND (temporary_password_expires_at > temporary_password_issued_at)) OR ((NOT password_change_required) AND (temporary_password_issued_at IS NULL) AND (temporary_password_expires_at IS NULL))))
);

ALTER TABLE ONLY public.api_key_channel_grants
    ADD CONSTRAINT api_key_channel_grants_pkey PRIMARY KEY (api_key_id, channel_id);

ALTER TABLE ONLY public.api_key_policies
    ADD CONSTRAINT api_key_policies_name_key UNIQUE (name);

ALTER TABLE ONLY public.api_key_policies
    ADD CONSTRAINT api_key_policies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.api_key_policy_channel_grants
    ADD CONSTRAINT api_key_policy_channel_grants_pkey PRIMARY KEY (policy_id, channel_id);

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_secret_value_key UNIQUE (secret_value);

ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_channel_operation_key UNIQUE (channel_id, operation);

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_id_operation_key UNIQUE (id, operation);

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_identity_registry
    ADD CONSTRAINT channel_identity_registry_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.codex_oauth_credentials
    ADD CONSTRAINT codex_oauth_credentials_pkey PRIMARY KEY (channel_id);

ALTER TABLE ONLY public.codex_oauth_flows
    ADD CONSTRAINT codex_oauth_flows_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.codex_quota_reset_events
    ADD CONSTRAINT codex_quota_reset_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.codex_quota_window_periods
    ADD CONSTRAINT codex_quota_window_periods_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.config_templates
    ADD CONSTRAINT config_templates_name_key UNIQUE (name);

ALTER TABLE ONLY public.config_templates
    ADD CONSTRAINT config_templates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.group_identity_registry
    ADD CONSTRAINT group_identity_registry_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.model_capability_candidates
    ADD CONSTRAINT model_capability_candidates_pkey PRIMARY KEY (tier_id, capability_id, upstream_model);

ALTER TABLE ONLY public.model_capability_tiers
    ADD CONSTRAINT model_capability_tiers_id_operation_key UNIQUE (id, operation);

ALTER TABLE ONLY public.model_capability_tiers
    ADD CONSTRAINT model_capability_tiers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.model_capability_tiers
    ADD CONSTRAINT model_capability_tiers_rule_priority_key UNIQUE (rule_id, priority);

ALTER TABLE ONLY public.model_operation_rules
    ADD CONSTRAINT model_operation_rules_id_operation_key UNIQUE (id, operation);

ALTER TABLE ONLY public.model_operation_rules
    ADD CONSTRAINT model_operation_rules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.model_operation_rules
    ADD CONSTRAINT model_operation_rules_profile_operation_key UNIQUE (model_routing_profile_id, operation);

ALTER TABLE ONLY public.model_routing_profiles
    ADD CONSTRAINT model_routing_profiles_model_id_key UNIQUE (model_id);

ALTER TABLE ONLY public.model_routing_profiles
    ADD CONSTRAINT model_routing_profiles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.model_rule_identity_registry
    ADD CONSTRAINT model_rule_identity_registry_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.plugin_artifacts
    ADD CONSTRAINT plugin_artifacts_pkey PRIMARY KEY (plugin_id, digest);

ALTER TABLE ONLY public.plugin_install_jobs
    ADD CONSTRAINT plugin_install_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.plugin_settings
    ADD CONSTRAINT plugin_settings_pkey PRIMARY KEY (plugin_id);

ALTER TABLE ONLY public.plugin_states
    ADD CONSTRAINT plugin_states_pkey PRIMARY KEY (plugin_id);

ALTER TABLE ONLY public.proxies
    ADD CONSTRAINT proxies_name_key UNIQUE (name);

ALTER TABLE ONLY public.proxies
    ADD CONSTRAINT proxies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.registration_invitation_codes
    ADD CONSTRAINT registration_invitation_codes_code_hash_key UNIQUE (code_hash);

ALTER TABLE ONLY public.registration_invitation_codes
    ADD CONSTRAINT registration_invitation_codes_name_key UNIQUE (name);

ALTER TABLE ONLY public.registration_invitation_codes
    ADD CONSTRAINT registration_invitation_codes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.request_log_ingest
    ADD CONSTRAINT request_log_ingest_pkey PRIMARY KEY (sequence);

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.request_settlement_pending
    ADD CONSTRAINT request_settlement_pending_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.request_settlements
    ADD CONSTRAINT request_settlements_pkey PRIMARY KEY (request_id);

ALTER TABLE ONLY public.routing_groups
    ADD CONSTRAINT routing_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.spend_leaderboard_entries
    ADD CONSTRAINT spend_leaderboard_entries_pkey PRIMARY KEY (period, period_start, user_id);

ALTER TABLE ONLY public.spend_leaderboard_periods
    ADD CONSTRAINT spend_leaderboard_periods_pkey PRIMARY KEY (period, period_start);

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_pkey PRIMARY KEY (setting_key);

ALTER TABLE ONLY public.upstream_accesses
    ADD CONSTRAINT upstream_accesses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.upstream_channels
    ADD CONSTRAINT upstream_channels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.upstream_credentials
    ADD CONSTRAINT upstream_credentials_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_group_codex_quota_visibility
    ADD CONSTRAINT user_group_codex_quota_visibility_pkey PRIMARY KEY (user_group_id, channel_group_id);

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_system_role_key UNIQUE (system_role);

ALTER TABLE ONLY public.user_invitations
    ADD CONSTRAINT user_invitations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_name_key UNIQUE (display_name);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

CREATE INDEX api_key_channel_grants_channel_id_idx ON public.api_key_channel_grants USING btree (channel_id);

CREATE INDEX api_key_policy_channel_grants_channel_id_idx ON public.api_key_policy_channel_grants USING btree (channel_id);

CREATE UNIQUE INDEX api_keys_active_user_name_idx ON public.api_keys USING btree (user_id, name) WHERE (deleted_at IS NULL);

CREATE INDEX api_keys_deleted_at_idx ON public.api_keys USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX api_keys_user_id_status_idx ON public.api_keys USING btree (user_id, status);

CREATE INDEX audit_logs_actor_occurred_at_idx ON public.audit_logs USING btree (actor_user_id, occurred_at DESC);

CREATE INDEX audit_logs_object_occurred_at_idx ON public.audit_logs USING btree (object_type, object_id, occurred_at DESC);

CREATE INDEX channel_capabilities_deleted_at_idx ON public.channel_capabilities USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX codex_credentials_account_identity_idx ON public.codex_oauth_credentials USING btree (account_id, user_id) WHERE (deleted_at IS NULL);

CREATE INDEX codex_oauth_credentials_quota_idx ON public.codex_oauth_credentials USING btree (runtime_status, quota_checked_at);

CREATE INDEX codex_oauth_credentials_refresh_idx ON public.codex_oauth_credentials USING btree (runtime_status, access_token_expires_at);

CREATE INDEX codex_oauth_flows_expiry_idx ON public.codex_oauth_flows USING btree (expires_at) WHERE (completed_at IS NULL);

CREATE INDEX codex_quota_reset_events_pending_idx ON public.codex_quota_reset_events USING btree (credential_id, requested_at DESC) WHERE ((outcome = ANY (ARRAY['reset'::text, 'already_redeemed'::text])) AND (windows_reset > (
CASE
    WHEN (primary_applied_at IS NULL) THEN 0
    ELSE 1
END +
CASE
    WHEN (secondary_applied_at IS NULL) THEN 0
    ELSE 1
END)));

CREATE UNIQUE INDEX codex_quota_window_periods_current_idx ON public.codex_quota_window_periods USING btree (credential_id, window_kind) WHERE (ended_at IS NULL);

CREATE INDEX codex_quota_window_periods_history_idx ON public.codex_quota_window_periods USING btree (credential_id, window_kind, started_at DESC);

CREATE UNIQUE INDEX codex_quota_window_periods_identity_idx ON public.codex_quota_window_periods USING btree (credential_id, window_kind, started_at, scheduled_reset_at);

CREATE INDEX model_capability_candidates_capability_id_idx ON public.model_capability_candidates USING btree (capability_id);

CREATE UNIQUE INDEX models_active_source_model_id_idx ON public.models USING btree (source_model_id) WHERE (deleted_at IS NULL);

CREATE INDEX models_deleted_at_idx ON public.models USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX plugin_install_jobs_status_idx ON public.plugin_install_jobs USING btree (status, created_at);

CREATE INDEX registration_invitation_codes_user_group_id_idx ON public.registration_invitation_codes USING btree (user_group_id);

CREATE INDEX request_metering_facts_upstream_credential_id_idx ON public.request_metering_facts (upstream_credential_id) WHERE upstream_credential_id IS NOT NULL;

CREATE INDEX request_log_ingest_metering_idx ON public.request_log_ingest USING btree (sequence) WHERE (metered_at IS NULL);

CREATE INDEX request_log_ingest_metering_retry_idx ON public.request_log_ingest USING btree (metering_next_attempt_at, sequence) WHERE ((metered_at IS NULL) AND (metering_attempt_count > 0));

CREATE INDEX request_log_ingest_projection_idx ON public.request_log_ingest USING btree (sequence) WHERE (metered_at IS NOT NULL);

CREATE INDEX request_log_ingest_retry_idx ON public.request_log_ingest USING btree (next_attempt_at, sequence) WHERE (attempt_count > 0);

CREATE INDEX request_logs_api_format_started_at_idx ON public.request_logs USING btree (api_format, started_at DESC, id DESC);

CREATE INDEX request_logs_api_key_id_started_at_idx ON public.request_logs USING btree (api_key_id, started_at DESC);

CREATE INDEX request_logs_channel_group_model_started_at_idx ON public.request_logs USING btree (channel_group_id, upstream_model, started_at DESC) WHERE (channel_group_id IS NOT NULL);

CREATE INDEX request_logs_channel_id_started_at_idx ON public.request_logs USING btree (channel_id, started_at DESC);

CREATE INDEX request_logs_channel_model_started_at_idx ON public.request_logs USING btree (channel_id, upstream_model, started_at DESC) WHERE (channel_id IS NOT NULL);

CREATE INDEX request_logs_client_model_started_at_idx ON public.request_logs USING btree (client_model, started_at DESC, id DESC);

CREATE INDEX request_logs_failed_started_at_idx ON public.request_logs USING btree (started_at DESC) WHERE (outcome = 'failed'::text);

CREATE INDEX request_logs_outcome_started_at_idx ON public.request_logs USING btree (outcome, started_at DESC, id DESC);

CREATE INDEX request_logs_scheduled_test_started_at_idx ON public.request_logs USING btree (started_at DESC, id DESC) WHERE (request_source = 'scheduled_test'::text);

CREATE INDEX request_logs_started_at_id_idx ON public.request_logs USING btree (started_at DESC, id DESC);

CREATE INDEX request_logs_upstream_model_started_at_idx ON public.request_logs USING btree (upstream_model, started_at DESC, id DESC);

CREATE INDEX request_logs_user_id_started_at_idx ON public.request_logs USING btree (user_id, started_at DESC);

CREATE INDEX request_metering_channel_time_idx ON public.request_metering_facts USING btree (channel_id, started_at);

CREATE INDEX request_metering_key_time_idx ON public.request_metering_facts USING btree (api_key_id, started_at);

CREATE INDEX request_metering_reconciliation_idx ON public.request_metering_facts USING btree (amount_state, id) WHERE (amount_state = ANY (ARRAY['unknown'::text, 'invalid'::text]));

CREATE INDEX request_metering_time_idx ON public.request_metering_facts USING btree (started_at);

CREATE INDEX request_metering_user_time_idx ON public.request_metering_facts USING btree (user_id, started_at);

CREATE INDEX request_settlement_pending_order_idx ON public.request_settlement_pending USING btree (completed_at, request_id);

CREATE UNIQUE INDEX routing_groups_active_name_idx ON public.routing_groups USING btree (name) WHERE (deleted_at IS NULL);

CREATE INDEX routing_groups_deleted_at_idx ON public.routing_groups USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX spend_leaderboard_entries_period_rank_idx ON public.spend_leaderboard_entries USING btree (period, period_start, rank);

CREATE INDEX upstream_accesses_active_name_idx ON public.upstream_accesses USING btree (name) WHERE (deleted_at IS NULL);

CREATE INDEX upstream_accesses_deleted_at_idx ON public.upstream_accesses USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX upstream_channels_access_id_idx ON public.upstream_channels USING btree (access_id);

CREATE UNIQUE INDEX upstream_channels_active_group_name_idx ON public.upstream_channels USING btree (group_id, name) WHERE (deleted_at IS NULL);

CREATE INDEX upstream_channels_credential_id_idx ON public.upstream_channels USING btree (credential_id) WHERE (credential_id IS NOT NULL);

CREATE INDEX upstream_channels_deleted_at_idx ON public.upstream_channels USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX user_group_codex_quota_visibility_channel_group_idx ON public.user_group_codex_quota_visibility USING btree (channel_group_id);

CREATE UNIQUE INDEX user_groups_active_name_idx ON public.user_groups USING btree (name) WHERE (deleted_at IS NULL);

CREATE INDEX user_groups_default_api_key_policy_id_idx ON public.user_groups USING btree (default_api_key_policy_id);

CREATE INDEX user_groups_deleted_at_idx ON public.user_groups USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX user_invitations_user_id_active_idx ON public.user_invitations USING btree (user_id, expires_at DESC) WHERE ((accepted_at IS NULL) AND (revoked_at IS NULL));

CREATE INDEX user_sessions_user_id_active_idx ON public.user_sessions USING btree (user_id, expires_at DESC) WHERE (revoked_at IS NULL);

CREATE INDEX users_default_api_key_policy_id_idx ON public.users USING btree (default_api_key_policy_id);

CREATE INDEX users_deleted_at_idx ON public.users USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE UNIQUE INDEX users_email_lower_unique_idx ON public.users USING btree (lower((email)::text)) WHERE (email IS NOT NULL);

CREATE INDEX users_user_group_id_idx ON public.users USING btree (user_group_id) WHERE (deleted_at IS NULL);

CREATE TRIGGER api_key_policies_set_updated_at BEFORE UPDATE ON public.api_key_policies FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER api_keys_prevent_hard_delete BEFORE DELETE ON public.api_keys FOR EACH ROW EXECUTE FUNCTION public.prevent_identity_tombstone_hard_delete();

CREATE TRIGGER api_keys_set_updated_at BEFORE UPDATE ON public.api_keys FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER audit_logs_prevent_mutation BEFORE DELETE OR UPDATE ON public.audit_logs FOR EACH ROW EXECUTE FUNCTION public.prevent_log_mutation();

CREATE TRIGGER channel_capabilities_enforce_active_test_pricing_model BEFORE INSERT OR UPDATE OF test_pricing_model_id ON public.channel_capabilities FOR EACH ROW EXECUTE FUNCTION public.enforce_active_test_pricing_model();

CREATE TRIGGER channel_capabilities_preserve_tombstones BEFORE DELETE OR UPDATE ON public.channel_capabilities FOR EACH ROW EXECUTE FUNCTION public.preserve_canonical_config_tombstones();

CREATE TRIGGER channel_capabilities_set_updated_at BEFORE UPDATE ON public.channel_capabilities FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER channel_identity_registry_immutable BEFORE DELETE OR UPDATE ON public.channel_identity_registry FOR EACH ROW EXECUTE FUNCTION public.preserve_identity_registry_history();

CREATE TRIGGER codex_oauth_credentials_set_updated_at BEFORE UPDATE ON public.codex_oauth_credentials FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER codex_quota_window_periods_set_updated_at BEFORE UPDATE ON public.codex_quota_window_periods FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER codex_token_revision AFTER UPDATE OF access_token ON public.codex_oauth_credentials FOR EACH ROW EXECUTE FUNCTION public.invalidate_codex_credential_token();

CREATE TRIGGER config_templates_set_updated_at BEFORE UPDATE ON public.config_templates FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER group_identity_registry_immutable BEFORE DELETE OR UPDATE ON public.group_identity_registry FOR EACH ROW EXECUTE FUNCTION public.preserve_identity_registry_history();

CREATE TRIGGER model_operation_rules_enforce_active_pricing_model BEFORE INSERT OR UPDATE ON public.model_operation_rules FOR EACH ROW EXECUTE FUNCTION public.enforce_active_model_routing_profile();

CREATE TRIGGER model_operation_rules_set_updated_at BEFORE UPDATE ON public.model_operation_rules FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER model_routing_profiles_enforce_active_model BEFORE INSERT OR UPDATE OF model_id ON public.model_routing_profiles FOR EACH ROW EXECUTE FUNCTION public.enforce_active_profile_model();

CREATE TRIGGER model_routing_profiles_set_updated_at BEFORE UPDATE ON public.model_routing_profiles FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER model_rule_identity_registry_immutable BEFORE DELETE OR UPDATE ON public.model_rule_identity_registry FOR EACH ROW EXECUTE FUNCTION public.preserve_identity_registry_history();

CREATE TRIGGER models_enforce_tombstone_lifecycle BEFORE UPDATE ON public.models FOR EACH ROW EXECUTE FUNCTION public.enforce_model_tombstone_lifecycle();

CREATE TRIGGER models_prevent_hard_delete BEFORE DELETE ON public.models FOR EACH ROW EXECUTE FUNCTION public.prevent_model_tombstone_hard_delete();

CREATE TRIGGER models_prevent_routed_identity_change BEFORE UPDATE OF source_model_id ON public.models FOR EACH ROW EXECUTE FUNCTION public.prevent_routed_model_identity_change();

CREATE TRIGGER models_set_updated_at BEFORE UPDATE ON public.models FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER proxies_set_updated_at BEFORE UPDATE ON public.proxies FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER registration_invitation_codes_set_updated_at BEFORE UPDATE ON public.registration_invitation_codes FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE FUNCTION public.validate_request_settlement() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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

CREATE TRIGGER request_log_ingest_ack_guard BEFORE DELETE ON public.request_log_ingest FOR EACH ROW EXECUTE FUNCTION public.protect_unfinished_request_ingress();

CREATE TRIGGER request_logs_prevent_mutation BEFORE DELETE OR UPDATE ON public.request_logs FOR EACH ROW EXECUTE FUNCTION public.prevent_log_mutation();

CREATE TRIGGER request_metering_facts_immutable BEFORE DELETE OR UPDATE ON public.request_metering_facts FOR EACH ROW EXECUTE FUNCTION public.protect_request_financial_facts();

CREATE TRIGGER request_settlement_pending_protect BEFORE DELETE OR UPDATE ON public.request_settlement_pending FOR EACH ROW EXECUTE FUNCTION public.protect_settlement_pending();

CREATE TRIGGER request_settlements_immutable BEFORE DELETE OR UPDATE ON public.request_settlements FOR EACH ROW EXECUTE FUNCTION public.protect_request_financial_facts();

CREATE TRIGGER request_settlements_validate BEFORE INSERT ON public.request_settlements FOR EACH ROW EXECUTE FUNCTION public.validate_request_settlement();

CREATE TRIGGER routing_groups_preserve_tombstones BEFORE DELETE OR UPDATE ON public.routing_groups FOR EACH ROW EXECUTE FUNCTION public.preserve_canonical_config_tombstones();

CREATE TRIGGER routing_groups_set_updated_at BEFORE UPDATE ON public.routing_groups FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER system_settings_set_updated_at BEFORE UPDATE ON public.system_settings FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER upstream_accesses_preserve_tombstones BEFORE DELETE OR UPDATE ON public.upstream_accesses FOR EACH ROW EXECUTE FUNCTION public.preserve_canonical_config_tombstones();

CREATE TRIGGER upstream_accesses_set_updated_at BEFORE UPDATE ON public.upstream_accesses FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER upstream_channels_preserve_tombstones BEFORE DELETE OR UPDATE ON public.upstream_channels FOR EACH ROW EXECUTE FUNCTION public.preserve_canonical_config_tombstones();

CREATE TRIGGER upstream_channels_set_updated_at BEFORE UPDATE ON public.upstream_channels FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER upstream_credentials_connector_guard BEFORE UPDATE ON public.upstream_credentials FOR EACH ROW EXECUTE FUNCTION public.guard_upstream_credential_connector();

CREATE TRIGGER upstream_credentials_lifecycle BEFORE DELETE OR UPDATE ON public.upstream_credentials FOR EACH ROW EXECUTE FUNCTION public.guard_upstream_credential_lifecycle();

CREATE TRIGGER upstream_credentials_updated_at BEFORE UPDATE ON public.upstream_credentials FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER user_groups_prevent_hard_delete BEFORE DELETE ON public.user_groups FOR EACH ROW EXECUTE FUNCTION public.prevent_identity_tombstone_hard_delete();

CREATE TRIGGER user_groups_set_updated_at BEFORE UPDATE ON public.user_groups FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

CREATE TRIGGER users_prevent_hard_delete BEFORE DELETE ON public.users FOR EACH ROW EXECUTE FUNCTION public.prevent_identity_tombstone_hard_delete();

CREATE TRIGGER users_set_updated_at BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

ALTER TABLE ONLY public.api_key_channel_grants
    ADD CONSTRAINT api_key_channel_grants_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.api_key_channel_grants
    ADD CONSTRAINT api_key_channel_grants_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.upstream_channels(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.api_key_policy_channel_grants
    ADD CONSTRAINT api_key_policy_channel_grants_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.upstream_channels(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.api_key_policy_channel_grants
    ADD CONSTRAINT api_key_policy_channel_grants_policy_id_fkey FOREIGN KEY (policy_id) REFERENCES public.api_key_policies(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.upstream_channels(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_config_template_id_fkey FOREIGN KEY (config_template_id) REFERENCES public.config_templates(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.channel_capabilities
    ADD CONSTRAINT channel_capabilities_test_pricing_model_id_fkey FOREIGN KEY (test_pricing_model_id) REFERENCES public.models(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.channel_identity_registry
    ADD CONSTRAINT channel_identity_registry_canonical_channel_id_fkey FOREIGN KEY (canonical_channel_id) REFERENCES public.upstream_channels(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.channel_identity_registry
    ADD CONSTRAINT channel_identity_registry_capability_id_fkey FOREIGN KEY (capability_id) REFERENCES public.channel_capabilities(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_oauth_credentials
    ADD CONSTRAINT codex_oauth_credentials_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.upstream_credentials(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_oauth_credentials
    ADD CONSTRAINT codex_oauth_credentials_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES public.proxies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_oauth_flows
    ADD CONSTRAINT codex_oauth_flows_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_oauth_flows
    ADD CONSTRAINT codex_oauth_flows_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES public.proxies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_quota_reset_events
    ADD CONSTRAINT codex_quota_reset_events_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_quota_reset_events
    ADD CONSTRAINT codex_quota_reset_events_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.codex_oauth_credentials(channel_id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.codex_quota_window_periods
    ADD CONSTRAINT codex_quota_window_periods_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.codex_oauth_credentials(channel_id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.group_identity_registry
    ADD CONSTRAINT group_identity_registry_canonical_group_id_fkey FOREIGN KEY (canonical_group_id) REFERENCES public.routing_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.model_capability_candidates
    ADD CONSTRAINT model_capability_candidates_capability_fk FOREIGN KEY (capability_id, operation) REFERENCES public.channel_capabilities(id, operation) ON DELETE RESTRICT;

ALTER TABLE ONLY public.model_capability_candidates
    ADD CONSTRAINT model_capability_candidates_tier_fk FOREIGN KEY (tier_id, operation) REFERENCES public.model_capability_tiers(id, operation) ON DELETE CASCADE;

ALTER TABLE ONLY public.model_capability_tiers
    ADD CONSTRAINT model_capability_tiers_rule_fk FOREIGN KEY (rule_id, operation) REFERENCES public.model_operation_rules(id, operation) ON DELETE CASCADE;

ALTER TABLE ONLY public.model_operation_rules
    ADD CONSTRAINT model_operation_rules_model_routing_profile_id_fkey FOREIGN KEY (model_routing_profile_id) REFERENCES public.model_routing_profiles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.model_routing_profiles
    ADD CONSTRAINT model_routing_profiles_model_id_fkey FOREIGN KEY (model_id) REFERENCES public.models(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.model_rule_identity_registry
    ADD CONSTRAINT model_rule_identity_registry_canonical_rule_id_fkey FOREIGN KEY (canonical_rule_id) REFERENCES public.model_operation_rules(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.plugin_install_jobs
    ADD CONSTRAINT plugin_install_jobs_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.plugin_states
    ADD CONSTRAINT plugin_states_plugin_id_artifact_digest_fkey FOREIGN KEY (plugin_id, artifact_digest) REFERENCES public.plugin_artifacts(plugin_id, digest);

ALTER TABLE ONLY public.registration_invitation_codes
    ADD CONSTRAINT registration_invitation_codes_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.registration_invitation_codes
    ADD CONSTRAINT registration_invitation_codes_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES public.user_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES public.group_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channel_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_model_id_fkey FOREIGN KEY (model_id) REFERENCES public.models(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_model_rule_id_fkey FOREIGN KEY (model_rule_id) REFERENCES public.model_rule_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_logs
    ADD CONSTRAINT request_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES public.group_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channel_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_model_id_fkey FOREIGN KEY (model_id) REFERENCES public.models(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_model_rule_id_fkey FOREIGN KEY (model_rule_id) REFERENCES public.model_rule_identity_registry(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_upstream_credential_id_fkey FOREIGN KEY (upstream_credential_id) REFERENCES public.upstream_credentials(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_metering_facts
    ADD CONSTRAINT request_metering_facts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_settlement_pending
    ADD CONSTRAINT request_settlement_pending_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.request_metering_facts(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.request_settlements
    ADD CONSTRAINT request_settlements_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.request_metering_facts(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.spend_leaderboard_entries
    ADD CONSTRAINT spend_leaderboard_entries_period_period_start_fkey FOREIGN KEY (period, period_start) REFERENCES public.spend_leaderboard_periods(period, period_start) ON DELETE CASCADE;

ALTER TABLE ONLY public.spend_leaderboard_entries
    ADD CONSTRAINT spend_leaderboard_entries_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.upstream_accesses
    ADD CONSTRAINT upstream_accesses_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES public.proxies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.upstream_channels
    ADD CONSTRAINT upstream_channels_access_id_fkey FOREIGN KEY (access_id) REFERENCES public.upstream_accesses(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.upstream_channels
    ADD CONSTRAINT upstream_channels_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.upstream_credentials(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.upstream_channels
    ADD CONSTRAINT upstream_channels_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.routing_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_group_codex_quota_visibility
    ADD CONSTRAINT user_group_codex_quota_visibility_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES public.routing_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_group_codex_quota_visibility
    ADD CONSTRAINT user_group_codex_quota_visibility_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES public.user_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_default_api_key_policy_id_fkey FOREIGN KEY (default_api_key_policy_id) REFERENCES public.api_key_policies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_groups
    ADD CONSTRAINT user_groups_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_invitations
    ADD CONSTRAINT user_invitations_invited_by_fkey FOREIGN KEY (invited_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_invitations
    ADD CONSTRAINT user_invitations_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_default_api_key_policy_id_fkey FOREIGN KEY (default_api_key_policy_id) REFERENCES public.api_key_policies(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES public.user_groups(id) ON DELETE RESTRICT;

INSERT INTO user_groups (id, name, description, system_role)
VALUES
    (
        '00000000-0000-0000-0000-000000000101',
        'Default Users',
        'Default group for newly invited users.',
        'user'
    ),
    (
        '00000000-0000-0000-0000-000000000102',
        'Default Administrators',
        'Default group for newly invited administrators.',
        'admin'
    );
