//! Plan additive routing edits against the topology read inside a management transaction.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ChannelBatchChanges, ChannelBatchUpdateInput, ChannelBatchUpdateTarget, ModelInput,
    OperationCandidateInput, OperationRuleInput, OperationTierInput, RepositoryError,
    UpstreamTopologyRecords,
};
use crate::domain::ApiOperation;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OnboardingModel<C> {
    Existing { id: Uuid, updated_at: DateTime<Utc> },
    Create(Box<ModelInput>),
    Catalog(C),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOnboardingInput<C> {
    pub model: OnboardingModel<C>,
    pub operations: Vec<OnboardingOperation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnboardingOperation {
    operation: ApiOperation,
    #[serde(deserialize_with = "Option::deserialize")]
    rule_updated_at: Option<DateTime<Utc>>,
    candidates: Vec<OnboardingCandidate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OnboardingCandidate {
    capability_id: Uuid,
    updated_at: DateTime<Utc>,
    upstream_model: String,
    priority: i32,
    register_model: bool,
}

impl<C> ModelOnboardingInput<C> {
    pub(crate) fn validate(&self) -> Result<(), RepositoryError> {
        let mut operations = HashSet::new();
        let mut versions = HashMap::new();
        if !(1..=6).contains(&self.operations.len())
            || self
                .operations
                .iter()
                .map(|op| op.candidates.len())
                .sum::<usize>()
                > 100
        {
            return Err(RepositoryError::Validation);
        }
        for op in &self.operations {
            let mut targets = HashSet::new();
            if !operations.insert(op.operation) || op.candidates.is_empty() {
                return Err(RepositoryError::Validation);
            }
            for target in &op.candidates {
                if target.priority < 0
                    || target.upstream_model.trim().is_empty()
                    || target.upstream_model.chars().count() > 300
                    || !targets.insert((
                        target.priority,
                        target.capability_id,
                        &target.upstream_model,
                    ))
                    || versions
                        .insert(target.capability_id, target.updated_at)
                        .is_some_and(|v| v != target.updated_at)
                {
                    return Err(RepositoryError::Validation);
                }
            }
        }
        Ok(())
    }
}

pub(super) fn capability_ids(operations: &[OnboardingOperation]) -> Vec<Uuid> {
    operations
        .iter()
        .flat_map(|op| op.candidates.iter().map(|target| target.capability_id))
        .collect()
}

pub(super) struct RouteChanges {
    pub registrations: BTreeMap<Uuid, (DateTime<Utc>, Vec<String>)>,
    pub rules: Vec<RuleAddition>,
}

pub(super) fn register_model(
    id: Uuid,
    updated_at: DateTime<Utc>,
    name: String,
) -> ChannelBatchUpdateInput {
    ChannelBatchUpdateInput {
        items: vec![ChannelBatchUpdateTarget { id, updated_at }],
        changes: ChannelBatchChanges {
            enabled: None,
            auto_disable_allowed: None,
            billing_multiplier: None,
            append_model: Some(name),
        },
    }
}

pub(super) struct RuleAddition {
    pub id: Uuid,
    pub input: OperationRuleInput,
    pub expected: Option<DateTime<Utc>>,
}

pub(super) fn route_mutations(
    profile_id: Uuid,
    operations: Vec<OnboardingOperation>,
    topology: &UpstreamTopologyRecords,
) -> Result<RouteChanges, RepositoryError> {
    let mut registrations = BTreeMap::new();
    let mut rules = Vec::new();
    for op in operations {
        let existing = topology.operation_rules.iter().find(|rule| {
            rule.model_routing_profile_id == profile_id && rule.operation == op.operation
        });
        if existing.map(|r| r.updated_at) != op.rule_updated_at {
            return Err(RepositoryError::Conflict);
        }
        let mut additions = BTreeMap::new();
        for target in op.candidates {
            let capability = topology
                .channel_capabilities
                .iter()
                .find(|cap| cap.id == target.capability_id && cap.deleted_at.is_none())
                .ok_or(RepositoryError::Conflict)?;
            if capability.updated_at != target.updated_at {
                return Err(RepositoryError::Conflict);
            }
            if capability.settings.operation != op.operation {
                return Err(RepositoryError::RoutingDependencyInvalid);
            }
            if !capability
                .settings
                .available_models
                .contains(&target.upstream_model)
            {
                if !target.register_model {
                    return Err(RepositoryError::RoutingDependencyInvalid);
                }
                let (_, names) = registrations
                    .entry(capability.id)
                    .or_insert_with(|| (capability.updated_at, Vec::new()));
                if !names.contains(&target.upstream_model) {
                    names.push(target.upstream_model.clone());
                }
            }
            let tier = existing.and_then(|r| {
                topology
                    .operation_tiers
                    .iter()
                    .find(|tier| tier.rule_id == r.id && tier.priority == target.priority)
            });
            if tier.is_some_and(|tier| {
                topology.operation_candidates.iter().any(|c| {
                    c.tier_id == tier.id
                        && c.capability_id == capability.id
                        && c.upstream_model == target.upstream_model
                })
            }) {
                continue;
            }
            additions
                .entry(target.priority)
                .or_insert_with(|| OperationTierInput {
                    priority: target.priority,
                    selection_strategy: tier
                        .map_or("weighted_random", |t| t.strategy.as_str())
                        .into(),
                    candidates: Vec::new(),
                })
                .candidates
                .push(OperationCandidateInput {
                    capability_id: capability.id,
                    upstream_model: target.upstream_model,
                    weight: 1,
                });
        }
        if !additions.is_empty() {
            rules.push(RuleAddition {
                id: existing.map_or_else(Uuid::new_v4, |r| r.id),
                input: OperationRuleInput {
                    model_routing_profile_id: profile_id,
                    operation: op.operation,
                    enabled: existing.is_none_or(|r| r.enabled),
                    routing_tiers: additions.into_values().collect(),
                },
                expected: op.rule_updated_at,
            });
        }
    }
    Ok(RouteChanges {
        registrations,
        rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn request() -> Value {
        let id = Uuid::nil();
        let version = "2026-01-01T00:00:00Z";
        json!({"model":{"existing":{"id":id,"updated_at":version}}, "operations":[{
            "operation":"responses", "rule_updated_at":null,
            "candidates":[{"capability_id":id, "updated_at":version, "upstream_model":"wire", "priority":0, "register_model":false}]
        }]})
    }

    fn valid(value: Value) -> bool {
        serde_json::from_value::<ModelOnboardingInput<Value>>(value)
            .is_ok_and(|input| input.validate().is_ok())
    }

    #[test]
    fn reviewed_versions_and_model_choice_are_explicit() {
        let mut value = request();
        assert!(valid(value.clone()));
        value["model"]["catalog"] = json!({});
        assert!(!valid(value));
        let mut value = request();
        value["operations"][0]
            .as_object_mut()
            .unwrap()
            .remove("rule_updated_at");
        assert!(!valid(value));
    }

    #[test]
    fn bound_and_deduplicate_the_whole_plan() {
        let mut value = request();
        let op = value["operations"][0].clone();
        let target = op["candidates"][0].clone();
        value["operations"] = json!([op, op]);
        assert!(!valid(value.clone()));
        value["operations"] = json!([]);
        assert!(!valid(value.clone()));
        value["operations"] = json!([op]);
        value["operations"][0]["candidates"] = json!([target, target]);
        assert!(!valid(value.clone()));
        value["operations"][0]["candidates"][1]["priority"] = json!(2);
        assert!(valid(value.clone()));
        value["operations"][0]["candidates"][1]["updated_at"] = json!("2026-01-02T00:00:00Z");
        assert!(!valid(value.clone()));
        let candidates = (0..100)
            .map(|priority| {
                let mut row = target.clone();
                row["priority"] = json!(priority);
                row
            })
            .collect::<Vec<_>>();
        value["operations"][0]["candidates"] = json!(candidates);
        assert!(valid(value.clone()));
        let mut second = op;
        second["operation"] = json!("responses-ws");
        value["operations"].as_array_mut().unwrap().push(second);
        assert!(!valid(value));
    }
}
