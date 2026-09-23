use chrono::DateTime;
use rust_decimal::Decimal;
use serde_json::json;

use super::*;
use crate::persistence::{ModelRuleRouteCandidate, ModelRuleRoutingTier};

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn group(value: u128, format: ApiFormat) -> LegacyGroupTarget {
    LegacyGroupTarget {
        id: id(value),
        management_group_id: id(900),
        api_format: format,
    }
}

fn channel(
    value: u128,
    group_id: u128,
    format: ApiFormat,
    operations: &[ApiOperation],
) -> LegacyChannelTarget {
    LegacyChannelTarget {
        id: id(value),
        group_id: id(group_id),
        logical_channel_id: id(800),
        api_format: format,
        capabilities: operations
            .iter()
            .enumerate()
            .map(|(index, operation)| LegacyCapabilityTarget {
                id: id(value * 10 + index as u128),
                operation: *operation,
            })
            .collect(),
    }
}

fn candidate(channel: u128, model: &str, weight: i32) -> ModelRuleRouteCandidate {
    ModelRuleRouteCandidate {
        channel_id: id(channel),
        upstream_model: model.into(),
        weight,
    }
}

fn tier(priority: i32, candidates: Vec<ModelRuleRouteCandidate>) -> ModelRuleRoutingTier {
    ModelRuleRoutingTier {
        priority,
        selection_strategy: "weighted_round_robin".into(),
        candidates,
    }
}

fn rule(format: ApiFormat, candidates: Vec<ModelRuleRouteCandidate>) -> ModelRuleRecord {
    ModelRuleRecord {
        id: id(1000),
        client_model: "priced-client-model".into(),
        api_format: format.as_str().into(),
        api_operation: ApiOperation::legacy_default(format),
        model_id: id(1001),
        model_enabled: true,
        model_currency: "USD".into(),
        price_unit_tokens: 1_000_000,
        price_effective_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        input_unit_price: Decimal::ONE,
        cached_input_unit_price: Decimal::ZERO,
        cache_write_unit_price: Decimal::ZERO,
        output_unit_price: Decimal::ONE,
        advanced_billing: json!({}),
        routing_tiers: vec![tier(0, candidates)],
        enabled: true,
    }
}

fn codex_index() -> CapabilityCutoverIndex {
    CapabilityCutoverIndex::new(
        vec![
            group(1, ApiFormat::OpenAiResponses),
            group(2, ApiFormat::OpenAiImages),
        ],
        vec![
            channel(
                10,
                1,
                ApiFormat::OpenAiResponses,
                &[ApiOperation::Responses, ApiOperation::StandaloneWebSearch],
            ),
            channel(
                20,
                2,
                ApiFormat::OpenAiImages,
                &[ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
            ),
        ],
    )
    .unwrap()
}

#[test]
fn image_rules_split_into_disjoint_operation_pools_without_doubling_weights() {
    let index = codex_index();
    let mut original = rule(
        ApiFormat::OpenAiImages,
        vec![candidate(20, "wire-a", 2), candidate(20, "wire-b", 7)],
    );
    original
        .routing_tiers
        .push(tier(10, vec![candidate(20, "wire-a", 3)]));
    let rewritten = index.rewrite_rules(&[original.clone()]).unwrap();
    assert_eq!(rewritten.len(), 2);
    for result in &rewritten {
        assert_eq!(result.model_id, original.model_id);
        assert_eq!(result.legacy_rule_id, original.id);
        assert!(result.enabled);
        let target = match result.operation {
            ApiOperation::ImagesGeneration => {
                assert_eq!(result.id, original.id);
                id(200)
            }
            ApiOperation::ImagesEdit => {
                assert_ne!(result.id, original.id);
                id(201)
            }
            _ => panic!("unexpected operation"),
        };
        assert_eq!(result.routing_tiers.len(), 2);
        for (before, after) in original.routing_tiers.iter().zip(&result.routing_tiers) {
            assert_eq!(after.priority, before.priority);
            assert_eq!(after.selection_strategy, before.selection_strategy);
            assert_eq!(after.candidates.len(), before.candidates.len());
            for (old, new) in before.candidates.iter().zip(&after.candidates) {
                assert_eq!(new.capability_id, target);
                assert_eq!(new.upstream_model, old.upstream_model);
                assert_eq!(new.weight, old.weight);
            }
        }
    }
    assert_eq!(rewritten, index.rewrite_rules(&[original]).unwrap());
}

#[test]
fn response_and_search_targets_have_separate_pools_without_losing_wire_models() {
    let rewritten = codex_index()
        .rewrite_rules(&[rule(
            ApiFormat::OpenAiResponses,
            vec![candidate(10, "wire-a", 5)],
        )])
        .unwrap();
    assert_eq!(rewritten.len(), 2);
    let responses = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::Responses)
        .unwrap();
    let search = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::StandaloneWebSearch)
        .unwrap();
    assert_eq!(
        responses.routing_tiers[0].candidates[0].upstream_model,
        "wire-a"
    );
    assert_eq!(
        search.routing_tiers[0].candidates[0],
        CapabilityRouteCandidate {
            capability_id: id(101),
            upstream_model: "wire-a".into(),
            weight: 5,
        }
    );
    assert_eq!(search.model_id, responses.model_id);
}

