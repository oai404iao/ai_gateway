use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use super::*;
use crate::domain::RequestCompression;
use crate::persistence::capability_cutover::legacy_settings::{
    CapabilitySettings, LogicalChannelRecord,
};
use crate::persistence::upstream_topology::{
    OperationRuleRecord, OperationTierRecord, UpstreamAccessRecord,
};

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).unwrap()
}

fn fixture(transports: &[&[CapabilityTransport]]) -> UpstreamTopologyRecords {
    UpstreamTopologyRecords {
        upstream_accesses: vec![UpstreamAccessRecord {
            id: id(1),
            name: "Access".into(),
            connector_kind: ConnectorKind::OpenAiCompatible,
            base_url: "https://upstream.example/v1".into(),
            proxy_id: None,
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            enabled: true,
            revision: id(2),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }],
        logical_channels: transports
            .iter()
            .enumerate()
            .map(|(index, _)| LogicalChannelRecord {
                id: id(10 + index as u128),
                group_id: id(3),
                access_id: id(1),
                credential_id: Some(id(4)),
                name: format!("Channel {index}"),
                enabled: true,
                binding_revision: id(5),
                created_at: at(),
                updated_at: at(),
                deleted_at: None,
            })
            .collect(),
        channel_capabilities: transports
            .iter()
            .enumerate()
            .map(|(index, transports)| ChannelCapabilityRecord {
                id: id(100 + index as u128),
                channel_id: id(10 + index as u128),
                settings: CapabilitySettings {
                    operation: ApiOperation::Responses,
                    transports: transports.to_vec(),
                    enabled: true,
                    available_models: vec!["wire".into()],
                    request_compression: RequestCompression::Default,
                    test_model: None,
                    test_pricing_model_id: None,
                    auto_disable_allowed: true,
                },
                auto_disabled: false,
                auto_disable_reason: None,
                auto_disable_at: None,
                status_statistics_enabled: true,
                config_template_id: None,
                override_document: json!({}),
                billing_multiplier: Decimal::ONE,
                revision: id(200 + index as u128),
                created_at: at(),
                updated_at: at(),
                deleted_at: None,
            })
            .collect(),
        ..Default::default()
    }
}

fn add_rule(input: &mut UpstreamTopologyRecords, tier_candidates: &[&[(u128, &str, i32)]]) {
    input.operation_rules.push(OperationRuleRecord {
        id: id(300),
        model_routing_profile_id: id(301),
        operation: ApiOperation::Responses,
        enabled: !tier_candidates.is_empty(),
        created_at: at(),
        updated_at: at(),
    });
    for (index, candidates) in tier_candidates.iter().enumerate() {
        let tier_id = id(400 + index as u128);
        input.operation_tiers.push(OperationTierRecord {
            id: tier_id,
            rule_id: id(300),
            operation: ApiOperation::Responses,
            priority: index as i32 * 5,
            strategy: "weighted_round_robin".into(),
        });
        input
            .operation_candidates
            .extend(candidates.iter().map(|(capability, model, weight)| {
                OperationCandidateRecord {
                    tier_id,
                    operation: ApiOperation::Responses,
                    capability_id: id(*capability),
                    upstream_model: (*model).into(),
                    weight: *weight,
                }
            }));
    }
}

#[test]
fn destination_wire_values_are_explicit_and_do_not_rename_credential_types() {
    assert_eq!(
        json!([
            Operation::ChatCompletion,
            Operation::Responses,
            Operation::ResponsesWs,
            Operation::WebSearch,
            Operation::ImagesEdit,
            Operation::ImagesGeneration,
        ]),
        json!([
            "chat_completion",
            "responses",
            "responses-ws",
            "web_search",
            "images_edit",
            "images_generation",
        ])
    );
    let mut input = fixture(&[&[CapabilityTransport::HttpSse, CapabilityTransport::Websocket]]);
    assert_eq!(
        json!(plan(&input).unwrap().accesses[0].connector),
        "general"
    );
    input.upstream_accesses[0].connector_kind = ConnectorKind::CodexOauth;
    let output = plan(&input).unwrap();
    assert_eq!(json!(output.accesses[0].connector), "codex");
    assert_eq!(output.capabilities.len(), 2);
    assert_eq!(input.logical_channels[0].credential_id, Some(id(4)));
    assert!(output.api_key_grants.is_empty());
    assert!(output.rules.is_empty());
}

