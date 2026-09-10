//! Fixed-seat monetary sharing policies, separate from account billing.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharingGroupInput {
    pub credential_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub seats: Vec<Option<Uuid>>,
    pub primary_limit_amount: Decimal,
    pub secondary_limit_amount: Decimal,
    pub request_reservation_amount: Decimal,
    pub user_requests_per_minute: u32,
    pub group_requests_per_minute: u32,
    pub user_max_concurrent_requests: u32,
    pub group_max_concurrent_requests: u32,
}

impl SharingGroupInput {
    pub fn valid(&self) -> bool {
        let users = self.seats.iter().flatten().collect::<HashSet<_>>();
        !self.name.trim().is_empty()
            && self.name.chars().count() <= 120
            && (1..=100).contains(&self.seats.len())
            && users.len() == self.seats.iter().flatten().count()
            && [
                self.primary_limit_amount,
                self.secondary_limit_amount,
                self.request_reservation_amount,
            ]
            .iter()
            .all(|amount| {
                *amount > Decimal::ZERO
                    && *amount <= Decimal::from(1_000_000)
                    && amount.scale() <= 8
            })
            && [
                self.user_requests_per_minute,
                self.group_requests_per_minute,
                self.user_max_concurrent_requests,
                self.group_max_concurrent_requests,
            ]
            .iter()
            .all(|limit| (1..=100_000).contains(limit))
            && self.request_reservation_amount
                <= self.primary_limit_amount / Decimal::from(self.seats.len())
            && self.request_reservation_amount
                <= self.secondary_limit_amount / Decimal::from(self.seats.len())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SharingGroup {
    pub id: Uuid,
    #[serde(flatten)]
    pub policy: SharingGroupInput,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct SharingRecord {
    pub group: SharingGroup,
    pub channel_ids: Vec<Uuid>,
    pub protected_channel_ids: Vec<Uuid>,
    pub windows: Vec<SharingWindow>,
}

#[derive(Clone, Debug, Default)]
pub struct SharingRegistry {
    groups: HashMap<Uuid, SharingGroup>,
    users: HashMap<Uuid, Uuid>,
    channels: HashMap<Uuid, Uuid>,
    protected: HashSet<Uuid>,
    windows: Vec<SharingWindow>,
}

impl SharingRegistry {
    pub fn compile(records: Vec<SharingRecord>) -> Result<Self, &'static str> {
        let mut result = Self::default();
        for record in records {
            if !record.group.policy.valid() || result.groups.contains_key(&record.group.id) {
                return Err("invalid Codex sharing policy");
            }
            let id = record.group.id;
            for user in record.group.policy.seats.iter().flatten() {
                if result.users.insert(*user, id).is_some() {
                    return Err("duplicate Codex sharing membership");
                }
            }
            for channel in record.channel_ids {
                if result.channels.insert(channel, id).is_some() {
                    return Err("duplicate Codex sharing credential");
                }
            }
            result.protected.extend(record.protected_channel_ids);
            result.windows.extend(record.windows);
            result.groups.insert(id, record.group);
        }
        Ok(result)
    }

    pub fn for_user(&self, user: Uuid) -> Option<&SharingGroup> {
        self.users.get(&user).and_then(|id| self.groups.get(id))
    }

    pub fn group(&self, id: Uuid) -> Option<&SharingGroup> {
        self.groups.get(&id)
    }

    pub fn groups(&self) -> impl Iterator<Item = &SharingGroup> {
        self.groups.values()
    }

    pub fn windows(&self) -> &[SharingWindow] {
        &self.windows
    }

    pub fn is_protected(&self, channel: Uuid) -> bool {
        self.protected.contains(&channel)
    }

    /// Group-only channels (including recognizable identity aliases) fail closed
    /// until a canonical projection has an eligible sharing seat.
    pub fn protect_channels(&mut self, channels: impl IntoIterator<Item = Uuid>) {
        self.protected.extend(channels);
    }

    pub fn for_channel(&self, channel: Uuid) -> Option<&SharingGroup> {
        self.channels
            .get(&channel)
            .and_then(|id| self.groups.get(id))
    }

    pub fn permits(&self, user: Uuid, channel: Uuid) -> bool {
        match self.for_channel(channel) {
            Some(group) => {
                group.policy.enabled
                    && group.policy.seats.contains(&Some(user))
                    && self.users.get(&user) == Some(&group.id)
            }
            None => !self.is_protected(channel),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, sqlx::FromRow)]
pub struct SharingWindow {
    pub id: Uuid,
    pub credential_id: Uuid,
    pub window_kind: String,
    pub scheduled_reset_at: DateTime<Utc>,
    pub checked_at: DateTime<Utc>,
    pub used_percent: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrictions_follow_channels_not_the_entire_user() {
        let user = Uuid::new_v4();
        let unseated = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        let channel = Uuid::new_v4();
        let image = Uuid::new_v4();
        let alias = Uuid::new_v4();
        let ordinary = Uuid::new_v4();
        let unbound = Uuid::new_v4();
        let group = SharingGroup {
            id: Uuid::new_v4(),
            updated_at: Utc::now(),
            policy: SharingGroupInput {
                credential_id: channel,
                name: "test".into(),
                enabled: true,
                seats: vec![Some(user)],
                primary_limit_amount: Decimal::ONE,
                secondary_limit_amount: Decimal::ONE,
                request_reservation_amount: Decimal::new(1, 1),
                user_requests_per_minute: 10,
                group_requests_per_minute: 10,
                user_max_concurrent_requests: 1,
                group_max_concurrent_requests: 1,
            },
        };
        for enabled in [true, false] {
            let mut group = group.clone();
            group.policy.enabled = enabled;
            let mut registry = SharingRegistry::compile(vec![SharingRecord {
                group,
                channel_ids: vec![channel, image],
                protected_channel_ids: vec![channel, image, alias],
                windows: vec![],
            }])
            .unwrap();
            registry.protect_channels([unbound]);
            for actor in [user, unseated, outsider] {
                assert!(registry.permits(actor, ordinary));
                assert!(!registry.permits(actor, unbound));
                assert!(!registry.permits(actor, alias));
                for projection in [channel, image] {
                    assert_eq!(
                        registry.permits(actor, projection),
                        actor == user && enabled
                    );
                }
            }
            assert!(registry.for_channel(ordinary).is_none());
            assert!(registry.for_channel(channel).is_some());
            assert!(registry.for_user(user).is_some());
            assert!(registry.for_user(unseated).is_none());
        }
    }

    #[test]
    fn a_user_cannot_occupy_seats_in_multiple_cars() {
        let user = Uuid::new_v4();
        let record = |id| SharingRecord {
            group: SharingGroup {
                id,
                updated_at: Utc::now(),
                policy: SharingGroupInput {
                    credential_id: Uuid::new_v4(),
                    name: "test".into(),
                    enabled: true,
                    seats: vec![None, Some(user)],
                    primary_limit_amount: Decimal::ONE,
                    secondary_limit_amount: Decimal::ONE,
                    request_reservation_amount: Decimal::new(1, 1),
                    user_requests_per_minute: 10,
                    group_requests_per_minute: 10,
                    user_max_concurrent_requests: 1,
                    group_max_concurrent_requests: 1,
                },
            },
            channel_ids: vec![Uuid::new_v4()],
            protected_channel_ids: vec![],
            windows: vec![],
        };
        assert_eq!(
            SharingRegistry::compile(vec![record(Uuid::new_v4()), record(Uuid::new_v4())])
                .unwrap_err(),
            "duplicate Codex sharing membership"
        );
    }
}