#[test]
fn search_only_retains_candidates_that_provided_search_and_keeps_tier_priorities() {
    let mut ordinary = channel(
        30,
        1,
        ApiFormat::OpenAiResponses,
        &[ApiOperation::Responses],
    );
    ordinary.logical_channel_id = id(801);
    let index = CapabilityCutoverIndex::new(
        vec![group(1, ApiFormat::OpenAiResponses)],
        vec![
            channel(
                10,
                1,
                ApiFormat::OpenAiResponses,
                &[ApiOperation::Responses, ApiOperation::StandaloneWebSearch],
            ),
            ordinary,
        ],
    )
    .unwrap();
    let mut original = rule(ApiFormat::OpenAiResponses, vec![candidate(30, "wire-a", 7)]);
    original.routing_tiers.push(tier(
        10,
        vec![candidate(10, "wire-a", 2), candidate(30, "wire-a", 4)],
    ));
    let rewritten = index.rewrite_rules(&[original]).unwrap();
    let search = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::StandaloneWebSearch)
        .unwrap();
    assert_eq!(search.routing_tiers.len(), 1);
    assert_eq!(search.routing_tiers[0].priority, 10);
    assert_eq!(search.routing_tiers[0].candidates.len(), 1);
    assert_eq!(search.routing_tiers[0].candidates[0].weight, 2);
}

#[test]
fn search_keeps_distinct_wire_targets_instead_of_collapsing_their_weights() {
    let index = codex_index();
    let rewritten = index
        .rewrite_rules(&[rule(
            ApiFormat::OpenAiResponses,
            vec![candidate(10, "wire-a", 2), candidate(10, "wire-b", 3)],
        )])
        .unwrap();
    let search = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::StandaloneWebSearch)
        .unwrap();
    assert_eq!(
        search.routing_tiers[0].candidates,
        vec![
            CapabilityRouteCandidate {
                capability_id: id(101),
                upstream_model: "wire-a".into(),
                weight: 2
            },
            CapabilityRouteCandidate {
                capability_id: id(101),
                upstream_model: "wire-b".into(),
                weight: 3
            },
        ]
    );
}

#[test]
fn cross_tier_search_reuse_preserves_each_wire_target() {
    let mut original = rule(ApiFormat::OpenAiResponses, vec![candidate(10, "wire-a", 2)]);
    original
        .routing_tiers
        .push(tier(10, vec![candidate(10, "wire-b", 3)]));
    let rewritten = codex_index().rewrite_rules(&[original]).unwrap();
    let search = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::StandaloneWebSearch)
        .unwrap();
    assert_eq!(search.routing_tiers.len(), 2);
    assert_eq!(search.routing_tiers[0].candidates[0].weight, 2);
    assert_eq!(search.routing_tiers[1].candidates[0].weight, 3);
    assert_eq!(
        search.routing_tiers[0].candidates[0].upstream_model,
        "wire-a"
    );
    assert_eq!(
        search.routing_tiers[1].candidates[0].upstream_model,
        "wire-b"
    );
}

#[test]
fn ordinary_search_aliases_keep_the_current_forwarding_contract() {
    let index = CapabilityCutoverIndex::new(
        vec![group(1, ApiFormat::OpenAiResponses)],
        vec![channel(
            10,
            1,
            ApiFormat::OpenAiResponses,
            &[ApiOperation::Responses, ApiOperation::StandaloneWebSearch],
        )],
    )
    .unwrap();
    let rewritten = index
        .rewrite_rules(&[rule(
            ApiFormat::OpenAiResponses,
            vec![candidate(10, "responses-model", 1)],
        )])
        .unwrap();
    let search = rewritten
        .iter()
        .find(|rule| rule.operation == ApiOperation::StandaloneWebSearch)
        .unwrap();
    assert_eq!(
        search.routing_tiers[0].candidates[0].upstream_model,
        "responses-model"
    );
}

