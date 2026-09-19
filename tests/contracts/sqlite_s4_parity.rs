//! The same financial and query operations, assertions and output comparison on both engines.

use super::*;
use ai_gateway::persistence::{
    ChannelGroupStatusWindow, CostStatisticsFilter, MeteringWriteOutcome,
    RequestLogBatchInsertOutcome, RequestLogFilter, RequestLogSettlementOutcome,
    SpendLeaderboardFilter, SpendLeaderboardPeriod, SpendLeaderboardRefresh, StatisticsGranularity,
    sqlite::SqliteDatabase,
};
use futures_util::FutureExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, sync::Arc};

const USER: Uuid = Uuid::from_u128(0x901);
const OTHER: Uuid = Uuid::from_u128(0x902);
const KEY: Uuid = Uuid::from_u128(0x903);
const MODEL: Uuid = Uuid::from_u128(0x904);
const GROUP: Uuid = Uuid::from_u128(0x905);
const CHANNEL: Uuid = Uuid::from_u128(0x906);

enum Backend {
    Pg(TestDatabase),
    Sq(tempfile::TempDir, Arc<SqliteDatabase>),
}
impl Backend {
    fn logs(&self) -> RequestLogRepository {
        match self {
            Self::Pg(db) => RequestLogRepository::new(db.pool.clone()),
            Self::Sq(_, db) => RequestLogRepository::from_sqlite(Arc::clone(db)),
        }
    }
    fn auth(&self) -> AuthRepository {
        match self {
            Self::Pg(db) => AuthRepository::new(db.pool.clone()),
            Self::Sq(_, db) => AuthRepository::from_sqlite(Arc::clone(db)),
        }
    }
    fn control(&self) -> ControlPlaneRepository {
        match self {
            Self::Pg(db) => ControlPlaneRepository::new(db.pool.clone()),
            Self::Sq(_, db) => ControlPlaneRepository::from_sqlite(Arc::clone(db)),
        }
    }
    async fn exec(&self, sql: &str) -> Result<(), sqlx::Error> {
        match self {
            Self::Pg(db) => {
                sqlx::raw_sql(sql).execute(&db.pool).await?;
            }
            Self::Sq(_, db) => {
                let mut tx = db.begin_write().await.unwrap();
                sqlx::raw_sql(&sql.replace("now()", "ag_now()"))
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }
    async fn count(&self, table: &str) -> i64 {
        let sql = format!("SELECT count(*) FROM {table}");
        match self {
            Self::Pg(db) => sqlx::query_scalar(&sql).fetch_one(&db.pool).await.unwrap(),
            Self::Sq(_, db) => sqlx::query_scalar(&sql)
                .fetch_one(&mut *db.acquire_read().await.unwrap())
                .await
                .unwrap(),
        }
    }
    async fn accounts(&self) -> (Decimal, Decimal) {
        (
            self.auth()
                .profile(USER)
                .await
                .unwrap()
                .unwrap()
                .balance_amount,
            self.control()
                .own_api_key(USER, KEY)
                .await
                .unwrap()
                .unwrap()
                .quota_used_amount,
        )
    }
    async fn seed(&self) {
        let (formats, permissions) = match self {
            Self::Pg(_) => ("'{open_ai_chat_completions}'", "'{proxy,models.read}'"),
            Self::Sq(..) => (
                "'[\"open_ai_chat_completions\"]'",
                "'[\"proxy\",\"models.read\"]'",
            ),
        };
        self.exec(&format!(
            "INSERT INTO users(id,display_name,role,status,balance_amount,user_group_id) VALUES
             ('{USER}','User','user','active','100','00000000-0000-0000-0000-000000000101'),
             ('{OTHER}','Other','user','active','0','00000000-0000-0000-0000-000000000101');
             INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions) VALUES
             ('{KEY}','{USER}','Key','s4-fixture-key','active',{formats},{permissions});
             INSERT INTO models(id,source_model_id,display_name,enabled,currency,price_unit_tokens,
               input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
             VALUES ('{MODEL}','model','Model',true,'USD',1000000,'1','0','0','2',now());
             INSERT INTO channel_groups(id,name,api_format,enabled,status_statistics_enabled)
             VALUES ('{GROUP}','Group','open_ai_chat_completions',true,true);
             INSERT INTO channels(id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind)
             VALUES ('{CHANNEL}','{GROUP}','open_ai_chat_completions','Channel','https://example.test',true,'none');"
        )).await.unwrap();
    }
    async fn finish(self) {
        match self {
            Self::Pg(db) => db.cleanup().await,
            Self::Sq(directory, db) => {
                db.close().await;
                drop(directory);
            }
        }
    }
}

fn amount(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap()
}
fn event(index: u128, now: DateTime<Utc>, outcome: RequestLogOutcome) -> RequestLogEvent {
    let seed = Seed {
        user: USER,
        model: MODEL,
        profile: Uuid::nil(),
        group: GROUP,
        other_group: Uuid::nil(),
        channel: CHANNEL,
        proxy: Uuid::nil(),
        template: Uuid::nil(),
        key: KEY,
        rule: Uuid::nil(),
        secret: String::new(),
        email: String::new(),
        password: String::new(),
        client_model: "model".into(),
    };
    let mut event = super::request_log_event(&seed, outcome);
    event.id = Uuid::from_u128(0x1000 + index);
    event.model_rule_id = None;
    event.started_at = now;
    event.completed_at = now + chrono::Duration::seconds(1);
    let billing = event.billing.as_mut().unwrap();
    billing.price.price_effective_at = now - chrono::Duration::days(1);
    billing.cost_amount = Some(amount("1.00000001"));
    event
}
fn filter(now: DateTime<Utc>) -> CostStatisticsFilter {
    CostStatisticsFilter {
        started_at: now - chrono::Duration::days(1),
        ended_at: now + chrono::Duration::hours(1),
        granularity: StatisticsGranularity::Hour,
        user_id: None,
        api_key_id: None,
        channel_id: None,
        codex_credential_id: None,
        include_channel_details: true,
    }
}
fn normalize_clocks(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "billed_at" | "refreshed_at") && !value.is_null() {
                    *value = json!("present");
                } else {
                    normalize_clocks(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_clocks(value);
            }
        }
        _ => {}
    }
}

