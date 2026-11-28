use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::domain::UserRole;

use super::RepositoryError;

#[derive(Debug, Serialize)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct KeysetCursor {
    ts: DateTime<Utc>,
    id: Uuid,
    #[serde(rename = "f")]
    filter_hash: u64,
}

impl KeysetCursor {
    fn encode(&self) -> String {
        let payload = serde_json::to_vec(self).expect("cursor fields are JSON serializable");
        format!("v1.{}", URL_SAFE_NO_PAD.encode(payload))
    }

    fn decode(token: &str, filter_hash: u64) -> Result<Self, RepositoryError> {
        if token.len() > 512 {
            return Err(RepositoryError::Validation);
        }
        let payload = token
            .strip_prefix("v1.")
            .ok_or(RepositoryError::Validation)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| RepositoryError::Validation)?;
        let cursor: Self =
            serde_json::from_slice(&bytes).map_err(|_| RepositoryError::Validation)?;
        let nanos = cursor.ts.timestamp_subsec_nanos();
        if cursor.filter_hash != filter_hash
            || !(0..=9999).contains(&cursor.ts.year())
            || nanos >= 1_000_000_000
            || !nanos.is_multiple_of(1_000)
        {
            return Err(RepositoryError::Validation);
        }
        Ok(cursor)
    }
}

#[derive(Default)]
pub struct PageOptions {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub q: Option<String>,
}

impl PageOptions {
    pub(super) fn window(&self, filters: impl Serialize) -> Result<KeysetPage, RepositoryError> {
        let q = self.q.as_deref();
        if q.is_some_and(|q| q.chars().count() > 200 || q.contains('\0')) {
            return Err(RepositoryError::Validation);
        }
        let q = q.map(str::trim).filter(|q| !q.is_empty());
        let mut page = KeysetPage::new(
            self.limit.unwrap_or(50),
            self.cursor.as_deref(),
            (filters, q),
        )?;
        page.pattern = q.map(|q| format!("%{}%", escape_like_pattern(q)));
        Ok(page)
    }
}

#[derive(Default)]
pub struct UserListFilter {
    pub page: PageOptions,
    pub role: Option<UserRole>,
    pub status: Option<String>,
    pub user_group_id: Option<Uuid>,
}

#[derive(Default)]
pub struct ApiKeyListFilter {
    pub page: PageOptions,
    pub status: Option<String>,
}

pub(super) struct KeysetPage {
    limit: i64,
    filter_hash: u64,
    cursor: Option<KeysetCursor>,
    pattern: Option<String>,
}

impl KeysetPage {
    pub(super) fn new(
        limit: i64,
        token: Option<&str>,
        filters: impl Serialize,
    ) -> Result<Self, RepositoryError> {
        let bytes = serde_json::to_vec(&filters).expect("filter fields are JSON serializable");
        let digest = Sha256::digest(bytes);
        let filter_hash = u64::from_be_bytes(digest[..8].try_into().expect("eight digest bytes"));
        Ok(Self {
            limit: limit.clamp(1, 100),
            filter_hash,
            cursor: token
                .map(|token| KeysetCursor::decode(token, filter_hash))
                .transpose()?,
            pattern: None,
        })
    }

    pub(super) fn pattern(&self) -> Option<&str> {
        self.pattern.as_deref()
    }

    pub(super) fn push_postgres(&self, query: &mut QueryBuilder<Postgres>, ts: &str, id: &str) {
        if let Some(cursor) = &self.cursor {
            query
                .push(format!(" AND ({ts} < "))
                .push_bind(cursor.ts)
                .push(format!(" OR ({ts} = "))
                .push_bind(cursor.ts)
                .push(format!(" AND {id} < "))
                .push_bind(cursor.id)
                .push("))");
        }
        query
            .push(format!(" ORDER BY {ts} DESC, {id} DESC LIMIT "))
            .push_bind(self.limit + 1);
    }

    #[cfg(feature = "sqlite-backend")]
    pub(super) fn push_sqlite(&self, query: &mut QueryBuilder<sqlx::Sqlite>, ts: &str, id: &str) {
        use super::sqlite::{SqliteTimestamp, SqliteUuid};

        if let Some(cursor) = &self.cursor {
            query
                .push(format!(" AND ({ts} < "))
                .push_bind(SqliteTimestamp(cursor.ts))
                .push(format!(" OR ({ts} = "))
                .push_bind(SqliteTimestamp(cursor.ts))
                .push(format!(" AND {id} < "))
                .push_bind(SqliteUuid(cursor.id))
                .push("))");
        }
        query
            .push(format!(" ORDER BY {ts} DESC, {id} DESC LIMIT "))
            .push_bind(self.limit + 1);
    }

    pub(super) fn finish<T>(
        &self,
        mut items: Vec<T>,
        total: Option<i64>,
        position: impl FnOnce(&T) -> (DateTime<Utc>, Uuid),
    ) -> PageResult<T> {
        let next_cursor = if items.len() > self.limit as usize {
            items.pop();
            let (ts, id) = position(items.last().expect("page limit is at least one"));
            Some(
                KeysetCursor {
                    ts,
                    id,
                    filter_hash: self.filter_hash,
                }
                .encode(),
            )
        } else {
            None
        };
        PageResult {
            items,
            next_cursor,
            total,
        }
    }
}

fn escape_like_pattern(input: &str) -> String {
    let mut pattern = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_requires_version_payload_and_filter_hash() {
        let cursor = KeysetCursor {
            ts: DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap(),
            id: Uuid::from_u128(17),
            filter_hash: 42,
        };
        let token = cursor.encode();
        assert_eq!(KeysetCursor::decode(&token, 42).unwrap(), cursor);
        for invalid in [
            "v1.!".to_owned(),
            "v1.e30".to_owned(),
            token.replacen("v1.", "v2.", 1),
        ] {
            assert!(matches!(
                KeysetCursor::decode(&invalid, 42),
                Err(RepositoryError::Validation)
            ));
        }
        assert!(matches!(
            KeysetCursor::decode(&token, 43),
            Err(RepositoryError::Validation)
        ));
    }
}
