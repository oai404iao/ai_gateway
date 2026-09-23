//! In-crate local-provider contract for the application's durable dispatch boundary.

use super::*;
use crate::{
    persistence::{
        AuthRepository, SystemPassiveHealthSettingsInput, SystemSettingsInput,
        SystemUpstreamSettingsInput, sqlite::SqliteDatabase,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::compile_runtime_config,
};
use axum::{
    Json, Router,
    http::{HeaderMap, Uri},
    routing::{get, post},
};
use std::{os::unix::fs::PermissionsExt, time::Duration};

async fn pending(database: &SqliteDatabase) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM _gateway_codex_operations")
        .fetch_one(&mut *database.acquire_read().await.unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn local_validation_precedes_intent_and_cancelled_http_remains_fenced() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("gateway.sqlite");
    let database = Arc::new(SqliteDatabase::open(&path).await.unwrap());
    database.install_schema().await.unwrap();
    let repo = ControlPlaneRepository::from_sqlite(Arc::clone(&database));
    let admin = AuthRepository::from_sqlite(Arc::clone(&database))
        .bootstrap_admin("sqlite-codex@example.test", "Admin", "$argon2id$fixture")
        .await
        .unwrap();
    repo.ensure_system_settings(SystemSettingsInput {
        api_hosts: vec![],
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 30,
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
    })
    .await
    .unwrap();
    let input = CodexCredentialCreate {
        label: "Fixture".into(),
        enabled: true,
        proxy_id: None,
        quota_threshold_percent: 95,
        base_url: "https://chatgpt.com/backend-api/codex".into(),
        email: Some("member@example.test".into()),
        account_id: Some("account".into()),
        user_id: Some("member".into()),
        plan_type: Some("business".into()),
        is_fedramp: false,
        id_token: "fixture-id".into(),
        access_token: "invalid\nheader".into(),
        refresh_token: "fixture-refresh".into(),
        access_token_expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        available_models: vec!["gpt-5-codex".into()],
        quota: None,
    };
    let id = repo
        .prepare_codex_credential_create(admin, input.clone(), None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repo.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repo.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let (received, mut requests) = tokio::sync::mpsc::channel(4);
    let reset_database = Arc::clone(&database);
    let reset_received = received.clone();
    let token_database = Arc::clone(&database);
    let router = Router::new()
        .route(
            "/oauth/token",
            post(move || {
                let db = Arc::clone(&token_database);
                let received = received.clone();
                async move {
                    assert_eq!(pending(&db).await, 1);
                    received.send("refresh").await.unwrap();
                    Json(serde_json::json!({"refresh_token":"rotated-fixture"}))
                }
            }),
        )
        .route(
            "/backend-api/wham/rate-limit-reset-credits/consume",
            post(move || {
                let db = Arc::clone(&reset_database);
                let received = reset_received.clone();
                async move {
                    assert_eq!(pending(&db).await, 1);
                    received.send("reset").await.unwrap();
                    std::future::pending::<Json<serde_json::Value>>().await
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let service = CodexConnectorService::new_with_endpoints(
        repo.clone(),
        coordinator,
        runtime,
        Arc::new(UpstreamClientRegistry::new()),
        CodexEndpoints {
            issuer: base.parse().unwrap(),
            responses_base_url: format!("{base}/backend-api/codex").parse().unwrap(),
        },
    )
    .await
    .unwrap();
    assert!(service.reset_quota(admin, id).await.is_err());
    assert_eq!(pending(&database).await, 0);
    assert!(requests.try_recv().is_err());
    service.refresh_credential(admin, id).await.unwrap();
    assert_eq!(requests.recv().await, Some("refresh"));
    assert_eq!(pending(&database).await, 0);
    let mut valid = input;
    valid.access_token = "valid-fixture-access".into();
    repo.prepare_codex_credential_create(admin, valid, None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let task_service = service.clone();
    let task = tokio::spawn(async move { task_service.reset_quota(admin, id).await });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), requests.recv())
            .await
            .unwrap(),
        Some("reset")
    );
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    assert_eq!(pending(&database).await, 1);
    assert!(service.reset_quota(admin, id).await.is_err());
    assert!(requests.try_recv().is_err());
    database.close().await;
    let reopened = Arc::new(SqliteDatabase::open(&path).await.unwrap());
    assert!(
        ControlPlaneRepository::from_sqlite(Arc::clone(&reopened))
            .lock_codex_quota_reset(id)
            .await
            .is_err()
    );
    reopened.close().await;
    server.abort();
}

#[tokio::test]
async fn model_discovery_fetches_supported_codex_models_for_the_credential() {
    use axum::serve;

    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("gateway.sqlite");
    let database = Arc::new(SqliteDatabase::open(&path).await.unwrap());
    database.install_schema().await.unwrap();
    let repo = ControlPlaneRepository::from_sqlite(Arc::clone(&database));
    let admin = AuthRepository::from_sqlite(Arc::clone(&database))
        .bootstrap_admin(
            "sqlite-codex-models@example.test",
            "Admin",
            "$argon2id$fixture",
        )
        .await
        .unwrap();
    repo.ensure_system_settings(SystemSettingsInput {
        api_hosts: vec![],
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 30,
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
    })
    .await
    .unwrap();
    let credential_id = repo
        .prepare_codex_credential_create(
            admin,
            CodexCredentialCreate {
                label: "Fixture".into(),
                enabled: true,
                proxy_id: None,
                quota_threshold_percent: 95,
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                email: Some("member@example.test".into()),
                account_id: Some("account".into()),
                user_id: Some("member".into()),
                plan_type: Some("business".into()),
                is_fedramp: false,
                id_token: "fixture-id".into(),
                access_token: "valid-fixture-access".into(),
                refresh_token: "fixture-refresh".into(),
                access_token_expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                available_models: vec!["stale-fixture".into()],
                quota: None,
            },
            None,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repo.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repo.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let models = Router::new().route(
        "/backend-api/codex/models",
        get(|headers: HeaderMap, uri: Uri| async move {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer valid-fixture-access")
            );
            assert_eq!(
                headers
                    .get("chatgpt-account-id")
                    .and_then(|value| value.to_str().ok()),
                Some("account")
            );
            assert!(
                uri.query()
                    .is_some_and(|query| query.contains("client_version=")),
                "models request must carry the configured client version"
            );
            Json(serde_json::json!({
                "models": [
                    {"slug": "gpt-5-codex"},
                    {"slug": "gpt-5", "supported_in_api": false},
                    {"slug": "gpt-5-codex"}
                ]
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { serve(listener, models).await.unwrap() });
    let service = CodexConnectorService::new_with_endpoints(
        repo,
        coordinator,
        runtime,
        Arc::new(UpstreamClientRegistry::new()),
        CodexEndpoints {
            issuer: base.parse().unwrap(),
            responses_base_url: format!("{base}/backend-api/codex").parse().unwrap(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        service.discover_models(credential_id).await.unwrap(),
        vec!["gpt-5-codex".to_owned()]
    );
    server.abort();
    database.close().await;
}