async fn run(case: u8) {
    let now = DateTime::from_timestamp(Utc::now().timestamp() - 3600, 0).unwrap();
    let pg = Backend::Pg(TestDatabase::new().await);
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let database = Arc::new(
        SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    database.install_schema().await.unwrap();
    let mut outputs = Vec::new();
    let mut failures = Vec::new();
    for (name, backend) in [
        ("postgres", pg),
        ("sqlite", Backend::Sq(directory, database)),
    ] {
        let result = std::panic::AssertUnwindSafe(async {
            backend.seed().await;
            match case {
                0 => replay_and_settlement(&backend, now).await,
                1 => classification(&backend, now).await,
                2 => projection_failure(&backend, now).await,
                3 => query_contract(&backend, now).await,
                4 => overflow(&backend, now).await,
                5 => aggregate_boundary(&backend, now).await,
                6 => time_boundaries(&backend, now).await,
                _ => panic!("unknown contract"),
            }
        })
        .catch_unwind()
        .await;
        backend.finish().await;
        match result {
            Ok(mut output) => {
                normalize_clocks(&mut output);
                outputs.push(output);
            }
            Err(error) => failures.push(format!(
                "{name}: {:?}",
                error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
            )),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(outputs[0], outputs[1]);
}

#[tokio::test]
async fn facts_replay_receipts_and_pending_match() {
    run(0).await;
}
#[tokio::test]
async fn financial_classification_matches() {
    run(1).await;
}
#[tokio::test]
async fn projection_failure_does_not_block_money() {
    run(2).await;
}
#[tokio::test]
async fn all_financial_and_log_query_views_match() {
    run(3).await;
}
#[tokio::test]
async fn account_overflow_rolls_back_entire_batch() {
    run(4).await;
}
#[tokio::test]
async fn aggregate_precision_matches_postgres_at_decimal_capacity() {
    run(5).await;
}

#[tokio::test]
async fn fractional_seconds_do_not_cross_status_or_calendar_boundaries() {
    run(6).await;
}

async fn time_boundaries(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    let bucket = DateTime::from_timestamp(now.timestamp().div_euclid(1800) * 1800, 0).unwrap();
    let mut events = [
        event(
            1,
            bucket - chrono::Duration::microseconds(1),
            RequestLogOutcome::Succeeded,
        ),
        event(2, bucket, RequestLogOutcome::Succeeded),
    ];
    for item in &mut events {
        item.request_source = ai_gateway::domain::RequestLogSource::ScheduledTest;
    }
    logs.insert_batch(&events).await.unwrap();
    let status = logs
        .queries()
        .channel_group_status(ChannelGroupStatusWindow::Last24Hours)
        .await
        .unwrap();
    let history = &status.groups[0].models[0].history;
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0].started_at,
        bucket - chrono::Duration::minutes(30)
    );
    assert_eq!(history[1].started_at, bucket);
    for item in history {
        assert_eq!(item.request_count, 1);
    }
    for (i, time) in [
        "2026-06-30T15:59:59.999999Z",
        "2026-06-30T16:00:00.000000Z",
        "2026-07-05T15:59:59.999999Z",
        "2026-07-05T16:00:00.000000Z",
    ]
    .into_iter()
    .enumerate()
    {
        logs.metering()
            .record_batch(&[event(
                10 + i as u128,
                time.parse().unwrap(),
                RequestLogOutcome::Succeeded,
            )])
            .await
            .unwrap();
    }
    let queries = logs.queries().metering();
    queries.refresh_spend_leaderboard_snapshots().await.unwrap();
    let mut boards = Vec::new();
    for (period, start, count) in [
        (SpendLeaderboardPeriod::Day, "2026-06-30", 1),
        (SpendLeaderboardPeriod::Day, "2026-07-01", 1),
        (SpendLeaderboardPeriod::Month, "2026-06-01", 1),
        (SpendLeaderboardPeriod::Month, "2026-07-01", 3),
        (SpendLeaderboardPeriod::Week, "2026-06-29", 3),
        (SpendLeaderboardPeriod::Week, "2026-07-06", 1),
    ] {
        let report = queries
            .spend_leaderboard(SpendLeaderboardFilter {
                period,
                period_start: start.parse().unwrap(),
                limit: 100,
            })
            .await
            .unwrap();
        assert_eq!(report.entries[0].request_count, count);
        assert_eq!(
            report.total_cost_amount,
            amount("1.00000001") * Decimal::from(count)
        );
        boards.push(report);
    }
    json!({"history":history,"boards":boards})
}

async fn aggregate_boundary(db: &Backend, now: DateTime<Utc>) -> Value {
    let id = match db {
        Backend::Pg(_) => "md5(CAST(n AS TEXT))::uuid",
        Backend::Sq(..) => "ag_md5_uuid(CAST(n AS TEXT))",
    };
    let time = now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    db.exec(&format!(
        "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<80000)
         INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,
           request_source,api_format,api_operation,request_protocol,client_model,outcome,cost_amount,peak_pricing)
         SELECT {id},'{time}','{time}','{USER}','{KEY}','client',
           'open_ai_chat_completions','chat_completions','sse','model-'||CAST(n%2 AS TEXT),
           'succeeded','9999999999999999.99999999',false FROM numbers"
    )).await.unwrap();
    let report = db
        .logs()
        .queries()
        .metering()
        .cost_statistics(filter(now))
        .await
        .unwrap();
    assert_eq!(report.summary.request_count, 80000);
    assert_eq!(
        report.summary.cost_amount,
        amount("799999999999999999999.9992")
    );
    assert_eq!(report.models.len(), 2);
    for model in &report.models {
        assert_eq!(model.cost_amount, amount("399999999999999999999.9996"));
    }
    serde_json::to_value(report).unwrap()
}

