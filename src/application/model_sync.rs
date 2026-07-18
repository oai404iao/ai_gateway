//! Explicit administrator-triggered models.dev price refresh and model import.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    models_dev::{
        ModelsDevCatalog, ModelsDevClient, ModelsDevError, ModelsDevModel, ModelsDevSelection,
    },
    persistence::{ModelsDevPriceTarget, SyncedModelInput, SyncedModelPrice},
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

    /// Shows which catalog models can refresh an existing local price and
    /// which models are eligible for a separate, explicit import.
    pub async fn preview(
        &self,
        request: ModelSyncPreviewRequest,
    ) -> Result<ModelSyncPreview, ModelSyncError> {
        let targets = self.targets(&request.provider_ids).await?;
        let source_model_ids = self.coordinator.model_source_ids().await?;
        let catalog = self.client.fetch_catalog(&request.provider_ids).await?;
        Ok(preview_from_catalog(catalog, &targets, &source_model_ids))
    }

    /// Refreshes prices for local models that were previously imported from
    /// models.dev. It never inserts a new `models` row.
    pub async fn sync_prices(
        &self,
        actor: Uuid,
        _request: ModelPriceSyncRequest,
    ) -> Result<ModelPriceSyncResponse, ModelSyncError> {
        let targets = self.targets(&[]).await?;
        if targets.is_empty() {
            return Ok(ModelPriceSyncResponse {
                updated_count: 0,
                unavailable_count: 0,
                correlation_id: None,
            });
        }
        let provider_ids = targets
            .iter()
            .map(|target| target.provider_id.clone())
            .collect::<Vec<_>>();
        let catalog = self.client.fetch_catalog(&provider_ids).await?;
        let mut updates = Vec::new();
        let mut unavailable_count = 0;
        for target in targets {
            let Some(model) = catalog.find(&target.provider_id, &target.source_model_id) else {
                unavailable_count += 1;
                continue;
            };
            updates.push(price_update(target, model));
        }
        if updates.is_empty() {
            return Ok(ModelPriceSyncResponse {
                updated_count: 0,
                unavailable_count,
                correlation_id: None,
            });
        }
        let result = self.coordinator.sync_model_prices(actor, updates).await?;
        Ok(ModelPriceSyncResponse {
            updated_count: result.model_count,
            unavailable_count,
            correlation_id: Some(result.correlation_id),
        })
    }

    /// Imports administrator-selected catalog entries. This path only creates
    /// models; a duplicate local `source_model_id` is rejected rather than
    /// changing an existing model's provider or prices.
    pub async fn import(
        &self,
        actor: Uuid,
        request: ModelImportRequest,
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
        let models = catalog.select(&selections).map_err(selection_error)?;
        let mut source_model_ids = HashSet::new();
        if models
            .iter()
            .any(|model| !source_model_ids.insert(model.model_id.clone()))
        {
            return Err(ModelSyncError::ConflictingSourceModelId);
        }
        let inputs = models.into_iter().map(import_input).collect();
        self.coordinator
            .import_models(actor, inputs)
            .await
            .map_err(ModelSyncError::from)
    }

    async fn targets(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<ModelsDevPriceTarget>, ModelSyncError> {
        let mut targets = self.coordinator.models_dev_price_targets().await?;
        if !provider_ids.is_empty() {
            targets.retain(|target| provider_ids.contains(&target.provider_id));
        }
        Ok(targets)
    }
}

fn preview_from_catalog(
    catalog: ModelsDevCatalog,
    targets: &[ModelsDevPriceTarget],
    source_model_ids: &[String],
) -> ModelSyncPreview {
    let target_ids = targets
        .iter()
        .map(|target| (&target.provider_id, &target.source_model_id))
        .collect::<HashSet<_>>();
    let local_ids = source_model_ids.iter().collect::<HashSet<_>>();
    let catalog_ids = catalog
        .models
        .iter()
        .map(|model| (&model.provider_id, &model.model_id))
        .collect::<HashSet<_>>();
    let unavailable_existing_count = targets
        .iter()
        .filter(|target| !catalog_ids.contains(&(&target.provider_id, &target.source_model_id)))
        .count();
    let models = catalog
        .models
        .into_iter()
        .map(|model| {
            let action = if target_ids.contains(&(&model.provider_id, &model.model_id)) {
                ModelSyncAction::PriceUpdate
            } else if local_ids.contains(&model.model_id) {
                ModelSyncAction::AlreadyExists
            } else {
                ModelSyncAction::Import
            };
            ModelSyncPreviewModel { model, action }
        })
        .collect();
    ModelSyncPreview {
        fetched_at: catalog.fetched_at,
        models,
        excluded_missing_prices: catalog.excluded_missing_prices,
        excluded_invalid_models: catalog.excluded_invalid_models,
        excluded_oversized_metadata: catalog.excluded_oversized_metadata,
        unavailable_existing_count,
    }
}

fn import_input(model: ModelsDevModel) -> SyncedModelInput {
    SyncedModelInput {
        source_model_id: model.model_id,
        display_name: model.display_name,
        provider_name: model.provider_name,
        input_unit_price: model.input_unit_price,
        cached_input_unit_price: model.cached_input_unit_price,
        cache_write_unit_price: model.cache_write_unit_price,
        output_unit_price: model.output_unit_price,
        source_payload: model.source_payload,
    }
}

fn price_update(target: ModelsDevPriceTarget, model: &ModelsDevModel) -> SyncedModelPrice {
    SyncedModelPrice {
        model_id: target.model_id,
        source_model_id: target.source_model_id,
        provider_id: target.provider_id,
        input_unit_price: model.input_unit_price,
        cached_input_unit_price: model.cached_input_unit_price,
        cache_write_unit_price: model.cache_write_unit_price,
        output_unit_price: model.output_unit_price,
        source_payload: model.source_payload.clone(),
    }
}

fn selection_error(error: ModelsDevError) -> ModelSyncError {
    match error {
        ModelsDevError::InvalidSelection => ModelSyncError::InvalidSelection,
        other => ModelSyncError::Catalog(other),
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
pub struct ModelPriceSyncRequest {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelImportRequest {
    pub selections: Vec<ModelSyncSelection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncSelection {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Serialize)]
pub struct ModelSyncPreview {
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub models: Vec<ModelSyncPreviewModel>,
    pub excluded_missing_prices: usize,
    pub excluded_invalid_models: usize,
    pub excluded_oversized_metadata: usize,
    pub unavailable_existing_count: usize,
}

#[derive(Serialize)]
pub struct ModelSyncPreviewModel {
    #[serde(flatten)]
    pub model: ModelsDevModel,
    pub action: ModelSyncAction,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSyncAction {
    PriceUpdate,
    Import,
    AlreadyExists,
}

#[derive(Serialize)]
pub struct ModelPriceSyncResponse {
    pub updated_count: usize,
    pub unavailable_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
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
