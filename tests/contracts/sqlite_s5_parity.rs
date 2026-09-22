//! Shared Codex contracts; external calls are represented by opaque repository operation guards.

use super::*;
#[path = "sqlite_s5_faults.rs"]
mod faults;
use ai_gateway::persistence::{
    AuthRepository, CodexCredentialBatchInput, CodexCredentialBatchOperation,
    CodexCredentialBatchTarget, CodexCredentialExportInput, CodexCredentialUpdateInput,
    CodexOauthStartInput, CodexQuotaResetOutcome, CodexQuotaUpdate, CodexTokenRefreshUpdate,
    sqlite::SqliteDatabase,
};
use futures_util::FutureExt;
use rust_decimal::Decimal;
use std::{os::unix::fs::PermissionsExt, sync::Arc};

enum Backend {
    Pg(TestDatabase),
    Sq(tempfile::TempDir, Arc<SqliteDatabase>),
}
impl Backend {
    async fn exec(&self, sql: &str) {
        match self {
            Self::Pg(db) => {
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
                    .execute(&db.pool)
                    .await
                    .unwrap();
            }
            Self::Sq(_, db) => {
                let mut tx = db.begin_write().await.unwrap();
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace("now()", "ag_now()")))
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
        }
    }
    fn logs(&self) -> RequestLogRepository {
        match self {
            Self::Pg(db) => RequestLogRepository::new(db.pool.clone()),
            Self::Sq(_, db) => RequestLogRepository::from_sqlite(Arc::clone(db)),
        }
    }
    fn repo(&self) -> ControlPlaneRepository {
        match self {
            Self::Pg(d) => ControlPlaneRepository::new(d.pool.clone()),
            Self::Sq(_, d) => ControlPlaneRepository::from_sqlite(Arc::clone(d)),
        }
    }
    fn auth(&self) -> AuthRepository {
        match self {
            Self::Pg(d) => AuthRepository::new(d.pool.clone()),
            Self::Sq(_, d) => AuthRepository::from_sqlite(Arc::clone(d)),
        }
    }
    async fn finish(self) {
        match self {
            Self::Pg(d) => d.cleanup().await,
            Self::Sq(dir, d) => {
                d.close().await;
                drop(dir);
            }
        }
    }
}
struct Context {
    repo: ControlPlaneRepository,
    admin: Uuid,
    group: Uuid,
    id: Uuid,
    input: CodexCredentialCreate,
}
async fn setup(backend: &Backend) -> Context {
    let repo = backend.repo();
    repo.ensure_system_settings(system_settings())
        .await
        .unwrap();
    let admin = backend
        .auth()
        .bootstrap_admin("s5@example.test", "S5 admin", "$argon2id$fixture")
        .await
        .unwrap();
    let (group, _) = repo
        .prepare_mutation(
            admin,
            ControlPlaneMutation::SaveRoutingGroup {
                id: Uuid::new_v4(),
                expected: None,
                input: ai_gateway::persistence::RoutingGroupInput {
                    name: "Codex".into(),
                    sharing_only: false,
                    enabled: true,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let group = group[0].id;
    let input = business_codex_credential(group, "First", "first@example.test", "member-one");
    let mut pending = repo
        .prepare_codex_credential_create(admin, input.clone(), None)
        .await
        .unwrap();
    compile_runtime_config(pending.runtime_records().await.unwrap()).unwrap();
    let (created, _) = pending.commit().await.unwrap();
    Context {
        repo,
        admin,
        group,
        id: created[0].id,
        input,
    }
}
async fn run(case: u8) {
    let pg = Backend::Pg(TestDatabase::new().await);
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
    let mut failures = Vec::new();
    for (name, backend) in [("postgres", pg), ("sqlite", Backend::Sq(dir, db))] {
        let result = std::panic::AssertUnwindSafe(async {
            let c = setup(&backend).await;
            match case {
                0 => crud(c).await,
                1 => oauth(c).await,
                2 => quota(c).await,
                3 => refresh(c).await,
                4 => sharing(c).await,
                5 => recovery_and_costs(&backend, c).await,
                6 => window_edges(c).await,
                7 => identity_and_proxy_export(&backend, c).await,
                _ => unreachable!(),
            }
        })
        .catch_unwind()
        .await;
        backend.finish().await;
        if let Err(e) = result {
            failures.push(format!(
                "{name}: {:?}",
                e.downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| e.downcast_ref::<&str>().copied())
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
#[tokio::test]
async fn codex_crud_projection_export_and_batches_match() {
    run(0).await;
}
#[tokio::test]
async fn oauth_flow_ownership_and_one_shot_commit_match() {
    run(1).await;
}
#[tokio::test]
async fn quota_window_state_machine_and_reset_audit_match() {
    run(2).await;
}
#[tokio::test]
async fn refresh_generation_and_confirmed_reset_match() {
    run(3).await;
}
#[tokio::test]
async fn sharing_membership_ledger_and_alias_protection_match() {
    run(4).await;
}
#[tokio::test]
async fn sharing_wal_recovery_facts_and_visible_projection_costs_match() {
    run(5).await;
}
#[tokio::test]
async fn sliding_zero_official_and_natural_windows_match() {
    run(6).await;
}
#[tokio::test]
async fn legacy_identity_proxy_exports_and_batch_rollback_match() {
    run(7).await;
}

async fn crud(c: Context) {
    let r = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    let identity = c
        .repo
        .upstream_credential_detail(c.id)
        .await
        .unwrap()
        .unwrap();
    assert!(identity.credential.provider_managed);
    assert_eq!(identity.credential.kind, "codex_oauth");
    assert!(identity.secret.is_none());
    assert_eq!(identity.credential.channel_ids, vec![c.id]);
    assert!(
        c.repo
            .prepare_mutation(
                c.admin,
                ai_gateway::persistence::ControlPlaneMutation::DeleteUpstreamCredential {
                    id: c.id,
                    expected_updated_at: identity.credential.updated_at,
                }
            )
            .await
            .is_err()
    );
    assert!(r.projection_channel_ids.is_empty());
    assert_eq!(r.refresh_generation, 0);
    assert_eq!(r.user_id.as_deref(), Some("member-one"));
    let records = c.repo.load_runtime().await.unwrap();
    let topology = c.repo.topology().await.unwrap();
    let expected_ids = topology
        .channel_capabilities
        .iter()
        .filter(|capability| capability.channel_id == c.id && capability.deleted_at.is_none())
        .map(|capability| capability.id)
        .collect::<Vec<_>>();
    assert_eq!(expected_ids.len(), 4);
    let runtime_channels = records
        .control_plane
        .channels
        .iter()
        .filter(|channel| expected_ids.contains(&channel.id))
        .collect::<Vec<_>>();
    assert_eq!(runtime_channels.len(), expected_ids.len());
    for channel in runtime_channels {
        assert_eq!(channel.credential.unwrap().id, c.id);
    }
    let images = topology
        .channel_capabilities
        .iter()
        .find(|capability| capability.settings.operation == ApiOperation::ImagesGeneration)
        .unwrap();
    assert!(!images.settings.enabled);
    assert_eq!(c.repo.codex_credentials(c.group).await.unwrap().len(), 1);
    let export = c
        .repo
        .export_codex_credentials(
            c.group,
            CodexCredentialExportInput {
                credential_ids: vec![c.id],
                include_proxies: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(export.credentials[0].refresh_token, "First-refresh-token");
    assert_eq!(export.credentials[0].user_id.as_deref(), Some("member-one"));
    assert!(
        c.repo
            .export_codex_credentials(
                c.group,
                CodexCredentialExportInput {
                    credential_ids: vec![c.id, c.id],
                    include_proxies: true
                }
            )
            .await
            .is_err()
    );
    assert!(
        c.repo
            .export_codex_credentials(
                c.group,
                CodexCredentialExportInput {
                    credential_ids: vec![Uuid::nil()],
                    include_proxies: true
                }
            )
            .await
            .is_err()
    );
    let (reimport, _) = c
        .repo
        .prepare_codex_credential_create(c.admin, c.input.clone(), None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(reimport[0].id, c.id);
    assert_eq!(
        c.repo
            .codex_credential(c.id)
            .await
            .unwrap()
            .unwrap()
            .refresh_generation,
        1
    );
    let input = CodexCredentialUpdateInput {
        label: "Renamed".into(),
        enabled: false,
        quota_threshold_percent: 80,
        proxy_id: None,
    };
    assert!(
        c.repo
            .prepare_codex_credential_update(c.admin, c.id, input.clone(), r.updated_at)
            .await
            .is_err()
    );
    let current = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    c.repo
        .prepare_codex_credential_update(c.admin, c.id, input, current.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(
        c.repo
            .codex_credential_view(c.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_status,
        "disabled"
    );
    let current = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    let identity = c
        .repo
        .upstream_credential_detail(c.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!identity.credential.enabled);
    assert_eq!(identity.credential.name, "Renamed");
    c.repo
        .prepare_codex_credentials_batch(
            c.admin,
            c.group,
            CodexCredentialBatchInput {
                operation: CodexCredentialBatchOperation::Enable,
                items: vec![CodexCredentialBatchTarget {
                    id: c.id,
                    updated_at: current.updated_at,
                }],
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let current = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    assert!(current.enabled);
    assert!(
        c.repo
            .upstream_credential_detail(c.id)
            .await
            .unwrap()
            .unwrap()
            .credential
            .enabled
    );
    assert_eq!(current.label, "Renamed");
    c.repo
        .prepare_codex_credential_delete(c.admin, c.id, current.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(c.repo.codex_credential(c.id).await.unwrap().is_none());
    assert!(
        c.repo
            .upstream_credential_detail(c.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(c.repo.load_codex_credentials().await.unwrap().is_empty());
    let listed = c.repo.topology().await.unwrap();
    assert!(
        listed
            .logical_channels
            .iter()
            .all(|channel| channel.id != c.id || channel.deleted_at.is_some())
    );
}

async fn oauth(c: Context) {
    let flow = c
        .repo
        .create_codex_oauth_flow(
            c.admin,
            c.group,
            CodexOauthStartInput {
                label: "OAuth".into(),
                proxy_id: None,
                quota_threshold_percent: 90,
            },
            "http://localhost:1455/auth/callback".into(),
            vec![42; 32],
            "v".repeat(64),
            Utc::now() + chrono::Duration::minutes(10),
        )
        .await
        .unwrap();
    assert!(
        c.repo
            .codex_oauth_flow(flow.id, Uuid::nil())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.repo
            .codex_oauth_flow(flow.id, c.admin)
            .await
            .unwrap()
            .unwrap()
            .state_hash,
        vec![42; 32]
    );
    let input = business_codex_credential(c.group, "OAuth", "second@example.test", "member-two");
    let change = c
        .repo
        .prepare_codex_credential_create(c.admin, input.clone(), Some(flow.id))
        .await
        .unwrap();
    drop(change);
    assert!(
        c.repo
            .codex_oauth_flow(flow.id, c.admin)
            .await
            .unwrap()
            .is_some()
    );
    c.repo
        .prepare_codex_credential_create(c.admin, input.clone(), Some(flow.id))
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        c.repo
            .codex_oauth_flow(flow.id, c.admin)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        c.repo
            .prepare_codex_credential_create(c.admin, input, Some(flow.id))
            .await
            .is_err()
    );
    assert_eq!(c.repo.cleanup_codex_oauth_flows().await.unwrap(), 1);
    assert_eq!(c.repo.load_codex_credentials().await.unwrap().len(), 2);
}
fn observation(now: DateTime<Utc>, used: i32) -> CodexQuotaUpdate {
    CodexQuotaUpdate {
        allowed: true,
        limit_reached: false,
        primary_used_percent: Some(used),
        primary_window_seconds: Some(18000),
        primary_reset_at: Some(now + chrono::Duration::hours(4)),
        secondary_used_percent: Some(used),
        secondary_window_seconds: Some(604800),
        secondary_reset_at: Some(now + chrono::Duration::days(6)),
        reset_credits_available: Some(3),
        checked_at: now,
    }
}
async fn quota(c: Context) {
    let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
    c.repo
        .persist_codex_quota(c.id, observation(now, 20))
        .await
        .unwrap();
    let first = c.repo.codex_quota_window_history(c.id, 10).await.unwrap();
    assert_eq!(first.periods.len(), 2);
    assert!(first.periods.iter().all(|p| p.cost_amount == Decimal::ZERO));
    let mut stale = observation(now - chrono::Duration::seconds(1), 80);
    c.repo
        .persist_codex_quota(c.id, stale.clone())
        .await
        .unwrap();
    assert_eq!(
        c.repo
            .codex_credential(c.id)
            .await
            .unwrap()
            .unwrap()
            .primary_used_percent,
        Some(20)
    );
    stale.checked_at = now + chrono::Duration::seconds(1);
    stale.primary_used_percent = Some(96);
    c.repo.persist_codex_quota(c.id, stale).await.unwrap();
    assert_eq!(
        c.repo
            .codex_credential_view(c.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_status,
        "draining"
    );
    let changed = c.repo.codex_quota_window_history(c.id, 1).await.unwrap();
    assert_eq!(changed.periods.len(), 2);
    assert_eq!(changed.periods[0].id, first.periods[0].id);
    assert!(c.repo.codex_quota_window_history(c.id, 0).await.is_err());
    assert!(
        c.repo
            .self_codex_quota_window_history(c.admin, c.id, 10)
            .await
            .is_err()
    );
    assert!(
        c.repo
            .self_codex_quota_credentials(c.admin)
            .await
            .unwrap()
            .is_empty()
    );
    c.repo
        .record_codex_quota_reset(
            c.admin,
            c.id,
            Uuid::new_v4(),
            now,
            CodexQuotaResetOutcome::Reset,
            2,
        )
        .await
        .unwrap();
    let mut reset = observation(now + chrono::Duration::seconds(2), 0);
    reset.primary_reset_at = Some(now + chrono::Duration::hours(5));
    reset.secondary_reset_at = Some(now + chrono::Duration::days(7));
    c.repo.persist_codex_quota(c.id, reset).await.unwrap();
    let periods = c
        .repo
        .codex_quota_window_history(c.id, 10)
        .await
        .unwrap()
        .periods;
    assert_eq!(periods.len(), 4);
    assert_eq!(
        periods
            .iter()
            .filter(|p| p.reset_reason.as_deref() == Some("manual"))
            .count(),
        2
    );
    let audit = c.repo.audit_logs(100).await.unwrap();
    assert_eq!(
        audit.iter().filter(|a| a.action == "reset_quota").count(),
        1
    );
    c.repo
        .mark_codex_credential_error(c.id, true, "invalid_grant", "safe summary")
        .await
        .unwrap();
    assert!(
        c.repo
            .codex_credential(c.id)
            .await
            .unwrap()
            .unwrap()
            .reauth_required
    );
}
fn token_update(generation: i64) -> CodexTokenRefreshUpdate {
    CodexTokenRefreshUpdate {
        expected_generation: generation,
        id_token: None,
        access_token: Some("refreshed-access".into()),
        refresh_token: Some("refreshed-refresh".into()),
        email: None,
        account_id: None,
        user_id: None,
        plan_type: None,
        is_fedramp: None,
        access_token_expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        refreshed_at: Utc::now(),
    }
}
async fn refresh(c: Context) {
    let (record, mut guard) = c.repo.lock_codex_refresh(c.id).await.unwrap().unwrap();
    guard.prepare_dispatch().await.unwrap();
    guard
        .complete(token_update(record.refresh_generation))
        .await
        .unwrap();
    let record = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    assert_eq!(record.refresh_generation, 1);
    assert_eq!(record.refresh_token, "refreshed-refresh");
    c.repo
        .lock_codex_refresh(c.id)
        .await
        .unwrap()
        .unwrap()
        .1
        .unchanged()
        .await
        .unwrap();
    let (_, mut guard) = c.repo.lock_codex_quota_reset(c.id).await.unwrap().unwrap();
    guard.prepare_dispatch().await.unwrap();
    guard
        .complete(
            c.admin,
            Uuid::new_v4(),
            Utc::now(),
            CodexQuotaResetOutcome::NothingToReset,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        c.repo
            .audit_logs(100)
            .await
            .unwrap()
            .iter()
            .filter(|a| a.action == "reset_quota")
            .count(),
        1
    );
}

async fn sharing(c: Context) {
    use ai_gateway::domain::codex_sharing::SharingGroupInput;
    let ledger = Uuid::new_v4();
    let mut owner = c.repo.claim_sharing_ledger(ledger).await.unwrap();
    owner.ping().await.unwrap();
    assert!(c.repo.claim_sharing_ledger(ledger).await.is_err());
    owner.close().await.unwrap();
    assert!(c.repo.claim_sharing_ledger(Uuid::new_v4()).await.is_err());
    let owner = c.repo.claim_sharing_ledger(ledger).await.unwrap();
    let input = SharingGroupInput {
        credential_id: c.id,
        name: "Car".into(),
        enabled: true,
        seats: vec![Some(c.admin), None],
        primary_limit_amount: Decimal::from(5),
        secondary_limit_amount: Decimal::from(10),
        request_reservation_amount: Decimal::new(1, 2),
        user_requests_per_minute: 10,
        group_requests_per_minute: 20,
        user_max_concurrent_requests: 2,
        group_max_concurrent_requests: 4,
    };
    let saved = c
        .repo
        .prepare_mutation(
            c.admin,
            ControlPlaneMutation::SaveCodexSharing {
                id: Uuid::new_v4(),
                input,
                expected_updated_at: None,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0;
    let groups = c.repo.sharing_groups(Some(c.admin)).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, saved[0].id);
    assert_eq!(
        groups[0].policy.request_reservation_amount,
        Decimal::new(1, 2)
    );
    assert!(
        c.repo
            .sharing_groups(Some(Uuid::nil()))
            .await
            .unwrap()
            .is_empty()
    );
    let r = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    assert!(
        c.repo
            .prepare_codex_credential_delete(c.admin, c.id, r.updated_at)
            .await
            .is_err()
    );
    let snapshot = c.repo.load_runtime().await.unwrap();
    let topology = c.repo.topology().await.unwrap();
    let mut capabilities = topology
        .channel_capabilities
        .iter()
        .filter(|capability| capability.channel_id == c.id)
        .map(|capability| capability.id)
        .collect::<Vec<_>>();
    capabilities.sort_unstable();
    assert_eq!(capabilities.len(), 4);
    let mut channels = snapshot.sharing[0].channel_ids.clone();
    channels.sort_unstable();
    assert_eq!(channels, capabilities);
    let mut protected = snapshot.sharing[0].protected_channel_ids.clone();
    protected.sort_unstable();
    assert_eq!(protected, capabilities);
    owner.close().await.unwrap();
}

async fn reopen_sharing(path: &std::path::Path) -> ai_gateway::codex_sharing::SharingRuntime {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ai_gateway::codex_sharing::SharingRuntime::open(path.to_path_buf()).await {
                Ok(runtime) => break runtime,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap()
}

async fn window_edges(c: Context) {
    let now = Utc::now() - chrono::Duration::hours(10);
    let mut update = observation(now, 0);
    update.secondary_used_percent = None;
    update.secondary_window_seconds = None;
    update.secondary_reset_at = None;
    c.repo
        .persist_codex_quota(c.id, update.clone())
        .await
        .unwrap();
    let original = c
        .repo
        .codex_quota_window_history(c.id, 10)
        .await
        .unwrap()
        .periods[0]
        .id;
    update.checked_at += chrono::Duration::minutes(10);
    update.primary_reset_at = update
        .primary_reset_at
        .map(|t| t + chrono::Duration::minutes(10));
    c.repo
        .persist_codex_quota(c.id, update.clone())
        .await
        .unwrap();
    let periods = c
        .repo
        .codex_quota_window_history(c.id, 10)
        .await
        .unwrap()
        .periods;
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].id, original);
    update.checked_at += chrono::Duration::seconds(1);
    update.primary_used_percent = Some(20);
    c.repo
        .persist_codex_quota(c.id, update.clone())
        .await
        .unwrap();
    update.checked_at += chrono::Duration::minutes(10);
    update.primary_reset_at = update
        .primary_reset_at
        .map(|t| t + chrono::Duration::minutes(10));
    c.repo
        .persist_codex_quota(c.id, update.clone())
        .await
        .unwrap();
    let periods = c
        .repo
        .codex_quota_window_history(c.id, 10)
        .await
        .unwrap()
        .periods;
    assert_eq!(periods.len(), 2);
    assert_eq!(periods[1].reset_reason.as_deref(), Some("openai_official"));
    update.checked_at += chrono::Duration::hours(5);
    update.primary_reset_at = update
        .primary_reset_at
        .map(|t| t + chrono::Duration::hours(5));
    c.repo.persist_codex_quota(c.id, update).await.unwrap();
    let periods = c
        .repo
        .codex_quota_window_history(c.id, 10)
        .await
        .unwrap()
        .periods;
    assert_eq!(periods.len(), 3);
    assert_eq!(periods[1].reset_reason.as_deref(), Some("natural"));
}

async fn identity_and_proxy_export(backend: &Backend, c: Context) {
    let proxy = Uuid::new_v4();
    backend.exec(&format!("INSERT INTO proxies(id,name,proxy_url,username,password)
        VALUES ('{proxy}','Export proxy','http://proxy.example.test','fixture-user','fixture-password');
        UPDATE codex_oauth_credentials SET user_id=NULL,updated_at=now() WHERE channel_id='{}';",c.id)).await;
    let mut input = c.input.clone();
    input.email = Some("FIRST@EXAMPLE.TEST".into());
    input.proxy_id = Some(proxy);
    let created = c
        .repo
        .prepare_codex_credential_create(c.admin, input, None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0;
    assert_eq!(created[0].id, c.id);
    assert_eq!(
        c.repo
            .codex_credential(c.id)
            .await
            .unwrap()
            .unwrap()
            .user_id
            .as_deref(),
        Some("member-one")
    );
    for include in [false, true] {
        let bundle = c
            .repo
            .export_codex_credentials(
                c.group,
                CodexCredentialExportInput {
                    credential_ids: vec![c.id],
                    include_proxies: include,
                },
            )
            .await
            .unwrap();
        assert_eq!(bundle.proxies.len(), usize::from(include));
        if include {
            assert_eq!(
                bundle.proxies[0].password.as_deref(),
                Some("fixture-password")
            );
        }
    }
    let before = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    assert!(
        c.repo
            .prepare_codex_credentials_batch(
                c.admin,
                c.group,
                CodexCredentialBatchInput {
                    operation: CodexCredentialBatchOperation::Disable,
                    items: vec![
                        CodexCredentialBatchTarget {
                            id: c.id,
                            updated_at: before.updated_at
                        },
                        CodexCredentialBatchTarget {
                            id: Uuid::nil(),
                            updated_at: before.updated_at
                        },
                    ],
                }
            )
            .await
            .is_err()
    );
    let after = c.repo.codex_credential(c.id).await.unwrap().unwrap();
    assert!(after.enabled);
    assert_eq!(after.updated_at, before.updated_at);
    let audit = serde_json::to_string(&c.repo.audit_logs(100).await.unwrap()).unwrap();
    assert!(!audit.contains("First-refresh-token"));
    assert!(!audit.contains("fixture-password"));
    let proxy_view = c
        .repo
        .control_plane_lists()
        .await
        .unwrap()
        .proxies
        .into_iter()
        .find(|item| item.id == proxy)
        .unwrap();
    assert!(matches!(
        c.repo
            .prepare_mutation(
                c.admin,
                ai_gateway::persistence::ControlPlaneMutation::DeleteProxy {
                    id: proxy,
                    expected_updated_at: proxy_view.updated_at,
                }
            )
            .await,
        Err(ai_gateway::persistence::RepositoryError::ProxyInUse)
    ));
    let topology = c.repo.topology().await.unwrap();
    let access_id = topology
        .logical_channels
        .iter()
        .find(|channel| channel.id == c.id)
        .unwrap()
        .access_id;
    c.repo
        .prepare_codex_credential_delete(c.admin, c.id, after.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let topology = c.repo.topology().await.unwrap();
    let access = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == access_id)
        .unwrap();
    assert!(access.deleted_at.is_some());
    assert_eq!(access.proxy_id, None);
    c.repo
        .prepare_mutation(
            c.admin,
            ai_gateway::persistence::ControlPlaneMutation::DeleteProxy {
                id: proxy,
                expected_updated_at: proxy_view.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
}

async fn recovery_and_costs(backend: &Backend, c: Context) {
    use ai_gateway::domain::codex_sharing::SharingGroupInput;
    let key = Uuid::new_v4();
    let model = Uuid::new_v4();
    let (formats, permissions) = match backend {
        Backend::Pg(_) => (
            "'{open_ai_responses,open_ai_images}'",
            "'{proxy,models.read}'",
        ),
        Backend::Sq(..) => (
            "'[\"open_ai_responses\",\"open_ai_images\"]'",
            "'[\"proxy\",\"models.read\"]'",
        ),
    };
    backend.exec(&format!(
        "INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
         VALUES ('{key}','{}','Key','s5-fixture-key','active',{formats},{permissions});
         INSERT INTO models(id,source_model_id,display_name,enabled,currency,price_unit_tokens,
          input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
         VALUES ('{model}','s5-model','Model',true,'USD',1000000,'1','0','0','2',now());
         INSERT INTO user_group_codex_quota_visibility(user_group_id,channel_group_id)
         SELECT user_group_id,'{}' FROM users WHERE id='{}';",c.admin,c.group,c.admin)).await;
    let now = Utc::now() - chrono::Duration::seconds(5);
    c.repo
        .persist_codex_quota(c.id, observation(now, 10))
        .await
        .unwrap();
    assert_eq!(
        c.repo
            .self_codex_quota_credentials(c.admin)
            .await
            .unwrap()
            .len(),
        1
    );
    c.repo
        .prepare_mutation(
            c.admin,
            ControlPlaneMutation::SaveCodexSharing {
                id: Uuid::new_v4(),
                expected_updated_at: None,
                input: SharingGroupInput {
                    credential_id: c.id,
                    name: "Durable car".into(),
                    enabled: true,
                    seats: vec![Some(c.admin), None],
                    primary_limit_amount: Decimal::from(5),
                    secondary_limit_amount: Decimal::from(10),
                    request_reservation_amount: Decimal::new(1, 2),
                    user_requests_per_minute: 100,
                    group_requests_per_minute: 100,
                    user_max_concurrent_requests: 10,
                    group_max_concurrent_requests: 10,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let record = c.repo.load_runtime().await.unwrap().sharing.remove(0);
    assert_eq!(record.windows.len(), 2);
    let group = record.group;
    let dir = tempfile::tempdir().unwrap();
    let runtime = reopen_sharing(dir.path()).await;
    let ledger = runtime.ledger_id().unwrap();
    let owner = c.repo.claim_sharing_ledger(ledger).await.unwrap();
    runtime
        .sync(vec![group.clone()], record.windows.clone())
        .await
        .unwrap();
    let topology = c.repo.topology().await.unwrap();
    assert_eq!(record.channel_ids.len(), 4);
    let metered_capabilities = topology
        .channel_capabilities
        .iter()
        .filter(|capability| {
            record.channel_ids.contains(&capability.id)
                && capability.settings.operation != ApiOperation::StandaloneWebSearch
        })
        .collect::<Vec<_>>();
    assert_eq!(metered_capabilities.len(), 3);
    let expected_amount = Decimal::new(6, 8);
    let mut events = Vec::new();
    let mut leases = Vec::new();
    for (index, capability) in metered_capabilities.iter().enumerate() {
        let mut event = request_log_event(c.admin, key, model, c.group, capability.id);
        event.channel_group_id = Some(
            backend
                .repo()
                .load_runtime()
                .await
                .unwrap()
                .control_plane
                .channels
                .iter()
                .find(|c| c.id == capability.id)
                .unwrap()
                .channel_group_id,
        );
        event.api_operation = capability.settings.operation;
        event.api_format = event.api_operation.api_format();
        event.request_protocol = RequestProtocol::NonStream;
        event.streamed = false;
        event.billing.as_mut().unwrap().cost_amount = Some(Decimal::new((index + 1) as i64, 8));
        let lease = runtime.reserve(&group, c.admin, event.id).await.unwrap();
        leases.push(lease);
        events.push(event);
    }
    for lease in leases {
        lease.settle(None);
    }
    runtime.flush().await.unwrap();
    assert!(runtime.inspect(&group, c.admin).await.uncertain);
    drop(runtime);
    owner.close().await.unwrap();
    let runtime = reopen_sharing(dir.path()).await;
    assert_eq!(runtime.ledger_id(), Some(ledger));
    let owner = c.repo.claim_sharing_ledger(ledger).await.unwrap();
    runtime
        .sync(vec![group.clone()], record.windows.clone())
        .await
        .unwrap();
    assert!(runtime.inspect(&group, c.admin).await.uncertain);
    let logs = backend.logs();
    logs.metering().record_batch(&events).await.unwrap();
    let costs = logs
        .queries()
        .metering()
        .sharing_completed_costs(&runtime.pending().await)
        .await
        .unwrap();
    assert_eq!(costs.len(), 3);
    for (id, cost) in &costs {
        runtime.finish(*id, Some(*cost));
        runtime.finish(*id, Some(*cost));
    }
    runtime.flush().await.unwrap();
    let usage = runtime.inspect(&group, c.admin).await;
    assert!(!usage.uncertain);
    assert_eq!(usage.pending_requests, 0);
    assert!(
        usage
            .windows
            .iter()
            .all(|w| w.used_amount == expected_amount)
    );
    let view = c.repo.codex_credential_view(c.id).await.unwrap().unwrap();
    assert_eq!(view.primary_window_cost_amount, Some(expected_amount));
    assert_eq!(view.secondary_window_cost_amount, Some(expected_amount));
    let history = c
        .repo
        .self_codex_quota_window_history(c.admin, c.id, 10)
        .await
        .unwrap();
    assert!(
        history
            .periods
            .iter()
            .all(|p| p.cost_amount == expected_amount)
    );
    drop(runtime);
    owner.close().await.unwrap();
    let runtime = reopen_sharing(dir.path()).await;
    runtime
        .sync(vec![group.clone()], record.windows)
        .await
        .unwrap();
    assert!(
        runtime
            .inspect(&group, c.admin)
            .await
            .windows
            .iter()
            .all(|w| w.used_amount == expected_amount)
    );
    drop(runtime);
}
