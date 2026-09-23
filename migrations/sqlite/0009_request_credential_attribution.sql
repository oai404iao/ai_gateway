CREATE TABLE request_credential_attributions (
    request_id TEXT PRIMARY KEY NOT NULL REFERENCES request_metering_facts(id) ON DELETE RESTRICT
        CHECK(ag_uuid_valid(request_id)),
    known INTEGER NOT NULL CHECK(known IN (0,1)),
    credential_id TEXT REFERENCES upstream_credentials(id) ON DELETE RESTRICT
        CHECK(credential_id IS NULL OR ag_uuid_valid(credential_id)),
    CHECK(known OR credential_id IS NULL)
) STRICT;
CREATE INDEX request_credential_attributions_credential_idx
    ON request_credential_attributions(credential_id,request_id) WHERE known;
CREATE TRIGGER request_credential_attributions_no_update BEFORE UPDATE ON request_credential_attributions
BEGIN SELECT RAISE(ABORT,'request_credential_attribution_immutable'); END;
CREATE TRIGGER request_credential_attributions_no_delete BEFORE DELETE ON request_credential_attributions
BEGIN SELECT RAISE(ABORT,'request_credential_attribution_immutable'); END;
CREATE VIEW request_credential_identities AS
SELECT fact.id AS request_id,
       CASE WHEN attribution.known THEN attribution.credential_id
            ELSE identity.codex_credential_id END AS credential_id
FROM request_metering_facts fact
LEFT JOIN request_credential_attributions attribution ON attribution.request_id=fact.id
LEFT JOIN channel_identity_registry identity ON identity.id=fact.channel_id;
