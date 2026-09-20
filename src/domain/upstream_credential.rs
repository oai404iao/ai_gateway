//! Upstream authentication material and exact credential destination scopes.

use std::{fmt, sync::Arc};

use reqwest::{
    Url,
    header::{HeaderName, HeaderValue},
};
use thiserror::Error;

use crate::request_policy::client_header_explicitly_ignored;

#[derive(Clone)]
pub enum UpstreamAuth {
    None,
    Bearer(Arc<str>),
    Header { name: HeaderName, value: Arc<str> },
}

impl UpstreamAuth {
    pub(crate) fn compile(
        kind: &str,
        header_name: Option<&str>,
        secret: Option<&str>,
    ) -> Result<Self, UpstreamCredentialError> {
        match kind {
            "none" if header_name.is_none() && secret.is_none() => Ok(Self::None),
            "bearer" if header_name.is_none() => Ok(Self::Bearer(compile_secret(secret)?)),
            "header" => {
                let name = header_name.ok_or(UpstreamCredentialError::MissingHeader)?;
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| UpstreamCredentialError::InvalidHeader)?;
                if matches!(
                    name.as_str(),
                    "authorization"
                        | "host"
                        | "content-length"
                        | "content-encoding"
                        | "connection"
                        | "transfer-encoding"
                        | "accept-encoding"
                        | "proxy-authorization"
                        | "proxy-authenticate"
                        | "keep-alive"
                        | "te"
                        | "trailer"
                        | "upgrade"
                        | "proxy-connection"
                ) || client_header_explicitly_ignored(&name)
                {
                    return Err(UpstreamCredentialError::UnsafeHeader);
                }
                Ok(Self::Header {
                    name,
                    value: compile_secret(secret)?,
                })
            }
            _ => Err(UpstreamCredentialError::InvalidAuth),
        }
    }
}

impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("UpstreamAuth::None"),
            Self::Bearer(_) => formatter.write_str("UpstreamAuth::Bearer(REDACTED)"),
            Self::Header { name, .. } => formatter
                .debug_struct("UpstreamAuth::Header")
                .field("name", name)
                .field("value", &"REDACTED")
                .finish(),
        }
    }
}

fn compile_secret(value: Option<&str>) -> Result<Arc<str>, UpstreamCredentialError> {
    let value = value.ok_or(UpstreamCredentialError::MissingSecret)?;
    if value.trim().is_empty() {
        return Err(UpstreamCredentialError::BlankSecret);
    }
    HeaderValue::from_str(value).map_err(|_| UpstreamCredentialError::InvalidSecret)?;
    Ok(Arc::from(value))
}

/// An exact Base URL scope, not an origin, host suffix, or path prefix.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CredentialTarget(Arc<str>);

