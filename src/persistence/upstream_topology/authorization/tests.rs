use super::*;
use crate::domain::{ApiOperation, CapabilitySettings, RequestCompression};
use crate::persistence::upstream_topology::{
    ApiKeyPolicyChannelGrantRecord, ChannelCapabilityRecord, LogicalChannelRecord,
    RoutingGroupRecord,
};
use chrono::Utc;
use serde_json::json;

const GROUP: Uuid = Uuid::from_u128(1);
const CHANNEL: Uuid = Uuid::from_u128(2);
const RESPONSES: Uuid = Uuid::from_u128(3);
const IMAGES: Uuid = Uuid::from_u128(4);
const SEARCH: Uuid = Uuid::from_u128(5);
const POLICY: Uuid = Uuid::from_u128(6);

fn fixture() -> (UpstreamTopologyRecords, SelfApiKeyPolicy) {
    let now = Utc::now();
    let group = RoutingGroupRecord {
        id: GROUP,
        name: "Mixed operations".into(),
        enabled: true,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    let channel = LogicalChannelRecord {
        id: CHANNEL,
        group_id: GROUP,
        access_id: Uuid::from_u128(7),
        credential_id: None,
        name: "Logical channel".into(),
        enabled: true,
        binding_revision: Uuid::from_u128(8),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    let capabilities = [
        (RESPONSES, ApiOperation::Responses),
        (IMAGES, ApiOperation::ImagesGeneration),
        (SEARCH, ApiOperation::StandaloneWebSearch),
    ]
    .map(|(id, operation)| ChannelCapabilityRecord {
        id,
        channel_id: CHANNEL,
        settings: CapabilitySettings {
            operation,
            enabled: true,
            available_models: vec!["wire-model".into()],
            request_compression: RequestCompression::Default,
            test_model: None,
            test_pricing_model_id: None,
            auto_disable_allowed: false,
        },
        auto_disabled: false,
        auto_disable_reason: None,
        auto_disable_at: None,
        status_statistics_enabled: true,
        config_template_id: None,
        override_document: json!({}),
        billing_multiplier: rust_decimal::Decimal::ONE,
        revision: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    })
    .to_vec();
    (
        UpstreamTopologyRecords {
            routing_groups: vec![group],
            logical_channels: vec![channel],
            channel_capabilities: capabilities,
            policy_grants: vec![ApiKeyPolicyChannelGrantRecord {
                policy_id: POLICY,
                channel_id: CHANNEL,
                origin_kind: GrantOriginKind::Group,
                origin_id: GROUP,
                created_at: now,
            }],
            ..Default::default()
        },
        SelfApiKeyPolicy {
            id: POLICY,
            name: "Group policy".into(),
            allowed_group_ids: vec![GROUP],
            enabled: true,
        },
    )
}

#[test]
fn explicit_policy_group_offers_current_members_and_survives_an_empty_group() {
    let (mut topology, policy) = fixture();
    let issue = |topology: &_, groups: &[Uuid], channels: &[Uuid]| {
        prepare(
            topology,
            GrantOwner::SelfKey(Some(&policy)),
            &json!({}),
            groups,
            channels,
        )
    };
    let (groups, channels) = options(&topology, Some(&policy));
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].api_formats,
        ["open_ai_images", "open_ai_responses"]
    );
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].api_formats, groups[0].api_formats);
    assert!(issue(&topology, &[GROUP], &[]).unwrap().added.is_empty());
    assert_eq!(issue(&topology, &[], &[CHANNEL]).unwrap().added, [CHANNEL]);

    let mut moved_group = topology.routing_groups[0].clone();
    moved_group.id = Uuid::from_u128(9);
    topology.logical_channels[0].group_id = moved_group.id;
    topology.routing_groups.push(moved_group);
    let (groups, channels) = options(&topology, Some(&policy));
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, GROUP);
    assert!(groups[0].api_formats.is_empty());
    assert!(channels.is_empty());
    assert!(issue(&topology, &[GROUP], &[]).is_ok());
    assert!(matches!(
        issue(&topology, &[], &[CHANNEL]),
        Err(RepositoryError::ApiKeyTargetNotAllowed)
    ));

    let mut joined = topology.logical_channels[0].clone();
    joined.id = Uuid::new_v4();
    joined.group_id = GROUP;
    topology.logical_channels.push(joined.clone());
    let (_, channels) = options(&topology, Some(&policy));
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, joined.id);
    assert!(channels[0].api_formats.is_empty());
    assert!(!channels[0].auto_disabled);
    assert_eq!(
        issue(&topology, &[], &[joined.id]).unwrap().added,
        [joined.id]
    );
}

