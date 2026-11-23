//! Generation-bound semantic preflight, independent of request credentials.

use ai_gateway_connector_sdk::{
    ATTEMPT_DESCRIBE, AttemptDescriptor, RESPONSE_EVENT, RESPONSE_JSON, ResponseMode, USAGE_PARSE,
    UsageDescriptor,
};
use serde_json::json;

use crate::*;

pub(super) fn descriptor_cache(
    manifest: &PluginManifest,
) -> HashMap<String, OnceLock<Result<AttemptDescriptor, ()>>> {
    manifest
        .operations
        .iter()
        .map(|operation| (operation.clone(), OnceLock::new()))
        .collect()
}

impl Plugin {
    pub fn attempt_descriptor(&self, operation: &str) -> Result<&AttemptDescriptor, PluginError> {
        let cache = self
            .attempt_descriptors
            .get(operation)
            .ok_or(PluginError::InvalidCapabilities)?;
        cache
            .get_or_init(|| self.describe_attempt(operation).map_err(|_| ()))
            .as_ref()
            .map_err(|_| PluginError::InvalidCapabilities)
    }

    pub fn validate_attempt_contract(&self) -> Result<(), PluginError> {
        for operation in &self.manifest.operations {
            self.attempt_descriptor(operation)?;
        }
        Ok(())
    }

    fn describe_attempt(&self, operation: &str) -> Result<AttemptDescriptor, PluginError> {
        let required = [
            "attempt.body",
            "attempt.target",
            "attempt.headers",
            ATTEMPT_DESCRIBE,
        ];
        if required.iter().any(|command| !self.has_command(command))
            || (operation == "images_edit"
                && ["attempt.image_edit_plan", "attempt.image_part_plan"]
                    .iter()
                    .any(|command| !self.has_command(command)))
        {
            return Err(PluginError::InvalidCapabilities);
        }
        let output = self.call(ATTEMPT_DESCRIBE, &json!({"operation": operation}), &[])?;
        if !output.body.is_empty() {
            return Err(PluginError::InvalidCapabilities);
        }
        let descriptor = serde_json::from_value::<AttemptDescriptor>(output.metadata.clone())
            .map_err(|_| PluginError::InvalidCapabilities)?;
        if !descriptor.validate_for_operation(operation)
            || matches!(&descriptor.usage, Some(UsageDescriptor::Plugin { .. }))
                && !self.has_command(USAGE_PARSE)
            || self.manifest.id == "codex"
                && descriptor
                    .protocols
                    .iter()
                    .any(|entry| entry.response != ResponseMode::Passthrough)
            || descriptor
                .protocols
                .iter()
                .any(|entry| match entry.response {
                    ResponseMode::Passthrough => false,
                    ResponseMode::Json => !self.has_command(RESPONSE_JSON),
                    ResponseMode::Sse => !self.has_command(RESPONSE_EVENT),
                })
        {
            return Err(PluginError::InvalidCapabilities);
        }
        Ok(descriptor)
    }

    fn has_command(&self, command: &str) -> bool {
        self.manifest.commands.iter().any(|entry| entry == command)
    }
}
