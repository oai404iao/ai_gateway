-- The Console reads precomputed user-spend rankings instead of aggregating
-- append-only request logs on every page load. Historical rows are retained
-- by their Asia/Shanghai natural day, ISO week, or calendar month.
CREATE TABLE spend_leaderboard_periods (
    period text NOT NULL CHECK (period IN ('day', 'week', 'month')),
    period_start date NOT NULL,
    period_end date NOT NULL,
    refreshed_at timestamptz NOT NULL,
    total_cost_amount numeric(24, 8) NOT NULL CHECK (total_cost_amount >= 0),
    PRIMARY KEY (period, period_start),
    CHECK (period_end > period_start)
);

CREATE TABLE spend_leaderboard_entries (
    period text NOT NULL,
    period_start date NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    rank bigint NOT NULL CHECK (rank > 0),
    request_count bigint NOT NULL CHECK (request_count >= 0),
    priced_request_count bigint NOT NULL CHECK (
        priced_request_count >= 0
        AND priced_request_count <= request_count
    ),
    total_tokens bigint NOT NULL CHECK (total_tokens >= 0),
    cost_amount numeric(24, 8) NOT NULL CHECK (cost_amount >= 0),
    PRIMARY KEY (period, period_start, user_id),
    FOREIGN KEY (period, period_start)
        REFERENCES spend_leaderboard_periods (period, period_start)
        ON DELETE CASCADE
);

CREATE INDEX spend_leaderboard_entries_period_rank_idx
    ON spend_leaderboard_entries (period, period_start, rank);