#[test]
fn responses_preserve_http_and_websocket_eligibility_and_primary_ids() {
    use CapabilityTransport as T;
    for (transports, expected) in [
        (vec![T::HttpJson], vec![Operation::Responses]),
        (vec![T::HttpSse], vec![Operation::Responses]),
        (vec![T::HttpJson, T::HttpSse], vec![Operation::Responses]),
        (vec![T::Websocket], vec![Operation::ResponsesWs]),
        (
            vec![T::HttpJson, T::Websocket],
            vec![Operation::Responses, Operation::ResponsesWs],
        ),
        (
            vec![T::HttpSse, T::Websocket],
            vec![Operation::Responses, Operation::ResponsesWs],
        ),
    ] {
        let mut input = fixture(&[&transports]);
        add_rule(&mut input, &[&[(100, "wire", 7)]]);
        let output = plan(&input).unwrap();
        assert_eq!(
            output
                .capabilities
                .iter()
                .map(|row| row.operation)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(output.capabilities[0].id, id(100));
        assert_eq!(output.rules[0].id, id(300));
        assert_eq!(output.tiers[0].id, id(400));
        assert_eq!(output.rules.len(), expected.len());
        assert_eq!(output.candidates.len(), expected.len());
        for capability in &output.capabilities {
            assert_eq!(
                capability.clear_http_settings(),
                capability.operation == Operation::ResponsesWs
            );
        }
    }
}

#[test]
fn mixed_graph_splits_only_eligible_candidates_and_omits_empty_tiers() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpJson], &[T::Websocket], &[T::HttpSse, T::Websocket]]);
    add_rule(
        &mut input,
        &[
            &[(100, "http", 2)],
            &[(101, "ws", 3), (102, "both", 5), (102, "alias", 7)],
            &[(102, "both", 11)],
        ],
    );
    let output = plan(&input).unwrap();
    assert_eq!(output.rules.len(), 2);
    assert_eq!(output.tiers.len(), 5);
    let http_rule = output
        .rules
        .iter()
        .find(|row| row.operation == Operation::Responses)
        .unwrap();
    let ws_rule = output
        .rules
        .iter()
        .find(|row| row.operation == Operation::ResponsesWs)
        .unwrap();
    assert_eq!(http_rule.id, id(300));
    assert_ne!(ws_rule.id, http_rule.id);
    assert!(
        !output
            .tiers
            .iter()
            .any(|tier| tier.rule_id == ws_rule.id && tier.source_id == id(400))
    );
    let candidates = |op| {
        output
            .candidates
            .iter()
            .filter(|row| row.operation == op)
            .map(|row| (row.upstream_model.as_str(), row.weight))
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        candidates(Operation::Responses),
        BTreeSet::from([("http", 2), ("both", 5), ("alias", 7), ("both", 11)])
    );
    assert_eq!(
        candidates(Operation::ResponsesWs),
        BTreeSet::from([("ws", 3), ("both", 5), ("alias", 7), ("both", 11)])
    );
    for tier in &output.tiers {
        assert!(
            input
                .operation_tiers
                .iter()
                .any(|source| source.id == tier.source_id)
        );
    }
}

#[test]
fn a_single_transport_tier_keeps_its_identity_even_when_its_rule_splits() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpJson], &[T::Websocket]]);
    add_rule(&mut input, &[&[(100, "http", 2)], &[(101, "ws", 3)]]);
    let output = plan(&input).unwrap();
    assert_eq!(output.rules.len(), 2);
    assert_eq!(output.tiers.len(), 2);
    assert!(output.tiers.iter().all(|tier| tier.id == tier.source_id));
    assert_eq!(
        output
            .tiers
            .iter()
            .map(|tier| (tier.id, tier.operation))
            .collect::<Vec<_>>(),
        vec![
            (id(400), Operation::Responses),
            (id(401), Operation::ResponsesWs)
        ],
    );
}

#[test]
fn non_responses_operations_only_rename_and_never_generate_siblings() {
    use ApiOperation as A;
    use CapabilityTransport as T;
    for (before, transport, after) in [
        (A::ChatCompletions, T::HttpJson, Operation::ChatCompletion),
        (A::StandaloneWebSearch, T::HttpJson, Operation::WebSearch),
        (A::ImagesEdit, T::Multipart, Operation::ImagesEdit),
        (
            A::ImagesGeneration,
            T::HttpJson,
            Operation::ImagesGeneration,
        ),
    ] {
        let mut input = fixture(&[&[transport]]);
        input.channel_capabilities[0].settings.operation = before;
        add_rule(&mut input, &[&[(100, "wire", 7)]]);
        input.operation_rules[0].operation = before;
        input.operation_tiers[0].operation = before;
        input.operation_candidates[0].operation = before;
        let output = plan(&input).unwrap();
        assert_eq!(output.capabilities.len(), 1);
        assert_eq!(output.capabilities[0].id, id(100));
        assert_eq!(output.capabilities[0].operation, after);
        assert_eq!(output.rules.len(), 1);
        assert_eq!(output.rules[0].id, id(300));
        assert_eq!(output.rules[0].operation, after);
        assert_eq!(output.tiers[0].id, id(400));
        assert_eq!(output.candidates[0].operation, after);
    }
}

