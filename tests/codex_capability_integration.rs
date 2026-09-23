//! Dual-backend Codex lifecycle, financial state, and fault contracts after capability cutover.

#![cfg(all(feature = "sqlite-backend", target_os = "linux"))]

use ai_gateway::{
    domain::{
        ApiFormat, ApiOperation, RequestBilling, RequestLogEvent, RequestLogOutcome,
        RequestPriceSnapshot, RequestProtocol, RequestUsage,
    },
    persistence::{
        CodexCredentialCreate, ControlPlaneMutation, ControlPlaneRepository, RequestLogRepository,
        SystemPassiveHealthSettingsInput, SystemSettingsInput, SystemUpstreamSettingsInput,
    },
    runtime_config::compile_runtime_config,
};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

#[path = "contracts/sqlite_s5_parity.rs"]
mod contracts;

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let admin_url = std::env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| {
            let mut url =
                reqwest::Url::parse("postgres://ai_gateway@127.0.0.1:5432/postgres").unwrap();
            let password = std::fs::read_to_string("./config/postgres-password")
                .expect("set TEST_DATABASE_ADMIN_URL or supply config/postgres-password");
            url.set_password(Some(password.trim())).unwrap();
            url.to_string()
        });
        let mut url = reqwest::Url::parse(&admin_url).unwrap();
        assert_ne!(url.path().trim_matches('/'), "ai_gateway");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .unwrap();
        let name = format!("ai_gateway_codex_capability_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin)
            .await
            .unwrap();
        url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(url.as_str())
            .await
            .unwrap();
        ai_gateway::persistence::run_migrations(&pool)
            .await
            .unwrap();
        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{}\" WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .unwrap();
        self.admin.close().await;
    }
}

fn system_settings() -> SystemSettingsInput {
    SystemSettingsInput {
        api_hosts: Vec::new(),
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 300,
            standalone_web_search_response_header_timeout_seconds: 300,
            stream_idle_timeout_seconds: 3,
        },
        passive_health: SystemPassiveHealthSettingsInput {
            connection_failure_threshold: 3,
            cooldown_seconds: 30,
        },
        request_retry: Default::default(),
        automatic_disable: Default::default(),
        scheduled_testing: Default::default(),
        session_affinity: Default::default(),
        websocket: Default::default(),
        codex: Default::default(),
    }
}

fn business_codex_credential(
    _channel_group_id: Uuid,
    label: &str,
    email: &str,
    user_id: &str,
) -> CodexCredentialCreate {
    CodexCredentialCreate {
        label: label.into(),
        enabled: true,
        proxy_id: None,
        quota_threshold_percent: 95,
        base_url: "https://chatgpt.com/backend-api/codex".into(),
        email: Some(email.into()),
        account_id: Some("business-workspace".into()),
        user_id: Some(user_id.into()),
        plan_type: Some("business".into()),
        is_fedramp: false,
        id_token: format!("{label}-id-token"),
        access_token: format!("{label}-access-token"),
        refresh_token: format!("{label}-refresh-token"),
        access_token_expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        available_models: vec!["gpt-5-codex".into()],
        quota: None,
    }
}

fn request_log_event(
    user: Uuid,
    key: Uuid,
    model: Uuid,
    group: Uuid,
    capability: Uuid,
) -> RequestLogEvent {
    use rust_decimal::Decimal;
    let now = Utc::now();
    RequestLogEvent {
        id: Uuid::new_v4(),
        started_at: now,
        completed_at: now,
        user_id: user,
        api_key_id: key,
        request_source: ai_gateway::domain::RequestLogSource::Client,
        api_format: ApiFormat::OpenAiResponses,
        api_operation: ApiOperation::Responses,
        request_protocol: RequestProtocol::NonStream,
        client_model: "s5-model".into(),
        reasoning_effort: Some("high".into()),
        fast_mode: true,
        upstream_model: Some("upstream-v1".into()),
        model_rule_id: None,
        channel_group_id: Some(group),
        channel_id: Some(capability),
        upstream_credential: None,
        model_id: Some(model),
        outcome: RequestLogOutcome::Succeeded,
        response_status_code: Some(200),
        streamed: false,
        ttft_ms: Some(1),
        total_duration_ms: 2,
        billing: Some(RequestBilling {
            usage: Some(RequestUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
                reasoning_tokens: 1,
            }),
            price: RequestPriceSnapshot {
                currency: "USD".into(),
                price_unit_tokens: 1_000_000,
                price_effective_at: now,
                input_unit_price: Decimal::new(100, 2),
                cached_input_unit_price: Decimal::new(20, 2),
                cache_write_unit_price: Decimal::new(30, 2),
                output_unit_price: Decimal::new(200, 2),
            },
            cost_amount: Some(Decimal::new(999, 8)),
            output_tokens_per_second: Some(Decimal::new(20, 2)),
            peak_pricing: true,
        }),
        error_code: None,
        error_summary: None,
    }
}
