//! Transaction-scoped application of the frozen operation split.

use sqlx::{Postgres, Transaction};

#[cfg(feature = "sqlite-backend")]
use crate::persistence::RepositoryError;
use crate::persistence::capability_cutover::io::CapabilityCutoverIoError;
use crate::persistence::upstream_topology;

pub async fn pg_prepare(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), CapabilityCutoverIoError> {
    let topology = upstream_topology::pg_load_legacy(transaction).await?;
    let plan = super::plan(&topology).map_err(|_| CapabilityCutoverIoError::InvalidRow)?;
    sqlx::query("CREATE TEMP TABLE _operation_upgrade_plan(plan jsonb NOT NULL) ON COMMIT DROP")
        .execute(&mut **transaction)
        .await?;
    sqlx::query("INSERT INTO _operation_upgrade_plan(plan) VALUES ($1)")
        .bind(sqlx::types::Json(plan))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub async fn pg_validate(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), CapabilityCutoverIoError> {
    let missing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM channel_capabilities c
             LEFT JOIN channel_identity_registry r ON r.id=c.id WHERE r.id IS NULL)
         OR EXISTS(SELECT 1 FROM model_operation_rules c
             LEFT JOIN model_rule_identity_registry r ON r.id=c.id WHERE r.id IS NULL)",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if missing {
        return Err(CapabilityCutoverIoError::InvalidRow);
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub async fn sqlite_apply(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), CapabilityCutoverIoError> {
    let topology = upstream_topology::sqlite_load_legacy(transaction).await?;
    let plan = super::plan(&topology).map_err(|_| CapabilityCutoverIoError::InvalidRow)?;
    let connection = &mut **transaction;
    if sqlx::query_scalar::<_, bool>("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?
    {
        return Err(CapabilityCutoverIoError::InvalidRow);
    }
    sqlx::query("CREATE TEMP TABLE _operation_upgrade_plan(plan TEXT NOT NULL)")
        .execute(&mut *connection)
        .await?;
    let mut payload =
        serde_json::to_value(&plan).map_err(|_| CapabilityCutoverIoError::InvalidRow)?;
    for (name, times) in [
        (
            "api_key_grants",
            plan.api_key_grants
                .iter()
                .map(|row| row.created_at)
                .collect::<Vec<_>>(),
        ),
        (
            "policy_grants",
            plan.policy_grants
                .iter()
                .map(|row| row.created_at)
                .collect::<Vec<_>>(),
        ),
    ] {
        for (row, time) in payload[name]
            .as_array_mut()
            .ok_or(CapabilityCutoverIoError::InvalidRow)?
            .iter_mut()
            .zip(times)
        {
            row["created_at"] = time
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
                .into();
        }
    }
    sqlx::query("INSERT INTO _operation_upgrade_plan VALUES (?)")
        .bind(sqlx::types::Json(payload))
        .execute(&mut *connection)
        .await?;
    let objects: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema WHERE type IN ('trigger','view') AND sql IS NOT NULL
         ORDER BY CASE type WHEN 'view' THEN 0 ELSE 1 END,name")
        .fetch_all(&mut *connection).await?;
    for (kind, name, _) in objects.iter().rev() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP {} {}",
            if kind == "view" { "VIEW" } else { "TRIGGER" },
            quote(name)
        )))
        .execute(&mut *connection)
        .await?;
    }
    for (table, mapping) in [
        ("connector_pools", ""),
        ("upstream_accesses", "accesses"),
        ("channel_capabilities", "capabilities"),
        ("model_operation_rules", "rules"),
        ("model_capability_tiers", "tiers"),
        ("model_capability_candidates", "candidates"),
        ("api_key_capability_grants", "api_key_grants"),
        ("api_key_policy_capability_grants", "policy_grants"),
        ("request_logs", ""),
        ("request_metering_facts", ""),
    ] {
        rebuild(connection, table, mapping).await?;
    }
    sqlx::raw_sql(
        "INSERT INTO channel_identity_registry
         (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id)
         SELECT json_extract(p.value,'$.id'),r.label,r.created_at,r.canonical_channel_id,
             r.codex_credential_id,json_extract(p.value,'$.id')
         FROM _operation_upgrade_plan,json_each(plan,'$.capabilities') p
         JOIN channel_identity_registry r ON r.id=json_extract(p.value,'$.source_id')
         WHERE json_extract(p.value,'$.id')<>json_extract(p.value,'$.source_id');
         INSERT INTO model_rule_identity_registry (id,label,created_at,canonical_rule_id)
         SELECT json_extract(p.value,'$.id'),r.label,r.created_at,json_extract(p.value,'$.id')
         FROM _operation_upgrade_plan,json_each(plan,'$.rules') p
         JOIN model_rule_identity_registry r ON r.id=json_extract(p.value,'$.source_id')
         WHERE json_extract(p.value,'$.id')<>json_extract(p.value,'$.source_id');",
    )
    .execute(&mut *connection)
    .await?;
    for (_, _, ddl) in objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
    }
    let missing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM channel_capabilities c LEFT JOIN channel_identity_registry r ON r.id=c.id WHERE r.id IS NULL)
         OR EXISTS(SELECT 1 FROM model_operation_rules c LEFT JOIN model_rule_identity_registry r ON r.id=c.id WHERE r.id IS NULL)")
        .fetch_one(&mut *connection).await?;
    if missing
        || sqlx::query("PRAGMA foreign_key_check")
            .fetch_optional(&mut *connection)
            .await?
            .is_some()
    {
        return Err(CapabilityCutoverIoError::InvalidRow);
    }
    sqlx::query("DROP TABLE _operation_upgrade_plan")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
