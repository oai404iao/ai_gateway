//! Deterministic static upstream identities for current-schema fixtures.

pub async fn insert(pool: &sqlx::PgPool, id: uuid::Uuid, target: &str, secret: &str) {
    sqlx::query(
        "INSERT INTO upstream_credentials(id,name,kind,secret,allowed_base_urls)
        VALUES($1,'test upstream identity','bearer',$2,jsonb_build_array($3::text))",
    )
    .bind(id)
    .bind(secret)
    .bind(target)
    .execute(pool)
    .await
    .unwrap();
}
