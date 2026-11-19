//! Compare the fully migrated databases against the current logical schema.

use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

use ai_gateway::persistence::sqlite::SqliteDatabase;
use futures_util::FutureExt;
use serde_json::{Value, json};
use sqlx::Row;

#[tokio::test]
#[ignore = "explicit fixture regeneration against a fresh isolated PostgreSQL schema"]
async fn regenerate_canonical_schema_inventory() {
    let postgres = super::TestDatabase::new().await;
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT tablename::text,jsonb_build_object(
            'columns',(SELECT jsonb_agg(a.attname ORDER BY a.attnum)
                FROM pg_attribute a WHERE a.attrelid=tablename::regclass AND a.attnum>0 AND NOT a.attisdropped),
            'types',(SELECT jsonb_object_agg(a.attname,
                CASE WHEN t.typcategory='A' THEN e.typname::text || '[]' ELSE t.typname::text END)
                FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid
                LEFT JOIN pg_type e ON e.oid=t.typelem
                WHERE a.attrelid=tablename::regclass AND a.attnum>0 AND NOT a.attisdropped),
            'checks',coalesce((SELECT jsonb_agg(conname ORDER BY conname) FROM pg_constraint
                WHERE conrelid=tablename::regclass AND contype='c'),'[]'::jsonb),
            'constraints',coalesce((SELECT jsonb_agg(conname ORDER BY conname) FROM pg_constraint
                WHERE conrelid=tablename::regclass AND contype IN ('p','u','f')),'[]'::jsonb))
         FROM pg_tables WHERE schemaname='public' AND tablename<>'_sqlx_migrations' ORDER BY tablename",
    ).fetch_all(&postgres.pool).await.unwrap();
    let inventory = rows.into_iter().collect::<BTreeMap<_, _>>();
    std::fs::write(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/canonical-schema-inventory.json"
        ),
        format!("{}\n", serde_json::to_string_pretty(&inventory).unwrap()),
    )
    .unwrap();
    postgres.cleanup().await;
}

