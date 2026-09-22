//! Independently versioned SQLite schema; never execute historical PostgreSQL migrations.

use super::SqliteMigration;

pub(crate) const MIGRATIONS: &[SqliteMigration<'static>] = &[
    SqliteMigration {
        version: 1,
        description: "business schema after PostgreSQL 0063",
        sql: include_str!("../../../migrations/sqlite/0001_baseline.sql"),
    },
    SqliteMigration {
        version: 2,
        description: "business constraints and derived projections",
        sql: include_str!("../../../migrations/sqlite/0002_guards.sql"),
    },
    SqliteMigration {
        version: 3,
        description: "durable Codex external-operation fences",
        sql: include_str!("../../../migrations/sqlite/0003_codex_operations.sql"),
    },
    SqliteMigration {
        version: 4,
        description: "independent upstream credential identities",
        sql: include_str!("../../../migrations/sqlite/0004_upstream_credentials.sql"),
    },
    SqliteMigration {
        version: 5,
        description: "canonical upstream operation capabilities",
        sql: include_str!("../../../migrations/sqlite/0005_upstream_capabilities.sql"),
    },
    SqliteMigration {
        version: 6,
        description: "six operation routing and connector names",
        sql: include_str!("../../../migrations/sqlite/0006_six_operations.sql"),
    },
];