#[test]
fn disabled_empty_drafts_remain_disabled_without_inventing_routes() {
    let index = codex_index();
    let mut original = rule(ApiFormat::OpenAiImages, vec![]);
    original.routing_tiers.clear();
    assert!(index.rewrite_rules(&[original.clone()]).is_err());
    original.enabled = false;
    let rewritten = index.rewrite_rules(&[original]).unwrap();
    assert_eq!(rewritten.len(), 2);
    assert!(
        rewritten
            .iter()
            .all(|rule| !rule.enabled && rule.routing_tiers.is_empty())
    );
}

#[test]
fn unknown_and_cross_format_targets_are_never_guessed() {
    let index = codex_index();
    for channel in [20, 404] {
        assert_eq!(
            index
                .rewrite_rules(&[rule(
                    ApiFormat::OpenAiResponses,
                    vec![candidate(channel, "wire-a", 1)]
                )])
                .unwrap_err(),
            CapabilityCutoverError::InvalidRouteTarget {
                rule_id: id(1000),
                channel_id: id(channel)
            },
        );
    }
}

#[test]
fn invalid_legacy_candidate_and_tier_metadata_are_rejected() {
    let index = codex_index();
    let base = rule(ApiFormat::OpenAiImages, vec![candidate(20, "wire-a", 2)]);
    let mut bad_rules = Vec::new();
    let mut value = base.clone();
    value.routing_tiers[0].candidates[0].weight = 0;
    bad_rules.push(value);
    let mut value = base.clone();
    value.routing_tiers[0]
        .candidates
        .push(candidate(20, "wire-a", 7));
    bad_rules.push(value);
    let mut value = base.clone();
    value.routing_tiers.push(value.routing_tiers[0].clone());
    bad_rules.push(value);
    let mut value = base.clone();
    value.routing_tiers[0].priority = -1;
    bad_rules.push(value);
    let mut value = base;
    value.routing_tiers[0].selection_strategy = "unknown".into();
    bad_rules.push(value);
    for value in bad_rules {
        assert!(matches!(
            index.rewrite_rules(&[value]),
            Err(CapabilityCutoverError::InvalidRule { .. })
        ));
    }
}