#[tokio::test]
async fn sqlite_schema_matches_current_postgres_columns_types_constraints_foreign_keys_and_seeds() {
    let postgres = super::TestDatabase::new().await;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let sqlite = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    let result = std::panic::AssertUnwindSafe(compare(&postgres, &sqlite))
        .catch_unwind()
        .await;
    sqlite.close().await;
    postgres.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn compare(postgres: &super::TestDatabase, sqlite: &SqliteDatabase) {
    sqlite.install_schema().await.unwrap();
    let inventory: Value =
        serde_json::from_str(include_str!("../fixtures/canonical-schema-inventory.json")).unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables WHERE schemaname='public' AND tablename<>'_sqlx_migrations' ORDER BY tablename",
    ).fetch_all(&postgres.pool).await.unwrap();
    assert_eq!(
        tables,
        inventory
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    );
    let mut reader = sqlite.acquire_read().await.unwrap();
    let sqlite_tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type='table'
         AND name NOT IN ('_gateway_sqlite_identity','_gateway_sqlite_migrations',
             '_gateway_true','_gateway_routing_assertions','_gateway_codex_operations','sqlite_sequence') ORDER BY name",
    )
    .fetch_all(&mut *reader)
    .await
    .unwrap();
    assert_eq!(sqlite_tables, tables);
    for table in tables {
        let expected = &inventory[&table];
        let pg_columns = sqlx::query(
            "SELECT a.attname::text AS name,
                CASE WHEN t.typcategory='A' THEN e.typname::text || '[]' ELSE t.typname::text END AS storage
             FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid
             LEFT JOIN pg_type e ON e.oid=t.typelem
             WHERE a.attrelid=$1::regclass AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum",
        ).bind(&table).fetch_all(&postgres.pool).await.unwrap();
        let names: Vec<String> = pg_columns.iter().map(|r| r.get("name")).collect();
        assert_eq!(
            serde_json::to_value(names).unwrap(),
            expected["columns"],
            "{table}"
        );
        for column in &pg_columns {
            let name: String = column.get("name");
            assert_eq!(
                column.get::<String, _>("storage"),
                expected["types"][&name].as_str().unwrap(),
                "{table}.{name}"
            );
        }
        let sqlite_columns: BTreeMap<String, String> =
            sqlx::query_as::<_, (String, String)>("SELECT name,type FROM pragma_table_xinfo(?)")
                .bind(&table)
                .fetch_all(&mut *reader)
                .await
                .unwrap()
                .into_iter()
                .collect();
        let expected_columns = expected["types"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, pg_type)| {
                let storage = match pg_type.as_str().unwrap() {
                    "bool" | "int2" | "int4" | "int8" => "INTEGER",
                    "bytea" => "BLOB",
                    "api_format" | "api_format[]" | "bpchar" | "cidr" | "date" | "jsonb"
                    | "numeric" | "text" | "text[]" | "timestamptz" | "uuid" | "uuid[]"
                    | "varchar" => "TEXT",
                    other => panic!("no documented SQLite mapping for {table}.{name}: {other}"),
                };
                (name.clone(), storage.to_owned())
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(sqlite_columns, expected_columns, "{table}");
        let pg_names: Vec<String> = sqlx::query_scalar(
            "SELECT conname::text FROM pg_constraint WHERE conrelid=$1::regclass AND contype IN ('p','u','f','c') ORDER BY conname"
        ).bind(&table).fetch_all(&postgres.pool).await.unwrap();
        let mut expected_names: Vec<&str> = expected["checks"]
            .as_array()
            .unwrap()
            .iter()
            .chain(expected["constraints"].as_array().unwrap())
            .map(|v| v.as_str().unwrap())
            .collect();
        expected_names.sort();
        assert_eq!(pg_names, expected_names, "{table}");

        let mut pg_fks: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object(
                'target',c.confrelid::regclass::text,
                'columns',(SELECT jsonb_agg(a.attname ORDER BY k.ord) FROM unnest(c.conkey) WITH ORDINALITY k(num,ord) JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.num),
                'target_columns',(SELECT jsonb_agg(a.attname ORDER BY k.ord) FROM unnest(c.confkey) WITH ORDINALITY k(num,ord) JOIN pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.num),
                'delete',CASE c.confdeltype WHEN 'c' THEN 'CASCADE' WHEN 'r' THEN 'RESTRICT' ELSE 'NO ACTION' END)
             FROM pg_constraint c WHERE c.conrelid=$1::regclass AND c.contype='f'"
        ).bind(&table).fetch_all(&postgres.pool).await.unwrap();
        let rows = sqlx::query("SELECT * FROM pragma_foreign_key_list(?)")
            .bind(&table)
            .fetch_all(&mut *reader)
            .await
            .unwrap();
        let mut groups = BTreeMap::<i64, Vec<_>>::new();
        for row in rows {
            groups.entry(row.get("id")).or_default().push(row);
        }
        let mut sq_fks = Vec::new();
        for mut rows in groups.into_values() {
            rows.sort_by_key(|row| row.get::<i64, _>("seq"));
            sq_fks.push(json!({
                "target": rows[0].get::<String,_>("table"),
                "columns": rows.iter().map(|r|r.get::<String,_>("from")).collect::<Vec<_>>(),
                "target_columns": rows.iter().map(|r|r.get::<String,_>("to")).collect::<Vec<_>>(),
                "delete": rows[0].get::<String,_>("on_delete"),
            }));
        }
        pg_fks.sort_by_key(Value::to_string);
        sq_fks.sort_by_key(Value::to_string);
        assert_eq!(pg_fks, sq_fks, "{table}");
    }
    let pg_seeds: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id::text,name,description,system_role FROM user_groups ORDER BY id")
            .fetch_all(&postgres.pool)
            .await
            .unwrap();
    let sq_seeds: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id,name,description,system_role FROM user_groups ORDER BY id")
            .fetch_all(&mut *reader)
            .await
            .unwrap();
    assert_eq!(pg_seeds, sq_seeds);
    let pg_formats: Vec<String> = sqlx::query_scalar(
        "SELECT v::text FROM unnest(enum_range(NULL::api_format)) v ORDER BY v::api_format",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    let sq_formats: Vec<String> = sqlx::query_scalar(
        "SELECT value FROM (SELECT 'open_ai_images' AS value UNION ALL
         SELECT 'open_ai_responses' UNION ALL SELECT 'open_ai_chat_completions')
         ORDER BY value COLLATE ag_api_format",
    )
    .fetch_all(&mut *reader)
    .await
    .unwrap();
    assert_eq!(pg_formats, sq_formats);
    for text in [
        "ASCII@example.test",
        "ÄÉΩ@example.test",
        "I@example.test",
        "straße@example.test",
        "İ@example.test",
        "ΟΣ@example.test",
    ] {
        let pg: String = sqlx::query_scalar("SELECT lower($1::text)")
            .bind(text)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
        let sq: String = sqlx::query_scalar("SELECT ag_lower(?)")
            .bind(text)
            .fetch_one(&mut *reader)
            .await
            .unwrap();
        assert_eq!(pg, sq);
    }
    let source = "ai-gateway:codex-images-group:40000000-0000-0000-0000-000000000002";
    let pg: String = sqlx::query_scalar("SELECT md5($1)::uuid::text")
        .bind(source)
        .fetch_one(&postgres.pool)
        .await
        .unwrap();
    let sq: String = sqlx::query_scalar("SELECT ag_md5_uuid(?)")
        .bind(source)
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert_eq!(pg, sq);
    for value in [
        "2026-01-01T12:34:56.123456789Z",
        "1999-12-31T23:59:59.999999999Z",
        "2016-12-31T23:59:60.123456Z",
    ] {
        let value = chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let pg: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT $1::timestamptz")
            .bind(value)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
        let sq: ai_gateway::persistence::sqlite::SqliteTimestamp = sqlx::query_scalar("SELECT ?")
            .bind(ai_gateway::persistence::sqlite::SqliteTimestamp(value))
            .fetch_one(&mut *reader)
            .await
            .unwrap();
        assert_eq!(pg, sq.0);
    }
    macro_rules! numeric_parity {
        ($wrapper:ty,$pg_type:literal,$values:expr) => {
            for text in $values {
                let value = text.parse::<rust_decimal::Decimal>().unwrap();
                let pg: rust_decimal::Decimal =
                    sqlx::query_scalar(concat!("SELECT $1::", $pg_type))
                        .bind(value)
                        .fetch_one(&postgres.pool)
                        .await
                        .unwrap();
                let sq: $wrapper = sqlx::query_scalar("SELECT ?")
                    .bind(<$wrapper>::new(value).unwrap())
                    .fetch_one(&mut *reader)
                    .await
                    .unwrap();
                assert_eq!(pg.to_string(), sq.0.to_string());
            }
        };
    }
    use ai_gateway::persistence::sqlite::{
        SqliteAmount, SqliteNumeric, SqliteTokenRate, SqliteUnitPrice,
    };
    numeric_parity!(
        SqliteAmount,
        "numeric(24,8)",
        [
            "0",
            "1.23",
            "9999999999999999.99999999",
            "-9999999999999999.99999999"
        ]
    );
    numeric_parity!(
        SqliteUnitPrice,
        "numeric(24,12)",
        ["0.000000000001", "999999999999.999999999999"]
    );
    numeric_parity!(
        SqliteNumeric<20, 8>,
        "numeric(20,8)",
        ["0.00000001", "999999999999.99999999"]
    );
    numeric_parity!(
        SqliteTokenRate,
        "numeric(14,4)",
        ["1.2345", "9999999999.9999"]
    );
    drop(reader);
    check_current_sqlite_fact_constraints(sqlite).await;
}

