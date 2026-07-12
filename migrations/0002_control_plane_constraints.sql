ALTER TABLE model_rules
    ADD CONSTRAINT model_rules_channel_group_ids_no_nulls
        CHECK (array_position(channel_group_ids, NULL::uuid) IS NULL),
    ADD CONSTRAINT model_rules_channel_ids_no_nulls
        CHECK (array_position(channel_ids, NULL::uuid) IS NULL);

ALTER TABLE channels
    ADD CONSTRAINT channels_available_models_no_nulls
        CHECK (array_position(available_models, NULL::text) IS NULL);

ALTER TABLE api_keys
    ADD CONSTRAINT api_keys_permissions_nonempty CHECK (cardinality(permissions) > 0);