#[test]
fn individual_policy_grant_never_allows_issuing_a_whole_group() {
    let (mut topology, mut policy) = fixture();
    policy.allowed_group_ids.clear();
    let grant = ApiKeyPolicyChannelGrantRecord {
        policy_id: POLICY,
        channel_id: CHANNEL,
        origin_kind: GrantOriginKind::Channel,
        origin_id: CHANNEL,
        created_at: Utc::now(),
    };
    topology.policy_grants.push(grant);
    let owner = GrantOwner::SelfKey(Some(&policy));
    let (groups, channels) = options(&topology, Some(&policy));
    assert!(groups.is_empty());
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, CHANNEL);
    assert!(matches!(
        prepare(&topology, owner, &json!({}), &[GROUP], &[CHANNEL]),
        Err(RepositoryError::ApiKeyTargetNotAllowed)
    ));
    assert_eq!(
        prepare(&topology, owner, &json!({}), &[], &[CHANNEL])
            .unwrap()
            .added,
        [CHANNEL]
    );

    topology
        .policy_grants
        .retain(|g| g.origin_kind == GrantOriginKind::Group);
    assert!(options(&topology, Some(&policy)).1.is_empty());
    assert!(matches!(
        prepare(&topology, owner, &json!({}), &[], &[CHANNEL]),
        Err(RepositoryError::ApiKeyTargetNotAllowed)
    ));
}

#[test]
fn only_changes_to_explicit_channel_selections_write_direct_grants() {
    let (mut topology, _) = fixture();
    let before = json!({"allowed_group_ids": [GROUP], "allowed_channel_ids": [CHANNEL]});
    for owner in [GrantOwner::Policy, GrantOwner::AdminKey] {
        let retained = prepare(&topology, owner, &before, &[], &[CHANNEL]).unwrap();
        assert!(retained.added.is_empty());
        assert_eq!(retained.removed, [("group", GROUP)]);
        let removed = prepare(&topology, owner, &before, &[GROUP], &[]).unwrap();
        assert!(removed.added.is_empty());
        assert_eq!(removed.removed, [("channel", CHANNEL)]);
        assert_eq!(
            prepare(&topology, owner, &json!({}), &[GROUP], &[CHANNEL])
                .unwrap()
                .added,
            [CHANNEL]
        );
    }
    topology.routing_groups[0].deleted_at = Some(Utc::now());
    for owner in [GrantOwner::Policy, GrantOwner::AdminKey] {
        assert!(prepare(&topology, owner, &before, &[GROUP], &[CHANNEL]).is_ok());
        assert!(prepare(&topology, owner, &json!({}), &[GROUP], &[]).is_err());
        assert!(prepare(&topology, owner, &json!({}), &[], &[CHANNEL]).is_err());
    }
}

#[test]
fn self_service_requires_an_enabled_policy_for_every_target() {
    let (topology, mut policy) = fixture();
    policy.enabled = false;
    for (groups, channels) in [(&[GROUP][..], &[][..]), (&[][..], &[CHANNEL][..])] {
        assert!(matches!(
            prepare(
                &topology,
                GrantOwner::SelfKey(None),
                &json!({}),
                groups,
                channels
            ),
            Err(RepositoryError::DefaultApiKeyPolicyRequired)
        ));
        assert!(matches!(
            prepare(
                &topology,
                GrantOwner::SelfKey(Some(&policy)),
                &json!({}),
                groups,
                channels
            ),
            Err(RepositoryError::DefaultApiKeyPolicyDisabled)
        ));
    }
    assert!(options(&topology, None).0.is_empty());
    assert!(options(&topology, Some(&policy)).1.is_empty());
}
