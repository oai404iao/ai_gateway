//! Credential-backed integration tests against configured real upstreams.
//!
//! Run every ignored test in this target through
//! `scripts/run-real-upstream-smoke.sh`.

#[path = "real_upstream/mod.rs"]
mod suite;
