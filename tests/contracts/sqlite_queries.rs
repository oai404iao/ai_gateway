//! SQLite S4 query and statistics contracts.
//!
//! These run against a real installed business schema and exercise the public
//! read surface the Console and Codex sharing depend on: owner-scoped and
//! redacted request-log views, the channel-group status aggregation with exact
//! PostgreSQL buckets/windows/log metrics, personal usage, cost statistics,
//! spend-leaderboard refresh snapshots/reads, and independent sharing cost
//! reads.
//!
//! Host wiring (the parent-owned `tests/sqlite_foundation.rs`):
//!
//! ```ignore
//! #[path = "contracts/sqlite_queries.rs"]
//! mod sqlite_queries;
//! ```
//!
//! The module reuses the host's `database()` helper and the `sqlite-backend`
//! feature gate declared there.

use super::*;
use ai_gateway::persistence::{
    ChannelGroupStatusWindow, CostStatisticsFilter, RequestLogFilter, SpendLeaderboardFilter,
    SpendLeaderboardPeriod, SpendLeaderboardRefresh, StatisticsGranularity,
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use rust_decimal::Decimal;
use std::sync::Arc;
use uuid::Uuid;

const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA";

const USER: Uuid = Uuid::from_u128(0x901);
const OTHER_USER: Uuid = Uuid::from_u128(0x902);
const ADMIN: Uuid = Uuid::from_u128(0x903);
const KEY: Uuid = Uuid::from_u128(0x911);
const OTHER_KEY: Uuid = Uuid::from_u128(0x912);
const MODEL: Uuid = Uuid::from_u128(0x921);
const PROFILE: Uuid = Uuid::from_u128(0x922);
const RULE: Uuid = Uuid::from_u128(0x923);
const GROUP: Uuid = Uuid::from_u128(0x931);
const OTHER_GROUP: Uuid = Uuid::from_u128(0x932);
const CHANNEL: Uuid = Uuid::from_u128(0x941);
const OTHER_CHANNEL: Uuid = Uuid::from_u128(0x942);
const LOGICAL_CHANNEL: Uuid = Uuid::from_u128(0x943);
const OTHER_LOGICAL_CHANNEL: Uuid = Uuid::from_u128(0x944);
const ACCESS: Uuid = Uuid::from_u128(0x945);
const CODEX_GROUP_RESPONSES: Uuid = Uuid::from_u128(0x952);

struct Queries {
    _directory: tempfile::TempDir,
    database: Arc<SqliteDatabase>,
    request_logs: ai_gateway::persistence::sqlite::SqliteRequestLogQueries,
    metering: ai_gateway::persistence::sqlite::SqliteMeteringQueries,
}

impl Queries {
    async fn new() -> Self {
        let (directory, database) = database().await;
        assert_eq!(database.install_schema().await.unwrap(), 6);
        let database = Arc::new(database);
        let request_logs =
            ai_gateway::persistence::sqlite::SqliteRequestLogQueries::new(Arc::clone(&database));
        let metering = request_logs.metering();
        Self {
            _directory: directory,
            database,
            request_logs,
            metering,
        }
    }

    async fn execute(&self, sql: &str) {
        let mut transaction = self.database.begin_write().await.unwrap();
        sqlx::Executor::execute(&mut *transaction, sqlx::AssertSqlSafe(sql.to_owned()))
            .await
            .unwrap_or_else(|error| panic!("SQL failed: {error}\n{sql}"));
        transaction.commit().await.unwrap();
    }

    async fn scalar<T>(&self, sql: &str) -> T
    where
        for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
    {
        let mut reader = self.database.acquire_read().await.unwrap();
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(&mut *reader)
            .await
            .unwrap()
    }

    async fn finish(self) {
        self.database.close().await;
    }
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

/// Seeds two ordinary users, the priced model, two channel groups with one
/// enabled status-statistics group, and one Codex credential pool.
async fn seed(queries: &Queries) -> Uuid {
    queries
        .execute(&format!(
            "INSERT INTO users(id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES ('{ADMIN}','admin@example.test','Query admin','admin','active','{PASSWORD_HASH}',ag_now(),'{DEFAULT_ADMIN_GROUP_ID}'),
                    ('{USER}','user@example.test','Query user','user','active','{PASSWORD_HASH}',ag_now(),'{DEFAULT_USER_GROUP_ID}'),
                    ('{OTHER_USER}','other@example.test','Other user','user','active','{PASSWORD_HASH}',ag_now(),'{DEFAULT_USER_GROUP_ID}');
             INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
             VALUES ('{KEY}','{USER}','Query key','query-key-secret','active','[\"open_ai_chat_completions\"]','[\"proxy\"]'),
                    ('{OTHER_KEY}','{OTHER_USER}','Other key','other-key-secret','active','[\"open_ai_chat_completions\"]','[\"proxy\"]');
             INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
                 cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
             VALUES ('{MODEL}','query-model','Query model',1000000,'1','0','0','2','2026-01-01T00:00:00.000000Z');
             INSERT INTO routing_groups(id,name)
             VALUES ('{GROUP}','Query group'),('{OTHER_GROUP}','Other group');
             INSERT INTO upstream_accesses(id,name,connector_kind,base_url)
             VALUES ('{ACCESS}','Query access','general','https://upstream.invalid');
             INSERT INTO upstream_channels(id,group_id,access_id,name)
             VALUES ('{LOGICAL_CHANNEL}','{GROUP}','{ACCESS}','Query channel'),
                    ('{OTHER_LOGICAL_CHANNEL}','{OTHER_GROUP}','{ACCESS}','Other channel');
             INSERT INTO channel_capabilities(id,channel_id,operation,enabled,available_models,status_statistics_enabled)
             VALUES ('{CHANNEL}','{LOGICAL_CHANNEL}','chat_completion',1,'[\"query-model\",\"idle-model\"]',1),
                    ('{OTHER_CHANNEL}','{OTHER_LOGICAL_CHANNEL}','responses',1,'[]',0);
             INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
             INSERT INTO model_operation_rules(id,model_routing_profile_id,operation,enabled)
             VALUES ('{RULE}','{PROFILE}','chat_completion',0);
             INSERT INTO group_identity_registry(id,label,canonical_group_id)
             SELECT id,name,id FROM routing_groups;
             INSERT INTO channel_identity_registry(id,label,canonical_channel_id,capability_id)
             SELECT cap.id,c.name,c.id,cap.id FROM channel_capabilities cap JOIN upstream_channels c ON c.id=cap.channel_id;
             INSERT INTO model_rule_identity_registry(id,label,created_at,canonical_rule_id) VALUES ('{RULE}','query-model',ag_now(),'{RULE}');"
        ))
        .await;
    let repository = ai_gateway::persistence::sqlite::SqliteControlPlaneRepository::new(
        Arc::clone(&queries.database),
    );
    repository
        .prepare_mutation(
            ADMIN,
            ai_gateway::persistence::ControlPlaneMutation::SaveRoutingGroup {
                id: CODEX_GROUP_RESPONSES,
                expected: None,
                input: ai_gateway::persistence::RoutingGroupInput {
                    name: "Codex car".into(),
                    enabled: true,
                    sharing_only: false,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    repository
        .prepare_codex_credential_create(
            ADMIN,
            ai_gateway::persistence::CodexCredentialCreate {
                channel_group_id: CODEX_GROUP_RESPONSES,
                label: "Codex".into(),
                enabled: true,
                proxy_id: None,
                quota_threshold_percent: 95,
                base_url: "https://codex.invalid".into(),
                email: None,
                account_id: Some("acct".into()),
                user_id: Some("provider-user".into()),
                plan_type: None,
                is_fedramp: false,
                id_token: "id".into(),
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                access_token_expires_at: None,
                available_models: vec!["query-model".into()],
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
        .id
}

struct Fact<'a> {
    id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    request_source: &'a str,
    started_at: &'a str,
    outcome: &'a str,
    model: &'a str,
    api_format: &'a str,
    channel_id: Option<Uuid>,
    channel_group_id: Option<Uuid>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    ttft_ms: Option<i64>,
    tps: Option<&'a str>,
    cost_amount: Option<&'a str>,
}

impl Fact<'_> {
    fn succeeded(id: Uuid, started_at: &'static str, cost_amount: &'static str) -> Self {
        Self {
            id,
            user_id: USER,
            api_key_id: KEY,
            request_source: "client",
            started_at,
            outcome: "succeeded",
            model: "query-model",
            api_format: "open_ai_chat_completions",
            channel_id: Some(CHANNEL),
            channel_group_id: Some(GROUP),
            input_tokens: Some(10),
            output_tokens: Some(5),
            ttft_ms: Some(100),
            tps: Some("2"),
            cost_amount: Some(cost_amount),
        }
    }
}

/// Writes one fact directly so facts and the log projection stay independent.
async fn insert_fact(queries: &Queries, fact: &Fact<'_>) {
    let api_format = fact.api_format;
    let operation = match api_format {
        "open_ai_chat_completions" => "chat_completion",
        "open_ai_responses" => "responses",
        _ => "images_generation",
    };
    let currency = fact.cost_amount.map(|_| "'USD'").unwrap_or("NULL");
    let cost_amount = fact
        .cost_amount
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let price = if fact.cost_amount.is_some() {
        "'1000000','2026-01-01T00:00:00.000000Z','1','0','0','2'"
    } else {
        "NULL,NULL,NULL,NULL,NULL,NULL"
    };
    let channel = fact
        .channel_id
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let group = fact
        .channel_group_id
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let optional = |value: Option<i64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".into())
    };
    let outcome = format!("'{}'", fact.outcome);
    queries
        .execute(&format!(
            "INSERT INTO request_metering_facts
             (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
              request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,
              model_id,outcome,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,
              reasoning_tokens,currency,price_unit_tokens,price_effective_at,input_unit_price,
              cached_input_unit_price,cache_write_unit_price,output_unit_price,cost_amount,peak_pricing)
             VALUES ('{id}','{started_at}','{started_at}','{user}','{key}','{request_source}','{api_format}',
                     '{operation}','non_stream','{client}','{upstream}','{rule}',{group},{channel},
                     {model_id},{outcome},{input},{cached},{cache_write},{output},NULL,
                     {currency},{price},{cost_amount},0)",
            id = fact.id,
            started_at = fact.started_at,
            user = fact.user_id,
            key = fact.api_key_id,
            request_source = fact.request_source,
            client = fact.model,
            upstream = fact.model,
            rule = RULE,
            model_id = if fact.cost_amount.is_some() {
                format!("'{MODEL}'")
            } else {
                "NULL".into()
            },
            outcome = outcome,
            input = optional(fact.input_tokens),
            cached = optional(None),
            cache_write = optional(None),
            output = optional(fact.output_tokens),
        ))
        .await;
}

/// Writes the matching query-log projection row. Kept explicit so a test can
/// omit it and prove the financial queries still answer.
async fn insert_log(queries: &Queries, fact: &Fact<'_>) {
    let api_format = fact.api_format;
    let operation = match api_format {
        "open_ai_chat_completions" => "chat_completion",
        "open_ai_responses" => "responses",
        _ => "images_generation",
    };
    let cost_amount = fact
        .cost_amount
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let price = if fact.cost_amount.is_some() {
        "'USD','1000000','2026-01-01T00:00:00.000000Z','1','0','0','2'"
    } else {
        "NULL,NULL,NULL,NULL,NULL,NULL,NULL"
    };
    let channel = fact
        .channel_id
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let group = fact
        .channel_group_id
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let tps = fact
        .tps
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NULL".into());
    let ttft = fact
        .ttft_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".into());
    let input = fact.input_tokens.unwrap_or(0);
    let output = fact.output_tokens.unwrap_or(0);
    queries
        .execute(&format!(
            "INSERT INTO request_logs
             (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
              request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,
              model_id,outcome,response_status_code,streamed,ttft_ms,total_duration_ms,
              output_tokens_per_second,input_tokens,cached_input_tokens,cache_write_tokens,
              output_tokens,reasoning_tokens,currency,price_unit_tokens,price_effective_at,
              input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,
              cost_amount,error_code,error_summary,reasoning_effort,fast_mode,peak_pricing)
             VALUES ('{id}','{started_at}','{started_at}','{user}','{key}','client','{api_format}',
                     '{operation}','non_stream','{client}','{upstream}','{rule}',{group},{channel},
                     {model_id},'{outcome}',200,0,{ttft},1,{tps},{input},0,0,{output},NULL,
                     {price},{cost_amount},NULL,NULL,'high',1,0)",
            id = fact.id,
            started_at = fact.started_at,
            user = fact.user_id,
            key = fact.api_key_id,
            client = fact.model,
            upstream = fact.model,
            rule = RULE,
            model_id = if fact.cost_amount.is_some() {
                format!("'{MODEL}'")
            } else {
                "NULL".into()
            },
            outcome = fact.outcome,
            api_format = api_format,
            tps = tps,
            ttft = ttft,
            input = input,
            output = output,
        ))
        .await;
}

/// Writes the settlement receipt the schema guard accepts: the receipt amount
/// and currency must equal the eligible fact exactly.
async fn settle(queries: &Queries, ids: &[Uuid]) {
    queries
        .execute(&format!(
            "INSERT INTO request_settlements(request_id,cost_amount,currency) \
             SELECT id,cost_amount,currency FROM request_metering_facts \
             WHERE id IN ({})",
            ids.iter()
                .map(|id| format!("'{id}'"))
                .collect::<Vec<_>>()
                .join(",")
        ))
        .await;
}

#[tokio::test]
async fn request_log_views_apply_owner_scope_redaction_and_receipt_billing() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let unsettled = Uuid::new_v4();
    let facts = [
        Fact::succeeded(mine, "2026-09-18T12:00:00.000000Z", "1.25"),
        Fact {
            id: theirs,
            user_id: OTHER_USER,
            api_key_id: OTHER_KEY,
            ..Fact::succeeded(theirs, "2026-09-18T12:00:01.000000Z", "0.5")
        },
        Fact::succeeded(unsettled, "2026-09-18T12:00:02.000000Z", "9"),
    ];
    for fact in &facts {
        insert_fact(&queries, fact).await;
        insert_log(&queries, fact).await;
    }
    settle(&queries, &[mine, theirs]).await;

    // Newest first, bounded by the page limit even when it is out of range.
    let all = queries
        .request_logs
        .list_all(RequestLogFilter {
            limit: 500,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(|log| log.id).collect::<Vec<_>>(),
        vec![unsettled, theirs, mine]
    );
    assert_eq!(all[0].billed_at, None);
    assert!(all[2].billed_at.is_some());
    // The administrator view keeps channel attribution and the owner name.
    assert_eq!(all[2].channel_id, Some(CHANNEL));
    assert_eq!(all[2].channel_name.as_deref(), Some("Query channel"));
    assert_eq!(all[2].user_name.as_deref(), Some("Query user"));

    // The self-service view is owner-scoped and redacts channel attribution.
    let own = queries
        .request_logs
        .list_for_user(
            USER,
            RequestLogFilter {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        own.iter().map(|log| log.id).collect::<Vec<_>>(),
        vec![unsettled, mine]
    );
    assert!(own.iter().all(|log| {
        log.user_name.is_none() && log.channel_id.is_none() && log.channel_name.is_none()
    }));
    assert_eq!(
        queries
            .request_logs
            .get_for_user(USER, theirs)
            .await
            .unwrap()
            .map(|log| log.id),
        None
    );
    assert_eq!(
        queries
            .request_logs
            .get_for_user(USER, mine)
            .await
            .unwrap()
            .map(|log| log.id),
        Some(mine)
    );

    // Field, time and receipt filters stay closed over the same projection.
    let filtered = queries
        .request_logs
        .list_all(RequestLogFilter {
            limit: 50,
            user_id: Some(USER),
            model: Some("query-model".into()),
            api_format: Some("open_ai_chat_completions".into()),
            api_operation: Some("chat_completion".into()),
            outcome: Some("succeeded".into()),
            started_after: Some(timestamp("2026-09-18T12:00:00.000000Z")),
            started_before: Some(timestamp("2026-09-18T12:00:00.000000Z")),
            billed: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        filtered.iter().map(|log| log.id).collect::<Vec<_>>(),
        vec![mine]
    );
    let unbilled = queries
        .request_logs
        .list_all(RequestLogFilter {
            limit: 50,
            billed: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        unbilled.iter().map(|log| log.id).collect::<Vec<_>>(),
        vec![unsettled]
    );

    // An administrator query for another user is available through `list_all`,
    // and every invalid filter is a typed rejection rather than a silent page.
    for filter in [
        RequestLogFilter {
            limit: 50,
            model: Some("   ".into()),
            ..Default::default()
        },
        RequestLogFilter {
            limit: 50,
            model: Some("x".repeat(301)),
            ..Default::default()
        },
        RequestLogFilter {
            limit: 50,
            api_format: Some("unknown".into()),
            ..Default::default()
        },
        RequestLogFilter {
            limit: 50,
            api_operation: Some("unknown".into()),
            ..Default::default()
        },
        RequestLogFilter {
            limit: 50,
            outcome: Some("unknown".into()),
            ..Default::default()
        },
        RequestLogFilter {
            limit: 50,
            started_after: Some(timestamp("2026-09-19T00:00:00.000000Z")),
            started_before: Some(timestamp("2026-09-18T00:00:00.000000Z")),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            queries.request_logs.list_all(filter).await,
            Err(ai_gateway::persistence::RepositoryError::Validation)
        ));
    }
    queries.finish().await;
}

#[tokio::test]
async fn channel_group_status_matches_postgres_buckets_windows_and_metrics() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let now = Utc::now();
    let bucket_seconds = 30 * 60;
    // 48 contiguous 30-minute buckets ending in the current bucket.
    let current = now.timestamp().div_euclid(bucket_seconds) * bucket_seconds;
    let window_start = current - 47 * bucket_seconds;
    let stamp = |offset: i64| {
        DateTime::from_timestamp(current + offset, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    };
    let mut facts = Vec::new();
    for (index, (offset, outcome, ttft, tps, cost)) in [
        (0, "succeeded", Some(100), Some("2"), Some("1")),
        (0, "succeeded", Some(300), Some("4"), Some("1")),
        (0, "failed", None, None, Some("0")),
        (0, "cancelled", None, None, Some("0")),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::new_v4();
        let fact = Fact {
            id,
            outcome,
            ttft_ms: ttft,
            tps,
            cost_amount: cost,
            started_at: Box::leak(stamp(offset).into_boxed_str()),
            ..Fact::succeeded(id, "2026-09-18T12:00:00.000000Z", "0")
        };
        let _ = index;
        insert_fact(&queries, &fact).await;
        insert_log(&queries, &fact).await;
        facts.push(id);
    }
    // A second bucket inside the window so history has two entries.
    let second = Uuid::new_v4();
    let fact = Fact {
        id: second,
        started_at: Box::leak(stamp(-bucket_seconds).into_boxed_str()),
        ..Fact::succeeded(second, "2026-09-18T12:00:00.000000Z", "1")
    };
    insert_fact(&queries, &fact).await;
    insert_log(&queries, &fact).await;
    // An out-of-window fact is excluded.
    let old = Uuid::new_v4();
    let fact = Fact {
        id: old,
        started_at: "2020-01-01T00:00:00.000000Z",
        ..Fact::succeeded(old, "2020-01-01T00:00:00.000000Z", "1")
    };
    insert_fact(&queries, &fact).await;
    insert_log(&queries, &fact).await;
    // A group with statistics disabled never appears.
    let disabled = Uuid::new_v4();
    let fact = Fact {
        id: disabled,
        api_format: "open_ai_responses",
        channel_id: Some(OTHER_CHANNEL),
        channel_group_id: Some(OTHER_GROUP),
        model: "other-model",
        ..Fact::succeeded(disabled, "2026-09-18T12:00:00.000000Z", "1")
    };
    insert_fact(&queries, &fact).await;
    insert_log(&queries, &fact).await;

    let report = queries
        .request_logs
        .channel_group_status(ChannelGroupStatusWindow::Last24Hours)
        .await
        .unwrap();
    assert_eq!(report.window, "24h");
    assert_eq!(report.bucket_seconds, bucket_seconds);
    assert_eq!(report.started_at.timestamp(), window_start);
    assert!(report.ended_at >= report.started_at);

    // Idle configured models are seeded, and the disabled group is absent.
    let models = report
        .models
        .iter()
        .map(|metric| (metric.model.as_str(), metric.request_count))
        .collect::<Vec<_>>();
    assert_eq!(models, vec![("idle-model", 0), ("query-model", 5)]);
    let idle = report
        .models
        .iter()
        .find(|metric| metric.model == "idle-model")
        .unwrap();
    assert_eq!(idle.success_rate, None);
    assert_eq!(idle.p90_ttft_ms, None);
    assert_eq!(idle.p50_tps, None);

    let overall = report
        .models
        .iter()
        .find(|metric| metric.model == "query-model")
        .unwrap();
    // 5 requests: 3 succeeded, 1 failed, 1 cancelled; the cancelled request is
    // excluded from the success-rate denominator.
    assert_eq!(overall.request_count, 5);
    assert_eq!(overall.success_rate, Some(3.0 / 4.0));
    // Succeeded TTFT samples are {100, 100, 300}: percentile_cont(0.9) interpolates.
    assert_eq!(overall.p90_ttft_ms, Some(100.0 + (300.0 - 100.0) * 0.8));
    // Succeeded TPS samples are {2, 2, 4}; the median is 2.
    assert_eq!(overall.p50_tps, Some(2.0));

    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.id, GROUP);
    assert_eq!(group.name, "Query group");
    assert!(group.enabled);
    let group_models = &group.models;
    assert_eq!(
        group_models
            .iter()
            .map(|metric| metric.model.as_str())
            .collect::<Vec<_>>(),
        vec!["idle-model", "query-model"]
    );
    let history = &group_models
        .iter()
        .find(|metric| metric.model == "query-model")
        .unwrap()
        .history;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].started_at.timestamp(), current - bucket_seconds);
    assert_eq!(history[1].started_at.timestamp(), current);
    assert_eq!(history[0].request_count, 1);
    assert_eq!(history[1].request_count, 4);
    // The current bucket holds 2 succeeded, 1 failed, 1 cancelled.
    assert_eq!(history[1].success_rate, Some(2.0 / 3.0));
    assert_eq!(history[1].p90_ttft_ms, Some(100.0 + (300.0 - 100.0) * 0.9));
    assert_eq!(history[1].p50_tps, Some(3.0));
    let _ = facts;

    // Window bucket widths and counts match the documented table.
    for (window, bucket_seconds, bucket_count) in [
        (ChannelGroupStatusWindow::Last3Days, 2 * 60 * 60, 36),
        (ChannelGroupStatusWindow::Last7Days, 4 * 60 * 60, 42),
    ] {
        let report = queries
            .request_logs
            .channel_group_status(window)
            .await
            .unwrap();
        assert_eq!(report.bucket_seconds, bucket_seconds);
        let expected_start = report.ended_at.timestamp().div_euclid(bucket_seconds)
            * bucket_seconds
            - (bucket_count - 1) * bucket_seconds;
        assert_eq!(report.started_at.timestamp(), expected_start);
    }
    queries.finish().await;
}

#[tokio::test]
async fn personal_usage_reads_facts_with_client_only_utc_days() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let ended_on = NaiveDate::from_ymd_opt(2026, 9, 18).unwrap();
    // The scheduled-test fact belongs to another user so the owner filter is
    // proven independently of the source filter.
    for (offset, user_id, source) in [
        (-400_i64, USER, "client"),
        (-1, USER, "client"),
        (0, USER, "client"),
        (0, OTHER_USER, "scheduled_test"),
    ] {
        let id = Uuid::new_v4();
        let started_at = timestamp("2026-09-18T12:00:00.000000Z") + ChronoDuration::days(offset);
        let fact = Fact {
            id,
            user_id,
            request_source: source,
            started_at: Box::leak(
                started_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
                    .into_boxed_str(),
            ),
            ..Fact::succeeded(id, "2026-09-18T12:00:00.000000Z", "1")
        };
        insert_fact(&queries, &fact).await;
    }
    let report = queries
        .metering
        .personal_usage(USER, ended_on)
        .await
        .unwrap();
    assert_eq!(
        report.started_on,
        NaiveDate::from_ymd_opt(2025, 9, 19).unwrap()
    );
    assert_eq!(report.ended_on, ended_on);
    assert_eq!(report.days.len(), 365);
    assert_eq!(report.days[0].date, report.started_on);
    assert_eq!(report.days[364].date, ended_on);
    // Only the -1 and 0 offsets fall inside the fixed 365-day window; the -400
    // fact and the other user's scheduled test are excluded.
    assert_eq!(report.days[364].request_count, 1);
    assert_eq!(report.days[363].request_count, 1);
    assert_eq!(report.days[0].request_count, 0);
    assert_eq!(report.total_request_count, 2);
    assert_eq!(report.active_day_count, 2);
    // The last two returned days are active, so the current streak is 2.
    assert_eq!(report.current_streak_days, 2);
    assert_eq!(report.longest_streak_days, 2);
    queries.finish().await;
}

#[tokio::test]
async fn cost_statistics_aggregates_groups_filters_ranges_and_orders() {
    let queries = Queries::new().await;
    let codex_credential = seed(&queries).await;
    let started_at = timestamp("2026-09-18T00:00:00.000000Z");
    let ended_at = timestamp("2026-09-20T00:00:00.000000Z");
    // Two users, two formats, exact amounts that expose float drift if any.
    let events = [
        (
            USER,
            KEY,
            CHANNEL,
            GROUP,
            "open_ai_chat_completions",
            "query-model",
            "2026-09-18T01:00:00.000000Z",
            "0.1",
        ),
        (
            USER,
            KEY,
            CHANNEL,
            GROUP,
            "open_ai_chat_completions",
            "query-model",
            "2026-09-18T02:00:00.000000Z",
            "0.2",
        ),
        (
            OTHER_USER,
            OTHER_KEY,
            CHANNEL,
            GROUP,
            "open_ai_chat_completions",
            "wire-model",
            "2026-09-19T03:00:00.000000Z",
            "0.3",
        ),
        (
            USER,
            KEY,
            OTHER_CHANNEL,
            OTHER_GROUP,
            "open_ai_responses",
            "other-model",
            "2026-09-19T04:00:00.000000Z",
            "0.4",
        ),
    ];
    for (user_id, key, channel_id, group_id, api_format, model, started, cost) in events {
        let id = Uuid::new_v4();
        let fact = Fact {
            id,
            user_id,
            api_key_id: key,
            channel_id: Some(channel_id),
            channel_group_id: Some(group_id),
            api_format,
            model,
            started_at: started,
            ..Fact::succeeded(id, started, cost)
        };
        insert_fact(&queries, &fact).await;
        insert_log(&queries, &fact).await;
    }

    let report = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at,
            ended_at,
            granularity: StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: true,
        })
        .await
        .unwrap();
    assert_eq!(report.granularity, "day");
    assert_eq!(report.summary.request_count, 4);
    assert_eq!(report.summary.priced_request_count, 4);
    // `input_tokens + output_tokens` per request, cached tokens folded into input.
    assert_eq!(report.summary.total_tokens, 60);
    assert_eq!(report.summary.input_tokens, 40);
    assert_eq!(report.summary.output_tokens, 20);
    assert_eq!(
        report.summary.cost_amount,
        Decimal::from_str_exact("1").unwrap()
    );
    // Two contiguous UTC day buckets, empty ones included.
    assert_eq!(report.buckets.len(), 2);
    assert_eq!(report.buckets[0].started_at, started_at);
    assert_eq!(report.buckets[0].request_count, 2);
    assert_eq!(
        report.buckets[0].cost_amount,
        Decimal::from_str_exact("0.3").unwrap()
    );
    assert_eq!(report.buckets[0].models.len(), 1);
    assert_eq!(report.buckets[1].request_count, 2);
    assert_eq!(report.buckets[1].total_tokens, 30);
    assert_eq!(report.buckets[1].models.len(), 2);
    // Models order by api_format rank, then model text.
    assert_eq!(
        report
            .models
            .iter()
            .map(|model| (model.api_format.as_str(), model.model.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("open_ai_chat_completions", "query-model"),
            ("open_ai_chat_completions", "wire-model"),
            ("open_ai_responses", "other-model"),
        ]
    );
    assert_eq!(report.models[0].success_rate, Some(1.0));
    // Channels order by group name, channel name, then format rank.
    assert_eq!(
        report
            .channels
            .iter()
            .map(|channel| (channel.name.as_str(), channel.api_format.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("Other channel", "open_ai_responses"),
            ("Query channel", "open_ai_chat_completions"),
        ]
    );
    assert_eq!(report.channels[1].request_count, 3);
    assert_eq!(
        report.channels[1].cost_amount,
        Decimal::from_str_exact("0.6").unwrap()
    );

    // User, key and capability filters are independent of credential filtering.
    let by_user = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at,
            ended_at,
            granularity: StatisticsGranularity::Hour,
            user_id: Some(USER),
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: false,
        })
        .await
        .unwrap();
    assert_eq!(by_user.summary.request_count, 3);
    assert!(by_user.channels.is_empty());
    let by_key = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at,
            ended_at,
            granularity: StatisticsGranularity::Hour,
            user_id: None,
            api_key_id: Some(OTHER_KEY),
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: false,
        })
        .await
        .unwrap();
    assert_eq!(by_key.summary.request_count, 1);
    let by_credential = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at,
            ended_at,
            granularity: StatisticsGranularity::Hour,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: Some(codex_credential),
            include_channel_details: true,
        })
        .await
        .unwrap();
    assert_eq!(by_credential.summary.request_count, 0);
    assert!(by_credential.channels.is_empty());

    // Closed-open range, granularity bounds, and mutually exclusive filters.
    for filter in [
        CostStatisticsFilter {
            started_at: ended_at,
            ended_at: started_at,
            granularity: StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: false,
        },
        CostStatisticsFilter {
            started_at: timestamp("2020-01-01T00:00:00.000000Z"),
            ended_at: timestamp("2021-01-01T00:00:00.000000Z"),
            granularity: StatisticsGranularity::Hour,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: false,
        },
        CostStatisticsFilter {
            started_at,
            ended_at,
            granularity: StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: Some(CHANNEL),
            codex_credential_id: Some(codex_credential),
            include_channel_details: false,
        },
    ] {
        assert!(matches!(
            queries.metering.cost_statistics(filter).await,
            Err(ai_gateway::persistence::RepositoryError::Validation)
        ));
    }
    queries.finish().await;
}

