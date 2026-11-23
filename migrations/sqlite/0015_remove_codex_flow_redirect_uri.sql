CREATE TABLE codex_oauth_flows_new (
    id TEXT NOT NULL CONSTRAINT codex_oauth_flows_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    actor_user_id TEXT NOT NULL CONSTRAINT codex_oauth_flows_actor_user_id_storage CHECK (actor_user_id IS NULL OR (ag_uuid_valid(actor_user_id))) CHECK (actor_user_id IS NULL OR instr(actor_user_id, char(0))=0),
    label TEXT NOT NULL CONSTRAINT codex_oauth_flows_label_storage CHECK (label IS NULL OR (length(label) <= 100)) CHECK (label IS NULL OR instr(label, char(0))=0),
    proxy_id TEXT CONSTRAINT codex_oauth_flows_proxy_id_storage CHECK (proxy_id IS NULL OR (ag_uuid_valid(proxy_id))) CHECK (proxy_id IS NULL OR instr(proxy_id, char(0))=0),
    quota_threshold_percent INTEGER NOT NULL CONSTRAINT codex_oauth_flows_quota_threshold_percent_storage CHECK (quota_threshold_percent IS NULL OR (quota_threshold_percent BETWEEN -32768 AND 32767)),
    state_hash BLOB NOT NULL,
    code_verifier TEXT NOT NULL CHECK (code_verifier IS NULL OR instr(code_verifier, char(0))=0),
    expires_at TEXT NOT NULL CONSTRAINT codex_oauth_flows_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    completed_at TEXT CONSTRAINT codex_oauth_flows_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_oauth_flows_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    CONSTRAINT codex_oauth_flows_code_verifier_check CHECK (((length(code_verifier) >= 43) AND (length(code_verifier) <= 128))),
    CONSTRAINT codex_oauth_flows_label_check CHECK ((trim(label) <> '')),
    CONSTRAINT codex_oauth_flows_quota_threshold_percent_check CHECK (((quota_threshold_percent >= 1) AND (quota_threshold_percent <= 100))),
    CONSTRAINT codex_oauth_flows_state_hash_check CHECK ((length(state_hash) = 32)),
    CONSTRAINT codex_oauth_flows_pkey PRIMARY KEY (id),
    CONSTRAINT codex_oauth_flows_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT codex_oauth_flows_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES proxies (id) ON DELETE RESTRICT
) STRICT;

INSERT INTO codex_oauth_flows_new (rowid,id,actor_user_id,label,proxy_id,quota_threshold_percent,
    state_hash,code_verifier,expires_at,completed_at,created_at)
SELECT rowid,id,actor_user_id,label,proxy_id,quota_threshold_percent,
    state_hash,code_verifier,expires_at,completed_at,created_at
FROM codex_oauth_flows;

DROP TABLE codex_oauth_flows;
ALTER TABLE codex_oauth_flows_new RENAME TO codex_oauth_flows;
CREATE INDEX codex_oauth_flows_expiry_idx ON codex_oauth_flows (expires_at) WHERE (completed_at IS NULL);
