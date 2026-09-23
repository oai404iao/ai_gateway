CREATE OR REPLACE FUNCTION enforce_model_tombstone_lifecycle()
RETURNS trigger LANGUAGE plpgsql AS $$
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

CREATE TRIGGER channel_capabilities_enforce_active_test_pricing_model
BEFORE INSERT OR UPDATE OF test_pricing_model_id ON channel_capabilities
FOR EACH ROW EXECUTE FUNCTION enforce_active_test_pricing_model();

CREATE TRIGGER model_operation_rules_enforce_active_pricing_model
BEFORE INSERT OR UPDATE ON model_operation_rules
FOR EACH ROW EXECUTE FUNCTION enforce_active_model_routing_profile();