#[tokio::test]
async fn channel_details_exclude_facts_without_resolvable_channel_attribution() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let id = Uuid::new_v4();
    let fact = Fact {
        id,
        channel_id: None,
        channel_group_id: None,
        ..Fact::succeeded(id, "2026-09-18T01:00:00.000000Z", "1")
    };
    insert_fact(&queries, &fact).await;
    let report = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at: timestamp("2026-09-18T00:00:00.000000Z"),
            ended_at: timestamp("2026-09-19T00:00:00.000000Z"),
            granularity: StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: true,
        })
        .await
        .unwrap();
    assert_eq!(report.summary.request_count, 1);
    assert!(report.channels.is_empty());
    assert_eq!(report.models.len(), 1);
    queries.finish().await;
}

#[tokio::test]
async fn cost_statistics_preserve_null_cost_and_token_columns() {
    let queries = Queries::new().await;
    seed(&queries).await;
    // An unknown-cost fact: no cost and no usage.
    let unknown = Uuid::new_v4();
    insert_fact(
        &queries,
        &Fact {
            id: unknown,
            input_tokens: None,
            output_tokens: None,
            cost_amount: None,
            ttft_ms: None,
            tps: None,
            ..Fact::succeeded(unknown, "2026-09-18T01:00:00.000000Z", "0")
        },
    )
    .await;
    let report = queries
        .metering
        .cost_statistics(CostStatisticsFilter {
            started_at: timestamp("2026-09-18T00:00:00.000000Z"),
            ended_at: timestamp("2026-09-19T00:00:00.000000Z"),
            granularity: StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: false,
        })
        .await
        .unwrap();
    assert_eq!(report.summary.request_count, 1);
    assert_eq!(report.summary.priced_request_count, 0);
    assert_eq!(report.summary.total_tokens, 0);
    assert_eq!(report.summary.cost_amount, Decimal::ZERO);
    assert_eq!(report.buckets[0].cost_amount, Decimal::ZERO);
    queries.finish().await;
}

