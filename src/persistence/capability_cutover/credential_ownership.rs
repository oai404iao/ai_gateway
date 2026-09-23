//! Frozen SQLite table rebuild for the removal of Codex pool ownership.

pub fn upgrade(
    old: &super::legacy_settings::ChannelAuthorizationTopology,
) -> crate::persistence::upstream_topology::UpstreamTopologyRecords {
    use crate::persistence::upstream_topology::{
        LogicalChannelRecord, RoutingGroupRecord, UpstreamTopologyRecords,
    };
    UpstreamTopologyRecords {
        routing_groups: old
            .routing_groups
            .iter()
            .map(|group| RoutingGroupRecord {
                id: group.id,
                name: group.name.clone(),
                enabled: group.enabled,
                created_at: group.created_at,
                updated_at: group.updated_at,
                deleted_at: group.deleted_at,
            })
            .collect(),
        logical_channels: old
            .logical_channels
            .iter()
            .map(|channel| LogicalChannelRecord {
                id: channel.id,
                group_id: channel.group_id,
                access_id: channel.access_id,
                credential_id: channel.credential_id,
                name: channel.name.clone(),
                enabled: channel.enabled,
                sharing_only: channel.deleted_at.is_none()
                    && old
                        .routing_groups
                        .iter()
                        .any(|group| group.id == channel.group_id && group.sharing_only),
                binding_revision: channel.binding_revision,
                created_at: channel.created_at,
                updated_at: channel.updated_at,
                deleted_at: channel.deleted_at,
            })
            .collect(),
        upstream_accesses: old.upstream_accesses.clone(),
        channel_capabilities: old.channel_capabilities.clone(),
        operation_rules: old.operation_rules.clone(),
        operation_tiers: old.operation_tiers.clone(),
        operation_candidates: old.operation_candidates.clone(),
        api_key_grants: old.api_key_grants.clone(),
        policy_grants: old.policy_grants.clone(),
    }
}

#[cfg(feature = "sqlite-backend")]
pub async fn sqlite_prepare(
    connection: &mut sqlx::SqliteConnection,
) -> Result<(), super::io::CapabilityCutoverIoError> {
    use super::io::CapabilityCutoverIoError;
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
    for table in [
        "codex_oauth_credentials",
        "codex_oauth_flows",
        "upstream_credentials",
    ] {
        let ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&mut *connection)
                .await?;
        let removed = ["channel_group_id", "connector_pool_id"];
        let mut lines = ddl
            .lines()
            .filter(|line| !removed.iter().any(|column| line.contains(column)))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let last = lines
            .len()
            .checked_sub(2)
            .ok_or(CapabilityCutoverIoError::InvalidRow)?;
        lines[last] = lines[last].trim_end_matches(',').to_owned();
        let replacement = format!("_credential_new_{table}");
        lines[0] = format!("CREATE TABLE {replacement} (");
        let mut ddl = lines.join("\n");
        if table == "upstream_credentials" {
            ddl = ddl.replace(
                "allowed_base_urls='[]'",
                "json_array_length(allowed_base_urls)>0",
            );
        }
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name=? AND sql IS NOT NULL",
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
            .filter(|column| !removed.contains(&column.as_str()))
            .map(|column| quote(column))
            .collect::<Vec<_>>()
            .join(",");
        let selection = if table == "upstream_credentials" {
            columns.replace("\"allowed_base_urls\"", "CASE WHEN kind='codex_oauth' AND deleted_at IS NULL THEN
                (SELECT json_group_array(DISTINCT a.base_url) FROM upstream_channels c
                 JOIN upstream_accesses a ON a.id=c.access_id WHERE c.credential_id=upstream_credentials.id)
                ELSE allowed_base_urls END")
        } else {
            columns.clone()
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "{ddl}; INSERT INTO {replacement}({columns}) SELECT {selection} FROM {table};
             DROP TABLE {table}; ALTER TABLE {replacement} RENAME TO {table};"
        )))
        .execute(&mut *connection)
        .await?;
        for index in indexes {
            if !removed.iter().any(|column| index.contains(column)) {
                sqlx::raw_sql(sqlx::AssertSqlSafe(index))
                    .execute(&mut *connection)
                    .await?;
            }
        }
    }
    for (_, name, ddl) in objects {
        if name == "credential_pool_insert"
            || name == "credential_pool_update"
            || name.starts_with("codex_oauth_credentials_channel_group_id_fkey_")
            || name.starts_with("codex_oauth_credentials_connector_pool_id_fkey_")
            || name.starts_with("codex_oauth_flows_channel_group_id_fkey_")
        {
            continue;
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
