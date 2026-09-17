//! Opt-in, synthetic-only schema-62 backup, cutover, and paired recovery rehearsal.

use super::*;
use ai_gateway::{
    application::RequestLogIntent,
    codex_sharing::SharingRuntime,
    domain::codex_sharing::{SharingGroup, SharingGroupInput},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

fn tree_manifest(directory: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "rehearsal evidence must not follow symlinks"
            );
            if kind.is_dir() {
                visit(root, &entry.path(), files);
            } else {
                assert!(kind.is_file());
                let mut file = File::open(entry.path()).unwrap();
                let mut digest = Sha256::new();
                let mut bytes = [0; 65_536];
                loop {
                    let read = file.read(&mut bytes).unwrap();
                    if read == 0 {
                        break;
                    }
                    digest.update(&bytes[..read]);
                }
                files.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .into(),
                    digest
                        .finalize()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(directory, directory, &mut files);
    files
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for name in tree_manifest(source).keys() {
        let destination = destination.join(name);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(source.join(name), destination).unwrap();
    }
    assert_eq!(tree_manifest(source), tree_manifest(destination));
}

async fn pg_tool(container: &str, args: &[&str], input: Option<&Path>, output: Option<&Path>) {
    let mut command = Command::new("docker");
    command
        .args([
            "exec",
            "-i",
            "-e",
            "PGOPTIONS=-c statement_timeout=600000 -c lock_timeout=10000",
            container,
        ])
        .args(["timeout", "-k", "5", "1800"])
        .args(args);
    command.stderr(Stdio::inherit());
    command.stdin(input.map_or_else(Stdio::null, |p| Stdio::from(File::open(p).unwrap())));
    command.stdout(output.map_or_else(Stdio::null, |p| Stdio::from(File::create_new(p).unwrap())));
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "PG backup/restore tool failed");
            break;
        }
        if start.elapsed() > Duration::from_secs(1810) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Docker command did not exit after the container-side PG deadline");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn database_bytes(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn measured_cutover(pool: &PgPool) -> serde_json::Value {
    let migration_pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('statement_timeout','590s',false),set_config('lock_timeout','10s',false)")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let before_bytes = database_bytes(pool).await;
    let lsn: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    let start = Instant::now();
    let mut migration = Box::pin(timeout(
        Duration::from_secs(600),
        run_migrations(&migration_pool),
    ));
    let mut lock_wait_samples = 0;
    loop {
        tokio::select! {
            result = &mut migration => {
                result.expect("cutover exceeded 10 minutes").unwrap();
                break;
            }
            () = tokio::time::sleep(Duration::from_millis(5)) => {
                let waiting = sqlx::query_scalar::<_,bool>(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE NOT granted
                     AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                     AND relation IN ('request_logs'::regclass,'request_log_ingest'::regclass))",
                );
                let waiting = timeout(Duration::from_secs(10), waiting.fetch_one(pool))
                    .await.expect("lock sampling timed out").unwrap();
                lock_wait_samples += u64::from(waiting);
            }
        }
    }
    let elapsed = start.elapsed().as_millis();
    migration_pool.close().await;
    let wal_bytes: String =
        sqlx::query_scalar("SELECT pg_wal_lsn_diff(pg_current_wal_insert_lsn(), $1::pg_lsn)::text")
            .bind(lsn)
            .fetch_one(pool)
            .await
            .unwrap();
    json!({
        "elapsed_ms": elapsed,
        "database_bytes_before": before_bytes,
        "database_bytes_after": database_bytes(pool).await,
        "cluster_wal_bytes_upper_bound": wal_bytes,
        "lock_wait_positive_samples": lock_wait_samples,
        "lock_sampling_interval_ms": 5,
    })
}