#[tokio::test]
async fn spend_leaderboard_refresh_writes_shanghai_snapshots_and_reads_bounded_pages() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let admin_created = queries
        .scalar::<String>(&format!("SELECT created_at FROM users WHERE id='{ADMIN}'"))
        .await;
    assert!(!admin_created.is_empty());
    queries
        .execute(&format!(
            "INSERT INTO users(id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES ('{}','ranked@example.test','Ranked user','user','active','{PASSWORD_HASH}',ag_now(),'{DEFAULT_USER_GROUP_ID}')",
            Uuid::from_u128(0x910)
        ))
        .await;
    let ranked = Uuid::from_u128(0x910);
    // Boundary facts: the UTC instant 16:00 is Shanghai midnight, so the facts
    // before and after it land in different Shanghai days.
    let before_midnight = timestamp("2026-01-01T15:59:59.000000Z");
    let after_midnight = timestamp("2026-01-01T16:00:00.000000Z");
    let mut facts = Vec::new();
    for (user_id, key, started_at, cost) in [
        (USER, KEY, before_midnight, "0.25"),
        (USER, KEY, after_midnight, "0.75"),
        (ranked, KEY, before_midnight, "0.1"),
    ] {
        let id = Uuid::new_v4();
        let fact = Fact {
            id,
            user_id,
            api_key_id: key,
            started_at: Box::leak(
                started_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
                    .into_boxed_str(),
            ),
            ..Fact::succeeded(id, "2026-01-01T12:00:00.000000Z", cost)
        };
        insert_fact(&queries, &fact).await;
        facts.push(id);
    }
    // A scheduled-test fact never enters the leaderboard.
    let scheduled = Uuid::new_v4();
    insert_fact(
        &queries,
        &Fact {
            id: scheduled,
            request_source: "scheduled_test",
            started_at: "2026-01-01T16:30:00.000000Z",
            ..Fact::succeeded(scheduled, "2026-01-01T16:30:00.000000Z", "9")
        },
    )
    .await;
    // An unpriced fact contributes to a ranked user's totals but cannot put a
    // user on the board.
    let unpriced = Uuid::new_v4();
    insert_fact(
        &queries,
        &Fact {
            id: unpriced,
            user_id: ranked,
            started_at: "2026-01-01T16:30:00.000000Z",
            cost_amount: None,
            input_tokens: Some(10),
            output_tokens: Some(5),
            ..Fact::succeeded(unpriced, "2026-01-01T16:30:00.000000Z", "0")
        },
    )
    .await;

    assert_eq!(
        queries
            .metering
            .refresh_spend_leaderboard_snapshots()
            .await
            .unwrap(),
        SpendLeaderboardRefresh::Updated
    );
    // The refreshed current period always exists, even when it has no priced
    // requests, so Console can render an empty period.
    let today = SpendLeaderboardPeriod::Day.current_start_at(Utc::now());
    let total: String = queries
        .scalar(&format!(
            "SELECT total_cost_amount FROM spend_leaderboard_periods              WHERE period='day' AND period_start='{today}'"
        ))
        .await;
    assert_eq!(total, "0");

    let first_day = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        first_day.period_end,
        NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()
    );
    assert_eq!(
        first_day.total_cost_amount,
        Decimal::from_str_exact("0.35000000").unwrap()
    );
    assert_eq!(
        first_day
            .entries
            .iter()
            .map(|entry| (entry.rank, entry.display_name.as_str(), entry.cost_amount))
            .collect::<Vec<_>>(),
        vec![
            (1, "Query user", Decimal::from_str_exact("0.25").unwrap()),
            (2, "Ranked user", Decimal::from_str_exact("0.1").unwrap()),
        ]
    );
    // Both users have priced requests before Shanghai midnight; history
    // navigation exposes the immediately previous and next retained period.
    assert_eq!(first_day.previous_period_start, None);
    assert_eq!(
        first_day.next_period_start,
        Some(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap())
    );

    let next_day = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        next_day.total_cost_amount,
        Decimal::from_str_exact("0.75").unwrap()
    );
    assert_eq!(next_day.entries.len(), 1);
    assert_eq!(next_day.entries[0].user_id, USER);
    // The unpriced fact contributes to the ranked user's totals on the previous
    // day but does not appear here.
    let ranked_day = first_day
        .entries
        .iter()
        .find(|entry| entry.user_id == ranked)
        .unwrap();
    assert_eq!(ranked_day.priced_request_count, 1);
    assert_eq!(ranked_day.request_count, 1);
    assert_eq!(ranked_day.total_tokens, 15);

    // `limit` is bounded and week/month starts are validated.
    let limited = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(limited.entries.len(), 1);
    assert_eq!(
        limited.total_cost_amount,
        Decimal::from_str_exact("0.35000000").unwrap(),
        "the snapshot total is not truncated by limit"
    );
    for filter in [
        SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 0,
        },
        SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 101,
        },
        SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Week,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 50,
        },
        SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Month,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            limit: 50,
        },
    ] {
        assert!(matches!(
            queries.metering.spend_leaderboard(filter).await,
            Err(ai_gateway::persistence::RepositoryError::Validation)
        ));
    }
    // Week and month snapshots bucket the same facts by Shanghai boundaries.
    let week = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Week,
            period_start: NaiveDate::from_ymd_opt(2025, 12, 29).unwrap(),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        week.total_cost_amount,
        Decimal::from_str_exact("1.1").unwrap()
    );
    let month = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Month,
            period_start: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        month.total_cost_amount,
        Decimal::from_str_exact("1.1").unwrap()
    );
    let _ = facts;

    // A second refresh is idempotent and keeps the previous snapshot on rows
    // whose numbers did not change.
    assert_eq!(
        queries
            .metering
            .refresh_spend_leaderboard_snapshots()
            .await
            .unwrap(),
        SpendLeaderboardRefresh::Updated
    );
    queries.finish().await;
}

