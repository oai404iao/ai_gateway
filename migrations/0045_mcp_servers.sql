-- Stateless MCP server definitions and request-log source attribution.

CREATE TYPE mcp_server_kind AS ENUM ('web_search');

CREATE TABLE mcp_servers (
    id uuid PRIMARY KEY,
    slug varchar(63) NOT NULL UNIQUE
        CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    kind mcp_server_kind NOT NULL,
    name varchar(100) NOT NULL,
    description varchar(1000),
    model_rule_id uuid NOT NULL REFERENCES model_rules (id) ON DELETE RESTRICT,
    settings_version smallint NOT NULL DEFAULT 1
        CHECK (settings_version > 0),
    settings jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(settings) = 'object'),
    enabled boolean NOT NULL DEFAULT false,
    deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (deleted_at IS NULL OR NOT enabled)
);

CREATE TRIGGER mcp_servers_set_updated_at
BEFORE UPDATE ON mcp_servers
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX mcp_servers_active_kind_idx
    ON mcp_servers (kind, slug)
    WHERE deleted_at IS NULL;

ALTER TABLE request_logs
    DROP CONSTRAINT request_logs_request_source_check,
    ADD CONSTRAINT request_logs_request_source_check
        CHECK (request_source IN ('client', 'mcp', 'scheduled_test'));

CREATE INDEX request_logs_mcp_started_at_idx
    ON request_logs (started_at DESC, id DESC)
    WHERE request_source = 'mcp';
