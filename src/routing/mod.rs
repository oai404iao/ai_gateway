//! Model-rule resolution and stage-1 channel selection.

use std::sync::Arc;

use crate::domain::{
    ApiFormat, CompiledApiKey, CompiledChannel, CompiledModelRule, CompiledRuntimeConfig,
};

#[derive(Clone)]
pub struct SelectedRoute {
    pub rule: Arc<CompiledModelRule>,
    pub channel: Arc<CompiledChannel>,
}

/// Returns the sole stage-1 candidate allowed by both the route and API-key
/// group restriction. Priority, weights, and health are intentionally deferred.
#[must_use]
pub fn select(
    snapshot: &CompiledRuntimeConfig,
    key: &CompiledApiKey,
    format: ApiFormat,
    model: &str,
) -> Option<SelectedRoute> {
    let rule = snapshot.model_rule(format, model)?;
    let channel = rule
        .candidate_channel_ids()
        .iter()
        .find_map(|id| snapshot.channel(*id))
        .filter(|channel| key.permits_group(channel.group_id()))?;
    Some(SelectedRoute { rule, channel })
}
