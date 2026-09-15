-- Model routing owns explicit weighted (channel, upstream model) candidates.
-- Channel groups remain resource and authorization containers, but selecting a
-- group no longer leaves a group-level dependency in a model protocol rule.
--
-- This is a hard schema cutover. Versions that still write
-- model_rule_routing_groups/model_rule_routing_channels cannot run against the
-- migrated schema.
LOCK TABLE model_rules, model_rule_routing_tiers, model_rule_routing_groups,
    model_rule_routing_channels, channel_groups, channels
    IN ACCESS EXCLUSIVE MODE;

CREATE TABLE model_rule_routing_candidates (
    model_rule_id uuid NOT NULL,
    api_format api_format NOT NULL,
    priority integer NOT NULL,
    channel_id uuid NOT NULL,
    upstream_model varchar(300) NOT NULL
        CHECK (btrim(upstream_model) <> ''),
    weight integer NOT NULL CHECK (weight > 0),
    PRIMARY KEY (model_rule_id, priority, channel_id, upstream_model),
    CONSTRAINT model_rule_candidates_tier_fk
    FOREIGN KEY (model_rule_id, api_format, priority)
        REFERENCES model_rule_routing_tiers (model_rule_id, api_format, priority)
        ON DELETE CASCADE,
    CONSTRAINT model_rule_candidates_channel_format_fk
    FOREIGN KEY (channel_id, api_format)
        REFERENCES channels (id, api_format) ON DELETE RESTRICT
);

CREATE INDEX model_rule_routing_candidates_channel_id_idx
    ON model_rule_routing_candidates (channel_id);

-- Selected targets already own one model and weight per channel.
INSERT INTO model_rule_routing_candidates (
    model_rule_id,
    api_format,
    priority,
    channel_id,
    upstream_model,
    weight
)
SELECT
    target.model_rule_id,
    target.api_format,
    target.priority,
    channel_target.channel_id,
    channel_target.upstream_model,
    channel_target.weight
FROM model_rule_routing_groups AS target
JOIN model_rule_routing_channels AS channel_target
  ON channel_target.model_rule_id = target.model_rule_id
 AND channel_target.api_format = target.api_format
 AND channel_target.channel_group_id = target.channel_group_id
JOIN channel_groups AS channel_group
  ON channel_group.id = target.channel_group_id
 AND channel_group.deleted_at IS NULL
JOIN channels AS channel
  ON channel.id = channel_target.channel_id
 AND channel.deleted_at IS NULL
WHERE target.channel_selection = 'selected';

-- An all-channel target currently considers every active member a structural
-- target, even when that member no longer advertises the target model. Preserve
-- that disconnected/reconnectable state while making the membership explicit.
INSERT INTO model_rule_routing_candidates (
    model_rule_id,
    api_format,
    priority,
    channel_id,
    upstream_model,
    weight
)
SELECT
    target.model_rule_id,
    target.api_format,
    target.priority,
    channel.id,
    target.upstream_model,
    COALESCE(channel_override.weight, target.default_weight)
FROM model_rule_routing_groups AS target
JOIN channel_groups AS channel_group
  ON channel_group.id = target.channel_group_id
 AND channel_group.deleted_at IS NULL
JOIN channels AS channel
  ON channel.channel_group_id = target.channel_group_id
 AND channel.api_format = target.api_format
 AND channel.deleted_at IS NULL
LEFT JOIN model_rule_routing_channels AS channel_override
  ON channel_override.model_rule_id = target.model_rule_id
 AND channel_override.api_format = target.api_format
 AND channel_override.channel_group_id = target.channel_group_id
 AND channel_override.channel_id = channel.id
WHERE target.channel_selection = 'all';

CREATE TRIGGER model_rule_routing_candidates_lock_parent
BEFORE INSERT OR UPDATE OR DELETE ON model_rule_routing_candidates
FOR EACH ROW EXECUTE FUNCTION lock_model_rule_routing_parent();

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
              FROM model_rule_routing_candidates AS candidate
              WHERE candidate.model_rule_id = tier.model_rule_id
                AND candidate.priority = tier.priority
          )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'every routing tier for model protocol rule %s must contain at least one route candidate',
                checked_model_rule_id
            );
    END IF;

    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER model_rule_routing_candidates_validate_shape
AFTER INSERT OR UPDATE OR DELETE ON model_rule_routing_candidates
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_model_rule_routing_shape();

DROP TABLE model_rule_routing_channels;
DROP TABLE model_rule_routing_groups;

-- A legacy all-channel target may reference an empty group. The flat model has
-- no synthetic group target to retain, so normalize empty tiers and disable a
-- protocol that no longer has any explicit route.
DELETE FROM model_rule_routing_tiers AS tier
WHERE NOT EXISTS (
    SELECT 1
    FROM model_rule_routing_candidates AS candidate
    WHERE candidate.model_rule_id = tier.model_rule_id
      AND candidate.priority = tier.priority
);

UPDATE model_rules AS rule
SET enabled = false
WHERE rule.enabled
  AND NOT EXISTS (
      SELECT 1
      FROM model_rule_routing_tiers AS tier
      WHERE tier.model_rule_id = rule.id
  );
