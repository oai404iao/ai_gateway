CREATE TABLE plugin_artifacts (
    plugin_id TEXT NOT NULL CHECK (plugin_id <> 'general'),
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    version TEXT NOT NULL,
    manifest JSONB NOT NULL CHECK (jsonb_typeof(manifest) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (plugin_id, digest)
);

CREATE TABLE plugin_states (
    plugin_id TEXT PRIMARY KEY CHECK (plugin_id <> 'general'),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    artifact_digest TEXT,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (NOT enabled OR artifact_digest IS NOT NULL),
    FOREIGN KEY (plugin_id, artifact_digest) REFERENCES plugin_artifacts(plugin_id, digest)
);

CREATE TABLE plugin_settings (
    plugin_id TEXT PRIMARY KEY CHECK (plugin_id <> 'general'),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    settings_value JSONB NOT NULL CHECK (jsonb_typeof(settings_value) = 'object'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE plugin_install_jobs (
    id UUID PRIMARY KEY,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    operation TEXT NOT NULL CHECK (operation IN ('install','discover')),
    status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed')),
    plugin_id TEXT,
    artifact_digest TEXT,
    error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (operation <> 'install' OR status <> 'succeeded' OR
           (plugin_id IS NOT NULL AND artifact_digest IS NOT NULL))
);
CREATE INDEX plugin_install_jobs_status_idx ON plugin_install_jobs(status, created_at);

INSERT INTO plugin_settings (plugin_id, schema_version, settings_value)
SELECT 'codex', 1, value->'codex'
FROM system_settings
WHERE setting_key='forwarding_policy' AND value ? 'codex';

INSERT INTO plugin_states (plugin_id)
SELECT plugin_id FROM plugin_settings;

UPDATE system_settings SET value=value-'codex'
WHERE setting_key='forwarding_policy' AND value ? 'codex';
