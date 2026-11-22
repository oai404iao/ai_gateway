-- Dropping a referenced parent with FK enforcement on would cascade-delete grants.
-- The startup migrator owns the transaction and verifies all FKs before commit.
CREATE TEMP TABLE api_keys_rebuild_guard (fk_enabled INTEGER NOT NULL CHECK (fk_enabled = 0));
INSERT INTO api_keys_rebuild_guard SELECT foreign_keys FROM pragma_foreign_keys;
DROP TABLE api_keys_rebuild_guard;

CREATE TABLE api_keys_new (
    id TEXT NOT NULL CONSTRAINT api_keys_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT api_keys_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT api_keys_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    secret_value TEXT NOT NULL CHECK (secret_value IS NULL OR instr(secret_value, char(0))=0),
    status TEXT NOT NULL CHECK (status IS NULL OR instr(status, char(0))=0),
    expires_at TEXT CONSTRAINT api_keys_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    permissions TEXT NOT NULL CONSTRAINT api_keys_permissions_storage CHECK (permissions IS NULL OR (ag_array_valid(permissions, 'text'))) CHECK (permissions IS NULL OR instr(permissions, char(0))=0),
    allowed_group_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_keys_allowed_group_ids_storage CHECK (allowed_group_ids IS NULL OR (ag_array_valid(allowed_group_ids, 'uuid'))) CHECK (allowed_group_ids IS NULL OR instr(allowed_group_ids, char(0))=0),
    requests_per_minute INTEGER CONSTRAINT api_keys_requests_per_minute_storage CHECK (requests_per_minute IS NULL OR (requests_per_minute BETWEEN -2147483648 AND 2147483647)),
    max_concurrent_requests INTEGER CONSTRAINT api_keys_max_concurrent_requests_storage CHECK (max_concurrent_requests IS NULL OR (max_concurrent_requests BETWEEN -2147483648 AND 2147483647)),
    quota_limit_amount TEXT CONSTRAINT api_keys_quota_limit_amount_storage CHECK (quota_limit_amount IS NULL OR (ag_decimal_valid(quota_limit_amount, 24, 8))) CHECK (quota_limit_amount IS NULL OR instr(quota_limit_amount, char(0))=0),
    quota_used_amount TEXT DEFAULT ('0') NOT NULL CONSTRAINT api_keys_quota_used_amount_storage CHECK (quota_used_amount IS NULL OR (ag_decimal_valid(quota_used_amount, 24, 8))) CHECK (quota_used_amount IS NULL OR instr(quota_used_amount, char(0))=0),
    last_used_at TEXT CONSTRAINT api_keys_last_used_at_storage CHECK (last_used_at IS NULL OR (ag_time_valid(last_used_at))) CHECK (last_used_at IS NULL OR instr(last_used_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_keys_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_keys_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    allowed_channel_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_keys_allowed_channel_ids_storage CHECK (allowed_channel_ids IS NULL OR (ag_array_valid(allowed_channel_ids, 'uuid'))) CHECK (allowed_channel_ids IS NULL OR instr(allowed_channel_ids, char(0))=0),
    is_system INTEGER DEFAULT (0) NOT NULL CONSTRAINT api_keys_is_system_storage CHECK (is_system IS NULL OR (is_system IN (0,1))),
    deleted_at TEXT CONSTRAINT api_keys_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    deleted_by TEXT CONSTRAINT api_keys_deleted_by_storage CHECK (deleted_by IS NULL OR (ag_uuid_valid(deleted_by))) CHECK (deleted_by IS NULL OR instr(deleted_by, char(0))=0),
    CONSTRAINT api_keys_allowed_channel_ids_no_nulls CHECK ((ag_array_position(allowed_channel_ids, NULL) IS NULL)),
    CONSTRAINT api_keys_allowed_group_ids_no_nulls CHECK ((ag_array_position(allowed_group_ids, NULL) IS NULL)),
    CONSTRAINT api_keys_check CHECK (((expires_at IS NULL) OR (expires_at > created_at))),
    CONSTRAINT api_keys_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT api_keys_deleted_state_check CHECK (((deleted_at IS NULL) OR (status = 'revoked'))),
    CONSTRAINT api_keys_max_concurrent_requests_check CHECK (((max_concurrent_requests IS NULL) OR (max_concurrent_requests > 0))),
    CONSTRAINT api_keys_permissions_check CHECK (ag_array_subset(permissions, json_array('proxy', 'models.read'))),
    CONSTRAINT api_keys_permissions_check1 CHECK ((ag_array_position(permissions, NULL) IS NULL)),
    CONSTRAINT api_keys_permissions_nonempty CHECK ((json_array_length(permissions) > 0)),
    CONSTRAINT api_keys_quota_limit_amount_check CHECK (((quota_limit_amount IS NULL) OR (ag_decimal_cmp(quota_limit_amount, '0') >= 0))),
    CONSTRAINT api_keys_quota_used_amount_check CHECK ((ag_decimal_cmp(quota_used_amount, '0') >= 0)),
    CONSTRAINT api_keys_requests_per_minute_check CHECK (((requests_per_minute IS NULL) OR (requests_per_minute > 0))),
    CONSTRAINT api_keys_status_check CHECK (ag_array_contains(json_array('active', 'disabled', 'revoked', 'expired'), status)),
    CONSTRAINT api_keys_pkey PRIMARY KEY (id),
    CONSTRAINT api_keys_secret_value_key UNIQUE (secret_value),
    CONSTRAINT api_keys_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

INSERT INTO api_keys_new (rowid, id, user_id, name, secret_value, status, expires_at, permissions,
    allowed_group_ids, requests_per_minute, max_concurrent_requests, quota_limit_amount, quota_used_amount,
    last_used_at, created_at, updated_at, allowed_channel_ids, is_system, deleted_at, deleted_by)
SELECT rowid, id, user_id, name, secret_value, status, expires_at, permissions,
    allowed_group_ids, requests_per_minute, max_concurrent_requests, quota_limit_amount, quota_used_amount,
    last_used_at, created_at, updated_at, allowed_channel_ids, is_system, deleted_at, deleted_by
FROM api_keys;

-- This cross-table trigger references api_keys during the name swap.
DROP TRIGGER request_settlements_validate;
DROP TABLE api_keys;
ALTER TABLE api_keys_new RENAME TO api_keys;

CREATE UNIQUE INDEX api_keys_active_user_name_idx ON api_keys (user_id, name) WHERE (deleted_at IS NULL);
CREATE INDEX api_keys_deleted_at_idx ON api_keys (deleted_at) WHERE (deleted_at IS NOT NULL);
CREATE INDEX api_keys_user_id_status_idx ON api_keys (user_id, status);

CREATE TRIGGER api_keys_no_hard_delete BEFORE DELETE ON api_keys BEGIN
    SELECT RAISE(ABORT, 'api_keys_no_hard_delete');
END;
CREATE TRIGGER api_keys_timestamp BEFORE UPDATE ON api_keys
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'api_keys_timestamp');
END;
CREATE TRIGGER request_settlements_validate BEFORE INSERT ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_eligibility_check')
    WHERE NOT EXISTS (
        SELECT 1 FROM request_metering_facts f JOIN api_keys k ON k.id=f.api_key_id AND k.user_id=f.user_id
        WHERE f.id=NEW.request_id AND f.amount_state IN ('priced','zero_by_policy')
          AND f.cost_amount IS NEW.cost_amount AND f.currency IS NEW.currency
    );
END;
