use super::*;
use ai_gateway::persistence::{RepositoryError, StorageFailureKind};

fn create_proxy(name: &str) -> ControlPlaneMutation {
    ControlPlaneMutation::CreateProxy(ProxyCreateInput {
        name: name.into(),
        proxy_url: "http://proxy.example.test:8080".into(),
        username: None,
        password: None,
        no_proxy_hosts: Vec::new(),
        enabled: true,
    })
}

fn coordinator(
    repository: ControlPlaneRepository,
    runtime: Arc<RuntimeConfig>,
) -> ControlPlaneCoordinator {
    ControlPlaneCoordinator::new(
        repository,
        runtime,
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    )
}

#[test]
fn application_and_http_do_not_depend_on_driver_types_or_errors() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root.join("application"), root.join("http")];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            for forbidden in ["sqlx::", "PgPool", "Postgres", ".as_database_error("] {
                assert!(
                    !source.contains(forbidden),
                    "{} contains {forbidden}",
                    path.display()
                );
            }
        }
    }
}

#[tokio::test]
async fn prepared_change_cancellation_rolls_back_and_releases_its_locks() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let (ready, observed) = tokio::sync::oneshot::channel();
    let task_repository = repository.clone();
    let actor = seed.user;
    let task = tokio::spawn(async move {
        let mut change = task_repository
            .prepare_mutation(actor, create_proxy("cancelled-change"))
            .await
            .unwrap();
        let records = change.runtime_records().await.unwrap();
        assert!(
            records
                .control_plane
                .proxies
                .iter()
                .any(|proxy| proxy.name == "cancelled-change")
        );
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(change);
    });
    timeout(Duration::from_secs(3), observed)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM proxies WHERE name='cancelled-change'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = coordinator(repository, runtime);
    let result = timeout(
        Duration::from_secs(3),
        coordinator.mutate(seed.user, create_proxy("cancelled-change")),
    )
    .await
    .unwrap()
    .unwrap();
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE object_id=$1")
        .bind(result.id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audits, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn audit_failure_rolls_back_prepared_data_and_never_publishes_a_candidate() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let before = runtime.snapshot();
    let coordinator = coordinator(repository, Arc::clone(&runtime));
    sqlx::raw_sql(
        "CREATE FUNCTION contract_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected audit failure'; END $$;
         CREATE TRIGGER contract_reject_audit BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION contract_reject_audit();",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        coordinator
            .mutate(seed.user, create_proxy("audit-failure"))
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(&before, &runtime.snapshot()));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proxies WHERE name='audit-failure'")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::raw_sql("DROP FUNCTION contract_reject_audit() CASCADE")
        .execute(&database.pool)
        .await
        .unwrap();
    let result = coordinator
        .mutate(seed.user, create_proxy("audit-failure"))
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&before, &runtime.snapshot()));
    let correlation: String =
        sqlx::query_scalar("SELECT correlation_id FROM audit_logs WHERE object_id=$1")
            .bind(result.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(correlation, result.correlation_id.unwrap().to_string());
    database.cleanup().await;
}

#[tokio::test]
async fn storage_classification_preserves_database_failure_distinctions() {
    let database = TestDatabase::new().await;
    for (code, constraint, expected) in [
        ("40001", "", StorageFailureKind::Conflict),
        ("40P01", "", StorageFailureKind::Conflict),
        ("22001", "", StorageFailureKind::InvalidInput),
        ("22007", "", StorageFailureKind::InvalidInput),
        ("22P02", "", StorageFailureKind::InvalidInput),
        ("23502", "", StorageFailureKind::InvalidInput),
        ("23503", "", StorageFailureKind::InvalidInput),
        ("23505", "", StorageFailureKind::InvalidInput),
        ("23514", "", StorageFailureKind::InvalidInput),
        (
            "23503",
            "channels_proxy_id_fkey",
            StorageFailureKind::RoutingDependency,
        ),
        ("22003", "", StorageFailureKind::Internal),
        ("08006", "", StorageFailureKind::Internal),
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DO $$ BEGIN RAISE EXCEPTION 'contract failure'
             USING ERRCODE='{code}', CONSTRAINT='{constraint}'; END $$"
        )))
        .execute(&database.pool)
        .await
        .unwrap_err();
        let RepositoryError::Storage(error) = RepositoryError::from(error) else {
            panic!("driver errors must not become business validation failures");
        };
        assert_eq!(error.kind(), expected, "{code}/{constraint}");
        assert!(std::error::Error::source(&error).is_some());
    }
    database.cleanup().await;
}

#[tokio::test]
async fn codex_operation_guards_preserve_locking_failure_commits_and_generation_checks() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let group = Uuid::new_v4();
    super::insert_routing_group_fixture(&database.pool, group, "guard-contract").await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = coordinator(repository.clone(), runtime);
    let created = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(group, "guard", "guard@example.test", "guard-user"),
            None,
        )
        .await
        .unwrap();
    let (before, guard) = repository
        .lock_codex_refresh(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(
            Duration::from_millis(100),
            repository.lock_codex_refresh(created.id),
        )
        .await
        .is_err()
    );
    drop(guard);
    let (_, guard) = timeout(
        Duration::from_secs(3),
        repository.lock_codex_refresh(created.id),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    guard
        .fail(true, "contract_failure", "refresh failed")
        .await
        .unwrap();
    let failed = repository
        .codex_credential(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(failed.reauth_required);
    assert_eq!(failed.last_error_code.as_deref(), Some("contract_failure"));
    assert_eq!(failed.refresh_generation, before.refresh_generation);
    assert_eq!(failed.access_token, before.access_token);

    let (_, guard) = repository
        .lock_codex_refresh(created.id)
        .await
        .unwrap()
        .unwrap();
    let update = CodexTokenRefreshUpdate {
        expected_generation: before.refresh_generation + 1,
        id_token: None,
        access_token: Some("must-not-commit".into()),
        refresh_token: Some("must-not-commit".into()),
        email: None,
        account_id: None,
        user_id: None,
        plan_type: None,
        is_fedramp: None,
        access_token_expires_at: None,
        refreshed_at: Utc::now(),
    };
    assert!(matches!(
        guard.complete(update).await,
        Err(RepositoryError::Conflict)
    ));
    let unchanged = repository
        .codex_credential(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.access_token, before.access_token);
    assert_eq!(unchanged.refresh_generation, before.refresh_generation);
    assert!(unchanged.reauth_required);

    let event_id = Uuid::new_v4();
    let (_, reset) = repository
        .lock_codex_quota_reset(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        reset
            .complete(
                seed.user,
                event_id,
                Utc::now(),
                CodexQuotaResetOutcome::Reset,
                3
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM codex_quota_reset_events WHERE id=$1")
            .bind(event_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let (_, reset) = timeout(
        Duration::from_secs(3),
        repository.lock_codex_quota_reset(created.id),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let correlation = reset
        .complete(
            seed.user,
            event_id,
            Utc::now(),
            CodexQuotaResetOutcome::Reset,
            1,
        )
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM codex_quota_reset_events AS event
         JOIN audit_logs AS audit ON audit.correlation_id=event.correlation_id::text
         WHERE event.id=$1 AND event.correlation_id=$2",
    )
    .bind(event_id)
    .bind(correlation)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    database.cleanup().await;
}