impl CredentialTarget {
    pub fn parse(value: &str) -> Result<Self, UpstreamCredentialError> {
        if value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        }) {
            return Err(UpstreamCredentialError::InvalidTarget);
        }
        let url = Url::parse(value).map_err(|_| UpstreamCredentialError::InvalidTarget)?;
        let authority = value
            .split_once("://")
            .map(|(_, remainder)| remainder.split(['/', '?', '#']).next().unwrap_or_default())
            .ok_or(UpstreamCredentialError::InvalidTarget)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || authority.is_empty()
            || authority.contains('@')
        {
            return Err(UpstreamCredentialError::InvalidTarget);
        }
        // Endpoint construction trims all trailing slashes before appending an operation.
        Ok(Self(Arc::from(url.as_str().trim_end_matches('/'))))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialTarget(REDACTED)")
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum UpstreamCredentialError {
    #[error("invalid upstream auth configuration")]
    InvalidAuth,
    #[error("header upstream auth requires a header name")]
    MissingHeader,
    #[error("invalid upstream auth header name")]
    InvalidHeader,
    #[error("unsafe upstream auth header name")]
    UnsafeHeader,
    #[error("upstream auth requires credentials")]
    MissingSecret,
    #[error("upstream auth credential must not be blank")]
    BlankSecret,
    #[error("upstream auth credential is not a valid HTTP header value")]
    InvalidSecret,
    #[error(
        "credential target must be an unambiguous HTTP(S) Base URL without userinfo, query or fragment"
    )]
    InvalidTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_targets_follow_endpoint_url_semantics() {
        for (left, right) in [
            ("HTTPS://EXAMPLE.TEST:443/v1/", "https://example.test/v1"),
            ("http://example.test:80", "http://example.test/"),
            ("https://example.test/v1///", "https://example.test/v1"),
            ("https://example.test/a/../v1", "https://example.test/v1"),
            ("https://[::1]:443/v1/", "https://[::1]/v1"),
        ] {
            let left = CredentialTarget::parse(left).unwrap();
            let right = CredentialTarget::parse(right).unwrap();
            assert_eq!(left, right);
            assert_eq!(CredentialTarget::parse(left.as_str()).unwrap(), left);
        }
    }

    #[test]
    fn scopes_do_not_widen_to_origins_or_path_prefixes() {
        let expected = CredentialTarget::parse("https://example.test/v1").unwrap();
        for value in [
            "http://example.test/v1",
            "https://example.test:444/v1",
            "https://example.test/v10",
            "https://example.test/v1/models",
            "https://example.test/V1",
            "https://example.test/%761",
            "https://sub.example.test/v1",
            "https://example.test./v1",
            "https://example.test",
        ] {
            assert_ne!(expected, CredentialTarget::parse(value).unwrap(), "{value}");
        }
        assert_ne!(
            CredentialTarget::parse("https://example.test/a//b").unwrap(),
            CredentialTarget::parse("https://example.test/a/b").unwrap()
        );
    }

    #[test]
    fn invalid_targets_never_echo_input() {
        for value in [
            "",
            "/v1",
            "https:example.test/v1",
            "https:///example.test/v1",
            "ftp://example.test/v1",
            "https://secret@example.test/v1",
            "https://@example.test/v1",
            "https://example.test/v1?secret=value",
            "https://example.test/v1#secret",
            " https://example.test/v1",
            "https://example.test/a b",
            "https://example.test/\nsecret",
            "https://example.test\\evil/v1",
        ] {
            assert_eq!(
                CredentialTarget::parse(value),
                Err(UpstreamCredentialError::InvalidTarget),
                "{value}"
            );
        }
        let target = CredentialTarget::parse("https://example.test/private-path").unwrap();
        assert_eq!(format!("{target:?}"), "CredentialTarget(REDACTED)");
    }

    #[test]
    fn authentication_modes_are_strict_and_redacted() {
        assert!(matches!(
            UpstreamAuth::compile("none", None, None),
            Ok(UpstreamAuth::None)
        ));
        assert!(matches!(
            UpstreamAuth::compile("bearer", None, Some("test-only-secret")),
            Ok(UpstreamAuth::Bearer(_))
        ));
        let header =
            UpstreamAuth::compile("header", Some("X-Api-Key"), Some("test-only-secret")).unwrap();
        assert!(matches!(&header, UpstreamAuth::Header { name, .. } if name == "x-api-key"));
        assert!(!format!("{header:?}").contains("test-only-secret"));
        for (kind, header, secret) in [
            ("none", None, Some("test-only-secret")),
            ("none", Some("x-api-key"), None),
            ("bearer", Some("x-api-key"), Some("test-only-secret")),
            ("bearer", None, None),
            ("bearer", None, Some(" \t")),
            ("bearer", None, Some("secret\r\ninjected: value")),
            ("header", None, Some("test-only-secret")),
            ("header", Some("bad header"), Some("test-only-secret")),
            ("codex_oauth", None, Some("test-only-secret")),
        ] {
            assert!(UpstreamAuth::compile(kind, header, secret).is_err());
        }
    }

    #[test]
    fn custom_auth_cannot_override_transport_or_ignored_headers() {
        for name in [
            "Authorization",
            "Host",
            "content-length",
            "content-encoding",
            "connection",
            "transfer-encoding",
            "accept-encoding",
            "proxy-authorization",
            "proxy-authenticate",
            "keep-alive",
            "te",
            "trailer",
            "upgrade",
            "proxy-connection",
            "X-Forwarded-For",
        ] {
            assert_eq!(
                UpstreamAuth::compile("header", Some(name), Some("test-only-secret")).unwrap_err(),
                UpstreamCredentialError::UnsafeHeader,
                "{name}"
            );
        }
    }
}
