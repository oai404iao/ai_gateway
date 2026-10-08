//! Plugin lifecycle changes share the control-plane publication transaction.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use ai_gateway_connector_sdk::{PluginSettingsDescriptor, PluginSettingsDocument};
use serde::Serialize;
use uuid::Uuid;

use super::{ControlPlaneCoordinator, ControlPlaneError};
use crate::{
    connector_plugins::{DirectoryPluginCatalog, PluginArtifact},
    persistence::{
        ControlPlaneMutation, MutationResult, PluginArtifactInput, PluginInstallJob,
        PluginJobCompletion, PluginRuntimeRecords, PluginSaveInput, PluginSettingsInput,
        PluginStateRecord, RepositoryError,
    },
};

struct UploadedPluginArchive(PathBuf);

impl Drop for UploadedPluginArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Serialize)]
pub struct ManagedPluginArtifactView {
    pub digest: String,
    pub version: String,
}

#[derive(Serialize)]
pub struct ManagedPluginView {
    pub id: String,
    pub built_in: bool,
    pub enabled: bool,
    pub revision: i64,
    pub artifact_digest: Option<String>,
    pub version: Option<String>,
    pub status: &'static str,
    pub artifacts: Vec<ManagedPluginArtifactView>,
    pub error_code: Option<&'static str>,
}

#[derive(Serialize)]
pub struct ManagedPluginSettingsView {
    pub plugin_id: String,
    pub artifact_digest: String,
    pub schema_version: u32,
    pub revision: i64,
    pub descriptor: PluginSettingsDescriptor,
    pub values: serde_json::Value,
}

pub fn plugin_etag(revision: i64, digest: Option<&str>) -> String {
    format!("\"plugin-{revision}-{}\"", digest.unwrap_or("none"))
}

fn state<'a>(
    records: &'a PluginRuntimeRecords,
    id: &str,
) -> Result<&'a PluginStateRecord, ControlPlaneError> {
    records
        .states
        .iter()
        .find(|state| state.plugin_id == id)
        .ok_or_else(|| RepositoryError::NotFound.into())
}

fn expect_revision(current: &PluginStateRecord, expected: &str) -> Result<(), ControlPlaneError> {
    if expected != plugin_etag(current.revision, current.artifact_digest.as_deref()) {
        return Err(RepositoryError::Conflict.into());
    }
    Ok(())
}

fn artifact_input(artifact: &PluginArtifact) -> PluginArtifactInput {
    PluginArtifactInput {
        plugin_id: artifact.id.clone(),
        digest: artifact.digest.clone(),
        version: artifact.version.clone(),
        manifest: serde_json::to_value(&artifact.manifest).expect("plugin manifest serializes"),
    }
}

impl ControlPlaneCoordinator {
    pub fn plugin_catalog(&self) -> Result<Arc<DirectoryPluginCatalog>, ControlPlaneError> {
        self.runtime
            .plugin_catalog()
            .cloned()
            .ok_or_else(|| RepositoryError::Validation.into())
    }

    pub async fn managed_plugins(&self) -> Result<Vec<ManagedPluginView>, ControlPlaneError> {
        let records = self.repository.plugin_records().await?;
        let snapshot = self.runtime.snapshot();
        let ids: BTreeSet<_> = records
            .states
            .iter()
            .map(|state| state.plugin_id.as_str())
            .collect();
        let mut views = vec![ManagedPluginView {
            id: "general".into(),
            built_in: true,
            enabled: true,
            revision: 0,
            artifact_digest: None,
            version: Some(env!("CARGO_PKG_VERSION").into()),
            status: "active",
            artifacts: Vec::new(),
            error_code: None,
        }];
        for id in ids {
            let current = state(&records, id)?;
            let artifacts: Vec<_> = records
                .artifacts
                .iter()
                .filter(|artifact| artifact.plugin_id == id)
                .map(|artifact| ManagedPluginArtifactView {
                    digest: artifact.digest.clone(),
                    version: artifact.version.clone(),
                })
                .collect();
            let version = artifacts
                .iter()
                .find(|artifact| Some(&artifact.digest) == current.artifact_digest.as_ref())
                .map(|artifact| artifact.version.clone());
            views.push(ManagedPluginView {
                id: id.into(),
                built_in: false,
                enabled: current.enabled,
                revision: current.revision,
                artifact_digest: current.artifact_digest.clone(),
                version,
                status: if current.enabled && snapshot.plugins().get(id).is_some() {
                    "active"
                } else if current.enabled {
                    "unavailable"
                } else if artifacts.is_empty() {
                    "not_installed"
                } else {
                    "disabled"
                },
                artifacts,
                error_code: snapshot.plugin_error(id),
            });
        }
        Ok(views)
    }