#[test]
fn malformed_or_partial_topologies_are_rejected_before_any_rule_is_rewritten() {
    let group = group(1, ApiFormat::OpenAiImages);
    let complete = channel(
        20,
        1,
        ApiFormat::OpenAiImages,
        &[ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
    );
    let mut incomplete = complete.clone();
    incomplete.capabilities.pop();
    let mut repeated = complete.clone();
    repeated.capabilities[1].id = repeated.capabilities[0].id;
    let mut wrong_group = complete.clone();
    wrong_group.group_id = id(404);
    let mut wrong_format = complete.clone();
    wrong_format.capabilities[1].operation = ApiOperation::Responses;
    for value in [incomplete, repeated, wrong_group, wrong_format] {
        assert!(CapabilityCutoverIndex::new(vec![group.clone()], vec![value]).is_err());
    }
    assert!(
        CapabilityCutoverIndex::new(vec![group.clone(), group.clone()], vec![complete.clone()])
            .is_err()
    );
    assert!(CapabilityCutoverIndex::new(vec![group], vec![complete.clone(), complete]).is_err());
}

#[test]
fn logical_identity_cannot_merge_unrelated_groups_or_duplicate_operations() {
    let response = channel(
        10,
        1,
        ApiFormat::OpenAiResponses,
        &[ApiOperation::Responses],
    );
    let images = channel(
        20,
        2,
        ApiFormat::OpenAiImages,
        &[ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
    );
    let mut images_group = group(2, ApiFormat::OpenAiImages);
    images_group.management_group_id = id(901);
    assert!(
        CapabilityCutoverIndex::new(
            vec![group(1, ApiFormat::OpenAiResponses), images_group],
            vec![response.clone(), images],
        )
        .is_err()
    );
    let other_response = channel(
        30,
        1,
        ApiFormat::OpenAiResponses,
        &[ApiOperation::Responses],
    );
    assert!(
        CapabilityCutoverIndex::new(
            vec![group(1, ApiFormat::OpenAiResponses)],
            vec![response, other_response],
        )
        .is_err()
    );
}

#[test]
fn group_merging_does_not_grant_images_to_response_only_keys() {
    let index = codex_index();
    let grants = index
        .rewrite_target_grants(&[id(1)], &[], Some(&[ApiFormat::OpenAiResponses]))
        .unwrap();
    assert_eq!(grants.len(), 2);
    assert_eq!(
        grants.iter().map(|g| g.capability_id).collect::<Vec<_>>(),
        vec![id(100), id(101)]
    );
    assert!(
        grants
            .iter()
            .all(|grant| grant.origin == CapabilityGrantOrigin::Group(id(900)))
    );
    let images = index
        .rewrite_target_grants(&[id(2)], &[], Some(&[ApiFormat::OpenAiImages]))
        .unwrap();
    assert_eq!(
        images.iter().map(|g| g.capability_id).collect::<Vec<_>>(),
        vec![id(200), id(201)]
    );
}

#[test]
fn grant_origins_survive_deduplication_and_no_formats_does_not_mean_all_formats() {
    let index = codex_index();
    let grants = index
        .rewrite_target_grants(&[id(1), id(1)], &[id(10)], None)
        .unwrap();
    assert_eq!(grants.len(), 4);
    assert!(
        grants
            .iter()
            .any(|g| g.origin == CapabilityGrantOrigin::Group(id(900)))
    );
    assert!(
        grants
            .iter()
            .any(|g| g.origin == CapabilityGrantOrigin::Channel(id(800)))
    );
    assert!(
        index
            .rewrite_target_grants(&[id(1), id(2)], &[id(10)], Some(&[]))
            .unwrap()
            .is_empty()
    );
    for (groups, channels) in [(vec![id(404)], vec![]), (vec![], vec![id(404)])] {
        assert_eq!(
            index
                .rewrite_target_grants(&groups, &channels, None)
                .unwrap_err(),
            CapabilityCutoverError::UnknownGrantTarget { id: id(404) },
        );
    }
}

#[test]
fn unrelated_legacy_targets_never_gain_grants_and_input_order_does_not_change_output() {
    let groups = vec![
        group(1, ApiFormat::OpenAiResponses),
        group(2, ApiFormat::OpenAiImages),
    ];
    let channels = vec![
        channel(
            10,
            1,
            ApiFormat::OpenAiResponses,
            &[ApiOperation::Responses],
        ),
        channel(
            20,
            2,
            ApiFormat::OpenAiImages,
            &[ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
        ),
    ];
    let index = CapabilityCutoverIndex::new(groups.clone(), channels.clone()).unwrap();
    let reversed = CapabilityCutoverIndex::new(
        groups.into_iter().rev().collect(),
        channels.into_iter().rev().collect(),
    )
    .unwrap();
    let grants = index.rewrite_target_grants(&[id(1)], &[], None).unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].capability_id, id(100));
    assert_eq!(
        grants,
        reversed.rewrite_target_grants(&[id(1)], &[], None).unwrap()
    );
}

#[test]
fn duplicate_generated_or_preserved_rule_ids_are_rejected() {
    let index = codex_index();
    let original = rule(ApiFormat::OpenAiImages, vec![candidate(20, "wire-a", 1)]);
    let plans = index
        .rewrite_rules(std::slice::from_ref(&original))
        .unwrap();
    let edit_id = plans
        .iter()
        .find(|p| p.operation == ApiOperation::ImagesEdit)
        .unwrap()
        .id;
    let mut collision = original.clone();
    collision.id = edit_id;
    collision.model_id = id(1002);
    assert!(matches!(
        index.rewrite_rules(&[original, collision]),
        Err(CapabilityCutoverError::InvalidRule { .. }),
    ));
}

#[test]
fn a_priced_model_cannot_acquire_duplicate_rules_for_the_same_operation() {
    let original = rule(ApiFormat::OpenAiImages, vec![candidate(20, "wire-a", 1)]);
    let mut duplicate = original.clone();
    duplicate.id = id(1002);
    assert!(matches!(
        codex_index().rewrite_rules(&[original, duplicate]),
        Err(CapabilityCutoverError::InvalidRule { .. }),
    ));
}

#[test]
fn validation_errors_do_not_include_rejected_wire_values() {
    let error = codex_index()
        .rewrite_rules(&[rule(
            ApiFormat::OpenAiImages,
            vec![candidate(20, "private-wire-value", 0)],
        )])
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains("private-wire-value"));
}