#[tokio::test]
async fn sharing_completed_costs_reads_only_eligible_facts_and_bounds_the_batch() {
    let queries = Queries::new().await;
    seed(&queries).await;
    let priced = Uuid::new_v4();
    let zero = Uuid::new_v4();
    let unknown = Uuid::new_v4();
    let rejected = Uuid::new_v4();
    insert_fact(
        &queries,
        &Fact::succeeded(priced, "2026-09-18T01:00:00.000000Z", "1.25"),
    )
    .await;
    insert_fact(
        &queries,
        &Fact {
            id: zero,
            outcome: "failed",
            cost_amount: Some("0"),
            input_tokens: None,
            output_tokens: None,
            ttft_ms: None,
            tps: None,
            ..Fact::succeeded(zero, "2026-09-18T01:00:00.000000Z", "0")
        },
    )
    .await;
    insert_fact(
        &queries,
        &Fact {
            id: unknown,
            cost_amount: None,
            input_tokens: None,
            output_tokens: None,
            ttft_ms: None,
            tps: None,
            ..Fact::succeeded(unknown, "2026-09-18T01:00:00.000000Z", "0")
        },
    )
    .await;
    insert_fact(
        &queries,
        &Fact {
            id: rejected,
            outcome: "rejected",
            cost_amount: None,
            input_tokens: None,
            output_tokens: None,
            ttft_ms: None,
            tps: None,
            ..Fact::succeeded(rejected, "2026-09-18T01:00:00.000000Z", "0")
        },
    )
    .await;

    let costs = queries
        .metering
        .sharing_completed_costs(&[priced, zero, unknown, rejected, Uuid::new_v4()])
        .await
        .unwrap();
    let mut costs = costs;
    costs.sort_by_key(|(id, _)| *id);
    let mut expected = vec![
        (priced, Decimal::from_str_exact("1.25").unwrap()),
        (zero, Decimal::ZERO),
    ];
    expected.sort_by_key(|(id, _)| *id);
    assert_eq!(costs, expected);
    // The read is independent of the log projection and the receipt.
    let projected: i64 = queries.scalar("SELECT count(*) FROM request_logs").await;
    assert_eq!(projected, 0);
    let receipts: i64 = queries
        .scalar("SELECT count(*) FROM request_settlements")
        .await;
    assert_eq!(receipts, 0);

    // The batch bound is a typed rejection, not a truncated answer.
    let oversized = (0..1001).map(Uuid::from_u128).collect::<Vec<_>>();
    assert!(matches!(
        queries.metering.sharing_completed_costs(&oversized).await,
        Err(ai_gateway::persistence::RepositoryError::Validation)
    ));
    assert!(
        queries
            .metering
            .sharing_completed_costs(&[])
            .await
            .unwrap()
            .is_empty()
    );
    queries.finish().await;
}