    pub async fn managed_plugin_settings(
        &self,
        id: &str,
    ) -> Result<ManagedPluginSettingsView, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let records = self.repository.plugin_records().await?;
        let current = state(&records, id)?;
        let digest = current
            .artifact_digest
            .as_deref()
            .ok_or(RepositoryError::NotFound)?;
        let catalog = self.plugin_catalog()?;
        let artifact = catalog
            .resolve(id, digest)
            .map_err(|_| RepositoryError::Validation)?;
        let plugin = catalog
            .load(&artifact)
            .map_err(|_| RepositoryError::Validation)?;
        let descriptor = plugin
            .settings_descriptor()
            .map_err(|_| RepositoryError::Validation)?
            .ok_or(RepositoryError::NotFound)?;
        let settings = records
            .settings
            .iter()
            .find(|settings| settings.plugin_id == id)
            .ok_or(RepositoryError::NotFound)?;
        if u32::try_from(settings.schema_version).ok() != Some(descriptor.schema_version) {
            return Err(RepositoryError::Validation.into());
        }
        Ok(ManagedPluginSettingsView {
            plugin_id: id.into(),
            artifact_digest: digest.into(),
            schema_version: descriptor.schema_version,
            revision: current.revision,
            descriptor,
            values: settings.values.clone(),
        })
    }

    pub async fn save_plugin_state(
        &self,
        actor: Uuid,
        id: String,
        enabled: bool,
        digest: Option<String>,
        expected: &str,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        self.verify_active_admin(actor).await?;
        let records = self.repository.plugin_records().await?;
        let current = state(&records, &id)?;
        expect_revision(current, expected)?;
        let mut settings = None;
        if let Some(digest) = &digest
            && (enabled || current.artifact_digest.as_ref() != Some(digest))
        {
            if !records
                .artifacts
                .iter()
                .any(|artifact| artifact.plugin_id == id && artifact.digest == *digest)
            {
                return Err(RepositoryError::Validation.into());
            }
            let catalog = self.plugin_catalog()?;
            let artifact = catalog
                .resolve(&id, digest)
                .map_err(|_| RepositoryError::Validation)?;
            let plugin = catalog
                .load(&artifact)
                .map_err(|_| RepositoryError::Validation)?;
            if let Some(descriptor) = plugin
                .settings_descriptor()
                .map_err(|_| RepositoryError::Validation)?
            {
                let mut document = records
                    .settings
                    .iter()
                    .find(|settings| settings.plugin_id == id)
                    .map(|settings| PluginSettingsDocument {
                        schema_version: settings.schema_version as u32,
                        values: settings.values.clone(),
                    })
                    .unwrap_or(PluginSettingsDocument {
                        schema_version: descriptor.schema_version,
                        values: descriptor.defaults,
                    });
                if document.schema_version != descriptor.schema_version {
                    document = plugin
                        .migrate_settings(&document)
                        .map_err(|_| RepositoryError::Validation)?;
                }
                let next_revision = current
                    .revision
                    .checked_add(1)
                    .and_then(|revision| u64::try_from(revision).ok())
                    .ok_or(RepositoryError::Validation)?;
                plugin
                    .configured(&document, next_revision)
                    .map_err(|_| RepositoryError::Validation)?;
                settings = Some(PluginSettingsInput {
                    schema_version: i32::try_from(document.schema_version)
                        .map_err(|_| RepositoryError::Validation)?,
                    values: document.values,
                });
            }
        } else if enabled && digest.is_none() {
            return Err(RepositoryError::Validation.into());
        }
        let change = self
            .repository
            .prepare_mutation(
                actor,
                ControlPlaneMutation::SavePlugin {
                    plugin_id: id.clone(),
                    input: PluginSaveInput {
                        enabled,
                        artifact_digest: digest,
                        settings,
                    },
                    expected_revision: current.revision,
                },
            )
            .await?;
        self.commit_plugin_change(change, &id, enabled).await
    }

    pub async fn save_plugin_settings(
        &self,
        actor: Uuid,
        id: String,
        settings: PluginSettingsInput,
        expected: &str,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        self.verify_active_admin(actor).await?;
        let records = self.repository.plugin_records().await?;
        let current = state(&records, &id)?;
        expect_revision(current, expected)?;
        let digest = current
            .artifact_digest
            .as_deref()
            .ok_or(RepositoryError::NotFound)?;
        let catalog = self.plugin_catalog()?;
        let artifact = catalog
            .resolve(&id, digest)
            .map_err(|_| RepositoryError::Validation)?;
        let plugin = catalog
            .load(&artifact)
            .map_err(|_| RepositoryError::Validation)?;
        let next_revision = current
            .revision
            .checked_add(1)
            .and_then(|revision| u64::try_from(revision).ok())
            .ok_or(RepositoryError::Validation)?;
        plugin
            .configured(
                &PluginSettingsDocument {
                    schema_version: settings
                        .schema_version
                        .try_into()
                        .map_err(|_| RepositoryError::Validation)?,
                    values: settings.values.clone(),
                },
                next_revision,
            )
            .map_err(|_| RepositoryError::Validation)?;
        let change = self
            .repository
            .prepare_mutation(
                actor,
                ControlPlaneMutation::SavePlugin {
                    plugin_id: id.clone(),
                    input: PluginSaveInput {
                        enabled: current.enabled,
                        artifact_digest: current.artifact_digest.clone(),
                        settings: Some(settings),
                    },
                    expected_revision: current.revision,
                },
            )
            .await?;
        self.commit_plugin_change(change, &id, current.enabled)
            .await
    }

    async fn commit_plugin_change(
        &self,
        mut change: crate::persistence::PreparedControlPlaneChange<'_>,
        id: &str,
        enabled: bool,
    ) -> Result<MutationResult, ControlPlaneError> {
        let candidate = Arc::new(self.runtime.compile(change.runtime_records().await?)?);
        if enabled && candidate.plugins().get(id).is_none() {
            return Err(RepositoryError::Validation.into());
        }
        self.validate_candidate(&candidate)?;
        let (mut results, _) = change.commit().await?;
        self.publish(candidate);
        results
            .pop()
            .ok_or_else(|| RepositoryError::Validation.into())
    }

    pub async fn delete_plugin_artifact(
        &self,
        actor: Uuid,
        id: String,
        digest: String,
        expected: &str,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        self.verify_active_admin(actor).await?;
        let records = self.repository.plugin_records().await?;
        let current = state(&records, &id)?;
        expect_revision(current, expected)?;
        if self.plugin_catalog()?.is_loaded(&digest) {
            return Err(RepositoryError::Conflict.into());
        }
        let change = self
            .repository
            .prepare_mutation(
                actor,
                ControlPlaneMutation::DeletePluginArtifact {
                    plugin_id: id,
                    digest,
                    expected_revision: current.revision,
                },
            )
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn plugin_job(&self, id: Uuid) -> Result<PluginInstallJob, ControlPlaneError> {
        self.repository
            .plugin_install_job(id)
            .await?
            .ok_or_else(|| RepositoryError::NotFound.into())
    }

    pub async fn begin_plugin_job(
        &self,
        actor: Uuid,
        id: Uuid,
        upload: Option<PathBuf>,
    ) -> Result<PluginInstallJob, ControlPlaneError> {
        let catalog = self.plugin_catalog()?;
        let job = self
            .repository
            .create_plugin_install_job(
                actor,
                id,
                if upload.is_some() {
                    "install"
                } else {
                    "discover"
                },
            )
            .await?;
        let service = self.clone();
        let upload = upload.map(UploadedPluginArchive);
        tokio::spawn(async move {
            let result = service.run_plugin_job(actor, id, catalog, upload).await;
            if result.is_err() {
                let _ = service
                    .repository
                    .fail_plugin_install_job(id, "plugin_install_failed")
                    .await;
            }
        });
        Ok(job)
    }

    async fn run_plugin_job(
        &self,
        actor: Uuid,
        id: Uuid,
        catalog: Arc<DirectoryPluginCatalog>,
        upload: Option<UploadedPluginArchive>,
    ) -> Result<(), ControlPlaneError> {
        self.repository.claim_plugin_install_job(actor, id).await?;
        let explicit_install = upload.is_some();
        let artifacts = tokio::task::spawn_blocking(move || match upload {
            Some(upload) => catalog
                .install_archive(&upload.0)
                .map(|artifact| vec![artifact]),
            None => discover_packages(&catalog),
        })
        .await
        .map_err(|_| RepositoryError::Validation)?
        .map_err(|_| RepositoryError::Validation)?;
        let mut result = PluginJobCompletion {
            plugin_id: None,
            artifact_digest: None,
            error_code: None,
        };
        for artifact in artifacts {
            if explicit_install {
                self.mutate(
                    actor,
                    ControlPlaneMutation::RegisterPluginArtifact(artifact_input(&artifact)),
                )
                .await?;
            } else {
                self.verify_active_admin(actor).await?;
                self.repository
                    .register_discovered_plugin(artifact_input(&artifact))
                    .await?;
            }
            result.plugin_id = Some(artifact.id);
            result.artifact_digest = Some(artifact.digest);
        }
        self.repository
            .finish_plugin_install_job(actor, id, result)
            .await?;
        Ok(())
    }

    pub async fn discover_plugin_directory(&self) -> Result<(), ControlPlaneError> {
        let catalog = self.plugin_catalog()?;
        let artifacts = tokio::task::spawn_blocking(move || discover_packages(&catalog))
            .await
            .map_err(|_| RepositoryError::Validation)?
            .map_err(|_| RepositoryError::Validation)?;
        let _guard = self.serial.lock().await;
        for artifact in artifacts {
            self.repository
                .register_discovered_plugin(artifact_input(&artifact))
                .await?;
        }
        Ok(())
    }
}

fn discover_packages(
    catalog: &DirectoryPluginCatalog,
) -> Result<Vec<PluginArtifact>, crate::connector_plugins::PluginError> {
    for entry in std::fs::read_dir(catalog.root().join("incoming"))?
        .filter(|entry| {
            entry.as_ref().map_or(true, |entry| {
                entry.file_name().to_string_lossy().ends_with(".tar.gz")
            })
        })
        .take(128)
    {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name().to_string_lossy().ends_with(".tar.gz")
        {
            match catalog.install_archive(&entry.path()) {
                Ok(_) => {
                    let mut completed = entry.file_name();
                    completed.push(".imported");
                    std::fs::rename(
                        entry.path(),
                        catalog.root().join("incoming").join(completed),
                    )?;
                }
                Err(error) => {
                    let mut rejected = entry.file_name();
                    rejected.push(".rejected");
                    std::fs::rename(entry.path(), catalog.root().join("incoming").join(rejected))?;
                    tracing::warn!(error = %error, "plugin package discovery rejected an archive");
                }
            }
        }
    }
    catalog.discover()
}
