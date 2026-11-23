//! Throwaway PostgreSQL lifecycle, migrations, control-plane seeding, and log summaries.

use std::{collections::HashMap, error::Error, time::Duration};

use ai_gateway::{
    persistence::{
        ControlPlaneRepository, SystemPassiveHealthSettingsInput,
        SystemScheduledTestingSettingsInput, SystemSettingsInput, SystemUpstreamSettingsInput,
        run_migrations,
    },
    runtime_config::compile_runtime_config,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::scenario::{ApiKind, CLIENT_API_KEY, Scenario, UPSTREAM_API_KEY};

pub struct TemporaryDatabase {
    admin: PgPool,
    pool: PgPool,
    name: String,
    database_url: String,
}

impl TemporaryDatabase {
    pub async fn create(
        mut database_url: Url,
        scenarios: &[Scenario],
        mock_base_url: &str,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url.as_str())
            .await?;
        let name = format!("ai_gateway_perf_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin)
            .await?;
        database_url.set_path(&format!("/{name}"));
        let database_url_string = database_url.to_string();

        let setup = async {
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .acquire_timeout(Duration::from_secs(5))
                .connect(&database_url_string)
                .await?;
            run_migrations(&pool).await?;
            seed(&pool, scenarios, mock_base_url).await?;
            let repository = ControlPlaneRepository::new(pool.clone());
            repository.ensure_system_settings(system_settings()).await?;
            compile_runtime_config(repository.load_runtime().await?)?;
            Ok::<PgPool, Box<dyn Error + Send + Sync>>(pool)
        }
        .await;

        match setup {
            Ok(pool) => Ok(Self {
                admin,
                pool,
                name,
                database_url: database_url_string,
            }),
            Err(error) => {
                let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DROP DATABASE \"{name}\" WITH (FORCE)"
                )))
                .execute(&admin)
                .await;
                admin.close().await;
                Err(error)
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub async fn request_log_stats(
        &self,
        scenario: &Scenario,
    ) -> Result<PersistedLogStats, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "WITH durable_logs AS (
                 SELECT id,outcome,0 AS source_order
                 FROM request_logs
                 WHERE request_source='client'
                   AND api_format=$1::api_format
                   AND client_model=$2
                 UNION ALL
                 SELECT request_log_id,document->>'outcome',1 AS source_order
                 FROM request_log_ingest
                 CROSS JOIN LATERAL (
                     SELECT convert_from(payload,'UTF8')::jsonb AS document
                 ) AS decoded
                 WHERE document->>'request_source'='client'
                   AND document->>'api_format'=$1
                   AND document->>'client_model'=$2
             ),
             deduplicated AS (
                 SELECT DISTINCT ON (id) id,outcome
                 FROM durable_logs
                 ORDER BY id,source_order
             )
             SELECT outcome,count(*)::bigint
             FROM deduplicated
             GROUP BY outcome",
        )
        .bind(scenario.api_kind.database_name())
        .bind(&scenario.model)
        .fetch_all(&self.pool)
        .await?;
        let mut stats = PersistedLogStats::default();
        for (outcome, count) in rows {
            let count = count.max(0) as u64;
            stats.total = stats.total.saturating_add(count);
            match outcome.as_str() {
                "succeeded" => stats.succeeded = stats.succeeded.saturating_add(count),
                "failed" => stats.failed = stats.failed.saturating_add(count),
                "rejected" => stats.rejected = stats.rejected.saturating_add(count),
                "cancelled" => stats.cancelled = stats.cancelled.saturating_add(count),
                _ => {}
            }
        }
        Ok(stats)
    }

    pub async fn cleanup(self, keep: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.pool.close().await;
        if !keep {
            if !self.name.starts_with("ai_gateway_perf_") {
                return Err("refusing to drop a database without the performance prefix".into());
            }
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE \"{}\" WITH (FORCE)",
                self.name
            )))
            .execute(&self.admin)
            .await?;
        }
        self.admin.close().await;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PersistedLogStats {
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub rejected: u64,
    pub cancelled: u64,
}

