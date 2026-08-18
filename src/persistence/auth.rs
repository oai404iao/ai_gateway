//! Backend-neutral Console authentication and session persistence contracts.

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use uuid::Uuid;

use crate::domain::{ConsoleSessionPurpose, UserRole};

#[derive(Clone)]
pub struct LoginUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub password_hash: Option<String>,
    pub auth_version: i64,
    pub password_change_required: bool,
    pub temporary_password_expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct LiveConsoleIdentity {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub auth_version: i64,
    pub session_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub session_purpose: String,
}

pub struct PasswordUser {
    pub id: Uuid,
    pub password_hash: Option<String>,
    pub status: String,
    pub role: String,
    pub auth_version: i64,
    pub password_change_required: bool,
    pub temporary_password_expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: UserRole,
    pub auth_version: i64,
    pub session_purpose: ConsoleSessionPurpose,
    pub temporary_password_expires_at: Option<DateTime<Utc>>,
}

pub enum SessionRotation {
    Rotated {
        user: SessionUser,
        refresh_expires_at: DateTime<Utc>,
    },
    Invalid,
    Replayed,
}

#[derive(Clone, Debug)]
pub struct TemporaryPasswordCreated {
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSessionState {
    Active,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsoleSession {
    pub id: Uuid,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub state: ConsoleSessionState,
    pub is_current: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsoleProfile {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub balance_amount: Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct InviteUserInput {
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub initial_balance_amount: Decimal,
    pub user_group_id: Option<Uuid>,
    pub default_api_key_policy_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct InvitationCreated {
    pub user_id: Uuid,
    pub invitation_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegistrationInvitationCode {
    pub id: Uuid,
    pub name: String,
    pub max_uses: Option<i64>,
    pub used_count: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: Decimal,
    pub created_by: Uuid,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct RegistrationInvitationCodeInput {
    pub name: String,
    pub max_uses: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: Decimal,
}

#[derive(Clone, Debug)]
pub struct RegistrationInvitationCodeMutation {
    pub id: Uuid,
    pub correlation_id: Uuid,
}

pub enum RegistrationAttempt {
    Registered(SessionUser),
    InvalidCode,
    EmailConflict,
}

/// Applies PostgreSQL `numeric(24,8)` scale semantics without floating point.
///
/// PostgreSQL rounds values to the declared scale before enforcing precision.
/// SQLite repositories must do the same explicitly before binding decimal
/// TEXT so both backends persist and audit the same value.
pub(super) fn normalize_numeric_24_8(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(8, RoundingStrategy::MidpointAwayFromZero)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;

    use super::normalize_numeric_24_8;

    #[test]
    fn numeric_24_8_rounds_midpoints_away_from_zero() {
        let positive = Decimal::from_str("1.234567885").unwrap();
        let negative = Decimal::from_str("-1.234567885").unwrap();

        assert_eq!(
            normalize_numeric_24_8(positive),
            Decimal::from_str("1.23456789").unwrap()
        );
        assert_eq!(
            normalize_numeric_24_8(negative),
            Decimal::from_str("-1.23456789").unwrap()
        );
    }

    #[test]
    fn numeric_24_8_normalization_is_idempotent() {
        for value in ["1.234567885", "-1.234567885", "9999999999999999.99999999"] {
            let normalized = normalize_numeric_24_8(Decimal::from_str(value).unwrap());
            assert_eq!(normalize_numeric_24_8(normalized), normalized);
        }
    }
}
