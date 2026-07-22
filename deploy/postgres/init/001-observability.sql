-- This script runs only when Docker initializes a brand-new PostgreSQL
-- volume. shared_preload_libraries is configured by docker-compose.yml.
-- Existing volumes can enable the view once with:
--   CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
