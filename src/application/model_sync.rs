//! Explicit administrator-triggered models.dev catalog preview and price application.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    models_dev::{
        ModelsDevCatalog, ModelsDevClient, ModelsDevError, ModelsDevModel, ModelsDevSelection,
    },
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

    /// Shows which catalog models will create a local model and which will
    /// explicitly refresh the billing configuration of an existing model.
    pub async fn preview(
        &self,
        request: ModelSyncPreviewRequest,
    ) -> Result<ModelSyncPreview, ModelSyncError> {
        let source_model_ids = self.coordinator.model_source_ids().await?;
        let catalog = self.client.fetch_catalog(&request.provider_ids).await?;
        Ok(preview_from_catalog(catalog, &source_model_ids))
    }

    /// Applies administrator-selected catalog entries. A selected local
    /// `source_model_id` refreshes its models.dev USD prices, long-context
    /// tiers, and any supported request multipliers; a new identifier creates
    /// a model. No catalog billing changes without an explicit selection.
    pub async fn apply(
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
            .apply_catalog_models(actor, inputs)
            .await
            .map_err(ModelSyncError::from)
    }
}

fn preview_from_catalog(
    catalog: ModelsDevCatalog,
    source_model_ids: &[String],
) -> ModelSyncPreview {
    let local_ids = source_model_ids.iter().collect::<HashSet<_>>();
    let models = catalog
        .models
        .into_iter()
        .map(|model| {
            let action = if local_ids.contains(&model.model_id) {
                ModelSyncAction::PriceUpdate
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
        advanced_billing: model.advanced_billing,
        source_payload: model.source_payload,
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
    pub imported_count: usize,
    pub updated_count: usize,
    pub correlation_id: Uuid,
}
