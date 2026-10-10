//! Settings compilation binds one immutable configuration to a native library.

use super::*;
use ai_gateway_connector_sdk::{
    MAX_SETTINGS_BYTES, PluginSettingsDescriptor, PluginSettingsDocument, PluginSettingsValidation,
    SETTINGS_COMPILE, SETTINGS_DESCRIBE, SETTINGS_MIGRATE, SETTINGS_VALIDATE,
};
use serde_json::json;

pub(super) struct CompiledSettings(pub Value);

impl Drop for CompiledSettings {
    fn drop(&mut self) {
        ai_gateway_connector_sdk::zeroize_json(&mut self.0);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn native_generic_settings_are_generation_bound_and_cannot_be_overridden() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = tempfile::tempdir_in(root).unwrap();
        let path = directory.path().join("fixture.so");
        let prepared = PathBuf::from(
            std::env::var_os("AI_GATEWAY_TEST_CODEX_PLUGIN")
                .expect("run scripts/prepare-connector-tests.sh before native tests"),
        );
        fs::copy(
            prepared.parent().unwrap().join("fixture-settings.so"),
            &path,
        )
        .expect("prepare-connector-tests.sh creates the generic settings fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let digest: String = Sha256::digest(fs::read(&path).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let library = Plugin::load(&path, &digest, "fixture").unwrap();
        let stateless_old = library.with_revision(3);
        let stateless_new = library.with_revision(4);
        assert_ne!(stateless_old.generation_id(), stateless_new.generation_id());
        assert!(
            stateless_old
                .call("echo", &json!({}), &[])
                .unwrap()
                .metadata
                .get("settings")
                .is_none()
        );
        let descriptor = library.settings_descriptor().unwrap().unwrap();
        let old = library
            .configured(&descriptor.default_document(), 1)
            .unwrap();
        let new = library
            .configured(
                &PluginSettingsDocument {
                    schema_version: 1,
                    values: json!({"mode":"alternate"}),
                },
                2,
            )
            .unwrap();
        for (plugin, value) in [(&old, "default"), (&new, "alternate"), (&old, "default")] {
            assert_eq!(
                plugin.call("echo", &json!({}), &[]).unwrap().metadata["settings"]["mode"],
                value
            );
        }
        assert_ne!(old.generation_id(), new.generation_id());
        let republished = old.with_revision(5);
        assert_ne!(old.generation_id(), republished.generation_id());
        assert_eq!(
            republished.call("echo", &json!({}), &[]).unwrap().metadata["settings"]["mode"],
            "default"
        );
        assert_eq!(old.artifact_digest(), new.artifact_digest());
        assert!(
            old.call("echo", &json!({"settings":{"mode":"attacker"}}), &[])
                .is_err()
        );
        assert!(
            library
                .configured(
                    &PluginSettingsDocument {
                        schema_version: 1,
                        values: json!({"mode":"default","unknown":true}),
                    },
                    3
                )
                .is_err()
        );
        assert_eq!(
            library
                .migrate_settings(&PluginSettingsDocument {
                    schema_version: 0,
                    values: json!({}),
                })
                .unwrap(),
            descriptor.default_document()
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            old.call("echo", &json!({}), &[]).unwrap().metadata["settings"]["mode"],
            "default"
        );
    }
}

impl Plugin {
    pub fn settings_descriptor(&self) -> Result<Option<PluginSettingsDescriptor>, PluginError> {
        let required = [SETTINGS_DESCRIBE, SETTINGS_VALIDATE, SETTINGS_COMPILE];
        let present = required
            .iter()
            .filter(|command| {
                self.manifest
                    .commands
                    .iter()
                    .any(|entry| entry == **command)
            })
            .count();
        if present == 0 {
            if self
                .manifest
                .commands
                .iter()
                .any(|command| command == SETTINGS_MIGRATE)
            {
                return Err(PluginError::Manifest);
            }
            return Ok(None);
        }
        if present != required.len() {
            return Err(PluginError::Manifest);
        }
        let output = self.settings_call(SETTINGS_DESCRIBE, &json!({}))?;
        let descriptor: PluginSettingsDescriptor =
            serde_json::from_value(output).map_err(|_| PluginError::InvalidOutput)?;
        if !descriptor.validate_descriptor() {
            return Err(PluginError::InvalidOutput);
        }
        Ok(Some(descriptor))
    }

    pub fn validate_settings(
        &self,
        document: &PluginSettingsDocument,
    ) -> Result<PluginSettingsValidation, PluginError> {
        let descriptor = self
            .settings_descriptor()?
            .ok_or(PluginError::UnsupportedCommand)?;
        if descriptor.schema_version != document.schema_version
            || !descriptor.validate_values(&document.values)
        {
            return Err(PluginError::InvalidInput);
        }
        let output = self.settings_call(SETTINGS_VALIDATE, &json!(document))?;
        let validation: PluginSettingsValidation =
            serde_json::from_value(output).map_err(|_| PluginError::InvalidOutput)?;
        if validation.errors.len() > 64
            || validation.valid != validation.errors.is_empty()
            || validation.errors.iter().any(|error| {
                !descriptor
                    .fields
                    .iter()
                    .any(|field| field.key == error.field)
                    || error.code.is_empty()
                    || error.code.len() > 64
                    || !error.code.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            })
        {
            return Err(PluginError::InvalidOutput);
        }
        Ok(validation)
    }

    pub fn configured(
        &self,
        document: &PluginSettingsDocument,
        revision: u64,
    ) -> Result<Arc<Self>, PluginError> {
        if !self.validate_settings(document)?.valid {
            return Err(PluginError::Rejected("invalid_settings".into()));
        }
        let mut output = self.settings_call(SETTINGS_COMPILE, &json!(document))?;
        let config = output
            .get_mut("config")
            .map(Value::take)
            .ok_or(PluginError::InvalidOutput)?;
        if !config.is_object() {
            return Err(PluginError::InvalidOutput);
        }
        Ok(Arc::new(Self {
            manifest: self.manifest.clone(),
            dispatch: self.dispatch,
            free_buffer: self.free_buffer,
            artifact_digest: self.artifact_digest.clone(),
            generation_id: format!("{}:{revision}", self.artifact_digest),
            settings: Some(Arc::new(CompiledSettings(config))),
            attempt_descriptors: contracts::descriptor_cache(&self.manifest),
        }))
    }

    pub fn migrate_settings(
        &self,
        document: &PluginSettingsDocument,
    ) -> Result<PluginSettingsDocument, PluginError> {
        let output = self.settings_call(
            SETTINGS_MIGRATE,
            &json!({
                "from_schema_version": document.schema_version,
                "values": document.values,
            }),
        )?;
        let migrated: PluginSettingsDocument =
            serde_json::from_value(output).map_err(|_| PluginError::InvalidOutput)?;
        if !self.validate_settings(&migrated)?.valid {
            return Err(PluginError::InvalidOutput);
        }
        Ok(migrated)
    }

    fn settings_call(&self, command: &str, metadata: &Value) -> Result<Value, PluginError> {
        if serde_json::to_vec(metadata).map_or(true, |bytes| bytes.len() > MAX_SETTINGS_BYTES) {
            return Err(PluginError::InvalidInput);
        }
        let output = self.call(command, metadata, &[])?;
        if !output.body.is_empty()
            || serde_json::to_vec(&output.metadata)
                .map_or(true, |bytes| bytes.len() > MAX_SETTINGS_BYTES)
        {
            return Err(PluginError::InvalidOutput);
        }
        Ok(output.metadata)
    }
}
