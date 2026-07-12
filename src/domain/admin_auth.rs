//! Bootstrap-only verifier for the separate management listener.

use std::fmt;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Stores only a SHA-256 digest; the bootstrap bearer token is consumed while
/// configuration is validated and is never retained by the running service.
#[derive(Clone)]
pub struct AdminTokenVerifier([u8; 32]);

impl AdminTokenVerifier {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        self.0.ct_eq(&digest).into()
    }
}

impl fmt::Debug for AdminTokenVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminTokenVerifier(REDACTED)")
    }
}