async fn check_current_sqlite_fact_constraints(sqlite: &SqliteDatabase) {
    let mut tx = sqlite.begin_write().await.unwrap();
    let user = "10000000-0000-0000-0000-000000000001";
    let key = "10000000-0000-0000-0000-000000000002";
    let request = "10000000-0000-0000-0000-000000000003";
    sqlx::query("INSERT INTO users(id,display_name) VALUES (?,'Schema parity user')")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
         VALUES (?,?,'Schema parity key','parity-test-secret','active','[\"open_ai_responses\"]','[\"proxy\"]')",
    )
    .bind(key)
    .bind(user)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (format, cost, rejected_by) in [
        (
            "open_ai_images",
            "0",
            Some("request_metering_operation_format_check"),
        ),
        (
            "open_ai_responses",
            "1",
            Some("request_metering_zero_policy_check"),
        ),
        ("open_ai_responses", "0", None),
    ] {
        let result = sqlx::query(
            "INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,
                 request_source,api_format,api_operation,request_protocol,client_model,outcome,cost_amount,peak_pricing)
             VALUES (?,ag_now(),ag_now(),?,?,'client',?,'responses-ws','websocket','test-model','failed',?,0)",
        )
        .bind(request)
        .bind(user)
        .bind(key)
        .bind(format)
        .bind(cost)
        .execute(&mut *tx)
        .await;
        if let Some(constraint) = rejected_by {
            let error = result.unwrap_err();
            let error = error.as_database_error().unwrap();
            assert_eq!(error.code().as_deref(), Some("275"), "{error}");
            assert!(error.message().ends_with(constraint), "{error}");
        } else {
            assert_eq!(result.unwrap().rows_affected(), 1);
        }
    }
    tx.commit().await.unwrap();
}
