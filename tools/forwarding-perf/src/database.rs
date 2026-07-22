//! Throwaway PostgreSQL lifecycle, migrations, control-plane seeding, and log summaries.

use std::{collections::HashMap, error::Error, fs, path::Path, time::Duration};

use ai_gateway::{
    persistence::{
        ControlPlaneRepository, MIGRATOR, SystemPassiveHealthSettingsInput,
        SystemScheduledTestingSettingsInput, SystemSettingsInput, SystemUpstreamSettingsInput,
    },
    runtime_config::compile_runtime_config,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::scenario::{ApiKind, CLIENT_API_KEY, Scenario, UPSTREAM_API_KEY};

const LEGACY_DATABASE_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";
const PASSWORD_FILE_DATABASE_ADMIN_URL: &str = "postgres://ai_gateway@127.0.0.1:5432/postgres";

pub fn default_database_admin_url() -> String {
    database_admin_url_from_password_file(Path::new("./config/postgres-password"))
        .unwrap_or_else(|| LEGACY_DATABASE_ADMIN_URL.into())
}

fn database_admin_url_from_password_file(path: &Path) -> Option<String> {
    let mut password = fs::read_to_string(path).ok()?;
    while matches!(password.as_bytes().last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    if password.is_empty() {
        return None;
    }
    let mut url = Url::parse(PASSWORD_FILE_DATABASE_ADMIN_URL).ok()?;
    url.set_password(Some(&password)).ok()?;
    Some(url.to_string())
}

pub struct TemporaryDatabase {
    admin: PgPool,
    pool: PgPool,
    name: String,
    database_url: String,
}

impl TemporaryDatabase {
    pub async fn create(
        admin_url: &str,
        scenarios: &[Scenario],
        mock_base_url: &str,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut database_url = Url::parse(admin_url)
            .map_err(|error| format!("invalid database admin URL: {error}"))?;
        let admin_database = database_url.path().trim_matches('/');
        if admin_database.is_empty() || admin_database == "ai_gateway" {
            return Err(
                "database admin URL must target an administrative database such as postgres".into(),
            );
        }
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(admin_url)
            .await?;
        let name = format!("ai_gateway_perf_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
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
            MIGRATOR.run(&pool).await?;
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
                let _ = sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
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
            sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
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
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 30,
            stream_idle_timeout_seconds: 60,
        },
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

    for (group_id, name, api_kind) in [
        (chat_group_id, "performance-chat", ApiKind::ChatCompletions),
        (
            responses_group_id,
            "performance-responses",
            ApiKind::Responses,
        ),
    ] {
        sqlx::query(
            "INSERT INTO channel_groups
             (id,name,api_format,priority,selection_strategy,enabled)
             VALUES ($1,$2,$3::api_format,0,'weighted_random',true)",
        )
        .bind(group_id)
        .bind(name)
        .bind(api_kind.database_name())
        .execute(&mut *transaction)
        .await?;
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
            "INSERT INTO channels
             (id,channel_group_id,api_format,name,base_url,enabled,weight,
              upstream_auth_kind,upstream_api_key,available_models,
              auto_disable_allowed)
             VALUES ($1,$2,$3::api_format,$4,$5,true,1,
                     'bearer',$6,$7,false)",
        )
        .bind(channel_id)
        .bind(group_id)
        .bind(api_kind.database_name())
        .bind(name)
        .bind(mock_base_url)
        .bind(UPSTREAM_API_KEY)
        .bind(models)
        .execute(&mut *transaction)
        .await?;
    }

    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
          allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,'performance',$3,'active',
                 ARRAY['open_ai_chat_completions','open_ai_responses']::api_format[],
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
        sqlx::query(
            "INSERT INTO model_rules
             (id,client_model,api_format,upstream_model_id,channel_ids,enabled)
             VALUES ($1,$2,$3::api_format,$4,ARRAY[$5]::uuid[],true)",
        )
        .bind(Uuid::new_v4())
        .bind(&scenario.model)
        .bind(scenario.api_kind.database_name())
        .bind(model_ids[&scenario.name])
        .bind(channel_id)
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{TemporaryDatabase, default_database_admin_url};
    use crate::scenario::{ProfileName, profile};

    #[tokio::test]
    #[ignore = "requires PostgreSQL; validates migrations and performance control-plane seeding"]
    async fn temporary_database_seeds_a_compilable_snapshot() {
        let admin_url = std::env::var("TEST_DATABASE_ADMIN_URL")
            .unwrap_or_else(|_| default_database_admin_url());
        let profile = profile(ProfileName::Quick);
        let database =
            TemporaryDatabase::create(&admin_url, &profile.scenarios, "http://127.0.0.1:9")
                .await
                .unwrap();
        for scenario in &profile.scenarios {
            assert_eq!(database.request_log_stats(scenario).await.unwrap().total, 0);
        }
        let scenario = &profile.scenarios[0];
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": Uuid::new_v4(),
            "request_source": "client",
            "api_format": scenario.api_kind.database_name(),
            "client_model": scenario.model,
            "outcome": "succeeded"
        }))
        .unwrap();
        sqlx::query(
            "INSERT INTO request_log_ingest
             (request_log_id,schema_version,payload)
             VALUES ($1,1,$2)",
        )
        .bind(Uuid::new_v4())
        .bind(payload)
        .execute(&database.pool)
        .await
        .unwrap();
        let stats = database.request_log_stats(scenario).await.unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.succeeded, 1);
        database.cleanup(false).await.unwrap();
    }
}
