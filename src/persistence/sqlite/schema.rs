//! Independently versioned SQLite schema; never execute historical PostgreSQL migrations.

use super::SqliteMigration;

pub(super) const MIGRATIONS: &[SqliteMigration<'static>] = &[
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
];
