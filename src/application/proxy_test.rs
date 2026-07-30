//! Administrator-triggered outbound proxy diagnostics via ip-api.com.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::http::{
    HeaderMap, HeaderValue, Method,
    header::{ACCEPT, USER_AGENT},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{sync::Mutex, time::timeout};
use uuid::Uuid;

use crate::{
    domain::{ApiFormat, ChannelTimeoutPolicy, CompiledChannelUpstreamPolicy},
    persistence::{ControlPlaneRepository, ProxyRecord, RepositoryError},
    runtime_config::{RuntimeConfig, compile_proxy_test_target},
    transforms::TransformPlan,
    upstream::{ResolvedUpstreamPolicy, UpstreamClientKey, UpstreamClientRegistry},
};

const IP_API_ENDPOINT: &str = "http://ip-api.com/json/";
const IP_API_FIELDS: &str = "status,message,continent,continentCode,country,countryCode,region,regionName,city,district,zip,lat,lon,timezone,offset,currency,isp,org,as,asname,mobile,proxy,hosting,query";
const MAX_RESPONSE_BYTES: usize = 64 * 1_024;
const MAX_RATE_LIMIT_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone)]
pub struct ProxyTestService {
    repository: ControlPlaneRepository,
    runtime: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
    endpoint: reqwest::Url,
    cooldowns: Arc<Mutex<HashMap<UpstreamClientKey, Instant>>>,
}

impl ProxyTestService {
    #[must_use]
    pub fn new(
        repository: ControlPlaneRepository,
        runtime: Arc<RuntimeConfig>,
        upstream_clients: Arc<UpstreamClientRegistry>,
    ) -> Self {
        Self::new_with_endpoint(
            repository,
            runtime,
            upstream_clients,
            reqwest::Url::parse(IP_API_ENDPOINT).expect("ip-api endpoint is valid"),
        )
    }

