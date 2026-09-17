use sqlx::PgPool;

// Legacy SQL display/statistics fixtures need explicit financial evidence;
// production and worker tests must use the terminal-event or metering ports.
pub async fn copy_log_fixtures(pool: &PgPool) {
    let columns: String = sqlx::query_scalar(
        "SELECT string_agg(quote_ident(column_name),',' ORDER BY ordinal_position)
         FROM information_schema.columns
         WHERE table_schema='public' AND table_name='request_metering_facts'
           AND is_generated='NEVER'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO request_metering_facts ({columns})
         SELECT {columns} FROM request_logs ON CONFLICT (id) DO NOTHING",
    ))
    .execute(pool)
    .await
    .unwrap();
}
