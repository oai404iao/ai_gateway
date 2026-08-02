ALTER TABLE codex_oauth_credentials
    ADD COLUMN quota_reset_credits_available bigint,
    ADD CONSTRAINT codex_oauth_credentials_reset_credits_check
        CHECK (
            quota_reset_credits_available IS NULL
            OR quota_reset_credits_available >= 0
        );

CREATE TABLE codex_quota_window_periods (
    id uuid PRIMARY KEY,
    credential_id uuid NOT NULL
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    window_kind text NOT NULL
        CHECK (window_kind IN ('primary', 'secondary')),
    window_seconds integer NOT NULL CHECK (window_seconds > 0),
    started_at timestamptz NOT NULL,
    scheduled_reset_at timestamptz NOT NULL,
    ended_at timestamptz,
    reset_reason text
        CHECK (reset_reason IN ('natural', 'manual', 'openai_official')),
    initial_used_percent integer NOT NULL
        CHECK (initial_used_percent BETWEEN 0 AND 100),
    last_used_percent integer NOT NULL
        CHECK (last_used_percent BETWEEN 0 AND 100),
    first_observed_at timestamptz NOT NULL,
    last_observed_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (scheduled_reset_at > started_at),
    CHECK (ended_at IS NULL OR ended_at >= started_at),
    CHECK (
        (ended_at IS NULL AND reset_reason IS NULL)
        OR (ended_at IS NOT NULL AND reset_reason IS NOT NULL)
    ),
    CHECK (last_observed_at >= first_observed_at)
);

CREATE UNIQUE INDEX codex_quota_window_periods_current_idx
    ON codex_quota_window_periods (credential_id, window_kind)
    WHERE ended_at IS NULL;

CREATE UNIQUE INDEX codex_quota_window_periods_identity_idx
    ON codex_quota_window_periods (
        credential_id,
        window_kind,
        started_at,
        scheduled_reset_at
    );

CREATE INDEX codex_quota_window_periods_history_idx
    ON codex_quota_window_periods (
        credential_id,
        window_kind,
        started_at DESC
    );

CREATE TRIGGER codex_quota_window_periods_set_updated_at
BEFORE UPDATE ON codex_quota_window_periods
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

INSERT INTO codex_quota_window_periods (
    id,
    credential_id,
    window_kind,
    window_seconds,
    started_at,
    scheduled_reset_at,
    initial_used_percent,
    last_used_percent,
    first_observed_at,
    last_observed_at
)
SELECT
    gen_random_uuid(),
    channel_id,
    'primary',
    primary_window_seconds,
    primary_reset_at - make_interval(secs => primary_window_seconds),
    primary_reset_at,
    primary_used_percent,
    primary_used_percent,
    COALESCE(quota_checked_at, updated_at),
    COALESCE(quota_checked_at, updated_at)
FROM codex_oauth_credentials
WHERE deleted_at IS NULL
  AND primary_used_percent IS NOT NULL
  AND primary_window_seconds IS NOT NULL
  AND primary_reset_at IS NOT NULL;

INSERT INTO codex_quota_window_periods (
    id,
    credential_id,
    window_kind,
    window_seconds,
    started_at,
    scheduled_reset_at,
    initial_used_percent,
    last_used_percent,
    first_observed_at,
    last_observed_at
)
SELECT
    gen_random_uuid(),
    channel_id,
    'secondary',
    secondary_window_seconds,
    secondary_reset_at - make_interval(secs => secondary_window_seconds),
    secondary_reset_at,
    secondary_used_percent,
    secondary_used_percent,
    COALESCE(quota_checked_at, updated_at),
    COALESCE(quota_checked_at, updated_at)
FROM codex_oauth_credentials
WHERE deleted_at IS NULL
  AND secondary_used_percent IS NOT NULL
  AND secondary_window_seconds IS NOT NULL
  AND secondary_reset_at IS NOT NULL;

CREATE TABLE codex_quota_reset_events (
    id uuid PRIMARY KEY,
    credential_id uuid NOT NULL
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    actor_user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    requested_at timestamptz NOT NULL,
    outcome text NOT NULL
        CHECK (
            outcome IN (
                'reset',
                'nothing_to_reset',
                'no_credit',
                'already_redeemed'
            )
        ),
    windows_reset integer NOT NULL CHECK (windows_reset BETWEEN 0 AND 2),
    primary_applied_at timestamptz,
    secondary_applied_at timestamptz,
    correlation_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX codex_quota_reset_events_pending_idx
    ON codex_quota_reset_events (credential_id, requested_at DESC)
    WHERE outcome IN ('reset', 'already_redeemed')
      AND windows_reset > (
          CASE WHEN primary_applied_at IS NULL THEN 0 ELSE 1 END
          + CASE WHEN secondary_applied_at IS NULL THEN 0 ELSE 1 END
      );
