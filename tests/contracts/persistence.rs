use super::*;
use ai_gateway::persistence::RepositoryError;
use rust_decimal::Decimal;
use serde_json::json;

fn driver_error(error: &ai_gateway::persistence::StorageError) -> &sqlx::Error {
    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref())
        .expect("PG integration diagnostics retain the driver error")
}

async fn account_state(pool: &PgPool, ids: &[Uuid]) -> Vec<(Uuid, Decimal, Decimal)> {
    sqlx::query_as(
        "SELECT key.id, account.balance_amount, key.quota_used_amount
         FROM api_keys AS key JOIN users AS account ON account.id = key.user_id
         WHERE key.id = ANY($1) ORDER BY key.id",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn log_state(pool: &PgPool, id: Uuid) -> (Option<Decimal>, Option<DateTime<Utc>>) {
    sqlx::query_as(
        "SELECT cost_amount,
        (SELECT settled_at FROM request_settlements WHERE request_id=fact.id) AS billed_at
        FROM request_metering_facts AS fact WHERE id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_raw_variant(
    pool: &PgPool,
    source: Uuid,
    changes: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO request_logs
         SELECT (jsonb_populate_record(NULL::request_logs, to_jsonb(log) || $2)).*
         FROM request_logs AS log WHERE id=$1",
    )
    .bind(source)
    .bind(changes)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn unknown_rejected_and_zero_by_policy_remain_distinct_during_recovery() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let mut unknown = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let billing = unknown.billing.as_mut().unwrap();
    billing.usage = None;
    billing.cost_amount = None;
    billing.output_tokens_per_second = None;
    let mut rejected = request_log_event(&seed, RequestLogOutcome::Rejected);
    rejected.billing = None;
    rejected.model_id = None;
    let mut failed = request_log_event(&seed, RequestLogOutcome::Failed);
    failed.billing = None;
    failed.model_id = None;
    let mut cancelled = request_log_event(&seed, RequestLogOutcome::Cancelled);
    cancelled.billing.as_mut().unwrap().usage = None;
    let events = [unknown, rejected, failed, cancelled];
    let before = account_state(&database.pool, &[seed.key]).await;
    repository.insert_batch(&events).await.unwrap();

    let costs = ai_gateway::persistence::MeteringQueries::new(database.pool.clone())
        .sharing_completed_costs(&events.iter().map(|event| event.id).collect::<Vec<_>>())
        .await
        .unwrap()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert_eq!(costs.len(), 2);
    assert_eq!(costs.get(&events[2].id), Some(&Decimal::ZERO));
    assert_eq!(costs.get(&events[3].id), Some(&Decimal::ZERO));
    assert_eq!(account_state(&database.pool, &[seed.key]).await, before);
    for event in &events[..2] {
        assert_eq!(
            repository.settlements().settle(event.id).await.unwrap(),
            RequestLogSettlementOutcome::NotBillable
        );
        assert_eq!(log_state(&database.pool, event.id).await, (None, None));
    }
    let outcomes = repository.settlements().settle_pending(10).await.unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
    );
    for event in &events[2..] {
        let (cost, billed) = log_state(&database.pool, event.id).await;
        assert_eq!(cost, Some(Decimal::ZERO));
        assert!(billed.is_some());
        assert_eq!(
            repository.settlements().settle(event.id).await.unwrap(),
            RequestLogSettlementOutcome::AlreadyBilled
        );
    }
    assert!(
        repository
            .settlements()
            .settle_pending(10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(account_state(&database.pool, &[seed.key]).await, before);
    database.cleanup().await;
}

#[tokio::test]
async fn missing_price_evidence_is_preserved_without_blocking_eligible_facts() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let valid = request_log_event(&seed, RequestLogOutcome::Succeeded);
    repository.insert(&valid).await.unwrap();
    let invalid = Uuid::new_v4();
    insert_raw_variant(
        &database.pool,
        valid.id,
        json!({
            "id": invalid, "model_id": null, "currency": null, "price_unit_tokens": null,
            "price_effective_at": null, "input_unit_price": null, "cached_input_unit_price": null,
            "cache_write_unit_price": null, "output_unit_price": null
        }),
    )
    .await
    .unwrap();
    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let before = account_state(&database.pool, &[seed.key]).await;
    let outcomes = repository
        .settlements()
        .settle_batch(&[valid.id, invalid])
        .await
        .unwrap();
    assert!(matches!(
        outcomes[0].1,
        RequestLogSettlementOutcome::Settled { .. }
    ));
    assert_eq!(outcomes[1].1, RequestLogSettlementOutcome::NotBillable);
    assert!(log_state(&database.pool, valid.id).await.1.is_some());
    assert!(log_state(&database.pool, invalid).await.1.is_none());
    let cost = valid.effective_cost_amount().unwrap();
    assert_eq!(
        account_state(&database.pool, &[seed.key]).await,
        vec![(seed.key, before[0].1 - cost, before[0].2 + cost)]
    );
    assert!(
        repository
            .settlements()
            .settle_pending(10)
            .await
            .unwrap()
            .is_empty()
    );
    database.cleanup().await;
}

#[tokio::test]
async fn settlement_rolls_back_claims_and_all_accounts_on_overflow_or_missing_updates() {
    for fault in [
        "balance_overflow",
        "quota_overflow",
        "skip_user",
        "skip_key",
    ] {
        let database = TestDatabase::new().await;
        let seed = seed(&database.pool).await;
        let repository = RequestLogRepository::new(database.pool.clone());
        let other_user = Uuid::new_v4();
        let other_key = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users(id,display_name,role,status) VALUES($1,'other','user','active')",
        )
        .bind(other_user)
        .execute(&database.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
             VALUES($1,$2,'other',$3,'active',ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'])",
        )
        .bind(other_key)
        .bind(other_user)
        .bind(format!("test-{other_key}"))
        .execute(&database.pool)
        .await
        .unwrap();
        let mut first = request_log_event(&seed, RequestLogOutcome::Succeeded);
        first.billing.as_mut().unwrap().cost_amount = Some(Decimal::ONE);
        let mut second = first.clone();
        second.id = Uuid::new_v4();
        second.user_id = other_user;
        second.api_key_id = other_key;
        repository
            .insert_batch(&[first.clone(), second.clone()])
            .await
            .unwrap();

        match fault {
            "balance_overflow" => {
                sqlx::query(
                    "UPDATE users SET balance_amount=-9999999999999999.99999999 WHERE id=$1",
                )
                .bind(seed.user)
                .execute(&database.pool)
                .await
                .unwrap();
            }
            "quota_overflow" => {
                sqlx::query(
                    "UPDATE api_keys SET quota_used_amount=9999999999999999.99999999 WHERE id=$1",
                )
                .bind(seed.key)
                .execute(&database.pool)
                .await
                .unwrap();
            }
            _ => {
                let (table, id) = if fault == "skip_user" {
                    ("users", seed.user)
                } else {
                    ("api_keys", seed.key)
                };
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                    "CREATE FUNCTION contract_skip_update() RETURNS trigger LANGUAGE plpgsql AS $$
                     BEGIN IF NEW.id = '{id}'::uuid THEN RETURN NULL; END IF; RETURN NEW; END $$;
                     CREATE TRIGGER contract_skip_update BEFORE UPDATE ON {table}
                     FOR EACH ROW EXECUTE FUNCTION contract_skip_update();"
                )))
                .execute(&database.pool)
                .await
                .unwrap();
            }
        }
        let before = account_state(&database.pool, &[seed.key, other_key]).await;
        let error = repository
            .settlements()
            .settle_batch(&[first.id, second.id])
            .await
            .unwrap_err();
        if fault.ends_with("overflow") {
            let RepositoryError::Storage(error) = error else {
                panic!("{fault}: expected numeric overflow, got {error:?}");
            };
            assert_eq!(
                driver_error(&error)
                    .as_database_error()
                    .unwrap()
                    .code()
                    .as_deref(),
                Some("22003")
            );
        } else {
            assert!(matches!(
                error,
                RepositoryError::SettlementClaimInvalidated { .. }
            ));
        }
        assert_eq!(
            account_state(&database.pool, &[seed.key, other_key]).await,
            before,
            "{fault}"
        );
        for id in [first.id, second.id] {
            assert!(log_state(&database.pool, id).await.1.is_none(), "{fault}");
        }

        sqlx::raw_sql(
            "DROP FUNCTION IF EXISTS contract_skip_update() CASCADE;
             UPDATE users SET balance_amount=0;
             UPDATE api_keys SET quota_used_amount=0;",
        )
        .execute(&database.pool)
        .await
        .unwrap();
        let recovered = repository.settlements().settle_pending(10).await.unwrap();
        assert_eq!(recovered.len(), 2);
        assert!(
            recovered
                .iter()
                .all(|outcome| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
        );
        for (_, balance, used) in account_state(&database.pool, &[seed.key, other_key]).await {
            assert_eq!(balance, -Decimal::ONE);
            assert_eq!(used, Decimal::ONE);
        }
        assert!(
            repository
                .settlements()
                .settle_pending(10)
                .await
                .unwrap()
                .is_empty()
        );
        database.cleanup().await;
    }
}

