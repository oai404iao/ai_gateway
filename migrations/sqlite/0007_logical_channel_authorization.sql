CREATE TABLE api_key_channel_grants (
    api_key_id TEXT NOT NULL CONSTRAINT api_key_channel_grants_api_key_id_storage CHECK (ag_uuid_valid(api_key_id) AND instr(api_key_id,char(0))=0),
    channel_id TEXT NOT NULL CONSTRAINT api_key_channel_grants_channel_id_storage CHECK (ag_uuid_valid(channel_id) AND instr(channel_id,char(0))=0),
    origin_kind TEXT NOT NULL CONSTRAINT api_key_channel_grants_origin_kind_storage CHECK (instr(origin_kind,char(0))=0),
    origin_id TEXT NOT NULL CONSTRAINT api_key_channel_grants_origin_id_storage CHECK (ag_uuid_valid(origin_id) AND instr(origin_id,char(0))=0),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CONSTRAINT api_key_channel_grants_created_at_storage CHECK (ag_time_valid(created_at) AND instr(created_at,char(0))=0),
    CONSTRAINT api_key_channel_grants_pkey PRIMARY KEY(api_key_id,channel_id,origin_kind,origin_id),
    CONSTRAINT api_key_channel_grants_origin_kind_check CHECK(origin_kind IN ('group','channel')),
    CONSTRAINT api_key_channel_grants_api_key_id_fkey FOREIGN KEY(api_key_id) REFERENCES api_keys(id) ON DELETE CASCADE,
    CONSTRAINT api_key_channel_grants_channel_id_fkey FOREIGN KEY(channel_id) REFERENCES upstream_channels(id) ON DELETE CASCADE
) STRICT;
CREATE INDEX api_key_channel_grants_channel_id_idx ON api_key_channel_grants(channel_id);

CREATE TABLE api_key_policy_channel_grants (
    policy_id TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_policy_id_storage CHECK (ag_uuid_valid(policy_id) AND instr(policy_id,char(0))=0),
    channel_id TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_channel_id_storage CHECK (ag_uuid_valid(channel_id) AND instr(channel_id,char(0))=0),
    origin_kind TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_origin_kind_storage CHECK (instr(origin_kind,char(0))=0),
    origin_id TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_origin_id_storage CHECK (ag_uuid_valid(origin_id) AND instr(origin_id,char(0))=0),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CONSTRAINT api_key_policy_channel_grants_created_at_storage CHECK (ag_time_valid(created_at) AND instr(created_at,char(0))=0),
    CONSTRAINT api_key_policy_channel_grants_pkey PRIMARY KEY(policy_id,channel_id,origin_kind,origin_id),
    CONSTRAINT api_key_policy_channel_grants_origin_kind_check CHECK(origin_kind IN ('group','channel')),
    CONSTRAINT api_key_policy_channel_grants_policy_id_fkey FOREIGN KEY(policy_id) REFERENCES api_key_policies(id) ON DELETE CASCADE,
    CONSTRAINT api_key_policy_channel_grants_channel_id_fkey FOREIGN KEY(channel_id) REFERENCES upstream_channels(id) ON DELETE CASCADE
) STRICT;
CREATE INDEX api_key_policy_channel_grants_channel_id_idx ON api_key_policy_channel_grants(channel_id);

INSERT INTO api_key_channel_grants
SELECT api_key_id,c.channel_id,
       CASE WHEN origin_kind='capability' THEN 'channel' ELSE origin_kind END,
       CASE WHEN origin_kind='capability' THEN c.channel_id ELSE origin_id END,
       min(g.created_at)
FROM api_key_capability_grants g JOIN channel_capabilities c ON c.id=g.capability_id
GROUP BY 1,2,3,4;
INSERT INTO api_key_policy_channel_grants
SELECT policy_id,c.channel_id,
       CASE WHEN origin_kind='capability' THEN 'channel' ELSE origin_kind END,
       CASE WHEN origin_kind='capability' THEN c.channel_id ELSE origin_id END,
       min(g.created_at)
FROM api_key_policy_capability_grants g JOIN channel_capabilities c ON c.id=g.capability_id
GROUP BY 1,2,3,4;

UPDATE api_keys SET allowed_channel_ids=(
    SELECT json_group_array(id) FROM (
        SELECT value AS id FROM json_each(api_keys.allowed_channel_ids)
        UNION SELECT c.channel_id FROM api_key_capability_grants g
        JOIN channel_capabilities c ON c.id=g.capability_id
        WHERE g.api_key_id=api_keys.id AND g.origin_kind='capability'
        ORDER BY id)),updated_at=ag_now()
WHERE deleted_at IS NULL AND EXISTS (
    SELECT 1 FROM api_key_capability_grants g WHERE g.api_key_id=api_keys.id AND g.origin_kind='capability');
UPDATE api_key_policies SET allowed_channel_ids=(
    SELECT json_group_array(id) FROM (
        SELECT value AS id FROM json_each(api_key_policies.allowed_channel_ids)
        UNION SELECT c.channel_id FROM api_key_policy_capability_grants g
        JOIN channel_capabilities c ON c.id=g.capability_id
        WHERE g.policy_id=api_key_policies.id AND g.origin_kind='capability'
        ORDER BY id)),updated_at=ag_now()
WHERE EXISTS (SELECT 1 FROM api_key_policy_capability_grants g WHERE g.policy_id=api_key_policies.id AND g.origin_kind='capability');

UPDATE api_keys SET allowed_api_formats='["open_ai_chat_completions","open_ai_responses","open_ai_images"]',updated_at=ag_now()
WHERE deleted_at IS NULL AND allowed_api_formats<>'["open_ai_chat_completions","open_ai_responses","open_ai_images"]';
DROP TABLE api_key_capability_grants;
DROP TABLE api_key_policy_capability_grants;
