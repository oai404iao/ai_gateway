//! Explicit administrator-triggered models.dev catalog preview and price application.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    models_dev::{ModelsDevCatalog, ModelsDevClient, ModelsDevError, ModelsDevModel},
    persistence::{ModelSourceVersions, SyncedModelInput},
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
        let catalog = self.client.fetch_catalog(&request.provider_ids).await?;
        let versions = self.coordinator.model_source_versions().await?;
        Ok(preview_from_catalog(catalog, &versions))
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
        let mut model_ids = HashSet::new();
        for selection in &request.selections {
            if !selection.valid() || !model_ids.insert(&selection.model_id) {
                return Err(ModelSyncError::InvalidSelection);
            }
        }
        let provider_ids = request
            .selections
            .iter()
            .map(|selection| selection.provider_id.clone())
            .collect::<Vec<_>>();
        let catalog = self.client.fetch_catalog(&provider_ids).await?;
        let inputs = request
            .selections
            .into_iter()
            .map(|selection| {
                let model = catalog
                    .find(&selection.provider_id, &selection.model_id)
                    .ok_or(ModelSyncError::StalePreview)?;
                Ok(SyncedModelInput {
                    model: model.clone(),
                    preview_token: selection.preview_token,
                })
            })
            .collect::<Result<_, ModelSyncError>>()?;
        self.coordinator
            .apply_catalog_models(actor, inputs)
            .await
            .map_err(ModelSyncError::from)
    }

    /// Resolve the create-only catalog preview before acquiring the database writer.
    pub async fn resolve_new(
        &self,
        selection: &ModelSyncSelection,
    ) -> Result<ModelsDevModel, ModelSyncError> {
        if !selection.valid() {
            return Err(ModelSyncError::InvalidSelection);
        }
        let catalog = self
            .client
            .fetch_catalog(std::slice::from_ref(&selection.provider_id))
            .await?;
        let model = catalog
            .find(&selection.provider_id, &selection.model_id)
            .filter(|model| model.preview_token(None) == selection.preview_token)
            .ok_or(ModelSyncError::StalePreview)?;
        Ok(model.clone())
    }
}

fn preview_from_catalog(
    catalog: ModelsDevCatalog,
    versions: &ModelSourceVersions,
) -> ModelSyncPreview {
    let models = catalog
        .models
        .into_iter()
        .map(|model| {
            let local = versions.get(&model.model_id).copied();
            let action = if local.is_some() {
                ModelSyncAction::PriceUpdate
            } else {
                ModelSyncAction::Import
            };
            let preview_token = model.preview_token(local);
            ModelSyncPreviewModel {
                model,
                action,
                preview_token,
            }
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
    pub preview_token: String,
}

impl ModelSyncSelection {
    fn valid(&self) -> bool {
        !self.provider_id.is_empty()
            && !self.model_id.is_empty()
            && self.preview_token.len() == 64
            && self
                .preview_token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
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
    pub preview_token: String,
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
    #[error("catalog preview is stale")]
    StalePreview,
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
