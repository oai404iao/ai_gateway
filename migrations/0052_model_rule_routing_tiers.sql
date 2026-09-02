-- Routing priority, strategy, and weight belong to a model rule rather than
-- to globally shared channel groups and channels. Validate every legacy
-- reference before normalizing it so malformed arrays fail with an actionable
-- error instead of being silently lost by the backfill joins.
LOCK TABLE codex_oauth_flows, channel_groups, channels, model_rules
    IN ACCESS EXCLUSIVE MODE;

DO $$
DECLARE
    invalid_rule_id uuid;
    invalid_target_id uuid;
    conflict_priority integer;
    conflict_strategies text[];
BEGIN
    SELECT rule.id, duplicate.channel_group_id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL (
        SELECT selected.channel_group_id
        FROM unnest(rule.channel_group_ids) AS selected(channel_group_id)
        GROUP BY selected.channel_group_id
        HAVING count(*) > 1
        ORDER BY selected.channel_group_id
        LIMIT 1
    ) AS duplicate
    ORDER BY rule.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s repeats channel group %s',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Deduplicate model_rules.channel_group_ids before retrying migration 0052.';
    END IF;

    SELECT rule.id, duplicate.channel_id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL (
        SELECT selected.channel_id
        FROM unnest(rule.channel_ids) AS selected(channel_id)
        GROUP BY selected.channel_id
        HAVING count(*) > 1
        ORDER BY selected.channel_id
        LIMIT 1
    ) AS duplicate
    ORDER BY rule.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s repeats channel %s',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Deduplicate model_rules.channel_ids before retrying migration 0052.';
    END IF;

    SELECT rule.id, selected.channel_group_id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_group_ids)
        AS selected(channel_group_id)
    LEFT JOIN channel_groups AS channel_group
      ON channel_group.id = selected.channel_group_id
    WHERE channel_group.id IS NULL
    ORDER BY rule.id, selected.channel_group_id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s references missing channel group %s',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Repair model_rules.channel_group_ids before retrying migration 0052.';
    END IF;

    SELECT rule.id, channel_group.id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_group_ids)
        AS selected(channel_group_id)
    JOIN channel_groups AS channel_group
      ON channel_group.id = selected.channel_group_id
    WHERE channel_group.api_format <> rule.api_format
    ORDER BY rule.id, channel_group.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s and channel group %s have different API formats',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Remove the cross-format channel group from model_rules.channel_group_ids before retrying migration 0052.';
    END IF;

    SELECT rule.id, selected.channel_id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_ids)
        AS selected(channel_id)
    LEFT JOIN channels AS channel
      ON channel.id = selected.channel_id
    WHERE channel.id IS NULL
    ORDER BY rule.id, selected.channel_id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s references missing channel %s',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Repair model_rules.channel_ids before retrying migration 0052.';
    END IF;

    SELECT rule.id, channel.id
    INTO invalid_rule_id, invalid_target_id
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_ids)
        AS selected(channel_id)
    JOIN channels AS channel
      ON channel.id = selected.channel_id
    WHERE channel.api_format <> rule.api_format
    ORDER BY rule.id, channel.id
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'foreign_key_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s and channel %s have different API formats',
                invalid_rule_id,
                invalid_target_id
            ),
            HINT = 'Remove the cross-format channel from model_rules.channel_ids before retrying migration 0052.';
    END IF;

    -- Include groups selected directly even when they currently have no
    -- channels, as well as the owning groups of directly selected channels.
    -- Enabled state is deliberately irrelevant: latent selections must remain
    -- structurally valid when they are enabled later.
    WITH selected_groups AS (
        SELECT rule.id AS model_rule_id, channel_group.id AS channel_group_id
        FROM model_rules AS rule
        CROSS JOIN LATERAL unnest(rule.channel_group_ids)
            AS selected(channel_group_id)
        JOIN channel_groups AS channel_group
          ON channel_group.id = selected.channel_group_id

        UNION

        SELECT rule.id, channel.channel_group_id
        FROM model_rules AS rule
        CROSS JOIN LATERAL unnest(rule.channel_ids)
            AS selected(channel_id)
        JOIN channels AS channel
          ON channel.id = selected.channel_id
    ),
    conflicts AS (
        SELECT
            selected_group.model_rule_id,
            channel_group.priority,
            array_agg(DISTINCT channel_group.selection_strategy
                      ORDER BY channel_group.selection_strategy) AS strategies
        FROM selected_groups AS selected_group
        JOIN channel_groups AS channel_group
          ON channel_group.id = selected_group.channel_group_id
        GROUP BY selected_group.model_rule_id, channel_group.priority
        HAVING count(DISTINCT channel_group.selection_strategy) > 1
    )
    SELECT model_rule_id, priority, strategies
    INTO invalid_rule_id, conflict_priority, conflict_strategies
    FROM conflicts
    ORDER BY model_rule_id, priority
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model rule routing migration aborted: model rule %s has conflicting selection strategies at priority %s (%s)',
                invalid_rule_id,
                conflict_priority,
                array_to_string(conflict_strategies, ', ')
            ),
            HINT = 'Make every selected group at this rule priority use the same strategy before retrying migration 0052.';
    END IF;