fn system_settings() -> SystemSettingsInput {
    SystemSettingsInput {
        api_hosts: Vec::new(),
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 30,
            images_response_header_timeout_seconds: 300,
            standalone_web_search_response_header_timeout_seconds: 300,
            stream_idle_timeout_seconds: 60,
        },
        request_retry: Default::default(),
        passive_health: SystemPassiveHealthSettingsInput {
            connection_failure_threshold: 3,
            cooldown_seconds: 30,
        },
        automatic_disable: Default::default(),
        scheduled_testing: SystemScheduledTestingSettingsInput {
            mode: "global".into(),
            auto_recover: true,
            interval_minutes: 60,
            prompt: "reply '1'".into(),
        },
        session_affinity: Default::default(),
        websocket: Default::default(),
    }
}

async fn seed(
    pool: &PgPool,
    scenarios: &[Scenario],
    mock_base_url: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let user_id = Uuid::new_v4();
    let api_key_id = Uuid::new_v4();
    let chat_group_id = Uuid::new_v4();
    let responses_group_id = Uuid::new_v4();
    let chat_channel_id = Uuid::new_v4();
    let responses_channel_id = Uuid::new_v4();
    let mut transaction = pool.begin().await?;

    sqlx::query(
        "INSERT INTO users
         (id,email,display_name,role,status,balance_amount)
         VALUES ($1,$2,'Forwarding performance user','user','active',0)",
    )
    .bind(user_id)
    .bind(format!("perf-{user_id}@example.test"))
    .execute(&mut *transaction)
    .await?;

    let mut model_ids = HashMap::new();
    for scenario in scenarios {
        let model_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO models
             (id,source_model_id,display_name,provider_name,enabled,currency,
              price_unit_tokens,input_unit_price,cached_input_unit_price,
              cache_write_unit_price,output_unit_price,price_effective_at)
             VALUES ($1,$2,$3,'performance-mock',true,'USD',
                     1000000,0,0,0,0,now())",
        )
        .bind(model_id)
        .bind(&scenario.model)
        .bind(format!("Performance {}", scenario.name))
        .execute(&mut *transaction)
        .await?;
        model_ids.insert(scenario.name.clone(), model_id);
    }

    for (group_id, name) in [
        (chat_group_id, "performance-chat"),
        (responses_group_id, "performance-responses"),
    ] {
        sqlx::query("INSERT INTO routing_groups (id,name,enabled) VALUES ($1,$2,true)")
            .bind(group_id)
            .bind(name)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("INSERT INTO group_identity_registry(id,label,created_at,canonical_group_id) VALUES($1,$2,now(),$1)")
            .bind(group_id).bind(name).execute(&mut *transaction).await?;
    }

    let chat_models = scenarios
        .iter()
        .filter(|scenario| scenario.api_kind == ApiKind::ChatCompletions)
        .map(|scenario| scenario.model.clone())
        .collect::<Vec<_>>();
    let responses_models = scenarios
        .iter()
        .filter(|scenario| scenario.api_kind == ApiKind::Responses)
        .map(|scenario| scenario.model.clone())
        .collect::<Vec<_>>();
    for (channel_id, group_id, name, api_kind, models) in [
        (
            chat_channel_id,
            chat_group_id,
            "performance-chat",
            ApiKind::ChatCompletions,
            chat_models,
        ),
        (
            responses_channel_id,
            responses_group_id,
            "performance-responses",
            ApiKind::Responses,
            responses_models,
        ),
    ] {
        sqlx::query(
            "INSERT INTO upstream_credentials(id,name,kind,secret,allowed_base_urls)
             VALUES($1,$2,'bearer',$3,jsonb_build_array($4::text))",
        )
        .bind(channel_id)
        .bind(name)
        .bind(UPSTREAM_API_KEY)
        .bind(mock_base_url)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO upstream_accesses(id,name,connector_kind,base_url,enabled) VALUES($1,$2,'general',$3,true)")
            .bind(channel_id).bind(name).bind(mock_base_url).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO upstream_channels(id,group_id,access_id,credential_id,name,enabled) VALUES($1,$2,$1,$1,$3,true)")
            .bind(channel_id).bind(group_id).bind(name).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO channel_capabilities(id,channel_id,operation,enabled,available_models,auto_disable_allowed) VALUES($1,$1,$2,true,$3,false)")
            .bind(channel_id).bind(operation(api_kind)).bind(models).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO channel_identity_registry(id,label,created_at,canonical_channel_id,capability_id) VALUES($1,$2,now(),$1,$1)")
            .bind(channel_id).bind(name).execute(&mut *transaction).await?;
    }

    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,permissions,
          allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,'performance',$3,'active',
                 ARRAY['proxy','models.read']::text[],
                 ARRAY[$4,$5]::uuid[],ARRAY[]::uuid[])",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(CLIENT_API_KEY)
    .bind(chat_group_id)
    .bind(responses_group_id)
    .execute(&mut *transaction)
    .await?;

    for scenario in scenarios {
        let channel_id = match scenario.api_kind {
            ApiKind::ChatCompletions => chat_channel_id,
            ApiKind::Responses => responses_channel_id,
        };
        let model_rule_profile_id = Uuid::new_v4();
        let model_rule_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO model_routing_profiles
             (id,model_id)
             VALUES ($1,$2)",
        )
        .bind(model_rule_profile_id)
        .bind(model_ids[&scenario.name])
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO model_operation_rules
             (id,model_routing_profile_id,operation,enabled)
             VALUES ($1,$2,$3,true)",
        )
        .bind(model_rule_id)
        .bind(model_rule_profile_id)
        .bind(operation(scenario.api_kind))
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO model_rule_identity_registry(id,label,created_at,canonical_rule_id) VALUES($1,$2,now(),$1)")
            .bind(model_rule_id).bind(&scenario.model).execute(&mut *transaction).await?;
        let tier_id = Uuid::new_v4();
        sqlx::query("INSERT INTO model_capability_tiers(id,rule_id,operation,priority,strategy) VALUES($1,$2,$3,0,'weighted_random')")
            .bind(tier_id).bind(model_rule_id).bind(operation(scenario.api_kind)).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO model_capability_candidates(tier_id,operation,capability_id,upstream_model,weight) VALUES($1,$2,$3,$4,1)")
        .bind(tier_id)
        .bind(operation(scenario.api_kind))
        .bind(channel_id)
        .bind(&scenario.model)
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;
    Ok(())
}

