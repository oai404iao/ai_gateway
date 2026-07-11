use std::fmt;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// SHA-256 digest used as the in-memory index for a client API key.
///
/// The digest is deliberately opaque: diagnostics must never reveal either a
/// client secret or its stable hash value.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ApiKeyHash([u8; 32]);

impl ApiKeyHash {
    #[must_use]
    pub fn from_secret(secret: &str) -> Self {
        Self(Sha256::digest(secret.as_bytes()).into())
    }

    /// Checks a candidate without short-circuiting on the digest bytes.
    #[must_use]
    pub fn matches_secret(&self, candidate: &str) -> bool {
        let candidate_hash = Self::from_secret(candidate);
        self.0.ct_eq(&candidate_hash.0).into()
    }
}

impl fmt::Debug for ApiKeyHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKeyHash(REDACTED)")
    }
}

#[cfg(test)]
mod tests {
    use super::ApiKeyHash;

    #[test]
    fn hashes_and_matches_a_secret_without_debug_leakage() {
        let hash = ApiKeyHash::from_secret("client-secret");

        assert!(hash.matches_secret("client-secret"));
        assert!(!hash.matches_secret("other-secret"));
        assert!(!format!("{hash:?}").contains("client-secret"));
    }
}