async fn sharing_fixture(
    pool: &PgPool,
    seed: &Seed,
    directory: &Path,
    terminal_id: Uuid,
    unknown_id: Uuid,
) -> (SharingRuntime, SharingGroup, serde_json::Value) {
    let repository = ControlPlaneRepository::new(pool.clone());
    repository
        .ensure_system_settings(system_settings())
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let sharing = SharingRuntime::open(directory.to_path_buf()).await.unwrap();
    let owner = repository
        .claim_sharing_ledger(sharing.ledger_id().unwrap())
        .await
        .unwrap();
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        runtime.clone(),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    )
    .with_sharing_runtime(sharing.clone());
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups(id,name,api_format,connector_kind,enabled,sharing_only)
         VALUES($1,'rehearsal-sharing','open_ai_responses','codex_oauth',true,true)",
    )
    .bind(codex_group)
    .execute(pool)
    .await
    .unwrap();
    let mut input = business_codex_credential(
        codex_group,
        "rehearsal",
        "rehearsal@example.test",
        "rehearsal-user",
    );
    input.quota = Some(CodexQuotaUpdate {
        allowed: true,
        limit_reached: false,
        primary_used_percent: Some(10),
        primary_window_seconds: Some(3600),
        primary_reset_at: Some(Utc::now() + chrono::Duration::hours(1)),
        secondary_used_percent: None,
        secondary_window_seconds: None,
        secondary_reset_at: None,
        reset_credits_available: None,
        checked_at: Utc::now(),
    });
    let credential = coordinator
        .create_codex_credential(seed.user, input, None)
        .await
        .unwrap();
    let group_id = Uuid::new_v4();
    coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::SaveCodexSharing {
                id: group_id,
                input: SharingGroupInput {
                    credential_id: credential.id,
                    name: "Rehearsal seats".into(),
                    enabled: true,
                    seats: vec![Some(seed.user)],
                    primary_limit_amount: Decimal::from(20),
                    secondary_limit_amount: Decimal::from(100),
                    request_reservation_amount: Decimal::ONE,
                    user_requests_per_minute: 30,
                    group_requests_per_minute: 60,
                    user_max_concurrent_requests: 4,
                    group_max_concurrent_requests: 4,
                },
                expected_updated_at: None,
            },
        )
        .await
        .unwrap();
    let group = runtime
        .snapshot()
        .sharing()
        .group(group_id)
        .unwrap()
        .clone();
    sharing
        .reserve(&group, seed.user, Uuid::new_v4())
        .await
        .unwrap()
        .settle(Some(Decimal::ONE));
    sharing.flush().await.unwrap();
    let terminal = sharing
        .reserve(&group, seed.user, terminal_id)
        .await
        .unwrap();
    let unknown = sharing
        .reserve(&group, seed.user, unknown_id)
        .await
        .unwrap();
    drop((terminal, unknown));
    sharing.flush().await.unwrap();
    let usage = serde_json::to_value(sharing.inspect(&group, seed.user).await).unwrap();
    assert_eq!(usage["pending_requests"], 2);
    drop((coordinator, owner));
    (sharing, group, usage)
}

async fn financial_snapshot(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'facts',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM request_metering_facts f),
          'receipts',(SELECT jsonb_agg(to_jsonb(s)-'settled_at' ORDER BY request_id) FROM request_settlements s),
          'users',(SELECT jsonb_agg(jsonb_build_array(id,balance_amount) ORDER BY id) FROM users),
          'keys',(SELECT jsonb_agg(jsonb_build_array(id,quota_used_amount) ORDER BY id) FROM api_keys),
          'pending',(SELECT count(*) FROM request_settlement_pending),
          'ingress',(SELECT count(*) FROM request_log_ingest))",
    ).fetch_one(pool).await.unwrap()
}

async fn assert_ineligible(pool: &PgPool, unknown: Uuid, rejected: Uuid) {
    for (id, state) in [(unknown, "unknown"), (rejected, "not_applicable")] {
        let row: (String, Option<Decimal>, bool) = sqlx::query_as(
            "SELECT f.amount_state,f.cost_amount,EXISTS(
               SELECT 1 FROM request_settlements s WHERE s.request_id=f.id)
             FROM request_metering_facts f WHERE f.id=$1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(row, (state.into(), None, false));
    }
}

