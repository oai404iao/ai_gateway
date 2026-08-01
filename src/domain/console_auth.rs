//! Console identities and role values shared by HTTP, application, and persistence.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Console authorization role. `admin` is deliberately a role rather than a
/// separate API or authentication mechanism.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    User,
    Admin,
}

impl UserRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

/// Scope carried by a Console session. Password-change sessions are issued
/// only after a temporary-password login and cannot access ordinary Console
/// resources.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSessionPurpose {
    #[default]
    Normal,
    PasswordChange,
}

impl ConsoleSessionPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::PasswordChange => "password_change",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "password_change" => Some(Self::PasswordChange),
            _ => None,
        }
    }

    #[must_use]
    pub const fn requires_password_change(self) -> bool {
        matches!(self, Self::PasswordChange)
    }
}

/// Authenticated Console identity after JWT validation and live session/user
/// checks. It is never used for the high-throughput OpenAI data plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConsolePrincipal {
    user_id: Uuid,
    session_id: Uuid,
    role: UserRole,
    auth_version: i64,
    session_purpose: ConsoleSessionPurpose,
}

impl ConsolePrincipal {
    #[must_use]
    pub const fn new(
        user_id: Uuid,
        session_id: Uuid,
        role: UserRole,
        auth_version: i64,
        session_purpose: ConsoleSessionPurpose,
    ) -> Self {
        Self {
            user_id,
            session_id,
            role,
            auth_version,
            session_purpose,
        }
    }

    #[must_use]
    pub const fn user_id(self) -> Uuid {
        self.user_id
    }

    #[must_use]
    pub const fn session_id(self) -> Uuid {
        self.session_id
    }

    #[must_use]
    pub const fn role(self) -> UserRole {
        self.role
    }

    #[must_use]
    pub const fn auth_version(self) -> i64 {
        self.auth_version
    }

    #[must_use]
    pub const fn session_purpose(self) -> ConsoleSessionPurpose {
        self.session_purpose
    }
}
