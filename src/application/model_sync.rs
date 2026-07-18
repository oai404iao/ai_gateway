//! Explicit administrator-triggered models.dev preview and synchronization.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    models_dev::{ModelsDevCatalog, ModelsDevClient, ModelsDevError, ModelsDevSelection},
    persistence::SyncedModelInput,
};

use super::{ControlPlaneCoordinator, ControlPlaneError, ModelSyncResult};

#[derive(Clone)]
pub struct ModelSyncService {
    coordinator: ControlPlaneCoordinator,
    client: ModelsDevClient,
    max_selections: usize,
}

impl ModelSyncService {
    #[must_use]
    pub fn new(
        coordinator: ControlPlaneCoordinator,
        client: ModelsDevClient,
        max_selections: usize,
    ) -> Self {
        Self {
            coordinator,
            client,
            max_selections,
        }
    }

    pub async fn preview(
        &self,
        request: ModelSyncPreviewRequest,
    ) -> Result<ModelsDevCatalog, ModelSyncError> {
        self.client
            .fetch_catalog(&request.provider_ids)
            .await
            .map_err(ModelSyncError::from)
    }

    pub async fn sync(
        &self,
        actor: Uuid,
        request: ModelSyncRequest,
    ) -> Result<ModelSyncResult, ModelSyncError> {
        if request.selections.is_empty() || request.selections.len() > self.max_selections {
            return Err(ModelSyncError::InvalidSelection);
        }
        let provider_ids = request
            .selections
            .iter()
            .map(|selection| selection.provider_id.clone())
            .collect::<Vec<_>>();
        let catalog = self.client.fetch_catalog(&provider_ids).await?;
        let selections = request
            .selections
            .iter()
            .map(|selection| ModelsDevSelection {
                provider_id: selection.provider_id.clone(),
                model_id: selection.model_id.clone(),
            })
            .collect::<Vec<_>>();
        let models = catalog.select(&selections).map_err(|error| match error {
            ModelsDevError::InvalidSelection => ModelSyncError::InvalidSelection,
            other => ModelSyncError::Catalog(other),
        })?;
        // `source_model_id` intentionally follows the operator-selected raw
        // models.dev model id. It is therefore unique across a single sync,
        // even when the catalog has the same id under multiple providers.
        let mut source_model_ids = HashSet::new();
        if models
            .iter()
            .any(|model| !source_model_ids.insert(model.model_id.clone()))
        {
            return Err(ModelSyncError::ConflictingSourceModelId);
        }
        let inputs = models
            .into_iter()
            .map(|model| SyncedModelInput {
                source_model_id: model.model_id,
                display_name: model.display_name,
                provider_name: model.provider_name,
                input_unit_price: model.input_unit_price,
                cached_input_unit_price: model.cached_input_unit_price,
                cache_write_unit_price: model.cache_write_unit_price,
                output_unit_price: model.output_unit_price,
                source_payload: model.source_payload,
            })
            .collect();
        self.coordinator
            .sync_models(actor, inputs)
            .await
            .map_err(ModelSyncError::from)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncPreviewRequest {
    #[serde(default)]
    pub provider_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncRequest {
    pub selections: Vec<ModelSyncSelection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncSelection {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Error)]
pub enum ModelSyncError {
    #[error("requested catalog selection is invalid")]
    InvalidSelection,
    #[error("selected catalog entries have duplicate source model ids")]
    ConflictingSourceModelId,
    #[error("models.dev request failed")]
    Catalog(#[from] ModelsDevError),
    #[error("control-plane update failed")]
    ControlPlane(#[from] ControlPlaneError),
}

#[derive(Serialize)]
pub struct ModelSyncResponse {
    pub model_count: usize,
    pub correlation_id: Uuid,
}
