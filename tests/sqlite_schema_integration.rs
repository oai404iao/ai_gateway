//! PostgreSQL/SQLite schema-shape parity for the feature-gated SQLite port.

#![cfg(feature = "sqlite-backend")]

use std::collections::{BTreeMap, BTreeSet};

use ai_gateway::persistence::{POSTGRES_MIGRATOR, SQLITE_MIGRATOR};
use sqlx::{
    PgPool, Row, SqlitePool,
    postgres::PgPoolOptions,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";
const PASSWORD_FILE_ADMIN_URL: &str = "postgres://ai_gateway@127.0.0.1:5432/postgres";

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let admin_url =
            std::env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| default_admin_url());
        let mut database_url = reqwest::Url::parse(&admin_url).expect("admin URL valid");
        assert_ne!(
            database_url.path().trim_matches('/'),
            "ai_gateway",
            "TEST_DATABASE_ADMIN_URL must not target the ai_gateway database"
        );
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("PostgreSQL admin database available");
        let name = format!("ai_gateway_sqlite_schema_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .expect("temp database creatable");
        database_url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url.as_str())
            .await
            .expect("temp database connectable");
        POSTGRES_MIGRATOR
            .run(&pool)
            .await
            .expect("PostgreSQL migrations apply");
        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .execute(&self.admin)
            .await
            .expect("temp database removable");
        self.admin.close().await;
    }
}

fn default_admin_url() -> String {
    let Ok(mut password) = std::fs::read_to_string("./config/postgres-password") else {
        return DEFAULT_ADMIN_URL.into();
    };
    while matches!(password.as_bytes().last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    if password.is_empty() {
        return DEFAULT_ADMIN_URL.into();
    }
    let mut url =
        reqwest::Url::parse(PASSWORD_FILE_ADMIN_URL).expect("default admin URL must be valid");
    url.set_password(Some(&password))
        .expect("PostgreSQL URL must accept a password");
    url.to_string()
}

async fn migrated_sqlite() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true),
        )
        .await
        .expect("SQLite available");
    SQLITE_MIGRATOR
        .run(&pool)
        .await
        .expect("SQLite migrations apply");
    pool
}

fn sqlite_affinity(data_type: &str, udt_name: &str) -> &'static str {
    if udt_name == "uuid" || data_type == "bytea" {
        "BLOB"
    } else if data_type == "boolean" || matches!(data_type, "smallint" | "integer" | "bigint") {
        "INTEGER"
    } else {
        // NUMERIC is deliberately exact decimal TEXT. PostgreSQL enums,
        // arrays, JSONB, CIDR, dates, timestamps, and ordinary strings also
        // use SQLite TEXT.
        "TEXT"
    }
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ForeignKey {
    source_table: String,
    source_columns: Vec<String>,
    target_table: String,
    target_columns: Vec<String>,
    on_delete: String,
}

async fn postgres_foreign_keys(pool: &PgPool) -> BTreeSet<ForeignKey> {
    let rows = sqlx::query(
        "SELECT con.conname,source_table.relname AS source_table, \
                target_table.relname AS target_table,con.confdeltype::text AS delete_action, \
                source_key.ordinality,source_column.attname AS source_column, \
                target_column.attname AS target_column \
         FROM pg_constraint con \
         JOIN pg_class source_table ON source_table.oid=con.conrelid \
         JOIN pg_class target_table ON target_table.oid=con.confrelid \
         JOIN LATERAL unnest(con.conkey) WITH ORDINALITY \
              AS source_key(attnum,ordinality) ON true \
         JOIN LATERAL unnest(con.confkey) WITH ORDINALITY \
              AS target_key(attnum,ordinality) \
              ON target_key.ordinality=source_key.ordinality \
         JOIN pg_attribute source_column \
              ON source_column.attrelid=con.conrelid \
             AND source_column.attnum=source_key.attnum \
         JOIN pg_attribute target_column \
              ON target_column.attrelid=con.confrelid \
             AND target_column.attnum=target_key.attnum \
         WHERE con.connamespace='public'::regnamespace AND con.contype='f' \
         ORDER BY con.conname,source_key.ordinality",
    )
    .fetch_all(pool)
    .await
    .unwrap();

    let mut grouped = BTreeMap::<String, (String, Vec<String>, String, Vec<String>, String)>::new();
    for row in rows {
        let constraint = row.get::<String, _>("conname");
        let delete_action = match row.get::<String, _>("delete_action").as_str() {
            "r" => "restrict",
            "c" => "cascade",
            "a" => "no action",
            "n" => "set null",
            "d" => "set default",
            other => panic!("unknown PostgreSQL delete action {other}"),
        };
        let entry = grouped.entry(constraint).or_insert_with(|| {
            (
                row.get("source_table"),
                Vec::new(),
                row.get("target_table"),
                Vec::new(),
                delete_action.into(),
            )
        });
        entry.1.push(row.get("source_column"));
        entry.3.push(row.get("target_column"));
    }
    grouped
        .into_values()
        .map(
            |(source_table, source_columns, target_table, target_columns, on_delete)| ForeignKey {
                source_table,
                source_columns,
                target_table,
                target_columns,
                on_delete,
            },
        )
        .collect()
}

