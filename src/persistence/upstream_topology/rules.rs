//! Atomic operation-rule graph replacement. The caller owns validation,
//! audit, commit, and publication of the complete pending runtime snapshot.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{OperationRuleInput, OperationRuleRecord, UpstreamTopologyRecords};
use crate::{
    domain::SelectionStrategy,
    persistence::{MutationResult, RepositoryError},
};

fn validate_graph(input: &OperationRuleInput) -> Result<(), RepositoryError> {
    if input.enabled && input.routing_tiers.is_empty() {
        return Err(RepositoryError::Validation);
    }
    let mut priorities = HashSet::new();
    for tier in &input.routing_tiers {
        if tier.priority < 0
            || !priorities.insert(tier.priority)
            || SelectionStrategy::parse(&tier.selection_strategy).is_none()
            || tier.candidates.is_empty()
        {
            return Err(RepositoryError::Validation);
        }
        let mut candidates = HashSet::new();
        for candidate in &tier.candidates {
            if candidate.weight <= 0
                || candidate.upstream_model.trim().is_empty()
                || candidate.upstream_model.chars().count() > 300
                || !candidates.insert((candidate.capability_id, &candidate.upstream_model))
            {
                return Err(RepositoryError::Validation);
            }
        }
    }
    Ok(())
}

fn validate_replacement(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &OperationRuleInput,
    expected: Option<DateTime<Utc>>,
) -> Result<(), RepositoryError> {
    validate_graph(input)?;
    let existing = topology.operation_rules.iter().find(|rule| rule.id == id);
    match (existing, expected) {
        (None, Some(_)) => return Err(RepositoryError::NotFound),
        (Some(_), None) => return Err(RepositoryError::Conflict),
        (Some(rule), Some(expected)) => {
            if rule.updated_at != expected {
                return Err(RepositoryError::Conflict);
            }
            if rule.model_routing_profile_id != input.model_routing_profile_id
                || rule.operation != input.operation
            {
                return Err(RepositoryError::Validation);
            }
        }
        (None, None) => {
            if topology.operation_rules.iter().any(|rule| {
                rule.model_routing_profile_id == input.model_routing_profile_id
                    && rule.operation == input.operation
            }) {
                return Err(RepositoryError::Conflict);
            }
        }
    }
    for candidate in input.routing_tiers.iter().flat_map(|tier| &tier.candidates) {
        let capability = topology
            .channel_capabilities
            .iter()
            .find(|capability| {
                capability.id == candidate.capability_id && capability.deleted_at.is_none()
            })
            .ok_or(RepositoryError::RoutingDependencyInvalid)?;
        if capability.settings.operation != input.operation
            || !capability
                .settings
                .available_models
                .contains(&candidate.upstream_model)
        {
            return Err(RepositoryError::RoutingDependencyInvalid);
        }
    }
    Ok(())
}

fn audit(topology: &UpstreamTopologyRecords, id: Uuid) -> serde_json::Value {
    let rule = topology.operation_rules.iter().find(|rule| rule.id == id);
    let tiers = topology
        .operation_tiers
        .iter()
        .filter(|tier| tier.rule_id == id)
        .collect::<Vec<_>>();
    let tier_ids = tiers.iter().map(|tier| tier.id).collect::<HashSet<_>>();
    let candidates = topology
        .operation_candidates
        .iter()
        .filter(|candidate| tier_ids.contains(&candidate.tier_id))
        .collect::<Vec<_>>();
    json!({ "rule": rule, "tiers": tiers, "candidates": candidates })
}

fn result(
    before: &UpstreamTopologyRecords,
    after: &UpstreamTopologyRecords,
    id: Uuid,
    creating: bool,
) -> Result<MutationResult, RepositoryError> {
    let record = after
        .operation_rules
        .iter()
        .find(|rule| rule.id == id)
        .ok_or(RepositoryError::NotFound)?;
    Ok(MutationResult {
        id,
        object_type: "model_operation_rule",
        action: if creating { "create" } else { "update" },
        before_redacted: audit(before, id),
        after_redacted: audit(after, id),
        created_secret: None,
        reason: None,
        updated_at: record.updated_at,
        correlation_id: None,
    })
}

