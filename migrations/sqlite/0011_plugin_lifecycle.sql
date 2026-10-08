CREATE TABLE plugin_artifacts (
    plugin_id TEXT NOT NULL CHECK (plugin_id <> 'general'),
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    version TEXT NOT NULL,
    manifest TEXT NOT NULL CHECK (json_valid(manifest) AND json_type(manifest) = 'object'),
    created_at TEXT NOT NULL DEFAULT (ag_now()),
    deleted_at TEXT,
    PRIMARY KEY (plugin_id, digest)
) STRICT;

CREATE TABLE plugin_states (
    plugin_id TEXT PRIMARY KEY NOT NULL CHECK (plugin_id <> 'general'),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
    artifact_digest TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TEXT NOT NULL DEFAULT (ag_now()),
    CHECK (NOT enabled OR artifact_digest IS NOT NULL),
    FOREIGN KEY (plugin_id, artifact_digest) REFERENCES plugin_artifacts(plugin_id, digest)
) STRICT;

CREATE TABLE plugin_settings (
    plugin_id TEXT PRIMARY KEY NOT NULL CHECK (plugin_id <> 'general'),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    settings_value TEXT NOT NULL CHECK (json_valid(settings_value) AND json_type(settings_value) = 'object'),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TEXT NOT NULL DEFAULT (ag_now())
) STRICT;

CREATE TABLE plugin_install_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    actor_user_id TEXT NOT NULL REFERENCES users(id),
    operation TEXT NOT NULL CHECK (operation IN ('install','discover')),
    status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed')),
    plugin_id TEXT,
    artifact_digest TEXT,
    error_code TEXT,
    created_at TEXT NOT NULL DEFAULT (ag_now()),
    updated_at TEXT NOT NULL DEFAULT (ag_now()),
    CHECK (operation <> 'install' OR status <> 'succeeded' OR
           (plugin_id IS NOT NULL AND artifact_digest IS NOT NULL))
) STRICT;
CREATE INDEX plugin_install_jobs_status_idx ON plugin_install_jobs(status, created_at);

INSERT INTO plugin_settings (plugin_id, schema_version, settings_value)
SELECT 'codex', 1, json_extract(value, '$.codex')
FROM system_settings
WHERE setting_key='forwarding_policy' AND json_type(value, '$.codex') IS NOT NULL;

INSERT INTO plugin_states (plugin_id)
SELECT plugin_id FROM plugin_settings;

UPDATE system_settings SET value=json_remove(value, '$.codex'), updated_at=ag_now()
WHERE setting_key='forwarding_policy' AND json_type(value, '$.codex') IS NOT NULL;
