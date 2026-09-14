-- Retain priced-model tombstones while releasing their client-visible model
-- IDs and preserving request-log and routing-history foreign keys.
ALTER TABLE models
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT,
    ADD CONSTRAINT models_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL)),
    ADD CONSTRAINT models_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled);

ALTER TABLE models
    DROP CONSTRAINT models_source_model_id_key;

CREATE UNIQUE INDEX models_active_source_model_id_idx
    ON models (source_model_id)
    WHERE deleted_at IS NULL;

CREATE INDEX models_deleted_at_idx
    ON models (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE FUNCTION enforce_model_tombstone_lifecycle()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'model tombstones are immutable';
    END IF;

    IF NEW.deleted_at IS NOT NULL THEN
        IF EXISTS (
            SELECT 1
            FROM channels
            WHERE test_pricing_model_id = NEW.id
        ) THEN
            RAISE EXCEPTION USING
                ERRCODE = 'check_violation',
                MESSAGE = 'a model cannot be tombstoned while scheduled tests reference it';
        END IF;
        IF EXISTS (
            SELECT 1
            FROM model_routing_profiles AS profile
            JOIN model_rules AS rule
              ON rule.model_routing_profile_id = profile.id
            WHERE profile.model_id = NEW.id
              AND rule.enabled
        ) THEN
            RAISE EXCEPTION USING
                ERRCODE = 'check_violation',
                MESSAGE = 'a model cannot be tombstoned while protocol rules are enabled';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER models_enforce_tombstone_lifecycle
BEFORE UPDATE ON models
FOR EACH ROW EXECUTE FUNCTION enforce_model_tombstone_lifecycle();

CREATE FUNCTION prevent_model_tombstone_hard_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = 'model rows must be retained as soft-delete tombstones';
END;
$$;

CREATE TRIGGER models_prevent_hard_delete
BEFORE DELETE ON models
FOR EACH ROW EXECUTE FUNCTION prevent_model_tombstone_hard_delete();

CREATE FUNCTION enforce_active_test_pricing_model()
RETURNS trigger
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

CREATE TRIGGER channels_enforce_active_test_pricing_model
BEFORE INSERT OR UPDATE OF test_pricing_model_id ON channels
FOR EACH ROW EXECUTE FUNCTION enforce_active_test_pricing_model();

CREATE FUNCTION enforce_active_profile_model()
RETURNS trigger
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

CREATE TRIGGER model_routing_profiles_enforce_active_model
BEFORE INSERT OR UPDATE OF model_id ON model_routing_profiles
FOR EACH ROW EXECUTE FUNCTION enforce_active_profile_model();

CREATE FUNCTION enforce_active_model_routing_profile()
RETURNS trigger
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

CREATE TRIGGER model_rules_enforce_active_pricing_model
BEFORE INSERT OR UPDATE ON model_rules
FOR EACH ROW EXECUTE FUNCTION enforce_active_model_routing_profile();