/// Must run in the coordinator's serializable control-plane transaction.
/// Replacing a rule never creates, enables, or grants a capability.
pub async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &OperationRuleInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM model_routing_profiles WHERE id=$1 FOR UPDATE")
        .bind(input.model_routing_profile_id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    let profile_live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
         WHERE p.id=$1 AND m.deleted_at IS NULL)",
    )
    .bind(input.model_routing_profile_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !profile_live {
        return Err(RepositoryError::Validation);
    }
    if let Some(expected) = expected {
        let changed = sqlx::query(
            "UPDATE model_operation_rules SET enabled=$2 WHERE id=$1 AND updated_at=$3",
        )
        .bind(id)
        .bind(input.enabled)
        .bind(expected)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(RepositoryError::Conflict);
        }
        sqlx::query("DELETE FROM model_capability_tiers WHERE rule_id=$1")
            .bind(id)
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO model_operation_rules (id,model_routing_profile_id,operation,enabled)
             VALUES ($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(input.model_routing_profile_id)
        .bind(input.operation.as_str())
        .bind(input.enabled)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO model_rule_identity_registry (id,label,created_at,canonical_rule_id)
             SELECT r.id,m.source_model_id,r.created_at,r.id FROM model_operation_rules r
             JOIN model_routing_profiles p ON p.id=r.model_routing_profile_id
             JOIN models m ON m.id=p.model_id WHERE r.id=$1",
        )
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    }
    for tier in &input.routing_tiers {
        let tier_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO model_capability_tiers (id,rule_id,operation,priority,strategy)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(tier_id)
        .bind(id)
        .bind(input.operation.as_str())
        .bind(tier.priority)
        .bind(&tier.selection_strategy)
        .execute(&mut **transaction)
        .await?;
        for candidate in &tier.candidates {
            sqlx::query(
                "INSERT INTO model_capability_candidates (tier_id,operation,capability_id,upstream_model,weight)
                 VALUES ($1,$2,$3,$4,$5)",
            ).bind(tier_id).bind(input.operation.as_str()).bind(candidate.capability_id)
                .bind(&candidate.upstream_model).bind(candidate.weight)
                .execute(&mut **transaction).await?;
        }
    }
    let after = super::pg_load(transaction).await?;
    result(&before, &after, id, expected.is_none())
}

#[cfg(feature = "sqlite-backend")]
/// Must run in the database owner's sole-writer transaction.
pub async fn sqlite_save(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    input: &OperationRuleInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    let profile_live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
         WHERE p.id=? AND m.deleted_at IS NULL)",
    )
    .bind(SqliteUuid(input.model_routing_profile_id))
    .fetch_one(&mut **transaction)
    .await?;
    if !profile_live {
        return Err(RepositoryError::Validation);
    }
    if let Some(expected) = expected {
        let changed = sqlx::query(
            "UPDATE model_operation_rules SET enabled=?,updated_at=ag_now() WHERE id=? AND updated_at=?",
        ).bind(input.enabled).bind(SqliteUuid(id)).bind(SqliteTimestamp(expected))
            .execute(&mut **transaction).await?.rows_affected();
        if changed != 1 {
            return Err(RepositoryError::Conflict);
        }
        sqlx::query("DELETE FROM model_capability_tiers WHERE rule_id=?")
            .bind(SqliteUuid(id))
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO model_operation_rules (id,model_routing_profile_id,operation,enabled)
             VALUES (?,?,?,?)",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(input.model_routing_profile_id))
        .bind(input.operation.as_str())
        .bind(input.enabled)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO model_rule_identity_registry (id,label,created_at,canonical_rule_id)
             SELECT r.id,m.source_model_id,r.created_at,r.id FROM model_operation_rules r
             JOIN model_routing_profiles p ON p.id=r.model_routing_profile_id
             JOIN models m ON m.id=p.model_id WHERE r.id=?",
        )
        .bind(SqliteUuid(id))
        .execute(&mut **transaction)
        .await?;
    }
    for tier in &input.routing_tiers {
        let tier_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO model_capability_tiers (id,rule_id,operation,priority,strategy)
             VALUES (?,?,?,?,?)",
        )
        .bind(SqliteUuid(tier_id))
        .bind(SqliteUuid(id))
        .bind(input.operation.as_str())
        .bind(tier.priority)
        .bind(&tier.selection_strategy)
        .execute(&mut **transaction)
        .await?;
        for candidate in &tier.candidates {
            sqlx::query(
                "INSERT INTO model_capability_candidates (tier_id,operation,capability_id,upstream_model,weight)
                 VALUES (?,?,?,?,?)",
            ).bind(SqliteUuid(tier_id)).bind(input.operation.as_str()).bind(SqliteUuid(candidate.capability_id))
                .bind(&candidate.upstream_model).bind(candidate.weight)
                .execute(&mut **transaction).await?;
        }
    }
    let after = super::sqlite_load(transaction).await?;
    result(&before, &after, id, expected.is_none())
}