END;
$$;

-- Carry api_format through the normalized relations so composite foreign keys
-- enforce the no-cross-format routing boundary in the database.
ALTER TABLE model_rules
    ADD CONSTRAINT model_rules_id_api_format_key UNIQUE (id, api_format);

ALTER TABLE channels
    ADD CONSTRAINT channels_id_group_api_format_key
        UNIQUE (id, channel_group_id, api_format);

CREATE TABLE model_rule_routing_tiers (
    model_rule_id uuid NOT NULL,
    api_format api_format NOT NULL,
    priority integer NOT NULL CHECK (priority >= 0),
    selection_strategy text NOT NULL
        CHECK (selection_strategy IN ('weighted_random', 'weighted_round_robin')),
    PRIMARY KEY (model_rule_id, priority),
    UNIQUE (model_rule_id, api_format, priority),
    CONSTRAINT model_rule_tiers_rule_format_fk
    FOREIGN KEY (model_rule_id, api_format)
        REFERENCES model_rules (id, api_format) ON DELETE CASCADE
);

CREATE TABLE model_rule_routing_groups (
    model_rule_id uuid NOT NULL,
    api_format api_format NOT NULL,
    priority integer NOT NULL,
    channel_group_id uuid NOT NULL,
    channel_selection text NOT NULL
        CHECK (channel_selection IN ('all', 'selected')),
    default_weight integer DEFAULT 100,
    PRIMARY KEY (model_rule_id, channel_group_id),
    UNIQUE (model_rule_id, api_format, channel_group_id),
    CONSTRAINT model_rule_groups_tier_fk
    FOREIGN KEY (model_rule_id, api_format, priority)
        REFERENCES model_rule_routing_tiers (model_rule_id, api_format, priority)
        ON DELETE CASCADE,
    CONSTRAINT model_rule_groups_group_format_fk
    FOREIGN KEY (channel_group_id, api_format)
        REFERENCES channel_groups (id, api_format) ON DELETE RESTRICT,
    CHECK (
        (
            channel_selection = 'all'
            AND default_weight IS NOT NULL
            AND default_weight > 0
        )
        OR (
            channel_selection = 'selected'
            AND default_weight IS NULL
        )
    )
);

CREATE INDEX model_rule_routing_groups_channel_group_id_idx
    ON model_rule_routing_groups (channel_group_id);

CREATE TABLE model_rule_routing_channels (
    model_rule_id uuid NOT NULL,
    api_format api_format NOT NULL,
    channel_group_id uuid NOT NULL,
    channel_id uuid NOT NULL,
    weight integer NOT NULL CHECK (weight > 0),
    PRIMARY KEY (model_rule_id, channel_group_id, channel_id),
    CONSTRAINT model_rule_channels_group_target_fk
    FOREIGN KEY (model_rule_id, api_format, channel_group_id)
        REFERENCES model_rule_routing_groups (
            model_rule_id,
            api_format,
            channel_group_id
        )
        ON DELETE CASCADE,
    CONSTRAINT model_rule_channels_channel_group_format_fk
    FOREIGN KEY (channel_id, channel_group_id, api_format)
        REFERENCES channels (id, channel_group_id, api_format)
        ON DELETE RESTRICT
);

