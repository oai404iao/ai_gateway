-- Request logs are append-heavy and each billable row is updated during
-- settlement. Trigger vacuum/analyze based on a small percentage of the table
-- instead of PostgreSQL's broad default scale factors, while retaining a
-- fixed threshold so tiny installations do not vacuum continuously.
ALTER TABLE request_logs SET (
    autovacuum_vacuum_threshold = 1000,
    autovacuum_vacuum_scale_factor = 0.02,
    autovacuum_analyze_threshold = 1000,
    autovacuum_analyze_scale_factor = 0.02
);

-- The durable ingress table continuously receives COPY batches and deletes
-- successfully projected rows. Its live row count is normally small, so a
-- low scale factor plus a fixed threshold prevents dead tuples from waiting
-- for the cluster-wide 20% default.
ALTER TABLE request_log_ingest SET (
    autovacuum_vacuum_threshold = 1000,
    autovacuum_vacuum_scale_factor = 0.01,
    autovacuum_analyze_threshold = 1000,
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_vacuum_insert_threshold = 1000,
    autovacuum_vacuum_insert_scale_factor = 0.01
);