async fn recover(pool: &PgPool, directory: &Path, ids: &[Uuid]) {
    let config = RequestLoggingConfig {
        spool_directory: directory.to_path_buf(),
        settlement_interval_milliseconds: 10,
        shutdown_drain_seconds: 2,
        ..RequestLoggingConfig::default()
    };
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(pool.clone()), &config)
            .await
            .unwrap();
    wait_for_receipts(pool, ids, ids.len() as i64).await;
    drop(sink);
    worker.shutdown().await;
    let ingress: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log_ingest")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(ingress, 0);
}

#[tokio::test]
#[ignore = "manual synthetic backup/restore rehearsal; requires Docker PG tools and a retained output directory"]
async fn schema62_backup_cutover_and_paired_restore() {
    let directory = std::path::PathBuf::from(
        env::var("PERSISTENCE_REHEARSAL_DIRECTORY")
            .expect("set a private retained output directory"),
    )
    .join(Uuid::new_v4().simple().to_string());
    fs::create_dir(&directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        directory.join("report.json"),
        b"{\"status\":\"incomplete\"}\n",
    )
    .unwrap();
    let container =
        env::var("PERSISTENCE_REHEARSAL_CONTAINER").expect("set the development PG container name");
    let database = legacy_database().await;
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "status":"incomplete","source_database":database.name,
        }))
        .unwrap(),
    )
    .unwrap();
    // Only a random database created by this test is ever passed to PG tools.
    assert!(database.name.starts_with("ai_gateway_test_"));
    let seed = seed(&database.pool).await;
    let billed = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let pending = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let queued = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let spooled = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut unknown = request_log_event(&seed, RequestLogOutcome::Succeeded);
    unknown.billing.as_mut().unwrap().usage = None;
    unknown.billing.as_mut().unwrap().cost_amount = None;
    let mut failed = request_log_event(&seed, RequestLogOutcome::Failed);
    failed.billing = None;
    let mut rejected = request_log_event(&seed, RequestLogOutcome::Rejected);
    rejected.billing = None;
    for (event, settled) in [
        (&billed, true),
        (&pending, false),
        (&unknown, false),
        (&failed, false),
        (&rejected, false),
    ] {
        insert_legacy_event(&database.pool, event, settled).await;
    }
    sqlx::query("UPDATE users SET balance_amount=10 WHERE id=$1")
        .bind(seed.user)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET quota_used_amount=$1 WHERE id=$2")
        .bind(billed.effective_cost_amount())
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    for event in [&billed, &billed, &queued] {
        sqlx::query(
            "INSERT INTO request_log_ingest(request_log_id,schema_version,payload) VALUES($1,6,$2)",
        )
        .bind(event.id)
        .bind(serde_json::to_vec(event).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();
    }
    let spool = directory.join("source-spool");
    let config = RequestLoggingConfig {
        spool_directory: spool.clone(),
        shutdown_drain_seconds: 0,
        ..RequestLoggingConfig::default()
    };
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), &config)
            .await
            .unwrap();
    worker.shutdown().await;
    let intent = RequestLogIntent {
        version: 1,
        id: Uuid::new_v4(),
        user_id: seed.user,
        api_key_id: seed.key,
        model_id: seed.model,
        started_at: Utc::now(),
        api_operation: spooled.api_operation,
        request_protocol: spooled.request_protocol,
    };
    sink.admit(&intent).unwrap();
    sink.admit(&RequestLogIntent {
        id: spooled.id,
        ..intent.clone()
    })
    .unwrap();
    sink.try_record(spooled.clone());
    sink.try_record(billed.clone());
    drop(sink);
    let (sharing, group, sharing_before) = sharing_fixture(
        &database.pool,
        &seed,
        &spool.join("codex-sharing"),
        spooled.id,
        intent.id,
    )
    .await;
    let ledger_id = sharing.ledger_id().unwrap();
    let before = accounts(&database.pool, seed.key).await;
    let backup = directory.join("backup");
    fs::create_dir(&backup).unwrap();
    let dump = backup.join("database.dump");
    let downtime = Instant::now();
    let backup_started = Instant::now();
    pg_tool(
        &container,
        &[
            "pg_dump",
            "-U",
            "ai_gateway",
            "--format=custom",
            "--no-owner",
            "--no-acl",
            &database.name,
        ],
        None,
        Some(&dump),
    )
    .await;
    copy_tree(&spool, &backup.join("spool"));
    let backup_ms = backup_started.elapsed().as_millis();
    let manifest = tree_manifest(&backup);
    fs::write(
        directory.join("backup-manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let cutover = measured_cutover(&database.pool).await;
    assert_eq!(accounts(&database.pool, seed.key).await, before);
    let original_receipt: DateTime<Utc> =
        sqlx::query_scalar("SELECT settled_at FROM request_settlements WHERE request_id=$1")
            .bind(billed.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        original_receipt.timestamp_micros(),
        billed.completed_at.timestamp_micros()
    );
    let old_claim = sqlx::query("UPDATE request_logs SET billed_at=now() WHERE id=$1")
        .bind(pending.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        old_claim
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("42703")
    );
    let old_ack = sqlx::query("DELETE FROM request_log_ingest")
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        old_ack
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    let mut old_migrator = sqlx::migrate::Migrator::new(Path::new("./migrations"))
        .await
        .unwrap();
    old_migrator.migrations = old_migrator
        .iter()
        .filter(|m| m.version <= 62)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    assert!(matches!(
        old_migrator.run(&database.pool).await,
        Err(sqlx::migrate::MigrateError::VersionMissing(63))
    ));
    let ids = [billed.id, pending.id, failed.id, queued.id, spooled.id];
    recover(&database.pool, &spool, &ids).await;
    let increment = [pending, queued, spooled.clone()]
        .iter()
        .map(|e| e.effective_cost_amount().unwrap())
        .sum::<Decimal>();
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (before.0 - increment, before.1 + increment)
    );
    let expected = financial_snapshot(&database.pool).await;
    assert_ineligible(&database.pool, unknown.id, rejected.id).await;
    assert_eq!(expected["facts"].as_array().unwrap().len(), 7);
    assert_eq!(expected["pending"], 0);
    assert!(
        spool
            .join(format!("admissions/{}.json", intent.id))
            .exists()
    );
    let upgrade_downtime_ms = downtime.elapsed().as_millis();

    let restore_started = Instant::now();
    assert_eq!(
        tree_manifest(&backup),
        manifest,
        "reject a mismatched database/evidence pair before restore"
    );
    let restored = TestDatabase::new_unmigrated().await;
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "status":"incomplete","source_database":database.name,"restore_database":restored.name,
        }))
        .unwrap(),
    )
    .unwrap();
    pg_tool(
        &container,
        &[
            "pg_restore",
            "-U",
            "ai_gateway",
            "--exit-on-error",
            "--single-transaction",
            "--no-owner",
            "--no-acl",
            "--dbname",
            &restored.name,
        ],
        Some(&dump),
        None,
    )
    .await;
    let restored_spool = directory.join("restored-spool");
    copy_tree(&backup.join("spool"), &restored_spool);
    let restored_schema: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(&restored.pool)
        .await
        .unwrap();
    assert_eq!(restored_schema, 62);
    assert_eq!(accounts(&restored.pool, seed.key).await, before);
    let restored_sharing = SharingRuntime::open(restored_spool.join("codex-sharing"))
        .await
        .unwrap();
    assert_eq!(restored_sharing.ledger_id(), Some(ledger_id));
    let repository = ControlPlaneRepository::new(restored.pool.clone());
    let owner = repository.claim_sharing_ledger(ledger_id).await.unwrap();
    let snapshot = compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap();
    restored_sharing.publish(snapshot.sharing());
    restored_sharing.flush().await.unwrap();
    assert_eq!(
        serde_json::to_value(restored_sharing.inspect(&group, seed.user).await).unwrap(),
        sharing_before
    );
    let restored_cutover = measured_cutover(&restored.pool).await;
    assert_eq!(accounts(&restored.pool, seed.key).await, before);
    recover(&restored.pool, &restored_spool, &ids).await;
    assert_eq!(financial_snapshot(&restored.pool).await, expected);
    assert_ineligible(&restored.pool, unknown.id, rejected.id).await;
    let restored_receipt: DateTime<Utc> =
        sqlx::query_scalar("SELECT settled_at FROM request_settlements WHERE request_id=$1")
            .bind(billed.id)
            .fetch_one(&restored.pool)
            .await
            .unwrap();
    assert_eq!(restored_receipt, original_receipt);
    let pending_sharing = restored_sharing.pending().await;
    let costs = MeteringQueries::new(restored.pool.clone())
        .sharing_completed_costs(&pending_sharing)
        .await
        .unwrap();
    assert_eq!(costs.len(), 1);
    for (id, cost) in costs {
        assert_eq!(id, spooled.id);
        restored_sharing.finish(id, Some(cost));
    }
    restored_sharing.flush().await.unwrap();
    assert_eq!(restored_sharing.pending().await, vec![intent.id]);
    let usage = restored_sharing.inspect(&group, seed.user).await;
    assert_eq!(
        usage.windows[0].used_amount,
        Decimal::ONE + spooled.effective_cost_amount().unwrap()
    );
    assert!(
        restored_spool
            .join(format!("admissions/{}.json", intent.id))
            .exists()
    );
    recover(&restored.pool, &restored_spool, &ids).await;
    assert_eq!(financial_snapshot(&restored.pool).await, expected);
    assert_ineligible(&restored.pool, unknown.id, rejected.id).await;
    let restore_total_ms = restore_started.elapsed().as_millis();
    drop((owner, restored_sharing, sharing));
    let report = json!({
        "status":"passed",
        "scope":"small synthetic data on development PostgreSQL; not production capacity certification",
        "schema_from":62,"schema_to":63,
        "source_database":database.name,"restore_database":restored.name,
        "fixture":{"historical_logs":5,"historical_receipts":1,"ingress_rows":3,
          "spool_events":2,"unresolved_intents":1,"sharing_pending_before":2,"sharing_pending_after":1},
        "backup_ms":backup_ms,"backup_bytes":manifest.keys().map(|p|fs::metadata(backup.join(p)).unwrap().len()).sum::<u64>(),
        "upgrade_downtime_ms":upgrade_downtime_ms,"upgrade_budget_ms":600_000,
        "restore_total_ms":restore_total_ms,"restore_budget_ms":1_800_000,
        "cutover":cutover,"restored_cutover":restored_cutover,
        "checks":["no backfill debit","old claim and ack rejected","old migration registry rejected",
          "paired backup integrity","schema62 restore","exact financial replay","no duplicate debit on restart",
          "unknown intent retained","sharing identity money and pending restored"],
        "post_snapshot_external_dispatches":0,
        "postgres_version":sqlx::query_scalar::<_,String>("SHOW server_version").fetch_one(&database.pool).await.unwrap(),
        "source_commit":String::from_utf8(Command::new("git").args(["rev-parse","HEAD"]).output().unwrap().stdout).unwrap().trim(),
        "working_tree_dirty":!Command::new("git").args(["status","--porcelain"]).output().unwrap().stdout.is_empty(),
        "cleanup":"passed",
    });
    database.cleanup().await;
    restored.cleanup().await;
    assert!(
        upgrade_downtime_ms <= 600_000,
        "upgrade exceeded 10 minutes"
    );
    assert!(restore_total_ms <= 1_800_000, "restore exceeded 30 minutes");
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "persistence rehearsal: {}",
        directory.join("report.json").display()
    );
}
