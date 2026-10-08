ALTER TABLE upstream_accesses DROP CONSTRAINT upstream_accesses_connector_kind_check;
ALTER TABLE upstream_accesses ADD CONSTRAINT upstream_accesses_connector_kind_check
    CHECK (connector_kind ~ '^[a-z][a-z0-9_-]{0,63}$');

ALTER TABLE upstream_credentials ALTER COLUMN connector_kind DROP EXPRESSION;
ALTER TABLE upstream_credentials ALTER COLUMN connector_kind SET DEFAULT 'general';
ALTER TABLE upstream_credentials ALTER COLUMN connector_kind SET NOT NULL;
ALTER TABLE upstream_credentials ADD CONSTRAINT upstream_credentials_connector_kind_check
    CHECK (connector_kind ~ '^[a-z][a-z0-9_-]{0,63}$'
        AND ((kind='codex_oauth') = (connector_kind='codex')));

CREATE FUNCTION guard_upstream_credential_connector()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.connector_kind<>OLD.connector_kind THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='credential connector is immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER upstream_credentials_connector_guard BEFORE UPDATE ON upstream_credentials
FOR EACH ROW EXECUTE FUNCTION guard_upstream_credential_connector();