    /// Alternate endpoint constructor used by deterministic integration tests.
    #[must_use]
    pub fn new_with_endpoint(
        repository: ControlPlaneRepository,
        runtime: Arc<RuntimeConfig>,
        upstream_clients: Arc<UpstreamClientRegistry>,
        endpoint: reqwest::Url,
    ) -> Self {
        Self {
            repository,
            runtime,
            upstream_clients,
            endpoint,
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Sends an IP lookup through the current proxy draft. Omitted credential
    /// components reuse the saved values only while the proxy endpoint is
    /// unchanged; this prevents hidden credentials from being replayed to a
    /// different proxy host.
    pub async fn test(&self, input: ProxyTestInput) -> Result<ProxyTestResponse, ProxyTestError> {
        let stored = match input.proxy_id {
            Some(id) => Some(
                self.repository
                    .proxy_record(id)
                    .await?
                    .ok_or(ProxyTestError::NotFound)?,
            ),
            None => None,
        };
        let endpoint_unchanged = stored
            .as_ref()
            .is_some_and(|stored| same_proxy_endpoint(&stored.proxy_url, &input.proxy_url));
        if let Some(stored) = stored.as_ref()
            && !endpoint_unchanged
            && ((input.username.is_none() && stored.username.is_some())
                || (input.password.is_none() && stored.password.is_some()))
        {
            return Err(ProxyTestError::CredentialsRequired);
        }

        let username = resolve_credential(
            input.username,
            endpoint_unchanged.then(|| stored.as_ref().and_then(|proxy| proxy.username.clone())),
        );
        let password = resolve_credential(
            input.password,
            endpoint_unchanged.then(|| stored.as_ref().and_then(|proxy| proxy.password.clone())),
        );
        let proxy = compile_proxy_test_target(ProxyRecord {
            id: input.proxy_id.unwrap_or_else(Uuid::nil),
            name: stored
                .as_ref()
                .map_or_else(|| "proxy-test".into(), |proxy| proxy.name.clone()),
            proxy_url: input.proxy_url,
            username,
            password,
            // A proxy test must never bypass the proxy because ip-api.com
            // appears in the saved no-proxy list.
            no_proxy_hosts: Vec::new(),
            enabled: true,
        })
        .map_err(|_| ProxyTestError::InvalidConfiguration)?;

        let snapshot = self.runtime.snapshot();
        let defaults = snapshot.system_settings().upstream_timeouts();
        let transforms = TransformPlan::noop(ApiFormat::OpenAiChatCompletions);
        let upstream_policy = CompiledChannelUpstreamPolicy::new_with_default_connect_timeout(
            Some(proxy),
            None,
            transforms.clone(),
            transforms,
            ChannelTimeoutPolicy::default(),
            defaults.connect(),
        );
        let policy = ResolvedUpstreamPolicy::try_resolve(&defaults, &upstream_policy)
            .map_err(|_| ProxyTestError::InvalidConfiguration)?;
        let key = UpstreamClientKey::resolve(&upstream_policy, policy);
        self.ensure_not_rate_limited(&key).await?;
        let client = self
            .upstream_clients
            .diagnostic_client_for(&upstream_policy, policy)
            .map_err(|_| ProxyTestError::RequestFailed)?;

        self.fetch(client, policy, key).await
    }

    async fn fetch(
        &self,
        client: reqwest::Client,
        policy: ResolvedUpstreamPolicy,
        key: UpstreamClientKey,
    ) -> Result<ProxyTestResponse, ProxyTestError> {
        let mut endpoint = self.endpoint.clone();
        endpoint
            .query_pairs_mut()
            .append_pair("fields", IP_API_FIELDS)
            .append_pair("lang", "en");
        let started = Instant::now();
        let response = timeout(
            policy.timeouts().response_header(),
            client
                .request(Method::GET, endpoint)
                .header(ACCEPT, HeaderValue::from_static("application/json"))
                .header(
                    USER_AGENT,
                    HeaderValue::from_static(concat!("ai-gateway/", env!("CARGO_PKG_VERSION"))),
                )
                .send(),
        )
        .await
        .map_err(|_| ProxyTestError::ResponseHeaderTimeout)?
        .map_err(|_| ProxyTestError::RequestFailed)?;

        let remaining = parse_header::<u32>(response.headers(), "x-rl");
        let reset_seconds = parse_header::<u64>(response.headers(), "x-ttl");
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            self.record_cooldown(key, reset_seconds).await;
            return Err(ProxyTestError::RateLimited);
        }
        if !response.status().is_success() {
            return Err(ProxyTestError::ProviderUnavailable);
        }

        let body = read_response_body(
            response,
            policy.timeouts().stream_idle(),
            MAX_RESPONSE_BYTES,
        )
        .await?;
        if remaining == Some(0) {
            self.record_cooldown(key, reset_seconds).await;
        }
        parse_response(&body, elapsed_millis(started), remaining, reset_seconds)
    }

    async fn ensure_not_rate_limited(&self, key: &UpstreamClientKey) -> Result<(), ProxyTestError> {
        let now = Instant::now();
        let mut cooldowns = self.cooldowns.lock().await;
        cooldowns.retain(|_, deadline| *deadline > now);
        if cooldowns.contains_key(key) {
            return Err(ProxyTestError::RateLimited);
        }
        Ok(())
    }

    async fn record_cooldown(&self, key: UpstreamClientKey, reset_seconds: Option<u64>) {
        let duration = Duration::from_secs(
            reset_seconds
                .unwrap_or(60)
                .clamp(1, MAX_RATE_LIMIT_TTL_SECONDS),
        );
        if let Some(deadline) = Instant::now().checked_add(duration) {
            self.cooldowns.lock().await.insert(key, deadline);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyTestInput {
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    pub proxy_url: String,
    /// Omitted reuses the saved component when `proxy_id` still points at the
    /// same proxy endpoint; null explicitly tests without a username.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub username: Option<Option<String>>,
    /// Omitted reuses the saved component when `proxy_id` still points at the
    /// same proxy endpoint; null explicitly tests without a password.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub password: Option<Option<String>>,
}

#[derive(Debug, Serialize)]
pub struct ProxyTestResponse {
    pub ip: String,
    pub continent: Option<String>,
    pub continent_code: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub region_code: Option<String>,
    pub region_name: Option<String>,
    pub city: Option<String>,
    pub district: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
    pub utc_offset_seconds: Option<i32>,
    pub currency: Option<String>,
    pub isp: Option<String>,
    pub organization: Option<String>,
    pub autonomous_system: Option<String>,
    pub autonomous_system_name: Option<String>,
    pub mobile: Option<bool>,
    pub proxy: Option<bool>,
    pub hosting: Option<bool>,
    pub latency_ms: u64,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_reset_seconds: Option<u64>,
}

#[derive(Debug, Error)]
pub enum ProxyTestError {
    #[error("proxy test configuration is invalid")]
    InvalidConfiguration,
    #[error("proxy credentials must be re-entered for a changed endpoint")]
    CredentialsRequired,
    #[error("proxy was not found")]
    NotFound,
    #[error("ip-api rate limit is active")]
    RateLimited,
    #[error("proxy test response header timed out")]
    ResponseHeaderTimeout,
    #[error("proxy test request failed")]
    RequestFailed,
    #[error("ip-api returned an unavailable response")]
    ProviderUnavailable,
    #[error("proxy test response body timed out")]
    ResponseBodyTimeout,
    #[error("proxy test response body failed")]
    ResponseBodyFailed,
    #[error("proxy test response is too large")]
    ResponseTooLarge,
    #[error("proxy test response is invalid")]
    InvalidResponse,
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IpApiResponse {
    status: String,
    #[serde(rename = "message")]
    _message: Option<String>,
    query: Option<String>,
    continent: Option<String>,
    continent_code: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    region_name: Option<String>,
    city: Option<String>,
    district: Option<String>,
    #[serde(rename = "zip")]
    postal_code: Option<String>,
    #[serde(rename = "lat")]
    latitude: Option<f64>,
    #[serde(rename = "lon")]
    longitude: Option<f64>,
    timezone: Option<String>,
    #[serde(rename = "offset")]
    utc_offset_seconds: Option<i32>,
    currency: Option<String>,
    isp: Option<String>,
    #[serde(rename = "org")]
    organization: Option<String>,
    #[serde(rename = "as")]
    autonomous_system: Option<String>,
    #[serde(rename = "asname")]
    autonomous_system_name: Option<String>,
    mobile: Option<bool>,
    proxy: Option<bool>,
    hosting: Option<bool>,
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

fn resolve_credential(
    submitted: Option<Option<String>>,
    stored: Option<Option<String>>,
) -> Option<String> {
    match submitted {
        Some(value) => value,
        None => stored.flatten(),
    }
}

fn same_proxy_endpoint(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (reqwest::Url::parse(left), reqwest::Url::parse(right)) else {
        return false;
    };
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn parse_header<T>(headers: &HeaderMap, name: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn read_response_body(
    response: reqwest::Response,
    stream_idle_timeout: Duration,
    max_response_bytes: usize,
) -> Result<Bytes, ProxyTestError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err(ProxyTestError::ResponseTooLarge);
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(max_response_bytes);
    let mut body = BytesMut::with_capacity(capacity);
    let mut stream = response.bytes_stream();
    loop {
        match timeout(stream_idle_timeout, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if body.len().saturating_add(chunk.len()) > max_response_bytes {
                    return Err(ProxyTestError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Some(Err(_))) => return Err(ProxyTestError::ResponseBodyFailed),
            Ok(None) => return Ok(body.freeze()),
            Err(_) => return Err(ProxyTestError::ResponseBodyTimeout),
        }
    }
}

fn parse_response(
    body: &[u8],
    latency_ms: u64,
    rate_limit_remaining: Option<u32>,
    rate_limit_reset_seconds: Option<u64>,
) -> Result<ProxyTestResponse, ProxyTestError> {
    let response: IpApiResponse =
        serde_json::from_slice(body).map_err(|_| ProxyTestError::InvalidResponse)?;
    if response.status != "success" {
        return Err(ProxyTestError::ProviderUnavailable);
    }
    let ip = response
        .query
        .as_deref()
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
        .ok_or(ProxyTestError::InvalidResponse)?
        .to_string();
    Ok(ProxyTestResponse {
        ip,
        continent: clean(response.continent),
        continent_code: clean(response.continent_code),
        country: clean(response.country),
        country_code: clean(response.country_code),
        region_code: clean(response.region),
        region_name: clean(response.region_name),
        city: clean(response.city),
        district: clean(response.district),
        postal_code: clean(response.postal_code),
        latitude: response.latitude,
        longitude: response.longitude,
        timezone: clean(response.timezone),
        utc_offset_seconds: response.utc_offset_seconds,
        currency: clean(response.currency),
        isp: clean(response.isp),
        organization: clean(response.organization),
        autonomous_system: clean(response.autonomous_system),
        autonomous_system_name: clean(response.autonomous_system_name),
        mobile: response.mobile,
        proxy: response.proxy,
        hosting: response.hosting,
        latency_ms,
        rate_limit_remaining,
        rate_limit_reset_seconds,
    })
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{ProxyTestError, parse_response, resolve_credential, same_proxy_endpoint};

    #[test]
    fn parses_ip_api_success_response() {
        let response = parse_response(
            br#"{
                "status":"success",
                "continent":"North America",
                "continentCode":"NA",
                "country":"United States",
                "countryCode":"US",
                "region":"CA",
                "regionName":"California",
                "city":"Los Angeles",
                "district":"",
                "zip":"90001",
                "lat":34.05,
                "lon":-118.24,
                "timezone":"America/Los_Angeles",
                "offset":-25200,
                "currency":"USD",
                "isp":"Example ISP",
                "org":"Example Org",
                "as":"AS64500 Example",
                "asname":"EXAMPLE",
                "mobile":false,
                "proxy":true,
                "hosting":false,
                "query":"203.0.113.10"
            }"#,
            42,
            Some(44),
            Some(60),
        )
        .unwrap();

        assert_eq!(response.ip, "203.0.113.10");
        assert_eq!(response.country_code.as_deref(), Some("US"));
        assert_eq!(response.district, None);
        assert_eq!(response.latency_ms, 42);
        assert_eq!(response.rate_limit_remaining, Some(44));
        assert_eq!(response.proxy, Some(true));
    }

    #[test]
    fn rejects_failed_or_missing_ip_responses() {
        assert!(matches!(
            parse_response(
                br#"{"status":"fail","message":"reserved range"}"#,
                1,
                None,
                None
            ),
            Err(ProxyTestError::ProviderUnavailable)
        ));
        assert!(matches!(
            parse_response(
                br#"{"status":"success","query":"not-an-ip"}"#,
                1,
                None,
                None
            ),
            Err(ProxyTestError::InvalidResponse)
        ));
    }

    #[test]
    fn credentials_only_fall_back_when_the_endpoint_is_equivalent() {
        assert!(same_proxy_endpoint(
            "http://proxy.example:80",
            "http://PROXY.example/"
        ));
        assert!(!same_proxy_endpoint(
            "http://proxy.example:8080",
            "http://proxy.example:8081"
        ));
        assert_eq!(
            resolve_credential(None, Some(Some("saved".into()))),
            Some("saved".into())
        );
        assert_eq!(
            resolve_credential(Some(None), Some(Some("saved".into()))),
            None
        );
    }
}
