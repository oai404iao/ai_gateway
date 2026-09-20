//! SQLite-only cancellation, fencing, atomic result and ownership contracts.

use super::*;
use ai_gateway::persistence::RepositoryError;
use std::time::Duration;

async fn database() -> Backend {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let db = Arc::new(
        SqliteDatabase::open(&dir.path().join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    db.install_schema().await.unwrap();
    Backend::Sq(dir, db)
}

fn sqlite(backend: &Backend) -> Arc<SqliteDatabase> {
    match backend {
        Backend::Sq(_, db) => Arc::clone(db),
        _ => unreachable!(),
    }
}

async fn exec(db: &SqliteDatabase, sql: &str) {
    let mut tx = db.begin_write().await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn pending(db: &SqliteDatabase) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM _gateway_codex_operations")
        .fetch_one(&mut *db.acquire_read().await.unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn sharing_lease_ping_ignores_pool_contention_but_rejects_identity_loss() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("gateway.sqlite");
    let db = Arc::new(
        SqliteDatabase::open_with_limits(&path, 2, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    db.install_schema().await.unwrap();
    let repository = ControlPlaneRepository::from_sqlite(Arc::clone(&db));
    let mut lease = repository
        .claim_sharing_ledger(Uuid::new_v4())
        .await
        .unwrap();
    let reader = db.acquire_read().await.unwrap();
    let writer = db.begin_write().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), lease.ping())
        .await
        .expect("ownership heartbeat must not wait for a database connection")
        .unwrap();
    drop(reader);
    writer.rollback().await.unwrap();
    lease.ping().await.unwrap();

    std::fs::write(
        dir.path().join("gateway.sqlite.identity"),
        Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(lease.ping().await.is_err());
    drop(lease);
    db.close().await;
}

#[tokio::test]
async fn preflight_drop_stale_version_and_dispatch_fencing() {
    let backend = database().await;
    let db = sqlite(&backend);
    let c = setup(&backend).await;
    let (_, guard) = c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap();
    assert_eq!(pending(&db).await, 0);
    drop(guard);
    let (record, mut stale) = c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap();
    c.repo
        .prepare_codex_credential_update(
            c.admin,
            c.id,
            CodexCredentialUpdateInput {
                label: "Changed".into(),
                enabled: true,
                quota_threshold_percent: 90,
                proxy_id: None,
            },
            record.updated_at,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        stale.prepare_dispatch().await,
        Err(RepositoryError::Conflict)
    ));
    drop(stale);
    assert_eq!(pending(&db).await, 0);

    let (record, mut guard) = c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap();
    guard.prepare_dispatch().await.unwrap();
    assert_eq!(pending(&db).await, 1);
    assert!(guard.prepare_dispatch().await.is_err());
    assert!(
        c.repo
            .prepare_codex_credential_create(c.admin, c.input.clone(), None)
            .await
            .is_err()
    );
    assert!(
        c.repo
            .prepare_codex_credential_update(
                c.admin,
                c.id,
                CodexCredentialUpdateInput {
                    label: "Blocked".into(),
                    enabled: false,
                    quota_threshold_percent: 90,
                    proxy_id: None,
                },
                record.updated_at
            )
            .await
            .is_err()
    );
    // A different credential still uses the sole writer while the provider operation is live.
    tokio::time::timeout(
        Duration::from_secs(2),
        c.repo.prepare_codex_credential_create(
            c.admin,
            business_codex_credential(c.group, "Other", "other@example.test", "other-member"),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .commit()
    .await
    .unwrap();
    let next_repo = c.repo.clone();
    let id = c.id;
    let mut waiting = tokio::spawn(async move {
        let (record, guard) = next_repo.lock_codex_refresh(id).await.unwrap().unwrap();
        guard.unchanged().await.unwrap();
        record.refresh_generation
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    guard
        .complete(token_update(record.refresh_generation))
        .await
        .unwrap();
    assert_eq!(waiting.await.unwrap(), 1);
    assert_eq!(pending(&db).await, 0);
    backend.finish().await;
}

#[tokio::test]
async fn cancelled_dispatch_survives_restart_until_explicit_reauthorization() {
    let backend = database().await;
    let db = sqlite(&backend);
    let c = setup(&backend).await;
    let repo = c.repo.clone();
    let id = c.id;
    let (sent, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (_, mut guard) = repo.lock_codex_refresh(id).await.unwrap().unwrap();
        guard.prepare_dispatch().await.unwrap();
        sent.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(guard);
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(pending(&db).await, 1);
    assert!(c.repo.lock_codex_refresh(id).await.is_err());
    let Backend::Sq(dir, old) = backend else {
        unreachable!()
    };
    old.close().await;
    let reopened = Arc::new(
        SqliteDatabase::open(&dir.path().join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    reopened.install_schema().await.unwrap();
    let repo = ControlPlaneRepository::from_sqlite(Arc::clone(&reopened));
    assert!(repo.lock_codex_refresh(id).await.is_err());
    let change = repo
        .prepare_codex_credential_create(c.admin, c.input.clone(), None)
        .await
        .unwrap();
    drop(change);
    assert_eq!(pending(&reopened).await, 1);
    repo.prepare_codex_credential_create(c.admin, c.input, None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(pending(&reopened).await, 0);
    let (record, guard) = repo.lock_codex_refresh(id).await.unwrap().unwrap();
    assert_eq!(record.refresh_generation, 1);
    guard.unchanged().await.unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn result_failures_preserve_intent_and_never_retry_reset() {
    let backend = database().await;
    let db = sqlite(&backend);
    let c = setup(&backend).await;
    let (_, mut guard) = c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap();
    guard.prepare_dispatch().await.unwrap();
    guard
        .fail(false, "upstream_error", "safe summary")
        .await
        .unwrap();
    assert_eq!(pending(&db).await, 1);
    assert!(c.repo.lock_codex_refresh(c.id).await.is_err());
    c.repo
        .prepare_codex_credential_create(c.admin, c.input.clone(), None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();

    exec(
        &db,
        "CREATE TRIGGER fail_reset_audit BEFORE INSERT ON audit_logs
        WHEN NEW.action='reset_quota' BEGIN SELECT RAISE(ABORT,'test_audit_failure'); END",
    )
    .await;
    let (_, mut guard) = c.repo.lock_codex_quota_reset(c.id).await.unwrap().unwrap();
    guard.prepare_dispatch().await.unwrap();
    assert!(
        guard
            .complete(
                c.admin,
                Uuid::new_v4(),
                Utc::now(),
                CodexQuotaResetOutcome::Reset,
                2
            )
            .await
            .is_err()
    );
    assert_eq!(pending(&db).await, 1);
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM codex_quota_reset_events")
        .fetch_one(&mut *db.acquire_read().await.unwrap())
        .await
        .unwrap();
    assert_eq!(events, 0);
    assert!(c.repo.lock_codex_quota_reset(c.id).await.is_err());
    assert!(
        c.repo
            .prepare_codex_credential_create(c.admin, c.input, None)
            .await
            .is_err()
    );
    backend.finish().await;
}

#[tokio::test]
async fn operation_and_ledger_guards_keep_database_ownership_until_drop() {
    for sharing in [false, true] {
        let backend = database().await;
        let db = sqlite(&backend);
        let c = setup(&backend).await;
        let operation = if sharing {
            None
        } else {
            Some(c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap().1)
        };
        let mut lease = if sharing {
            Some(c.repo.claim_sharing_ledger(Uuid::new_v4()).await.unwrap())
        } else {
            None
        };
        let closing = Arc::clone(&db);
        let mut close = tokio::spawn(async move {
            closing.close().await;
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut close)
                .await
                .is_err()
        );
        let Backend::Sq(dir, _) = &backend else {
            unreachable!()
        };
        assert!(
            SqliteDatabase::open(&dir.path().join("gateway.sqlite"))
                .await
                .is_err()
        );
        if let Some(lease) = lease.as_mut() {
            assert!(lease.ping().await.is_err());
        }
        drop(operation);
        drop(lease);
        tokio::time::timeout(Duration::from_secs(2), close)
            .await
            .unwrap()
            .unwrap();
        let reopened = SqliteDatabase::open(&dir.path().join("gateway.sqlite"))
            .await
            .unwrap();
        reopened.close().await;
        backend.finish().await;
    }
}

#[tokio::test]
async fn cancellation_inside_intent_and_result_transactions_is_atomic() {
    for result_stage in [false, true] {
        let backend = database().await;
        let db = sqlite(&backend);
        let c = setup(&backend).await;
        let table = if result_stage {
            "codex_oauth_credentials"
        } else {
            "_gateway_codex_operations"
        };
        let (started, ready) = tokio::sync::oneshot::channel();
        let (resume, paused) = std::sync::mpsc::sync_channel(1);
        let mut started = Some(started);
        let mut tx = db.begin_write().await.unwrap();
        tx.lock_handle()
            .await
            .unwrap()
            .set_update_hook(move |update| {
                if update.table == table
                    && let Some(started) = started.take()
                {
                    started.send(()).unwrap();
                    paused.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            });
        tx.rollback().await.unwrap();
        let repo = c.repo.clone();
        let id = c.id;
        let task = tokio::spawn(async move {
            let (record, mut guard) = repo.lock_codex_refresh(id).await.unwrap().unwrap();
            guard.prepare_dispatch().await.unwrap();
            if result_stage {
                guard
                    .complete(token_update(record.refresh_generation))
                    .await
                    .unwrap();
            }
        });
        tokio::time::timeout(Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        resume.send(()).unwrap();
        db.begin_write().await.unwrap().rollback().await.unwrap();
        assert_eq!(pending(&db).await, i64::from(result_stage));
        let record = c.repo.codex_credential(c.id).await.unwrap().unwrap();
        assert_eq!(record.refresh_generation, 0);
        assert_eq!(record.refresh_token, "First-refresh-token");
        if result_stage {
            assert!(c.repo.lock_codex_refresh(c.id).await.is_err());
        } else {
            c.repo
                .lock_codex_refresh(c.id)
                .await
                .unwrap()
                .unwrap()
                .1
                .unchanged()
                .await
                .unwrap();
        }
        backend.finish().await;
    }
}
