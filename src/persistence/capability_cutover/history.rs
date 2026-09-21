//! Retarget historical references without rewriting dispatch or financial identities.

use sqlx::{Postgres, Transaction};

use crate::persistence::RepositoryError;

pub async fn pg_retarget_history(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), RepositoryError> {
    sqlx::raw_sql(
        "ALTER TABLE request_logs
            DROP CONSTRAINT request_logs_channel_group_id_fkey,
            DROP CONSTRAINT request_logs_channel_id_fkey,
            DROP CONSTRAINT request_logs_model_rule_id_fkey,
            ADD CONSTRAINT request_logs_channel_group_id_fkey
                FOREIGN KEY (channel_group_id) REFERENCES group_identity_registry(id) ON DELETE RESTRICT,
            ADD CONSTRAINT request_logs_channel_id_fkey
                FOREIGN KEY (channel_id) REFERENCES channel_identity_registry(id) ON DELETE RESTRICT,
            ADD CONSTRAINT request_logs_model_rule_id_fkey
                FOREIGN KEY (model_rule_id) REFERENCES model_rule_identity_registry(id) ON DELETE RESTRICT;
         ALTER TABLE request_metering_facts
            DROP CONSTRAINT request_metering_facts_channel_group_id_fkey,
            DROP CONSTRAINT request_metering_facts_channel_id_fkey,
            DROP CONSTRAINT request_metering_facts_model_rule_id_fkey,
            ADD CONSTRAINT request_metering_facts_channel_group_id_fkey
                FOREIGN KEY (channel_group_id) REFERENCES group_identity_registry(id) ON DELETE RESTRICT,
            ADD CONSTRAINT request_metering_facts_channel_id_fkey
                FOREIGN KEY (channel_id) REFERENCES channel_identity_registry(id) ON DELETE RESTRICT,
            ADD CONSTRAINT request_metering_facts_model_rule_id_fkey
                FOREIGN KEY (model_rule_id) REFERENCES model_rule_identity_registry(id) ON DELETE RESTRICT;",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Only the startup migrator may use this operation. It must disable
/// `foreign_keys` before BEGIN on its dedicated, close-on-drop writer, populate
/// the identity registries, and roll back the entire batch on any failure.
/// No connection with disabled foreign keys may return to a serving pool.
#[cfg(feature = "sqlite-backend")]
pub async fn sqlite_retarget_history(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), RepositoryError> {
    let connection = &mut **transaction;
    let enabled: bool = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?;
    if enabled {
        return Err(RepositoryError::Validation);
    }

    // Cross-table triggers and views would otherwise temporarily reference a
    // missing table during DROP/RENAME. Suspend and restore their exact DDL
    // inside this transaction, including the append-only financial guards.
    let schema_objects: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema
         WHERE type IN ('trigger','view') AND sql IS NOT NULL
         ORDER BY CASE type WHEN 'view' THEN 0 ELSE 1 END,name",
    )
    .fetch_all(&mut *connection)
    .await?;
    for (kind, name, _) in schema_objects.iter().rev() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP {} {}",
            if kind == "view" { "VIEW" } else { "TRIGGER" },
            quote_identifier(name),
        )))
        .execute(&mut *connection)
        .await?;
    }
    for table in ["request_logs", "request_metering_facts"] {
        let ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&mut *connection)
                .await?;
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema
             WHERE type='index' AND tbl_name=? AND sql IS NOT NULL ORDER BY name",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await?;
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_xinfo(?) WHERE hidden=0 ORDER BY cid",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await?;
        let columns = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(",");
        let replacement = format!("_cutover_{table}");
        let prefix = format!("CREATE TABLE {table} (");
        if !ddl.starts_with(&prefix) {
            return Err(RepositoryError::Validation);
        }
        let mut ddl = ddl.replacen(&prefix, &format!("CREATE TABLE {replacement} ("), 1);
        for (old, new) in [
            ("channels", "channel_identity_registry"),
            ("channel_groups", "group_identity_registry"),
            ("model_rules", "model_rule_identity_registry"),
        ] {
            let reference = format!("REFERENCES {old} (id)");
            if ddl.matches(&reference).count() != 1 {
                return Err(RepositoryError::Validation);
            }
            ddl = ddl.replace(&reference, &format!("REFERENCES {new} (id)"));
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {replacement} ({columns}) SELECT {columns} FROM {table}"
        )))
        .execute(&mut *connection)
        .await?;
        let changed: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT * FROM {table} EXCEPT SELECT * FROM {replacement})
                 OR EXISTS(SELECT * FROM {replacement} EXCEPT SELECT * FROM {table})"
        )))
        .fetch_one(&mut *connection)
        .await?;
        if changed {
            return Err(RepositoryError::Validation);
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP TABLE {table}; ALTER TABLE {replacement} RENAME TO {table};"
        )))
        .execute(&mut *connection)
        .await?;
        for index in indexes {
            sqlx::raw_sql(sqlx::AssertSqlSafe(index))
                .execute(&mut *connection)
                .await?;
        }
    }
    for (_, _, ddl) in schema_objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
    }
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(connection)
        .await?
        .is_some()
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
