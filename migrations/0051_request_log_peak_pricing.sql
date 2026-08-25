-- Preserve whether the immutable request-time billing decision selected a
-- weekly UTC multiplier above one so Console history never depends on the
-- model's current pricing configuration.
ALTER TABLE request_logs
    ADD COLUMN peak_pricing boolean NOT NULL DEFAULT false;
