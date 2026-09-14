-- Separate priced client models, format-specific protocol rules, and
-- per-target upstream wire models.
LOCK TABLE models, model_rules, model_rule_routing_tiers, channel_groups,
    model_rule_routing_groups, model_rule_routing_channels, channels
    IN ACCESS EXCLUSIVE MODE;

DO $$
DECLARE
    invalid_rule_id uuid;
    invalid_client_model text;
    priced_model text;
    invalid_channel_id uuid;
    invalid_test_model text;
    invalid_channel_format api_format;
    invalid_connector_kind text;
BEGIN
    SELECT rule.id, rule.client_model, model.source_model_id
    INTO invalid_rule_id, invalid_client_model, priced_model
    FROM model_rules AS rule
    JOIN models AS model ON model.id = rule.upstream_model_id
    WHERE rule.client_model <> model.source_model_id
    ORDER BY rule.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model-rule hierarchy migration aborted: rule %s exposes client model %s but prices model %s',
                invalid_rule_id,
                invalid_client_model,
                priced_model
            ),
            HINT = 'Make every model_rules.client_model equal its priced models.source_model_id before retrying migration 0057.';
    END IF;

    SELECT channel.id, channel.test_model, channel.api_format, channel_group.connector_kind
    INTO invalid_channel_id, invalid_test_model, invalid_channel_format, invalid_connector_kind
    FROM channels AS channel
    JOIN channel_groups AS channel_group ON channel_group.id = channel.channel_group_id
    WHERE channel.test_model IS NOT NULL
      AND (
          channel.api_format = 'open_ai_images'
          OR channel_group.connector_kind <> 'openai_compatible'
      )
    ORDER BY channel.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model-rule hierarchy migration aborted: channel %s cannot schedule test model %s for format %s and connector %s',
                invalid_channel_id,
                invalid_test_model,
                invalid_channel_format,
                invalid_connector_kind
            ),
            HINT = 'Clear channels.test_model on Images and provider-managed channels before retrying migration 0057.';
    END IF;

    SELECT channel.id, channel.test_model
    INTO invalid_channel_id, invalid_test_model
    FROM channels AS channel
    LEFT JOIN models AS model ON model.source_model_id = channel.test_model
    WHERE channel.test_model IS NOT NULL
      AND model.id IS NULL
    ORDER BY channel.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'model-rule hierarchy migration aborted: channel %s test model %s has no priced model',
                invalid_channel_id,
                invalid_test_model
            ),
            HINT = 'Create the missing priced model or clear channels.test_model before retrying migration 0057.';
    END IF;
END;
$$;