CREATE INDEX model_rule_routing_channels_channel_id_idx
    ON model_rule_routing_channels (channel_id);

-- Serialize every child-graph mutation through its parent rule. This closes
-- the write-skew window where concurrent transactions could each remove a
-- different remaining target after observing the other one. Reparenting is
-- intentionally unsupported: control-plane updates replace a rule's graph
-- atomically instead of moving child rows between concurrency boundaries.
CREATE FUNCTION lock_model_rule_routing_parent()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    checked_model_rule_id uuid;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.model_rule_id <> OLD.model_rule_id THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'model-rule routing rows cannot be moved between rules';
    END IF;

    checked_model_rule_id := COALESCE(NEW.model_rule_id, OLD.model_rule_id);
    PERFORM id
    FROM model_rules
    WHERE id = checked_model_rule_id
    FOR UPDATE;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER model_rule_routing_tiers_lock_parent
BEFORE INSERT OR UPDATE OR DELETE ON model_rule_routing_tiers
FOR EACH ROW EXECUTE FUNCTION lock_model_rule_routing_parent();

CREATE TRIGGER model_rule_routing_groups_lock_parent
BEFORE INSERT OR UPDATE OR DELETE ON model_rule_routing_groups
FOR EACH ROW EXECUTE FUNCTION lock_model_rule_routing_parent();

CREATE TRIGGER model_rule_routing_channels_lock_parent
BEFORE INSERT OR UPDATE OR DELETE ON model_rule_routing_channels
FOR EACH ROW EXECUTE FUNCTION lock_model_rule_routing_parent();

-- Cross-table non-empty rules require deferred constraint triggers so one
-- model-rule replacement can delete and rebuild the complete child graph
-- atomically while no committed rule, tier, or selected target is empty.
CREATE FUNCTION validate_model_rule_routing_shape()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    checked_model_rule_id uuid;
BEGIN
    IF TG_TABLE_NAME = 'model_rules' THEN
        checked_model_rule_id := COALESCE(NEW.id, OLD.id);
    ELSE
        checked_model_rule_id := COALESCE(NEW.model_rule_id, OLD.model_rule_id);
    END IF;

    IF NOT EXISTS (SELECT 1 FROM model_rules WHERE id = checked_model_rule_id) THEN
        RETURN NULL;
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM model_rule_routing_tiers
        WHERE model_rule_id = checked_model_rule_id
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'model rule %s must contain at least one routing tier',
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
                'every routing tier for model rule %s must contain at least one channel group',
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
              FROM model_rule_routing_channels AS channel_weight
              WHERE channel_weight.model_rule_id = target.model_rule_id
                AND channel_weight.channel_group_id = target.channel_group_id
          )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format(
                'selected channel-group targets for model rule %s must contain at least one channel',
                checked_model_rule_id
            );
    END IF;

    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER model_rules_validate_routing_shape
AFTER INSERT OR UPDATE ON model_rules
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_model_rule_routing_shape();

CREATE CONSTRAINT TRIGGER model_rule_routing_tiers_validate_shape
AFTER INSERT OR UPDATE OR DELETE ON model_rule_routing_tiers
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_model_rule_routing_shape();

CREATE CONSTRAINT TRIGGER model_rule_routing_groups_validate_shape
AFTER INSERT OR UPDATE OR DELETE ON model_rule_routing_groups
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_model_rule_routing_shape();

CREATE CONSTRAINT TRIGGER model_rule_routing_channels_validate_shape
AFTER INSERT OR UPDATE OR DELETE ON model_rule_routing_channels
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_model_rule_routing_shape();

