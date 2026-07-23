ALTER TABLE channels
    ADD COLUMN billing_multiplier numeric(24, 12) NOT NULL DEFAULT 1,
    ADD CONSTRAINT channels_billing_multiplier_non_negative
        CHECK (billing_multiplier >= 0);