#[tokio::test]
async fn maximum_amount_is_exact_and_aggregate_overflow_never_partially_settles() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let maximum: Decimal = "9999999999999999.99999999".parse().unwrap();
    let mut first = request_log_event(&seed, RequestLogOutcome::Succeeded);
    first.billing.as_mut().unwrap().cost_amount = Some(maximum);
    let mut second = request_log_event(&seed, RequestLogOutcome::Succeeded);
    second.billing.as_mut().unwrap().cost_amount = Some(Decimal::new(1, 8));
    repository
        .insert_batch(&[first.clone(), second.clone()])
        .await
        .unwrap();
    let before = account_state(&database.pool, &[seed.key]).await;
    let error = repository
        .settlements()
        .settle_batch(&[first.id, second.id])
        .await
        .unwrap_err();
    let RepositoryError::Storage(error) = error else {
        panic!("expected aggregate overflow, got {error:?}");
    };
    assert_eq!(
        driver_error(&error)
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("22003")
    );
    assert_eq!(account_state(&database.pool, &[seed.key]).await, before);
    assert_eq!(
        log_state(&database.pool, first.id).await,
        (Some(maximum), None)
    );
    assert_eq!(
        log_state(&database.pool, second.id).await,
        (Some(Decimal::new(1, 8)), None)
    );

    assert!(matches!(
        repository.settlements().settle(first.id).await.unwrap(),
        RequestLogSettlementOutcome::Settled { .. }
    ));
    let settled = account_state(&database.pool, &[seed.key]).await;
    assert_eq!(settled, vec![(seed.key, -maximum, maximum)]);
    let first_state = log_state(&database.pool, first.id).await;
    assert!(first_state.1.is_some());
    assert!(repository.settlements().settle(second.id).await.is_err());
    assert_eq!(account_state(&database.pool, &[seed.key]).await, settled);
    assert_eq!(log_state(&database.pool, first.id).await, first_state);
    assert!(log_state(&database.pool, second.id).await.1.is_none());
    database.cleanup().await;
}

