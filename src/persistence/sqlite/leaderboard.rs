//! File-backed staging keeps rebuild memory bounded independently of historical fact cardinality.

use chrono::NaiveDate;
use sqlx::{Executor, FromRow, Sqlite, Transaction};
use uuid::Uuid;

use super::{
    SqliteAmount, SqliteDatabase, SqliteDate, SqliteTimestamp, SqliteUuid, aggregate::CostSum,
};
use crate::persistence::{RepositoryError, SpendLeaderboardPeriod, SpendLeaderboardRefresh};

const PAGE: i64 = 512;
type Key = (String, String, String);

pub(super) async fn refresh(
    database: &SqliteDatabase,
) -> Result<SpendLeaderboardRefresh, RepositoryError> {
    let Ok(_guard) = database.leaderboard_refresh.try_lock() else {
        return Ok(SpendLeaderboardRefresh::AlreadyRunning);
    };
    let mut tx = database
        .begin_write()
        .await
        .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;
    let now = sqlx::query_scalar::<_, SqliteTimestamp>("SELECT ag_now()")
        .fetch_one(&mut *tx)
        .await?
        .0;
    // Trim subseconds before SQLite date math can round across a Shanghai day boundary.
    (&mut *tx).execute(
        "CREATE TEMP TABLE IF NOT EXISTS leaderboard_input (
           period TEXT NOT NULL, period_start TEXT NOT NULL, user_id TEXT NOT NULL,
           id TEXT NOT NULL, cost_amount TEXT, input_tokens INTEGER, output_tokens INTEGER,
           PRIMARY KEY(period,period_start,user_id,id)) WITHOUT ROWID, STRICT;
         CREATE TEMP TABLE IF NOT EXISTS leaderboard_totals (
           period TEXT NOT NULL, period_start TEXT NOT NULL, user_id TEXT NOT NULL,
           request_count INTEGER NOT NULL, priced_request_count INTEGER NOT NULL,
           total_tokens INTEGER NOT NULL, cost_amount TEXT NOT NULL,
           PRIMARY KEY(period,period_start,user_id)) WITHOUT ROWID, STRICT;
         DELETE FROM leaderboard_input;
         DELETE FROM leaderboard_totals;
         INSERT INTO leaderboard_input
           SELECT 'day',date(substr(started_at,1,19)||'Z','+8 hours'),user_id,id,cost_amount,input_tokens,output_tokens
           FROM request_metering_facts WHERE request_source='client'
           UNION ALL
           SELECT 'week',date(substr(started_at,1,19)||'Z','+8 hours',
             '-' || ((CAST(strftime('%w',substr(started_at,1,19)||'Z','+8 hours') AS INTEGER)+6)%7) || ' days'),
             user_id,id,cost_amount,input_tokens,output_tokens
           FROM request_metering_facts WHERE request_source='client'
           UNION ALL
           SELECT 'month',date(substr(started_at,1,19)||'Z','+8 hours','start of month'),
             user_id,id,cost_amount,input_tokens,output_tokens
           FROM request_metering_facts WHERE request_source='client';"
    ).await?;

    let mut cursor = (String::new(), String::new(), String::new(), String::new());
    let mut current: Option<(Key, Entry)> = None;
    loop {
        let rows = sqlx::query_as::<_, Input>(
            "SELECT * FROM leaderboard_input
             WHERE (period,period_start,user_id,id)>(?1,?2,?3,?4)
             ORDER BY period,period_start,user_id,id LIMIT ?5",
        )
        .bind(&cursor.0)
        .bind(&cursor.1)
        .bind(&cursor.2)
        .bind(&cursor.3)
        .bind(PAGE)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let key = (
                row.period.clone(),
                row.period_start.clone(),
                row.user_id.clone(),
            );
            if current.as_ref().is_some_and(|(old, _)| old != &key) {
                let (key, entry) = current.take().expect("existing group");
                store_entry(&mut tx, key, entry).await?;
            }
            let (_, entry) = current.get_or_insert_with(|| (key, Entry::default()));
            entry.add(&row)?;
            cursor = (row.period, row.period_start, row.user_id, row.id);
        }
    }
    if let Some((key, entry)) = current {
        store_entry(&mut tx, key, entry).await?;
    }

    for period in [
        SpendLeaderboardPeriod::Day,
        SpendLeaderboardPeriod::Week,
        SpendLeaderboardPeriod::Month,
    ] {
        let start = period.current_start_at(now);
        sqlx::query(
            "INSERT INTO spend_leaderboard_periods(period,period_start,period_end,refreshed_at,total_cost_amount)
             VALUES (?,?,?,ag_now(),'0') ON CONFLICT(period,period_start) DO UPDATE SET refreshed_at=excluded.refreshed_at")
            .bind(period.as_str()).bind(SqliteDate(start)).bind(SqliteDate(period.end_after(start)))
            .execute(&mut *tx).await?;
    }
    let mut cursor = (String::new(), String::new(), String::new());
    let mut period_sum: Option<((String, String), CostSum)> = None;
    loop {
        let rows = sqlx::query_as::<_, Total>(
            "SELECT period,period_start,user_id,cost_amount FROM leaderboard_totals
             WHERE (period,period_start,user_id)>(?1,?2,?3)
             ORDER BY period,period_start,user_id LIMIT ?4",
        )
        .bind(&cursor.0)
        .bind(&cursor.1)
        .bind(&cursor.2)
        .bind(PAGE)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let key = (row.period.clone(), row.period_start.clone());
            if period_sum.as_ref().is_some_and(|(old, _)| old != &key) {
                let (key, sum) = period_sum.take().expect("existing period");
                store_period(&mut tx, key, sum).await?;
            }
            period_sum
                .get_or_insert_with(|| (key, CostSum::default()))
                .1
                .add(row.cost_amount.0);
            cursor = (row.period, row.period_start, row.user_id);
        }
    }
    if let Some((key, sum)) = period_sum {
        store_period(&mut tx, key, sum).await?;
    }
    sqlx::query(
        "INSERT INTO spend_leaderboard_entries
           (period,period_start,user_id,rank,request_count,priced_request_count,total_tokens,cost_amount)
         SELECT period,period_start,user_id,
           row_number() OVER (PARTITION BY period,period_start
             ORDER BY cost_amount COLLATE ag_decimal DESC,request_count DESC,user_id),
           request_count,priced_request_count,total_tokens,cost_amount
         FROM leaderboard_totals WHERE true
         ON CONFLICT(period,period_start,user_id) DO UPDATE SET
           rank=excluded.rank,request_count=excluded.request_count,
           priced_request_count=excluded.priced_request_count,
           total_tokens=excluded.total_tokens,cost_amount=excluded.cost_amount
         WHERE (spend_leaderboard_entries.rank,spend_leaderboard_entries.request_count,
                spend_leaderboard_entries.priced_request_count,spend_leaderboard_entries.total_tokens,
                spend_leaderboard_entries.cost_amount)
           IS NOT (excluded.rank,excluded.request_count,excluded.priced_request_count,
                   excluded.total_tokens,excluded.cost_amount)")
        .execute(&mut *tx).await?;
    (&mut *tx)
        .execute("DELETE FROM leaderboard_input; DELETE FROM leaderboard_totals")
        .await?;
    tx.commit().await?;
    Ok(SpendLeaderboardRefresh::Updated)
}

