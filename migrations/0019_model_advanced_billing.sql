ALTER TABLE models
    ADD COLUMN advanced_billing jsonb NOT NULL DEFAULT
        '{"long_context_tiers":[],"request_multipliers":[]}'::jsonb,
    ADD CONSTRAINT models_advanced_billing_object
        CHECK (jsonb_typeof(advanced_billing) = 'object');
