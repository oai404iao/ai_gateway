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
        sharing_only: false,
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
            name: "Frozen policy".into(),
            enabled: true,
        },
    )
}

#[test]
fn policy_options_and_issuance_use_fixed_channels_and_current_origin() {
    let (mut topology, policy) = fixture();
    let sharing = SelfApiKeySharingAccess::default();
    let plan = resolve(&topology, &[GROUP], &[], Some((Some(&policy), &sharing))).unwrap();
    assert_eq!(
        plan.grants.iter().map(|g| g.channel_id).collect::<Vec<_>>(),
        [CHANNEL]
    );
    assert_eq!(plan.formats, all_formats());
    let (groups, channels) = options(&topology, Some(&policy), &sharing);
    assert_eq!(
        groups[0].api_formats,
        ["open_ai_images", "open_ai_responses"]
    );
    assert_eq!(
        channels[0].api_formats,
        ["open_ai_images", "open_ai_responses"]
    );
    let mut moved_group = topology.routing_groups[0].clone();
    moved_group.id = Uuid::from_u128(9);
    topology.logical_channels[0].group_id = moved_group.id;
    topology.routing_groups.push(moved_group);
    assert!(resolve(&topology, &[], &[CHANNEL], Some((Some(&policy), &sharing))).is_err());
    assert!(options(&topology, Some(&policy), &sharing).1.is_empty());
}

#[test]
fn metadata_edits_and_adding_targets_do_not_expand_retained_origins() {
    let (topology, _) = fixture();
    let existing = Grant {
        channel_id: CHANNEL,
        group: true,
        origin_id: GROUP,
    };
    let before = json!({"allowed_group_ids": [GROUP], "allowed_channel_ids": []});
    let plan = resolve(&topology, &[GROUP], &[], None).unwrap();
    assert_eq!(
        reconcile(vec![existing], plan, &before, &[GROUP], &[]).unwrap(),
        [existing]
    );
    let plan = resolve(&topology, &[GROUP], &[CHANNEL], None).unwrap();
    let grants = reconcile(vec![existing], plan, &before, &[GROUP], &[CHANNEL]).unwrap();
    assert_eq!(
        grants
            .iter()
            .filter(|g| g.group)
            .copied()
            .collect::<Vec<_>>(),
        [existing]
    );
    assert_eq!(grants.iter().filter(|g| !g.group).count(), 1);
    let plan = resolve(&topology, &[], &[CHANNEL], None).unwrap();
    assert!(
        reconcile(vec![existing], plan, &before, &[], &[CHANNEL])
            .unwrap()
            .iter()
            .all(|g| !g.group)
    );
}

#[test]
fn empty_and_deleted_retained_targets_never_implicitly_gain_grants() {
    let (mut topology, _) = fixture();
    let before = json!({"allowed_group_ids": [GROUP], "allowed_channel_ids": []});
    let plan = resolve(&topology, &[GROUP], &[], None).unwrap();
    assert!(
        reconcile(vec![], plan, &before, &[GROUP], &[])
            .unwrap()
            .is_empty()
    );
    topology.routing_groups[0].deleted_at = Some(Utc::now());
    assert!(
        resolve_added(&topology, &before, &[GROUP], &[])
            .unwrap()
            .grants
            .is_empty()
    );
    assert!(resolve_added(&topology, &json!({}), &[GROUP], &[]).is_err());
}

#[test]
fn sharing_requires_exact_seated_channel_and_never_authorizes_aliases_or_group() {
    let (topology, mut policy) = fixture();
    let sharing = SelfApiKeySharingAccess {
        owned_channels: HashSet::from([CHANNEL]),
        protected_channels: HashSet::from([CHANNEL]),
    };
    policy.enabled = false;
    for policy in [None, Some(&policy)] {
        let plan = resolve(&topology, &[], &[CHANNEL], Some((policy, &sharing))).unwrap();
        assert_eq!(plan.grants.len(), 1);
        assert_eq!(plan.grants[0].channel_id, CHANNEL);
        assert!(resolve(&topology, &[GROUP], &[], Some((policy, &sharing))).is_err());
    }
    let alias = SelfApiKeySharingAccess {
        owned_channels: HashSet::new(),
        ..sharing
    };
    policy.enabled = true;
    assert!(resolve(&topology, &[], &[CHANNEL], Some((Some(&policy), &alias))).is_err());
}

#[test]
fn capabilities_are_not_authorization_targets_and_new_group_members_are_not_inherited() {
    let (mut topology, policy) = fixture();
    let sharing = SelfApiKeySharingAccess::default();
    topology.channel_capabilities.clear();
    let existing = resolve(&topology, &[GROUP], &[], Some((Some(&policy), &sharing)))
        .unwrap()
        .grants;
    assert_eq!(existing.len(), 1);
    assert_eq!(existing[0].channel_id, CHANNEL);
    let (groups, channels) = options(&topology, Some(&policy), &sharing);
    assert_eq!(groups.len(), 1);
    assert_eq!(channels.len(), 1);
    assert!(!channels[0].auto_disabled);
    let mut added = topology.logical_channels[0].clone();
    added.id = Uuid::new_v4();
    topology.logical_channels.push(added.clone());
    let before = json!({
        "allowed_group_ids": [GROUP], "allowed_channel_ids": [],
        "allowed_api_formats": ["open_ai_responses"]
    });
    let plan = resolve_added(&topology, &before, &[GROUP], &[]).unwrap();
    let retained = reconcile(existing.clone(), plan, &before, &[GROUP], &[]).unwrap();
    assert_eq!(retained, existing);
    assert!(retained.iter().all(|grant| grant.channel_id != added.id));
    assert!(resolve(&topology, &[], &[added.id], Some((Some(&policy), &sharing))).is_err());
}