#[tokio::test]
async fn database_constraints_reject_invalid_money_and_illegal_log_updates() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    repository.insert(&event).await.unwrap();
    for mut changes in [
        json!({"outcome": "failed", "cost_amount": "1"}),
        json!({"outcome": "cancelled", "cost_amount": "1"}),
        json!({"currency": null}),
        json!({"currency": "EUR"}),
        json!({"price_unit_tokens": 0}),
        json!({"cost_amount": "-0.00000001"}),
        json!({"reasoning_tokens": 5}),
    ] {
        changes["id"] = json!(Uuid::new_v4());
        let error = insert_raw_variant(&database.pool, event.id, changes.clone())
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514"),
            "{changes}"
        );
    }
    let error = sqlx::query("UPDATE request_logs SET cost_amount=1 WHERE id=$1")
        .bind(event.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("P0001")
    );
    repository.settlements().settle(event.id).await.unwrap();
    let before = log_state(&database.pool, event.id).await;
    let error = sqlx::query("UPDATE request_settlements SET settled_at=NULL WHERE request_id=$1")
        .bind(event.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("P0001")
    );
    assert_eq!(log_state(&database.pool, event.id).await, before);
    database.cleanup().await;
}

#[tokio::test]
async fn recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let mut client = request_log_event(&seed, RequestLogOutcome::Succeeded);
    client.started_at = "2026-09-15T00:00:00Z".parse().unwrap();
    client.completed_at = client.started_at;
    let mut probe = request_log_event(&seed, RequestLogOutcome::Succeeded);
    probe.request_source = RequestLogSource::ScheduledTest;
    probe.started_at = "2026-09-16T00:00:00Z".parse().unwrap();
    probe.completed_at = probe.started_at;
    let cost = client.billing.as_ref().unwrap().cost_amount.unwrap();
    repository
        .insert_batch(&[probe.clone(), client.clone()])
        .await
        .unwrap();
    sqlx::query("UPDATE models SET input_unit_price=999,output_unit_price=999 WHERE id=$1")
        .bind(seed.model)
        .execute(&database.pool)
        .await
        .unwrap();
    for event in [&probe, &client] {
        assert_eq!(
            repository.insert(event).await.unwrap(),
            RequestLogInsertOutcome::ExactDuplicate
        );
    }
    let first = repository.settlements().settle_pending(0).await.unwrap();
    assert_eq!(first.len(), 1);
    assert!(
        matches!(first[0], RequestLogSettlementOutcome::Settled { request_log_id, .. } if request_log_id == client.id)
    );
    assert!(log_state(&database.pool, probe.id).await.1.is_none());
    let second = repository.settlements().settle_pending(1).await.unwrap();
    assert_eq!(second.len(), 1);
    assert!(
        matches!(second[0], RequestLogSettlementOutcome::Settled { request_log_id, .. } if request_log_id == probe.id)
    );
    for event in [&client, &probe] {
        assert_eq!(log_state(&database.pool, event.id).await.0, Some(cost));
        let price: Decimal =
            sqlx::query_scalar("SELECT input_unit_price FROM request_logs WHERE id=$1")
                .bind(event.id)
                .fetch_one(&database.pool)
                .await
                .unwrap();
        assert_eq!(
            price,
            event.billing.as_ref().unwrap().price.input_unit_price
        );
    }
    assert_eq!(
        account_state(&database.pool, &[seed.key]).await,
        vec![(
            seed.key,
            -(cost * Decimal::from(2)),
            cost * Decimal::from(2)
        )]
    );
    assert!(
        repository
            .settlements()
            .settle_pending(1)
            .await
            .unwrap()
            .is_empty()
    );
    database.cleanup().await;
}