async fn sqlite_foreign_keys(pool: &SqlitePool, tables: &[String]) -> BTreeSet<ForeignKey> {
    let mut foreign_keys = BTreeSet::new();
    for table in tables {
        let escaped_table = table.replace('"', "\"\"");
        let rows = sqlx::query(&format!("PRAGMA foreign_key_list(\"{escaped_table}\")"))
            .fetch_all(pool)
            .await
            .unwrap();
        let mut grouped =
            BTreeMap::<i64, (String, BTreeMap<i64, String>, BTreeMap<i64, String>, String)>::new();
        for row in rows {
            let entry = grouped.entry(row.get("id")).or_insert_with(|| {
                (
                    row.get("table"),
                    BTreeMap::new(),
                    BTreeMap::new(),
                    row.get::<String, _>("on_delete").to_ascii_lowercase(),
                )
            });
            let sequence = row.get("seq");
            entry.1.insert(sequence, row.get("from"));
            entry.2.insert(sequence, row.get("to"));
        }
        foreign_keys.extend(grouped.into_values().map(
            |(target_table, source_columns, target_columns, on_delete)| ForeignKey {
                source_table: table.clone(),
                source_columns: source_columns.into_values().collect(),
                target_table,
                target_columns: target_columns.into_values().collect(),
                on_delete,
            },
        ));
    }
    foreign_keys
}

#[tokio::test]
async fn sqlite_tables_columns_and_storage_affinities_match_postgres() {
    let database = TestDatabase::new().await;
    let sqlite = migrated_sqlite().await;

    let postgres_rows = sqlx::query(
        "SELECT table_name,column_name,data_type,udt_name \
         FROM information_schema.columns \
         WHERE table_schema='public' AND table_name <> '_sqlx_migrations' \
         ORDER BY table_name,ordinal_position",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();
    let postgres = postgres_rows
        .into_iter()
        .map(|row| {
            let table = row.get::<String, _>("table_name");
            let column = row.get::<String, _>("column_name");
            let data_type = row.get::<String, _>("data_type");
            let udt_name = row.get::<String, _>("udt_name");
            ((table, column), sqlite_affinity(&data_type, &udt_name))
        })
        .collect::<BTreeMap<_, _>>();

    let tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_schema \
         WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' \
         ORDER BY name",
    )
    .fetch_all(&sqlite)
    .await
    .unwrap();
    let mut sqlite_columns = BTreeMap::new();
    for table in &tables {
        let escaped_table = table.replace('"', "\"\"");
        let rows = sqlx::query(&format!("PRAGMA table_info(\"{escaped_table}\")"))
            .fetch_all(&sqlite)
            .await
            .unwrap();
        for row in rows {
            sqlite_columns.insert(
                (table.clone(), row.get::<String, _>("name")),
                row.get::<String, _>("type"),
            );
        }
    }

    let postgres_foreign_keys = postgres_foreign_keys(&database.pool).await;
    let sqlite_foreign_keys = sqlite_foreign_keys(&sqlite, &tables).await;
    let postgres_indexes = sqlx::query_scalar::<_, String>(
        "SELECT indexname FROM pg_indexes \
         WHERE schemaname='public' \
           AND indexname NOT IN ( \
               SELECT conname FROM pg_constraint \
               WHERE connamespace='public'::regnamespace AND contype IN ('p','u') \
           ) \
         ORDER BY indexname",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap()
    .into_iter()
    .collect::<BTreeSet<_>>();
    let sqlite_indexes = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_schema \
         WHERE type='index' AND sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&sqlite)
    .await
    .unwrap()
    .into_iter()
    .collect::<BTreeSet<_>>();

    sqlite.close().await;
    database.cleanup().await;

    assert_eq!(sqlite_columns.len(), 334);
    assert_eq!(postgres.len(), 334);
    assert_eq!(
        sqlite_columns
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str()))
            .collect::<BTreeMap<_, _>>(),
        postgres
    );
    assert_eq!(postgres_foreign_keys.len(), 38);
    assert_eq!(sqlite_foreign_keys, postgres_foreign_keys);
    assert_eq!(sqlite_indexes, postgres_indexes);
}