-- A legacy direct channel inherits its owning group's tier. A selected group
-- contributes a tier even when it is disabled or empty.
WITH selected_tiers AS (
    SELECT
        rule.id AS model_rule_id,
        rule.api_format,
        channel_group.priority,
        channel_group.selection_strategy
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_group_ids)
        AS selected(channel_group_id)
    JOIN channel_groups AS channel_group
      ON channel_group.id = selected.channel_group_id

    UNION

    SELECT
        rule.id,
        rule.api_format,
        channel_group.priority,
        channel_group.selection_strategy
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_ids)
        AS selected(channel_id)
    JOIN channels AS channel
      ON channel.id = selected.channel_id
    JOIN channel_groups AS channel_group
      ON channel_group.id = channel.channel_group_id
)
INSERT INTO model_rule_routing_tiers (
    model_rule_id,
    api_format,
    priority,
    selection_strategy
)
SELECT model_rule_id, api_format, priority, selection_strategy
FROM selected_tiers;

-- A group selected through the legacy group array keeps selecting all of its
-- channels with the legacy default weight of 100. An owning group reached
-- only through directly selected channels becomes a selected-only target.
-- The primary key guarantees that overlap becomes one all-channel target.
WITH group_targets AS (
    SELECT DISTINCT
        rule.id AS model_rule_id,
        rule.api_format,
        channel_group.priority,
        channel_group.id AS channel_group_id,
        'all'::text AS channel_selection,
        100::integer AS default_weight
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_group_ids)
        AS selected(channel_group_id)
    JOIN channel_groups AS channel_group
      ON channel_group.id = selected.channel_group_id

    UNION ALL

    SELECT DISTINCT
        rule.id,
        rule.api_format,
        channel_group.priority,
        channel_group.id,
        'selected'::text,
        NULL::integer
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_ids)
        AS selected(channel_id)
    JOIN channels AS channel
      ON channel.id = selected.channel_id
    JOIN channel_groups AS channel_group
      ON channel_group.id = channel.channel_group_id
    WHERE NOT (channel_group.id = ANY (rule.channel_group_ids))
)
INSERT INTO model_rule_routing_groups (
    model_rule_id,
    api_format,
    priority,
    channel_group_id,
    channel_selection,
    default_weight
)
SELECT
    model_rule_id,
    api_format,
    priority,
    channel_group_id,
    channel_selection,
    default_weight
FROM group_targets;

-- Every directly selected channel remains explicit, including weight 100.
-- Group-selected channels need rows only when their old global weight
-- overrides the new default. UNION (not UNION ALL) represents overlap once.
WITH channel_assignments AS (
    SELECT
        rule.id AS model_rule_id,
        rule.api_format,
        channel_group.id AS channel_group_id,
        channel.id AS channel_id,
        channel.weight
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_ids)
        AS selected(channel_id)
    JOIN channels AS channel
      ON channel.id = selected.channel_id
    JOIN channel_groups AS channel_group
      ON channel_group.id = channel.channel_group_id
    WHERE NOT (channel_group.id = ANY (rule.channel_group_ids))
       OR channel.weight <> 100

    UNION

    SELECT
        rule.id,
        rule.api_format,
        channel_group.id,
        channel.id,
        channel.weight
    FROM model_rules AS rule
    CROSS JOIN LATERAL unnest(rule.channel_group_ids)
        AS selected(channel_group_id)
    JOIN channel_groups AS channel_group
      ON channel_group.id = selected.channel_group_id
    JOIN channels AS channel
      ON channel.channel_group_id = channel_group.id
    WHERE channel.weight <> 100
)
INSERT INTO model_rule_routing_channels (
    model_rule_id,
    api_format,
    channel_group_id,
    channel_id,
    weight
)
SELECT model_rule_id, api_format, channel_group_id, channel_id, weight
FROM channel_assignments;

-- Codex managed Images groups and channels are projections of their canonical
-- Responses records. Priority, strategy, and routing weight are now
-- model-rule assignments, so projection creation must not copy those removed
-- global columns.
CREATE OR REPLACE FUNCTION create_codex_images_group()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.connector_kind = 'codex_oauth'
       AND NEW.api_format = 'open_ai_responses'
       AND NOT EXISTS (
           SELECT 1
           FROM channel_groups
           WHERE connector_pool_id = NEW.connector_pool_id
             AND api_format = 'open_ai_images'
       )
    THEN
        INSERT INTO channel_groups (
            id,
            name,
            api_format,
            connector_kind,
            connector_pool_id,
            enabled
        )
        VALUES (
            md5('ai-gateway:codex-images-group:' || NEW.id::text)::uuid,
            left(NEW.name, 55) || ' Images ' || NEW.id::text,
            'open_ai_images',
            'codex_oauth',
            NEW.connector_pool_id,
            false
        );
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION create_codex_credential_projections()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    images_group_id uuid;
    images_channel_id uuid;
    response_channel channels%ROWTYPE;