#[tokio::test]
async fn metering_facts_are_immutable_and_leaderboard_tables_are_snapshot_only() {
    let queries = Queries::new().await;
    seed(&queries).await;
    assert_eq!(
        queries
            .metering
            .refresh_spend_leaderboard_snapshots()
            .await
            .unwrap(),
        SpendLeaderboardRefresh::Updated
    );
    // A refresh without facts still records every current period with a zero
    // total, and snapshot rows remain readable.
    let periods: i64 = queries
        .scalar("SELECT count(*) FROM spend_leaderboard_periods")
        .await;
    assert_eq!(periods, 3);
    let day = queries
        .metering
        .spend_leaderboard(SpendLeaderboardFilter {
            period: SpendLeaderboardPeriod::Day,
            period_start: SpendLeaderboardPeriod::Day.current_start_at(Utc::now()),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(day.total_cost_amount, Decimal::ZERO);
    assert!(day.entries.is_empty());
    assert!(day.refreshed_at.is_some());
    // Financial evidence cannot be updated or deleted through any query path.
    let id = Uuid::new_v4();
    insert_fact(
        &queries,
        &Fact::succeeded(id, "2026-09-18T01:00:00.000000Z", "1"),
    )
    .await;
    for (sql, guard) in [
        (
            "UPDATE request_metering_facts SET cost_amount='0'",
            "request_metering_facts_immutable_update",
        ),
        (
            "DELETE FROM request_metering_facts",
            "request_metering_facts_immutable_delete",
        ),
    ] {
        let mut transaction = queries.database.begin_write().await.unwrap();
        let error = sqlx::Executor::execute(&mut *transaction, sql)
            .await
            .expect_err("financial facts are append-only");
        assert!(error.to_string().contains(guard), "{guard}: {error}");
        transaction.rollback().await.unwrap();
    }
    queries.finish().await;
}