CREATE TABLE model_routing_profiles (
    id uuid PRIMARY KEY,
    model_id uuid NOT NULL UNIQUE REFERENCES models (id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER model_routing_profiles_set_updated_at
BEFORE UPDATE ON model_routing_profiles
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

INSERT INTO model_routing_profiles (id, model_id, created_at, updated_at)
SELECT
    md5('ai-gateway:model-routing-profile:' || rule.upstream_model_id::text)::uuid,
    rule.upstream_model_id,
    min(rule.created_at),
    max(rule.updated_at)
FROM model_rules AS rule
GROUP BY rule.upstream_model_id;

ALTER TABLE model_rules
    ADD COLUMN model_routing_profile_id uuid;

UPDATE model_rules AS rule
SET model_routing_profile_id = profile.id
FROM model_routing_profiles AS profile
WHERE profile.model_id = rule.upstream_model_id;

SET CONSTRAINTS ALL IMMEDIATE;
SET CONSTRAINTS ALL DEFERRED;

ALTER TABLE model_rules
    ALTER COLUMN model_routing_profile_id SET NOT NULL,
    ADD CONSTRAINT model_rules_routing_profile_fk
        FOREIGN KEY (model_routing_profile_id)
        REFERENCES model_routing_profiles (id) ON DELETE CASCADE,
    ADD CONSTRAINT model_rules_profile_format_key
        UNIQUE (model_routing_profile_id, api_format);

ALTER TABLE model_rule_routing_groups
    ADD COLUMN upstream_model varchar(300);

ALTER TABLE model_rule_routing_channels
    ADD COLUMN upstream_model varchar(300);

UPDATE model_rule_routing_groups AS target
SET upstream_model = model.source_model_id
FROM model_rules AS rule
JOIN models AS model ON model.id = rule.upstream_model_id
WHERE rule.id = target.model_rule_id
  AND target.channel_selection = 'all';

UPDATE model_rule_routing_channels AS channel_target
SET upstream_model = model.source_model_id
FROM model_rule_routing_groups AS group_target
JOIN model_rules AS rule ON rule.id = group_target.model_rule_id
JOIN models AS model ON model.id = rule.upstream_model_id
WHERE channel_target.model_rule_id = group_target.model_rule_id
  AND channel_target.channel_group_id = group_target.channel_group_id
  AND group_target.channel_selection = 'selected';

SET CONSTRAINTS ALL IMMEDIATE;
SET CONSTRAINTS ALL DEFERRED;

ALTER TABLE model_rule_routing_groups
    DROP CONSTRAINT model_rule_routing_groups_check,
    ADD CONSTRAINT model_rule_routing_groups_selection_check
        CHECK (
            (
                channel_selection = 'all'
                AND default_weight IS NOT NULL
                AND default_weight > 0
                AND upstream_model IS NOT NULL
                AND btrim(upstream_model) <> ''
            )
            OR
            (
                channel_selection = 'selected'
                AND default_weight IS NULL
                AND upstream_model IS NULL
            )
        );

ALTER TABLE model_rule_routing_channels
    ADD CONSTRAINT model_rule_routing_channels_upstream_model_check
        CHECK (upstream_model IS NULL OR btrim(upstream_model) <> '');

CREATE OR REPLACE FUNCTION validate_model_rule_routing_shape()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    checked_model_rule_id uuid;
    rule_enabled boolean;
BEGIN
    IF TG_TABLE_NAME = 'model_rules' THEN
        checked_model_rule_id := COALESCE(NEW.id, OLD.id);
    ELSE
        checked_model_rule_id := COALESCE(NEW.model_rule_id, OLD.model_rule_id);
    END IF;

    SELECT enabled
    INTO rule_enabled
    FROM model_rules
    WHERE id = checked_model_rule_id;

    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF rule_enabled AND NOT EXISTS (
        SELECT 1
        FROM model_rule_routing_tiers
        WHERE model_rule_id = checked_model_rule_id
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'enabled model protocol rule %s must contain at least one routing tier',
                checked_model_rule_id
            );
    END IF;

    IF EXISTS (
        SELECT 1
        FROM model_rule_routing_tiers AS tier
        WHERE tier.model_rule_id = checked_model_rule_id
          AND NOT EXISTS (
              SELECT 1
              FROM model_rule_routing_groups AS target
              WHERE target.model_rule_id = tier.model_rule_id
                AND target.priority = tier.priority
          )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'every routing tier for model protocol rule %s must contain at least one channel group',
                checked_model_rule_id
            );
    END IF;

    IF EXISTS (
        SELECT 1
        FROM model_rule_routing_groups AS target
        WHERE target.model_rule_id = checked_model_rule_id
          AND target.channel_selection = 'selected'
          AND NOT EXISTS (
              SELECT 1
              FROM model_rule_routing_channels AS channel_target
              WHERE channel_target.model_rule_id = target.model_rule_id
                AND channel_target.channel_group_id = target.channel_group_id
          )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'selected channel-group targets for model protocol rule %s must contain at least one channel',
                checked_model_rule_id
            );
    END IF;

    IF EXISTS (
        SELECT 1
        FROM model_rule_routing_groups AS target
        JOIN model_rule_routing_channels AS channel_target
          ON channel_target.model_rule_id = target.model_rule_id
         AND channel_target.channel_group_id = target.channel_group_id
        WHERE target.model_rule_id = checked_model_rule_id
          AND (
              (
                  target.channel_selection = 'all'
                  AND channel_target.upstream_model IS NOT NULL
              )
              OR
              (
                  target.channel_selection = 'selected'
                  AND (
                      channel_target.upstream_model IS NULL
                      OR btrim(channel_target.upstream_model) = ''
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model protocol rule %s has an invalid channel-level upstream model',
                checked_model_rule_id
            );
    END IF;

    RETURN NULL;
END;
$$;

CREATE FUNCTION touch_model_routing_profile()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    profile_id uuid;
BEGIN
    profile_id := CASE
        WHEN TG_OP = 'DELETE' THEN OLD.model_routing_profile_id
        ELSE NEW.model_routing_profile_id
    END;
    UPDATE model_routing_profiles
    SET updated_at = now()
    WHERE id = profile_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER model_rules_touch_routing_profile
AFTER INSERT OR UPDATE OR DELETE ON model_rules
FOR EACH ROW EXECUTE FUNCTION touch_model_routing_profile();

CREATE FUNCTION prevent_routed_model_identity_change()
RETURNS trigger
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

CREATE TRIGGER models_prevent_routed_identity_change
BEFORE UPDATE OF source_model_id ON models
FOR EACH ROW EXECUTE FUNCTION prevent_routed_model_identity_change();

ALTER TABLE channels
    ADD COLUMN test_pricing_model_id uuid REFERENCES models (id) ON DELETE RESTRICT;

UPDATE channels AS channel
SET test_pricing_model_id = model.id
FROM models AS model
WHERE model.source_model_id = channel.test_model;

ALTER TABLE channels
    ADD CONSTRAINT channels_test_pricing_model_pair
        CHECK (
            (test_model IS NULL AND test_pricing_model_id IS NULL)
            OR
            (test_model IS NOT NULL AND test_pricing_model_id IS NOT NULL)
        );

ALTER TABLE model_rules
    DROP CONSTRAINT model_rules_client_model_api_format_key,
    DROP CONSTRAINT model_rules_upstream_model_id_fkey,
    DROP COLUMN client_model,
    DROP COLUMN upstream_model_id;