BEGIN
    INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
    VALUES (NEW.channel_id, 'open_ai_responses', NEW.channel_id);

    SELECT id
    INTO images_group_id
    FROM channel_groups
    WHERE connector_pool_id = NEW.connector_pool_id
      AND api_format = 'open_ai_images';

    IF images_group_id IS NULL THEN
        RAISE EXCEPTION 'Codex connector pool has no Images group';
    END IF;

    SELECT *
    INTO response_channel
    FROM channels
    WHERE id = NEW.channel_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Codex credential has no Responses channel';
    END IF;

    images_channel_id :=
        md5('ai-gateway:codex-images-channel:' || NEW.channel_id::text)::uuid;

    INSERT INTO channels (
        id,
        channel_group_id,
        api_format,
        name,
        base_url,
        enabled,
        billing_multiplier,
        proxy_id,
        override_document,
        connect_timeout_ms,
        response_header_timeout_ms,
        stream_idle_timeout_ms,
        upstream_auth_kind,
        available_models,
        auto_disable_allowed,
        supports_websocket
    )
    VALUES (
        images_channel_id,
        images_group_id,
        'open_ai_images',
        response_channel.name,
        response_channel.base_url,
        true,
        response_channel.billing_multiplier,
        response_channel.proxy_id,
        '{}'::jsonb,
        response_channel.connect_timeout_ms,
        response_channel.response_header_timeout_ms,
        response_channel.stream_idle_timeout_ms,
        'none',
        ARRAY['gpt-image-2']::text[],
        false,
        false
    );

    INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
    VALUES (NEW.channel_id, 'open_ai_images', images_channel_id);
    RETURN NEW;
END;
$$;

DROP TRIGGER channels_sync_codex_images_projection ON channels;

CREATE OR REPLACE FUNCTION sync_codex_images_projection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.api_format = 'open_ai_responses' THEN
        UPDATE channels AS images_channel
        SET name = NEW.name,
            base_url = NEW.base_url,
            billing_multiplier = NEW.billing_multiplier,
            proxy_id = NEW.proxy_id,
            connect_timeout_ms = NEW.connect_timeout_ms,
            response_header_timeout_ms = NEW.response_header_timeout_ms,
            stream_idle_timeout_ms = NEW.stream_idle_timeout_ms
        FROM codex_oauth_credential_channels AS response_projection
        JOIN codex_oauth_credential_channels AS images_projection
          ON images_projection.credential_id = response_projection.credential_id
         AND images_projection.api_format = 'open_ai_images'
        JOIN codex_oauth_credentials AS credential
          ON credential.channel_id = response_projection.credential_id
         AND credential.deleted_at IS NULL
        WHERE response_projection.channel_id = NEW.id
          AND response_projection.api_format = 'open_ai_responses'
          AND images_channel.id = images_projection.channel_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channels_sync_codex_images_projection
AFTER UPDATE OF name, base_url, billing_multiplier, proxy_id,
                connect_timeout_ms, response_header_timeout_ms, stream_idle_timeout_ms
ON channels
FOR EACH ROW EXECUTE FUNCTION sync_codex_images_projection();

-- Remove the denormalized sources only after every legacy assignment and
-- weight has been represented by the new relations.
ALTER TABLE codex_oauth_flows
    DROP COLUMN weight;

ALTER TABLE model_rules
    DROP COLUMN channel_group_ids,
    DROP COLUMN channel_ids;

ALTER TABLE channel_groups
    DROP COLUMN priority,
    DROP COLUMN selection_strategy;

ALTER TABLE channels
    DROP COLUMN weight;
