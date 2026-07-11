//! Application use cases: proxying, model listing, and configuration management.

mod proxy;

pub use proxy::{ModelsResponse, ProxyError, ProxyService};