pub fn rule_input(
    topology: &UpstreamTopologyRecords,
    rule: &OperationRuleRecord,
) -> OperationRuleInput {
    let mut routing_tiers = topology
        .operation_tiers
        .iter()
        .filter(|tier| tier.rule_id == rule.id)
        .map(|tier| super::OperationTierInput {
            priority: tier.priority,
            selection_strategy: tier.strategy.clone(),
            candidates: topology
                .operation_candidates
                .iter()
                .filter(|candidate| candidate.tier_id == tier.id)
                .map(|candidate| super::OperationCandidateInput {
                    capability_id: candidate.capability_id,
                    upstream_model: candidate.upstream_model.clone(),
                    weight: candidate.weight,
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    routing_tiers.sort_by_key(|tier| tier.priority);
    OperationRuleInput {
        model_routing_profile_id: rule.model_routing_profile_id,
        operation: rule.operation,
        enabled: rule.enabled,
        routing_tiers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ApiOperation;
    use crate::persistence::upstream_topology::{OperationCandidateInput, OperationTierInput};

    fn input() -> OperationRuleInput {
        OperationRuleInput {
            model_routing_profile_id: Uuid::from_u128(1),
            operation: ApiOperation::Responses,
            enabled: true,
            routing_tiers: vec![OperationTierInput {
                priority: 0,
                selection_strategy: "weighted_random".into(),
                candidates: vec![OperationCandidateInput {
                    capability_id: Uuid::from_u128(2),
                    upstream_model: "wire".into(),
                    weight: 3,
                }],
            }],
        }
    }

    #[test]
    fn graph_validation_preserves_pair_and_tier_semantics() {
        let mut value = input();
        assert!(validate_graph(&value).is_ok());
        let duplicate = value.routing_tiers[0].candidates[0].clone();
        value.routing_tiers[0].candidates.push(duplicate);
        assert!(validate_graph(&value).is_err());
        value.routing_tiers[0].candidates[1].upstream_model = "another-wire-model".into();
        assert!(validate_graph(&value).is_ok());
        value.routing_tiers.push(value.routing_tiers[0].clone());
        assert!(validate_graph(&value).is_err());
        value.routing_tiers[1].priority = 1;
        assert!(validate_graph(&value).is_ok());
        value.routing_tiers[1].candidates.clear();
        assert!(validate_graph(&value).is_err());
        value.routing_tiers.clear();
        assert!(validate_graph(&value).is_err());
        value.enabled = false;
        assert!(validate_graph(&value).is_ok());
    }

    #[test]
    fn rule_owner_and_operation_cannot_be_rebound() {
        let rule_id = Uuid::from_u128(3);
        let now = Utc::now();
        let mut value = input();
        value.enabled = false;
        value.routing_tiers.clear();
        let topology = UpstreamTopologyRecords {
            operation_rules: vec![OperationRuleRecord {
                id: rule_id,
                model_routing_profile_id: value.model_routing_profile_id,
                operation: value.operation,
                enabled: false,
                created_at: now,
                updated_at: now,
            }],
            ..Default::default()
        };
        assert!(validate_replacement(&topology, rule_id, &value, Some(now)).is_ok());
        assert!(matches!(
            validate_replacement(&topology, rule_id, &value, None),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            validate_replacement(
                &topology,
                rule_id,
                &value,
                Some(now - chrono::Duration::seconds(1))
            ),
            Err(RepositoryError::Conflict)
        ));
        value.operation = ApiOperation::StandaloneWebSearch;
        assert!(matches!(
            validate_replacement(&topology, rule_id, &value, Some(now)),
            Err(RepositoryError::Validation)
        ));
        value.operation = ApiOperation::Responses;
        value.model_routing_profile_id = Uuid::new_v4();
        assert!(matches!(
            validate_replacement(&topology, rule_id, &value, Some(now)),
            Err(RepositoryError::Validation)
        ));
    }
}