#[test]
fn fixed_grants_keep_provenance_and_never_authorize_ungranted_capabilities() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpSse, T::Websocket], &[T::HttpJson, T::Websocket]]);
    for (kind, origin) in [
        (GrantOriginKind::Group, 999),
        (GrantOriginKind::Channel, 10),
        (GrantOriginKind::Capability, 100),
    ] {
        input.api_key_grants.push(ApiKeyCapabilityGrantRecord {
            api_key_id: id(500),
            capability_id: id(100),
            origin_kind: kind,
            origin_id: id(origin),
            created_at: at(),
        });
        input.policy_grants.push(ApiKeyPolicyCapabilityGrantRecord {
            policy_id: id(501),
            capability_id: id(100),
            origin_kind: kind,
            origin_id: id(origin),
            created_at: at(),
        });
    }
    let output = plan(&input).unwrap();
    assert_eq!(output.api_key_grants.len(), 6);
    assert_eq!(output.policy_grants.len(), 6);
    let granted: HashSet<_> = output
        .capabilities
        .iter()
        .filter(|row| row.source_id == id(100))
        .map(|row| row.id)
        .collect();
    for grant in &output.api_key_grants {
        assert!(granted.contains(&grant.capability_id));
        assert_eq!(grant.created_at, at());
        match grant.origin_kind {
            GrantOriginKind::Group => assert_eq!(grant.origin_id, id(999)),
            GrantOriginKind::Channel => assert_eq!(grant.origin_id, id(10)),
            GrantOriginKind::Capability => assert_eq!(grant.origin_id, grant.capability_id),
        }
    }
    for grant in &output.policy_grants {
        assert!(granted.contains(&grant.capability_id));
        if grant.origin_kind == GrantOriginKind::Capability {
            assert_eq!(grant.origin_id, grant.capability_id);
        }
    }
}

#[test]
fn disabled_drafts_and_tombstones_are_retained_without_implicit_routes() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpSse, T::Websocket]]);
    input.channel_capabilities[0].settings.enabled = false;
    input.channel_capabilities[0].deleted_at = Some(at());
    let before = json!(input.channel_capabilities);
    add_rule(&mut input, &[]);
    let output = plan(&input).unwrap();
    assert_eq!(output.capabilities.len(), 2);
    assert_eq!(output.rules.len(), 1);
    assert_eq!(output.rules[0].operation, Operation::Responses);
    assert!(output.tiers.is_empty());
    assert!(output.candidates.is_empty());
    assert_eq!(json!(input.channel_capabilities), before);
    input.operation_rules[0].enabled = true;
    assert_eq!(plan(&input).unwrap_err(), SplitError::Route);
}

#[test]
fn deterministic_plans_do_not_depend_on_database_row_order() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpSse, T::Websocket], &[T::HttpJson]]);
    add_rule(
        &mut input,
        &[&[(100, "wire", 1), (101, "wire", 2)], &[(100, "alias", 3)]],
    );
    let before = json!(plan(&input).unwrap());
    input.logical_channels.reverse();
    input.channel_capabilities.reverse();
    input.operation_tiers.reverse();
    input.operation_candidates.reverse();
    assert_eq!(json!(plan(&input).unwrap()), before);
}

#[test]
fn invalid_transports_and_cross_operation_graphs_fail_closed() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::Multipart]]);
    assert_eq!(plan(&input).unwrap_err(), SplitError::Capability);
    input.channel_capabilities[0].settings.transports = vec![T::HttpJson];
    add_rule(&mut input, &[&[(100, "wire", 1)]]);
    input.operation_candidates[0].operation = ApiOperation::ImagesEdit;
    assert_eq!(plan(&input).unwrap_err(), SplitError::Route);
    input.operation_candidates[0].operation = ApiOperation::Responses;
    input.operation_candidates[0].capability_id = id(999);
    assert_eq!(plan(&input).unwrap_err(), SplitError::Route);
}

#[test]
fn duplicate_and_generated_id_collisions_fail_closed() {
    use CapabilityTransport as T;
    let mut input = fixture(&[&[T::HttpSse, T::Websocket], &[T::HttpJson]]);
    let generated = plan(&input)
        .unwrap()
        .capabilities
        .iter()
        .find(|row| row.operation == Operation::ResponsesWs)
        .unwrap()
        .id;
    input.channel_capabilities[1].id = generated;
    assert_eq!(plan(&input).unwrap_err(), SplitError::IdentityCollision);
    input.channel_capabilities[1].id = id(100);
    assert_eq!(plan(&input).unwrap_err(), SplitError::Duplicate);
}

#[test]
fn invalid_direct_grant_provenance_is_not_repaired_into_an_authorization() {
    let mut input = fixture(&[&[CapabilityTransport::HttpJson]]);
    input.api_key_grants.push(ApiKeyCapabilityGrantRecord {
        api_key_id: id(500),
        capability_id: id(100),
        origin_kind: GrantOriginKind::Capability,
        origin_id: id(999),
        created_at: at(),
    });
    assert_eq!(plan(&input).unwrap_err(), SplitError::Grant);
}
