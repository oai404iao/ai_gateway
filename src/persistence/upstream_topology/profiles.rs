//! Priced-model bindings, independent of legacy protocol-rule configuration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::RepositoryError;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoutingProfileView {
    pub id: Uuid,
    pub model_id: Uuid,
    pub client_model: String,
    pub model_display_name: String,
    pub model_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn postgres(
    connection: &mut sqlx::PgConnection,
) -> Result<Vec<RoutingProfileView>, RepositoryError> {
    super::decode_json_rows(
        sqlx::query_scalar::<_, String>(
            "SELECT jsonb_build_object(
            'id',p.id,'model_id',m.id,'client_model',m.source_model_id,
            'model_display_name',m.display_name,'model_enabled',m.enabled,
            'created_at',p.created_at,'updated_at',p.updated_at)::text
         FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
         WHERE m.deleted_at IS NULL ORDER BY m.source_model_id,p.id",
        )
        .fetch_all(connection)
        .await?,
    )
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<RoutingProfileView>, RepositoryError> {
    super::decode_json_rows(sqlx::query_scalar::<_, String>(
        "SELECT json_object(
            'id',p.id,'model_id',m.id,'client_model',m.source_model_id,
            'model_display_name',m.display_name,'model_enabled',json(CASE m.enabled WHEN 1 THEN 'true' ELSE 'false' END),
            'created_at',p.created_at,'updated_at',p.updated_at)
         FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
         WHERE m.deleted_at IS NULL ORDER BY m.source_model_id,p.id",
    ).fetch_all(connection).await?)
}