async fn replay_and_settlement(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    let mut first = event(1, now, RequestLogOutcome::Succeeded);
    first.billing.as_mut().unwrap().cost_amount = Some(amount("1.000000001"));
    first.billing.as_mut().unwrap().price.input_unit_price = amount("1.0000000000001");
    let mut conflict = first.clone();
    conflict.billing.as_mut().unwrap().cost_amount = Some(amount("2"));
    assert_eq!(
        logs.metering()
            .record_batch(&[first.clone(), first.clone(), conflict])
            .await
            .unwrap(),
        [
            MeteringWriteOutcome::Accepted,
            MeteringWriteOutcome::Accepted,
            MeteringWriteOutcome::Conflict
        ]
    );
    assert_eq!(
        logs.metering()
            .record_batch(&[first.clone()])
            .await
            .unwrap(),
        [MeteringWriteOutcome::Accepted]
    );
    assert_eq!(db.count("request_metering_facts").await, 1);
    assert_eq!(db.count("request_settlement_pending").await, 1);
    assert_eq!(
        logs.settlements().settle_pending(10).await.unwrap().len(),
        1
    );
    assert_eq!(db.accounts().await, (amount("99"), amount("1")));
    assert!(matches!(
        logs.settlements().settle(first.id).await.unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    ));
    logs.metering()
        .record_batch(&[first.clone()])
        .await
        .unwrap();
    assert_eq!(db.count("request_settlement_pending").await, 0);
    assert!(
        db.exec("UPDATE request_metering_facts SET cost_amount='3'")
            .await
            .is_err()
    );
    assert!(db.exec("DELETE FROM request_settlements").await.is_err());
    let second = event(2, now, RequestLogOutcome::Succeeded);
    let outcomes = logs
        .insert_batch(&[second.clone(), second.clone()])
        .await
        .unwrap();
    assert_eq!(outcomes[0].outcome, RequestLogBatchInsertOutcome::Inserted);
    assert_eq!(
        outcomes[1].outcome,
        RequestLogBatchInsertOutcome::ExactDuplicate
    );
    let mut diagnostic = second.clone();
    diagnostic.error_summary = Some("different diagnostic".into());
    assert_eq!(
        logs.insert_batch(&[diagnostic]).await.unwrap()[0].outcome,
        RequestLogBatchInsertOutcome::DuplicateConflict
    );
    logs.settlements().settle(second.id).await.unwrap();
    db.exec("DELETE FROM request_logs").await.unwrap();
    logs.insert(&second).await.unwrap();
    assert!(matches!(
        logs.settlements().settle(second.id).await.unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    ));
    assert_eq!(
        db.accounts().await,
        (amount("97.99999999"), amount("2.00000001"))
    );
    assert_eq!(db.count("request_settlements").await, 2);
    json!({"accounts":[db.accounts().await.0,db.accounts().await.1]})
}