fn operation(kind: ApiKind) -> &'static str {
    match kind {
        ApiKind::ChatCompletions => "chat_completion",
        ApiKind::Responses => "responses",
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{TemporaryDatabase, operation};
    use crate::scenario::{CLIENT_API_KEY, ProfileName, profile};

    #[tokio::test]
    #[ignore = "requires PostgreSQL; validates migrations and performance control-plane seeding"]
    async fn temporary_database_seeds_a_compilable_snapshot() {
        let admin_url = crate::postgres::admin_url(None).unwrap();
        let profile = profile(ProfileName::Quick);
        let database =
            TemporaryDatabase::create(admin_url, &profile.scenarios, "http://127.0.0.1:9")
                .await
                .unwrap();
        for scenario in &profile.scenarios {
            assert_eq!(database.request_log_stats(scenario).await.unwrap().total, 0);
        }
        let scenario = &profile.scenarios[0];
        let (user, key): (Uuid, Uuid) =
            sqlx::query_as("SELECT user_id,id FROM api_keys WHERE secret_value=$1")
                .bind(CLIENT_API_KEY)
                .fetch_one(&database.pool)
                .await
                .unwrap();
        let now = chrono::Utc::now();
        let event: ai_gateway::domain::RequestLogEvent = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "started_at": now, "completed_at": now, "user_id": user, "api_key_id": key,
            "request_source": "client",
            "api_format": scenario.api_kind.database_name(),
            "api_operation": operation(scenario.api_kind),
            "request_protocol": ai_gateway::domain::RequestProtocol::from_http_streamed(scenario.streamed),
            "client_model": scenario.model,
            "upstream_credential_id": null, "fast_mode": false,
            "outcome": "succeeded", "response_status_code": 200, "total_duration_ms": 0, "error_summary": null
        }))
        .unwrap();
        sqlx::query(
            "INSERT INTO request_log_ingest
             (request_log_id,schema_version,payload)
             VALUES ($1,10,$2)",
        )
        .bind(event.id)
        .bind(serde_json::to_vec(&event).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();
        let stats = database.request_log_stats(scenario).await.unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.succeeded, 1);
        database.cleanup(false).await.unwrap();
    }
}