#[derive(FromRow)]
struct Input {
    period: String,
    period_start: String,
    user_id: String,
    id: String,
    cost_amount: Option<SqliteAmount>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}
#[derive(FromRow)]
struct Total {
    period: String,
    period_start: String,
    user_id: String,
    cost_amount: SqliteAmount,
}
#[derive(Default)]
struct Entry {
    requests: i64,
    priced: i64,
    tokens: i64,
    cost: CostSum,
}
impl Entry {
    fn add(&mut self, row: &Input) -> Result<(), RepositoryError> {
        self.requests = self
            .requests
            .checked_add(1)
            .ok_or(RepositoryError::Validation)?;
        if let Some(amount) = row.cost_amount {
            self.priced = self
                .priced
                .checked_add(1)
                .ok_or(RepositoryError::Validation)?;
            self.cost.add(amount.0);
        }
        self.tokens = self
            .tokens
            .checked_add(row.input_tokens.unwrap_or(0))
            .and_then(|v| v.checked_add(row.output_tokens.unwrap_or(0)))
            .ok_or(RepositoryError::Validation)?;
        Ok(())
    }
}
async fn store_entry(
    tx: &mut Transaction<'_, Sqlite>,
    key: Key,
    entry: Entry,
) -> Result<(), RepositoryError> {
    if entry.priced == 0 {
        return Ok(());
    }
    sqlx::query("INSERT INTO leaderboard_totals VALUES (?,?,?,?,?,?,?)")
        .bind(key.0)
        .bind(key.1)
        .bind(SqliteUuid(
            key.2
                .parse::<Uuid>()
                .map_err(|_| RepositoryError::Validation)?,
        ))
        .bind(entry.requests)
        .bind(entry.priced)
        .bind(entry.tokens)
        .bind(SqliteAmount::new(entry.cost.finish()?).map_err(|_| RepositoryError::Validation)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn store_period(
    tx: &mut Transaction<'_, Sqlite>,
    key: (String, String),
    sum: CostSum,
) -> Result<(), RepositoryError> {
    let start: NaiveDate = key.1.parse().map_err(|_| RepositoryError::Validation)?;
    let period = match key.0.as_str() {
        "day" => SpendLeaderboardPeriod::Day,
        "week" => SpendLeaderboardPeriod::Week,
        "month" => SpendLeaderboardPeriod::Month,
        _ => return Err(RepositoryError::Validation),
    };
    sqlx::query(
        "INSERT INTO spend_leaderboard_periods(period,period_start,period_end,refreshed_at,total_cost_amount)
         VALUES (?,?,?,ag_now(),?) ON CONFLICT(period,period_start) DO UPDATE SET
           period_end=excluded.period_end,refreshed_at=excluded.refreshed_at,total_cost_amount=excluded.total_cost_amount")
        .bind(period.as_str()).bind(SqliteDate(start)).bind(SqliteDate(period.end_after(start)))
        .bind(SqliteAmount::new(sum.finish()?).map_err(|_|RepositoryError::Validation)?)
        .execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

    #[tokio::test]
    async fn refresh_guard_is_shared_nonblocking_and_released_on_cancellation() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let db = Arc::new(
            SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
                .await
                .unwrap(),
        );
        db.install_schema().await.unwrap();
        let writer = db.begin_write().await.unwrap();
        let active = Arc::clone(&db);
        let task = tokio::spawn(async move { refresh(&active).await });
        tokio::time::timeout(Duration::from_secs(3), async {
            while db.leaderboard_refresh.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let independent = super::super::SqliteMeteringQueries::new(Arc::clone(&db));
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                independent.refresh_spend_leaderboard_snapshots()
            )
            .await
            .unwrap()
            .unwrap(),
            SpendLeaderboardRefresh::AlreadyRunning
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        writer.rollback().await.unwrap();
        assert_eq!(
            refresh(&db).await.unwrap(),
            SpendLeaderboardRefresh::Updated
        );
        db.close().await;
    }
}