async fn classification(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    let mut events = Vec::new();
    for (index, outcome) in [
        RequestLogOutcome::Failed,
        RequestLogOutcome::Cancelled,
        RequestLogOutcome::Succeeded,
        RequestLogOutcome::Rejected,
    ]
    .into_iter()
    .enumerate()
    {
        let mut item = event(index as u128, now, outcome);
        item.billing = None;
        events.push(item);
    }
    let mut invalid = event(10, now, RequestLogOutcome::Succeeded);
    invalid.model_id = None;
    events.push(invalid);
    let mut mismatch = event(11, now, RequestLogOutcome::Succeeded);
    mismatch.user_id = OTHER;
    events.push(mismatch);
    let mut search = event(12, now, RequestLogOutcome::Succeeded);
    search.api_format = ApiFormat::OpenAiResponses;
    search.api_operation = ApiOperation::StandaloneWebSearch;
    search.request_protocol = RequestProtocol::NonStream;
    search.streamed = false;
    search.billing = None;
    search.model_id = None;
    search.channel_id = None;
    search.channel_group_id = None;
    events.push(search);
    logs.metering().record_batch(&events).await.unwrap();
    let counts = logs.metering().reconciliation_counts().await.unwrap();
    assert_eq!(
        (counts.unknown, counts.invalid, counts.account_mismatch),
        (1, 1, 1)
    );
    assert_eq!(
        logs.settlements().settle_pending(100).await.unwrap().len(),
        2
    );
    assert_eq!(db.accounts().await, (amount("100"), Decimal::ZERO));
    assert_eq!(db.count("request_settlement_pending").await, 1);
    let shared = logs
        .queries()
        .metering()
        .sharing_completed_costs(&events.iter().map(|e| e.id).collect::<Vec<_>>())
        .await
        .unwrap();
    assert_eq!(shared.len(), 3);
    assert!(
        logs.queries()
            .metering()
            .sharing_completed_costs(&vec![Uuid::nil(); 1001])
            .await
            .is_err()
    );
    json!({"unknown":counts.unknown,"invalid":counts.invalid,"mismatch":counts.account_mismatch})
}