async fn rebuild(
    connection: &mut sqlx::SqliteConnection,
    table: &str,
    mapping: &str,
) -> Result<(), RepositoryError> {
    let ddl: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?")
            .bind(table)
            .fetch_one(&mut *connection)
            .await?;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name=? AND sql IS NOT NULL ORDER BY name")
        .bind(table).fetch_all(&mut *connection).await?;
    let mut columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_xinfo(?) WHERE hidden=0 ORDER BY cid")
            .bind(table)
            .fetch_all(&mut *connection)
            .await?;
    let mut ddl = upgrade_ddl(table, ddl)?;
    let prefix = format!("CREATE TABLE {table} (");
    let quoted_prefix = format!("CREATE TABLE \"{table}\" (");
    let prefix = if ddl.starts_with(&prefix) {
        prefix
    } else {
        quoted_prefix
    };
    if !ddl.starts_with(&prefix) {
        return Err(RepositoryError::Validation);
    }
    let replacement = format!("_operation_new_{table}");
    ddl = ddl.replacen(&prefix, &format!("CREATE TABLE {replacement} ("), 1);
    if table == "channel_capabilities" {
        columns.retain(|column| column != "transports");
    }
    let names = columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(",");
    let expressions = columns
        .iter()
        .map(|column| {
            let json = |field: &str| format!("json_extract(p.value,'$.{field}')");
            let source = || format!("source.{}", quote(column));
            match (table, column.as_str()) {
                ("connector_pools", "connector_kind") => "'codex'".to_owned(),
                (
                    "model_capability_candidates"
                    | "api_key_capability_grants"
                    | "api_key_policy_capability_grants",
                    _,
                ) => json(column),
                ("upstream_accesses", "connector_kind") => json("connector"),
                (
                    "channel_capabilities" | "model_operation_rules" | "model_capability_tiers",
                    "id" | "operation",
                ) => json(column),
                ("model_capability_tiers", "rule_id") => json(column),
                ("channel_capabilities", "request_compression") => format!(
                    "CASE WHEN {}='responses-ws' THEN 'default' ELSE {} END",
                    json("operation"),
                    source()
                ),
                ("channel_capabilities", "test_model" | "test_pricing_model_id") => format!(
                    "CASE WHEN {}='responses-ws' THEN NULL ELSE {} END",
                    json("operation"),
                    source()
                ),
                _ => source(),
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let from = if mapping.is_empty() {
        format!("{table} source")
    } else if matches!(
        table,
        "model_capability_candidates"
            | "api_key_capability_grants"
            | "api_key_policy_capability_grants"
    ) {
        format!("_operation_upgrade_plan,json_each(plan,'$.{mapping}') p")
    } else {
        let source_id = if table == "upstream_accesses" {
            "id"
        } else {
            "source_id"
        };
        format!(
            "_operation_upgrade_plan,json_each(plan,'$.{mapping}') p JOIN {table} source ON source.id=json_extract(p.value,'$.{source_id}')"
        )
    };
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&mut *connection)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {replacement} ({names}) SELECT {expressions} FROM {from}"
    )))
    .execute(&mut *connection)
    .await?;
    if matches!(table, "request_logs" | "request_metering_facts") {
        let changed: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT * FROM {table} EXCEPT SELECT * FROM {replacement})
             OR EXISTS(SELECT * FROM {replacement} EXCEPT SELECT * FROM {table})"
        )))
        .fetch_one(&mut *connection)
        .await?;
        if changed {
            return Err(RepositoryError::Validation);
        }
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP TABLE {table}; ALTER TABLE {replacement} RENAME TO {table};"
    )))
    .execute(&mut *connection)
    .await?;
    for ddl in indexes {
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
fn upgrade_ddl(table: &str, mut ddl: String) -> Result<String, RepositoryError> {
    let operations = "operation IN ('chat_completion','responses','responses-ws','web_search','images_edit','images_generation')";
    match table {
        "connector_pools" => replace_check(
            &mut ddl,
            "connector_pools_connector_kind_check",
            Some("connector_kind='codex'"),
        )?,
        "upstream_accesses" => replace_check(
            &mut ddl,
            "upstream_accesses_connector_kind_check",
            Some("connector_kind IN ('general','codex')"),
        )?,
        "channel_capabilities" => {
            let lines: Vec<_> = ddl.lines().filter(|line| !line.contains("transports TEXT NOT NULL CONSTRAINT channel_capabilities_transports_storage")).collect();
            if lines.len() == ddl.lines().count() {
                return Err(RepositoryError::Validation);
            }
            ddl = lines.join("\n");
            replace_check(&mut ddl, "channel_capabilities_transports_check", None)?;
            replace_check(
                &mut ddl,
                "channel_capabilities_operation_check",
                Some(operations),
            )?;
            replace_check(
                &mut ddl,
                "channel_capabilities_request_compression_operation_check",
                Some("request_compression='default' OR operation='responses'"),
            )?;
            replace_check(
                &mut ddl,
                "channel_capabilities_probe_check",
                Some("test_model IS NULL OR operation IN ('chat_completion','responses')"),
            )?;
        }
        "model_operation_rules" => replace_check(
            &mut ddl,
            "model_operation_rules_operation_check",
            Some(operations),
        )?,
        "request_logs" | "request_metering_facts" => {
            if table == "request_metering_facts" {
                let legacy = "(api_operation = 'standalone_web_search')";
                if ddl.matches(legacy).count() != 1 {
                    return Err(RepositoryError::Validation);
                }
                ddl = ddl.replace(
                    legacy,
                    "(api_operation IN ('standalone_web_search','web_search'))",
                );
            }
            let constraint = if table == "request_logs" {
                "request_logs_api_operation_format_check"
            } else {
                "request_metering_operation_format_check"
            };
            replace_check(&mut ddl, constraint, Some(
                "(api_format='open_ai_chat_completions' AND api_operation IN ('chat_completion','chat_completions'))
                 OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search','standalone_web_search'))
                 OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit'))"))?;
        }
        _ => {}
    }
    Ok(ddl)
}

#[cfg(feature = "sqlite-backend")]
fn replace_check(
    ddl: &mut String,
    name: &str,
    expression: Option<&str>,
) -> Result<(), RepositoryError> {
    let marker = format!("CONSTRAINT {name}");
    if ddl.matches(&marker).count() != 1 {
        return Err(RepositoryError::Validation);
    }
    let start = ddl.find(&marker).ok_or(RepositoryError::Validation)?;
    let body = start
        + ddl[start..]
            .find("CHECK")
            .ok_or(RepositoryError::Validation)?;
    let open = body + ddl[body..].find('(').ok_or(RepositoryError::Validation)?;
    let mut depth = 0;
    let mut end = None;
    let mut quoted = false;
    for (index, character) in ddl[open..].char_indices() {
        if character == '\'' {
            quoted = !quoted;
        }
        if quoted {
            continue;
        }
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + index + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let mut end = end.ok_or(RepositoryError::Validation)?;
    let replacement = if let Some(expression) = expression {
        format!("{marker} CHECK ({expression})")
    } else {
        if ddl.as_bytes().get(end) != Some(&b',') {
            return Err(RepositoryError::Validation);
        }
        end += 1;
        String::new()
    };
    ddl.replace_range(start..end, &replacement);
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
