CREATE TRIGGER capabilities_active_probe_insert BEFORE INSERT ON channel_capabilities
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'capabilities_active_probe');
END;
CREATE TRIGGER capabilities_active_probe_update BEFORE UPDATE OF test_pricing_model_id ON channel_capabilities
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'capabilities_active_probe');
END;
CREATE TRIGGER operation_rules_active_model_insert BEFORE INSERT ON model_operation_rules
WHEN EXISTS (SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
             WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'operation_rules_active_model');
END;
CREATE TRIGGER operation_rules_active_model_update BEFORE UPDATE ON model_operation_rules
WHEN EXISTS (SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
             WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'operation_rules_active_model');
END;

DROP TRIGGER models_tombstone;
CREATE TRIGGER models_tombstone BEFORE UPDATE ON models
WHEN OLD.deleted_at IS NOT NULL OR (
    NEW.deleted_at IS NOT NULL AND (
        EXISTS (SELECT 1 FROM channel_capabilities WHERE test_pricing_model_id=NEW.id AND deleted_at IS NULL)
        OR EXISTS (SELECT 1 FROM model_routing_profiles p JOIN model_operation_rules r
                   ON r.model_routing_profile_id=p.id WHERE p.model_id=NEW.id AND r.enabled)
    )
) BEGIN
    SELECT RAISE(ABORT,'models_tombstone');
END;