async fn projection_failure(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    let mut item = event(1, now, RequestLogOutcome::Succeeded);
    item.ttft_ms = Some(-1);
    assert!(logs.insert(&item).await.is_err());
    assert_eq!(db.count("request_logs").await, 0);
    assert_eq!(db.count("request_metering_facts").await, 1);
    assert_eq!(
        logs.settlements().settle_pending(10).await.unwrap().len(),
        1
    );
    item.ttft_ms = Some(1);
    logs.insert(&item).await.unwrap();
    let viewed = logs.queries().get(item.id).await.unwrap().unwrap();
    assert!(viewed.billed_at.is_some());
    assert_eq!(
        db.accounts().await,
        (amount("98.99999999"), amount("1.00000001"))
    );
    serde_json::to_value(viewed).unwrap()
}

async fn query_contract(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    let mut events = Vec::new();
    for i in 0..37 {
        let outcome = match i % 4 {
            0 => RequestLogOutcome::Succeeded,
            1 => RequestLogOutcome::Failed,
            2 => RequestLogOutcome::Cancelled,
            _ => RequestLogOutcome::Rejected,
        };
        let mut item = event(i, now - chrono::Duration::minutes(i as i64 * 40), outcome);
        item.ttft_ms = Some((i * 17) as i32);
        if outcome != RequestLogOutcome::Succeeded {
            item.billing = None;
        }
        events.push(item);
    }
    logs.insert_batch(&events).await.unwrap();
    logs.settlements().settle_pending(100).await.unwrap();
    let queries = logs.queries();
    let reports = queries.metering();
    assert!(
        queries
            .get_for_user(OTHER, events[0].id)
            .await
            .unwrap()
            .is_none()
    );
    let own = queries
        .get_for_user(USER, events[0].id)
        .await
        .unwrap()
        .unwrap();
    assert!(own.channel_id.is_none());
    let mut views = Vec::new();
    for mut f in [
        RequestLogFilter::default(),
        RequestLogFilter {
            user_id: Some(USER),
            ..Default::default()
        },
        RequestLogFilter {
            api_key_id: Some(KEY),
            ..Default::default()
        },
        RequestLogFilter {
            model: Some("model".into()),
            ..Default::default()
        },
        RequestLogFilter {
            outcome: Some("succeeded".into()),
            ..Default::default()
        },
        RequestLogFilter {
            api_format: Some("open_ai_chat_completions".into()),
            ..Default::default()
        },
        RequestLogFilter {
            api_operation: Some("chat_completions".into()),
            ..Default::default()
        },
        RequestLogFilter {
            billed: Some(true),
            ..Default::default()
        },
        RequestLogFilter {
            billed: Some(false),
            ..Default::default()
        },
    ] {
        f.limit = 100;
        views.push(serde_json::to_value(queries.list_all(f).await.unwrap()).unwrap());
    }
    let mut costs = Vec::new();
    for f in [
        filter(now),
        CostStatisticsFilter {
            api_key_id: Some(KEY),
            ..filter(now)
        },
        CostStatisticsFilter {
            channel_id: Some(CHANNEL),
            ..filter(now)
        },
        CostStatisticsFilter {
            user_id: Some(OTHER),
            ..filter(now)
        },
    ] {
        costs.push(serde_json::to_value(reports.cost_statistics(f).await.unwrap()).unwrap());
    }
    let mut status = Vec::new();
    for window in [
        ChannelGroupStatusWindow::Last24Hours,
        ChannelGroupStatusWindow::Last3Days,
        ChannelGroupStatusWindow::Last7Days,
    ] {
        let mut value =
            serde_json::to_value(queries.channel_group_status(window).await.unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("started_at");
        value.as_object_mut().unwrap().remove("ended_at");
        status.push(value);
    }
    assert_eq!(
        reports.refresh_spend_leaderboard_snapshots().await.unwrap(),
        SpendLeaderboardRefresh::Updated
    );
    let mut boards = Vec::new();
    for period in [
        SpendLeaderboardPeriod::Day,
        SpendLeaderboardPeriod::Week,
        SpendLeaderboardPeriod::Month,
    ] {
        boards.push(
            serde_json::to_value(
                reports
                    .spend_leaderboard(SpendLeaderboardFilter {
                        period,
                        period_start: period.current_start_at(now),
                        limit: 1,
                    })
                    .await
                    .unwrap(),
            )
            .unwrap(),
        );
    }
    json!({"logs":views,"own":own,"costs":costs,"status":status,"boards":boards,
        "usage":reports.personal_usage(USER,now.date_naive()).await.unwrap()})
}

async fn overflow(db: &Backend, now: DateTime<Utc>) -> Value {
    let logs = db.logs();
    db.exec(&format!("UPDATE users SET balance_amount='-9999999999999999.99999999',updated_at=now() WHERE id='{USER}'")).await.unwrap();
    let events = [
        event(1, now, RequestLogOutcome::Succeeded),
        event(2, now, RequestLogOutcome::Succeeded),
    ];
    logs.metering().record_batch(&events).await.unwrap();
    assert!(
        logs.settlements()
            .settle_batch(&events.iter().map(|e| e.id).collect::<Vec<_>>())
            .await
            .is_err()
    );
    assert_eq!(db.count("request_settlements").await, 0);
    assert_eq!(db.count("request_settlement_pending").await, 2);
    assert_eq!(
        db.accounts().await,
        (amount("-9999999999999999.99999999"), Decimal::ZERO)
    );
    db.exec(&format!(
        "UPDATE users SET balance_amount='100',updated_at=now() WHERE id='{USER}';
         UPDATE api_keys SET quota_used_amount='9999999999999999.99999999',updated_at=now() WHERE id='{KEY}'"
    )).await.unwrap();
    assert!(
        logs.settlements()
            .settle_batch(&events.iter().map(|e| e.id).collect::<Vec<_>>())
            .await
            .is_err()
    );
    assert_eq!(db.count("request_settlements").await, 0);
    assert_eq!(db.count("request_settlement_pending").await, 2);
    assert_eq!(
        db.accounts().await,
        (amount("100"), amount("9999999999999999.99999999"))
    );
    json!({"rolled_back":true})
}
