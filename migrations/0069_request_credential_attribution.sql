CREATE TABLE request_credential_attributions (
    request_id uuid PRIMARY KEY REFERENCES request_metering_facts(id) ON DELETE RESTRICT,
    known boolean NOT NULL,
    credential_id uuid REFERENCES upstream_credentials(id) ON DELETE RESTRICT,
    CHECK (known OR credential_id IS NULL)
);
CREATE INDEX request_credential_attributions_credential_idx
    ON request_credential_attributions(credential_id,request_id) WHERE known;
CREATE TRIGGER request_credential_attributions_immutable
BEFORE UPDATE OR DELETE ON request_credential_attributions
FOR EACH ROW EXECUTE FUNCTION protect_request_financial_facts();

-- Known null means no authentication; only legacy events use the frozen
-- capability identity mapping. Rebinding a channel never rewrites that map.
CREATE VIEW request_credential_identities AS
SELECT fact.id AS request_id,
       CASE WHEN attribution.known THEN attribution.credential_id
            ELSE identity.codex_credential_id END AS credential_id
FROM request_metering_facts fact
LEFT JOIN request_credential_attributions attribution ON attribution.request_id=fact.id
LEFT JOIN channel_identity_registry identity ON identity.id=fact.channel_id;
