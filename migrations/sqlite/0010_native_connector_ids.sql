-- The startup writer rebuilds the access and credential tables in this same
-- transaction, preserving explicit IDs, timestamps, indexes and all fences.
CREATE TRIGGER upstream_credentials_connector_guard BEFORE UPDATE ON upstream_credentials
WHEN NEW.connector_kind<>OLD.connector_kind
BEGIN SELECT RAISE(ABORT,'credential connector is immutable'); END;
