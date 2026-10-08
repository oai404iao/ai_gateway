//! Transactional conversion from generated credential ownership to explicit IDs.

use sqlx::SqliteConnection;

use crate::persistence::capability_cutover::io::CapabilityCutoverIoError;

pub(super) async fn prepare(
    connection: &mut SqliteConnection,
) -> Result<(), CapabilityCutoverIoError> {
    if sqlx::query_scalar::<_, bool>("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?
    {
        return Err(CapabilityCutoverIoError::InvalidRow);
    }
    let objects: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema WHERE type IN ('trigger','view') AND sql IS NOT NULL
         ORDER BY CASE type WHEN 'view' THEN 0 ELSE 1 END,name",
    )
    .fetch_all(&mut *connection)
    .await?;
    for (kind, name, _) in objects.iter().rev() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP {kind} {}", quote(name))))
            .execute(&mut *connection)
            .await?;
    }
    let syntax = "length(connector_kind) BETWEEN 1 AND 64 AND connector_kind GLOB '[a-z]*' AND connector_kind NOT GLOB '*[^a-z0-9_-]*'";
    for table in ["upstream_accesses", "upstream_credentials"] {
        let mut ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&mut *connection)
                .await?;
        if table == "upstream_accesses" {
            let old = "CHECK (connector_kind IN ('general','codex'))";
            if ddl.matches(old).count() != 1 {
                return Err(CapabilityCutoverIoError::InvalidRow);
            }
            ddl = ddl.replace(old, &format!("CHECK ({syntax})"));
        } else {
            let start = ddl
                .find("connector_kind TEXT")
                .ok_or(CapabilityCutoverIoError::InvalidRow)?;
            let end = start
                + ddl[start..]
                    .find("VIRTUAL")
                    .ok_or(CapabilityCutoverIoError::InvalidRow)?
                + "VIRTUAL".len();
            ddl.replace_range(start..end, &format!(
                "connector_kind TEXT NOT NULL DEFAULT 'general' CONSTRAINT upstream_credentials_connector_kind_check CHECK ({syntax} AND ((kind='codex_oauth')=(connector_kind='codex')))"
            ));
        }
        let replacement = format!("_connector_new_{table}");
        let open = ddl.find('(').ok_or(CapabilityCutoverIoError::InvalidRow)?;
        ddl.replace_range(..open, &format!("CREATE TABLE {replacement} "));
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name=? AND sql IS NOT NULL",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await?;
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_xinfo(?) ORDER BY cid")
                .bind(table)
                .fetch_all(&mut *connection)
                .await?;
        let columns = columns
            .iter()
            .map(|column| quote(column))
            .collect::<Vec<_>>()
            .join(",");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "{ddl}; INSERT INTO {replacement}({columns}) SELECT {columns} FROM {table};
             DROP TABLE {table}; ALTER TABLE {replacement} RENAME TO {table};"
        )))
        .execute(&mut *connection)
        .await?;
        for index in indexes {
            sqlx::raw_sql(sqlx::AssertSqlSafe(index))
                .execute(&mut *connection)
                .await?;
        }
    }
    for (_, _, ddl) in objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::super::{SqliteDatabase, schema::MIGRATIONS};
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn migration_preserves_identities_and_enforces_explicit_plugin_ownership() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap();
        database.migrate(&MIGRATIONS[..9]).await.unwrap();
        let mut tx = database.begin_write().await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO upstream_credentials(id,name,kind,secret,allowed_base_urls)
             VALUES('10000000-0000-0000-0000-000000000001','static','bearer','secret','[\"https://example.test\"]'),
                   ('10000000-0000-0000-0000-000000000002','oauth','codex_oauth',NULL,'[\"https://example.test\"]');",
        ).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let before: Vec<(String, String, String, String)> = {
            let mut reader = database.acquire_read().await.unwrap();
            sqlx::query_as("SELECT id,connector_kind,revision,updated_at FROM upstream_credentials ORDER BY id")
                .fetch_all(&mut *reader).await.unwrap()
        };
        assert_eq!(database.install_schema().await.unwrap(), 1);
        {
            let mut reader = database.acquire_read().await.unwrap();
            let after: Vec<(String, String, String, String)> =
                sqlx::query_as("SELECT id,connector_kind,revision,updated_at FROM upstream_credentials ORDER BY id")
                    .fetch_all(&mut *reader).await.unwrap();
            assert_eq!(before, after);
            assert_eq!(after[0].1, "general");
            assert_eq!(after[1].1, "codex");
            assert!(
                sqlx::query("PRAGMA foreign_key_check")
                    .fetch_all(&mut *reader)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let mut tx = database.begin_write().await.unwrap();
        sqlx::query(
            "INSERT INTO upstream_credentials(id,name,kind,connector_kind,secret,allowed_base_urls)
             VALUES('10000000-0000-0000-0000-000000000003','plugin','bearer','acme','secret','[\"https://example.test\"]')",
        ).execute(&mut *tx).await.unwrap();
        sqlx::query(
            "INSERT INTO upstream_accesses(id,name,connector_kind,base_url)
             VALUES('20000000-0000-0000-0000-000000000001','plugin','acme','https://example.test')",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(
            sqlx::query(
                "UPDATE upstream_credentials SET connector_kind='general',updated_at=ag_now()
             WHERE id='10000000-0000-0000-0000-000000000003'",
            )
            .execute(&mut *tx)
            .await
            .is_err()
        );
        for (kind, connector) in [
            ("bearer", "codex"),
            ("codex_oauth", "acme"),
            ("bearer", "BAD"),
        ] {
            assert!(sqlx::query(
                "INSERT INTO upstream_credentials(id,name,kind,connector_kind,secret,allowed_base_urls)
                 VALUES('10000000-0000-0000-0000-000000000004','invalid',?,?,?,'[\"https://example.test\"]')",
            ).bind(kind).bind(connector).bind((kind == "bearer").then_some("secret"))
                .execute(&mut *tx).await.is_err());
        }
        tx.rollback().await.unwrap();
        assert_eq!(database.install_schema().await.unwrap(), 0);
        database.close().await;
    }
}
