CREATE TABLE api_key_channel_grants (
    api_key_id uuid NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES upstream_channels(id) ON DELETE CASCADE,
    origin_kind text NOT NULL CHECK (origin_kind IN ('group','channel')),
    origin_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (api_key_id,channel_id,origin_kind,origin_id)
);
CREATE INDEX api_key_channel_grants_channel_id_idx ON api_key_channel_grants(channel_id);

CREATE TABLE api_key_policy_channel_grants (
    policy_id uuid NOT NULL REFERENCES api_key_policies(id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES upstream_channels(id) ON DELETE CASCADE,
    origin_kind text NOT NULL CHECK (origin_kind IN ('group','channel')),
    origin_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (policy_id,channel_id,origin_kind,origin_id)
);
CREATE INDEX api_key_policy_channel_grants_channel_id_idx ON api_key_policy_channel_grants(channel_id);

-- Project existing targets, never the current membership of a selected group.
-- Dormant group/channel origins remain dormant; direct capability origins become
-- explicit authorization of that capability's logical channel.
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

UPDATE api_keys k SET allowed_channel_ids=ARRAY(
    SELECT DISTINCT id FROM (
        SELECT unnest(k.allowed_channel_ids) AS id
        UNION ALL SELECT c.channel_id FROM api_key_capability_grants g
        JOIN channel_capabilities c ON c.id=g.capability_id
        WHERE g.api_key_id=k.id AND g.origin_kind='capability'
    ) targets ORDER BY id)
WHERE k.deleted_at IS NULL AND EXISTS (
    SELECT 1 FROM api_key_capability_grants g WHERE g.api_key_id=k.id AND g.origin_kind='capability');
UPDATE api_key_policies p SET allowed_channel_ids=ARRAY(
    SELECT DISTINCT id FROM (
        SELECT unnest(p.allowed_channel_ids) AS id
        UNION ALL SELECT c.channel_id FROM api_key_policy_capability_grants g
        JOIN channel_capabilities c ON c.id=g.capability_id
        WHERE g.policy_id=p.id AND g.origin_kind='capability'
    ) targets ORDER BY id)
WHERE EXISTS (SELECT 1 FROM api_key_policy_capability_grants g WHERE g.policy_id=p.id AND g.origin_kind='capability');

-- Retained for v1 client compatibility, not an independent authorization gate.
UPDATE api_keys SET allowed_api_formats=ARRAY['open_ai_chat_completions','open_ai_responses','open_ai_images']::api_format[]
WHERE deleted_at IS NULL AND allowed_api_formats IS DISTINCT FROM
    ARRAY['open_ai_chat_completions','open_ai_responses','open_ai_images']::api_format[];

DROP TABLE api_key_capability_grants;
DROP TABLE api_key_policy_capability_grants;
