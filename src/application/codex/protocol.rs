//! Host-owned execution, resource limits, and OAuth state for connector protocol calls.
use super::CodexConnectorError;
use crate::{
    connector_plugins::Plugin,
    persistence::{CodexQuotaResetOutcome, CodexQuotaUpdate},
};
use base64::Engine;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rand::RngCore;
use reqwest::{
    Client, Response, Url,
    header::{HeaderName, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::time::timeout;
const MAX_TOKEN_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MODELS_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_QUOTA_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_QUOTA_RESET_RESPONSE_BYTES: usize = 1024 * 1024;
const CONTROL_PLANE_COMMANDS: &[&str] = &[
    "settings.describe/v1",
    "settings.validate/v1",
    "settings.compile/v1",
    "attempt.context",
    "endpoints",
    "authorize_url",
    "parse_callback",
    "parse_identity",
    "parse_expiration",
    "exchange_plan",
    "exchange_parse",
    "refresh_plan",
    "refresh_parse",
    "models_plan",
    "models_parse",
    "quota_plan",
    "quota_parse",
    "quota_reset_plan",
    "quota_reset_parse",
];

pub fn validate_plugin_manifest(
    manifest: &crate::connector_plugins::PluginManifest,
) -> Result<(), CodexConnectorError> {
    if manifest.id != "codex"
        || CONTROL_PLANE_COMMANDS
            .iter()
            .any(|required| !manifest.commands.iter().any(|command| command == required))
    {
        return Err(CodexConnectorError::PluginUnavailable);
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct CodexEndpoints {
    pub issuer: Url,
    pub responses_base_url: Url,
}
impl CodexEndpoints {
    fn metadata(&self) -> Value {
        json!({"issuer":self.issuer.as_str(),"responses_base_url":self.responses_base_url.as_str()})
    }
    pub fn from_plugin(plugin: &Plugin) -> Result<Self, CodexConnectorError> {
        validate_plugin_manifest(plugin.manifest())?;
        let value = call(plugin, "endpoints", &json!({}), &[])?.metadata;
        Ok(Self {
            issuer: parse_url(&value, "issuer")?,
            responses_base_url: parse_url(&value, "responses_base_url")?,
        })
    }
}
pub fn redirect_uri(plugin: &Plugin) -> Result<String, CodexConnectorError> {
    call(plugin, "endpoints", &json!({}), &[])?.metadata["redirect_uri"]
        .as_str()
        .map(str::to_owned)
        .ok_or(CodexConnectorError::InvalidEndpoint)
}
fn parse_url(value: &Value, key: &str) -> Result<Url, CodexConnectorError> {
    let url = Url::parse(
        value[key]
            .as_str()
            .ok_or(CodexConnectorError::InvalidEndpoint)?,
    )
    .map_err(|_| CodexConnectorError::InvalidEndpoint)?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(CodexConnectorError::InvalidEndpoint);
    }
    Ok(url)
}
fn call(
    plugin: &Plugin,
    command: &str,
    metadata: &Value,
    body: &[u8],
) -> Result<ai_gateway_connector_sdk::PluginOutput, CodexConnectorError> {
    let mut output = plugin
        .call(command, metadata, body)
        .map_err(|_| CodexConnectorError::PluginUnavailable)?;
    if let Some(error) = output.metadata.get("protocol_error") {
        return Err(match error.as_str() {
            Some("InvalidEndpoint") => CodexConnectorError::InvalidEndpoint,
            Some("InvalidCallback") => CodexConnectorError::InvalidCallback,
            Some("OauthDenied") => CodexConnectorError::OauthDenied,
            Some("InvalidCredential") => CodexConnectorError::InvalidCredential,
            Some("InvalidJwt") => CodexConnectorError::InvalidJwt,
            Some("InvalidTokenResponse") => CodexConnectorError::InvalidTokenResponse,
            Some("RefreshTokenInvalid") => CodexConnectorError::RefreshTokenInvalid,
            Some("InvalidModelsResponse") => CodexConnectorError::InvalidModelsResponse,
            Some("NoModels") => CodexConnectorError::NoModels,
            Some("InvalidQuotaResponse") => CodexConnectorError::InvalidQuotaResponse,
            _ => {
                if let Some(status) = error
                    .get("TokenEndpointStatus")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                {
                    CodexConnectorError::TokenEndpointStatus(status)
                } else if let Some(status) = error
                    .get("CodexBackendStatus")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                {
                    CodexConnectorError::CodexBackendStatus(status)
                } else {
                    CodexConnectorError::PluginUnavailable
                }
            }
        });
    }
    output.metadata = output
        .metadata
        .get_mut("result")
        .map(Value::take)
        .ok_or(CodexConnectorError::PluginUnavailable)?;
    Ok(output)
}
fn decode<T: serde::de::DeserializeOwned>(
    plugin: &Plugin,
    command: &str,
    metadata: &Value,
    body: &[u8],
) -> Result<T, CodexConnectorError> {
    serde_json::from_value(call(plugin, command, metadata, body)?.metadata)
        .map_err(|_| CodexConnectorError::PluginUnavailable)
}
#[derive(Clone, Deserialize)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CodexIdentity {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
}

#[derive(Clone, Deserialize)]
pub struct ExchangedTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Clone, Deserialize)]
pub struct RefreshedTokens {
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct CallbackCode {
    pub code: String,
    pub state: String,
}

#[derive(Clone, Copy, Debug)]
pub struct CodexQuotaResetResult {
    pub outcome: CodexQuotaResetOutcome,
    pub windows_reset: i32,
}

pub fn generate_pkce() -> PkceCodes {
    let mut bytes = [0_u8; 64];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    PkceCodes {
        verifier,
        challenge,
    }
}
pub fn generate_oauth_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
pub fn state_hash(generation: &str, state: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"ai-gateway/oauth-generation/v1\0");
    hash.update((generation.len() as u64).to_be_bytes());
    hash.update(generation.as_bytes());
    hash.update(state.as_bytes());
    hash.finalize().into()
}
pub fn state_matches(expected_hash: &[u8], generation: &str, state: &str) -> bool {
    let actual = state_hash(generation, state);
    expected_hash.len() == actual.len() && expected_hash.ct_eq(&actual).into()
}
async fn read_body(
    response: Response,
    stream_idle_timeout: Duration,
    maximum_bytes: usize,
) -> Result<Bytes, CodexConnectorError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes as u64)
    {
        return Err(CodexConnectorError::UpstreamResponseTooLarge);
    }
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    loop {
        match timeout(stream_idle_timeout, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if body.len().saturating_add(chunk.len()) > maximum_bytes {
                    return Err(CodexConnectorError::UpstreamResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Some(Err(_))) => return Err(CodexConnectorError::UpstreamUnavailable),
            Ok(None) => return Ok(body.freeze()),
            Err(_) => return Err(CodexConnectorError::UpstreamTimeout),
        }
    }
}

pub fn build_authorize_url(
    plugin: &Plugin,
    endpoints: &CodexEndpoints,
    pkce: &PkceCodes,
    state: &str,
) -> Result<String, CodexConnectorError> {
    let url: String = decode(
        plugin,
        "authorize_url",
        &json!({"endpoints":endpoints.metadata(),"challenge":pkce.challenge,"state":state}),
        &[],
    )?;
    let parsed = parse_url(&json!({"url":url}), "url")?;
    if parsed.origin() != endpoints.issuer.origin() {
        return Err(CodexConnectorError::InvalidEndpoint);
    }
    Ok(url)
}
pub fn parse_callback_url(plugin: &Plugin, url: &str) -> Result<CallbackCode, CodexConnectorError> {
    decode(plugin, "parse_callback", &json!({"url":url}), &[])
}
pub fn parse_identity(plugin: &Plugin, token: &str) -> Result<CodexIdentity, CodexConnectorError> {
    decode(plugin, "parse_identity", &json!({"token":token}), &[])
}
pub fn parse_jwt_expiration(
    plugin: &Plugin,
    token: &str,
) -> Result<Option<DateTime<Utc>>, CodexConnectorError> {
    decode(plugin, "parse_expiration", &json!({"token":token}), &[])
}
fn prepare(
    client: &Client,
    plugin: &Plugin,
    command: &str,
    metadata: &Value,
    scope: &Url,
    method: &str,
    access_token: Option<&str>,
) -> Result<reqwest::Request, CodexConnectorError> {
    let plan = call(plugin, command, metadata, &[])?;
    let url = parse_url(&plan.metadata, "url")?;
    if url.origin() != scope.origin() || plan.metadata["method"].as_str() != Some(method) {
        return Err(CodexConnectorError::InvalidEndpoint);
    }
    let headers = validate_plan_headers(&plan.metadata, metadata, access_token)?;
    client
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| CodexConnectorError::InvalidEndpoint)?,
            url,
        )
        .headers(headers)
        .body(plan.body)
        .build()
        .map_err(|_| CodexConnectorError::UpstreamUnavailable)
}

fn validate_plan_headers(
    plan: &Value,
    metadata: &Value,
    access_token: Option<&str>,
) -> Result<reqwest::header::HeaderMap, CodexConnectorError> {
    let headers = plan["headers"]
        .as_object()
        .ok_or(CodexConnectorError::InvalidCredential)?;
    let mut planned_headers = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CodexConnectorError::InvalidCredential)?;
        if crate::request_policy::connector_header_is_forbidden(&name)
            || planned_headers.contains_key(&name)
        {
            return Err(CodexConnectorError::InvalidCredential);
        }
        let value = value
            .as_str()
            .ok_or(CodexConnectorError::InvalidCredential)?;
        planned_headers.insert(
            name,
            HeaderValue::from_str(value).map_err(|_| CodexConnectorError::InvalidCredential)?,
        );
    }
    if !super::credential_headers_match(
        &planned_headers,
        access_token,
        metadata["account_id"].as_str(),
        metadata["is_fedramp"].as_bool().unwrap_or(false),
    ) {
        return Err(CodexConnectorError::InvalidCredential);
    }
    Ok(planned_headers)
}
async fn execute(
    client: &Client,
    plugin: &Plugin,
    request: reqwest::Request,
    command: &str,
    header_timeout: Duration,
    idle_timeout: Duration,
    maximum: usize,
) -> Result<Value, CodexConnectorError> {
    let response = timeout(header_timeout, client.execute(request))
        .await
        .map_err(|_| CodexConnectorError::UpstreamTimeout)?
        .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status().as_u16();
    let body = read_body(response, idle_timeout, maximum).await?;
    Ok(call(plugin, command, &json!({"status":status}), &body)?.metadata)
}
fn decoded<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CodexConnectorError> {
    serde_json::from_value(value).map_err(|_| CodexConnectorError::PluginUnavailable)
}
pub async fn exchange_code(
    plugin: &Plugin,
    client: &Client,
    endpoints: &CodexEndpoints,
    code: &str,
    verifier: &str,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<ExchangedTokens, CodexConnectorError> {
    let request = prepare(
        client,
        plugin,
        "exchange_plan",
        &json!({"endpoints":endpoints.metadata(),"code":code,"verifier":verifier}),
        &endpoints.issuer,
        "POST",
        None,
    )?;
    decoded(
        execute(
            client,
            plugin,
            request,
            "exchange_parse",
            response_header_timeout,
            stream_idle_timeout,
            MAX_TOKEN_RESPONSE_BYTES,
        )
        .await?,
    )
}
pub fn prepare_refresh_request(
    plugin: &Plugin,
    client: &Client,
    endpoints: &CodexEndpoints,
    refresh_token: &str,
) -> Result<reqwest::Request, CodexConnectorError> {
    prepare(
        client,
        plugin,
        "refresh_plan",
        &json!({"endpoints":endpoints.metadata(),"refresh_token":refresh_token}),
        &endpoints.issuer,
        "POST",
        None,
    )
}
pub async fn refresh_tokens(
    plugin: &Plugin,
    client: &Client,
    request: reqwest::Request,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<RefreshedTokens, CodexConnectorError> {
    decoded(
        execute(
            client,
            plugin,
            request,
            "refresh_parse",
            response_header_timeout,
            stream_idle_timeout,
            MAX_TOKEN_RESPONSE_BYTES,
        )
        .await?,
    )
}
fn backend_metadata(
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> Value {
    json!({"endpoints":endpoints.metadata(),"access_token":access_token,"account_id":account_id,"is_fedramp":is_fedramp})
}
#[allow(clippy::too_many_arguments)]
pub async fn fetch_models(
    plugin: &Plugin,
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<Vec<String>, CodexConnectorError> {
    let request = prepare(
        client,
        plugin,
        "models_plan",
        &backend_metadata(endpoints, access_token, account_id, is_fedramp),
        &endpoints.responses_base_url,
        "GET",
        Some(access_token),
    )?;
    decoded(
        execute(
            client,
            plugin,
            request,
            "models_parse",
            response_header_timeout,
            stream_idle_timeout,
            MAX_MODELS_RESPONSE_BYTES,
        )
        .await?,
    )
}
#[derive(Deserialize)]
struct QuotaObservation {
    allowed: bool,
    limit_reached: bool,
    primary_used_percent: Option<i32>,
    primary_window_seconds: Option<i32>,
    primary_reset_at: Option<DateTime<Utc>>,
    secondary_used_percent: Option<i32>,
    secondary_window_seconds: Option<i32>,
    secondary_reset_at: Option<DateTime<Utc>>,
    reset_credits_available: Option<i64>,
}
#[allow(clippy::too_many_arguments)]
pub async fn fetch_quota(
    plugin: &Plugin,
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<CodexQuotaUpdate, CodexConnectorError> {
    let checked_at = Utc::now();
    let request = prepare(
        client,
        plugin,
        "quota_plan",
        &backend_metadata(endpoints, access_token, account_id, is_fedramp),
        &endpoints.responses_base_url,
        "GET",
        Some(access_token),
    )?;
    let quota: QuotaObservation = decoded(
        execute(
            client,
            plugin,
            request,
            "quota_parse",
            response_header_timeout,
            stream_idle_timeout,
            MAX_QUOTA_RESPONSE_BYTES,
        )
        .await?,
    )?;
    Ok(CodexQuotaUpdate {
        allowed: quota.allowed,
        limit_reached: quota.limit_reached,
        primary_used_percent: quota.primary_used_percent,
        primary_window_seconds: quota.primary_window_seconds,
        primary_reset_at: quota.primary_reset_at,
        secondary_used_percent: quota.secondary_used_percent,
        secondary_window_seconds: quota.secondary_window_seconds,
        secondary_reset_at: quota.secondary_reset_at,
        reset_credits_available: quota.reset_credits_available,
        checked_at,
    })
}
#[allow(clippy::too_many_arguments)]
pub fn prepare_quota_reset_request(
    plugin: &Plugin,
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
    redeem_request_id: &str,
) -> Result<reqwest::Request, CodexConnectorError> {
    let mut metadata = backend_metadata(endpoints, access_token, account_id, is_fedramp);
    metadata["redeem_request_id"] = json!(redeem_request_id);
    prepare(
        client,
        plugin,
        "quota_reset_plan",
        &metadata,
        &endpoints.responses_base_url,
        "POST",
        Some(access_token),
    )
}
pub async fn consume_quota_reset_credit(
    plugin: &Plugin,
    client: &Client,
    request: reqwest::Request,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<CodexQuotaResetResult, CodexConnectorError> {
    let value = execute(
        client,
        plugin,
        request,
        "quota_reset_parse",
        response_header_timeout,
        stream_idle_timeout,
        MAX_QUOTA_RESET_RESPONSE_BYTES,
    )
    .await?;
    let outcome = match value["outcome"].as_str() {
        Some("reset") => CodexQuotaResetOutcome::Reset,
        Some("nothing_to_reset") => CodexQuotaResetOutcome::NothingToReset,
        Some("no_credit") => CodexQuotaResetOutcome::NoCredit,
        Some("already_redeemed") => CodexQuotaResetOutcome::AlreadyRedeemed,
        _ => return Err(CodexConnectorError::InvalidQuotaResponse),
    };
    let windows_reset = value["windows_reset"]
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| (0..=2).contains(v))
        .ok_or(CodexConnectorError::InvalidQuotaResponse)?;
    Ok(CodexQuotaResetResult {
        outcome,
        windows_reset,
    })
}
#[cfg(test)]
mod tests {
    use reqwest::header::USER_AGENT;
    use std::sync::Arc;

    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap as AxumHeaderMap, StatusCode as AxumStatusCode},
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::net::TcpListener;

    fn test_plugin() -> Arc<Plugin> {
        crate::connector_plugins::test_plugins()
            .get("codex")
            .expect("Codex test plugin must be configured")
    }

    #[test]
    fn maintenance_header_plan_cannot_change_credential_binding_or_shadow_auth() {
        let metadata = json!({"account_id":"account", "is_fedramp":true});
        let valid = json!({"headers":{
            "authorization":"Bearer selected-token",
            "chatgpt-account-id":"account",
            "x-openai-fedramp":"true",
        }});
        validate_plan_headers(&valid, &metadata, Some("selected-token")).unwrap();
        for (name, value) in [
            ("Authorization", "Bearer other-token"),
            ("ChatGPT-Account-ID", "account"),
            ("chatgpt-account-id", "other-account"),
            ("x-openai-fedramp", "false"),
            ("content-encoding", "gzip"),
            ("accept-encoding", "gzip"),
        ] {
            let mut invalid = valid.clone();
            invalid["headers"][name] = json!(value);
            assert!(
                validate_plan_headers(&invalid, &metadata, Some("selected-token")).is_err(),
                "{name} accepted"
            );
        }
        assert!(validate_plan_headers(&valid, &json!({}), None).is_err());
        assert!(validate_plan_headers(&valid, &json!({}), Some("selected-token")).is_err());
        let accountless = json!({"headers":{"authorization":"Bearer selected-token"}});
        validate_plan_headers(&accountless, &json!({}), Some("selected-token")).unwrap();
        assert!(validate_plan_headers(&accountless, &metadata, Some("selected-token")).is_err());
        validate_plan_headers(&json!({"headers":{}}), &json!({}), None).unwrap();
    }
    #[test]
    fn startup_requires_every_control_plane_command_even_without_bound_credentials() {
        let plugin = test_plugin();
        let manifest = plugin.manifest();
        validate_plugin_manifest(manifest).unwrap();
        for missing in CONTROL_PLANE_COMMANDS {
            let mut partial = manifest.clone();
            partial.commands.retain(|command| command != missing);
            assert!(
                matches!(
                    validate_plugin_manifest(&partial),
                    Err(CodexConnectorError::PluginUnavailable)
                ),
                "missing {missing} was accepted"
            );
        }
    }
    #[test]
    fn oauth_state_comparison_accepts_only_the_original_value() {
        let hash = state_hash("artifact:1", "original");
        assert!(state_matches(&hash, "artifact:1", "original"));
        assert!(!state_matches(&hash, "artifact:1", "different"));
        assert!(!state_matches(&hash, "artifact:2", "original"));
        assert!(!state_matches(&hash, "upgraded-artifact:1", "original"));
        assert!(!state_matches(&hash[..16], "artifact:1", "original"));
    }

    async fn mock_models(headers: AxumHeaderMap) -> Result<Json<Value>, AxumStatusCode> {
        validate_mock_codex_headers(&headers)?;
        Ok(Json(json!({
            "models": [
                {"slug": "gpt-5-codex", "supported_in_api": true},
                {"slug": "gpt-5-codex", "supported_in_api": true},
                {"slug": "internal-only", "supported_in_api": false}
            ]
        })))
    }

    async fn mock_quota(headers: AxumHeaderMap) -> Result<Json<Value>, AxumStatusCode> {
        validate_mock_codex_headers(&headers)?;
        Ok(Json(json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 42,
                    "limit_window_seconds": 10800,
                    "reset_after_seconds": 60,
                    "reset_at": 1800000000
                },
                "secondary_window": null
            },
            "rate_limit_reset_credits": {"available_count": 2}
        })))
    }

    async fn mock_quota_reset(
        headers: AxumHeaderMap,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, AxumStatusCode> {
        validate_mock_codex_headers(&headers)?;
        if body["redeem_request_id"] != "redeem-123" {
            return Err(AxumStatusCode::BAD_REQUEST);
        }
        Ok(Json(json!({
            "code": "reset",
            "windows_reset": 2
        })))
    }

    fn validate_mock_codex_headers(headers: &AxumHeaderMap) -> Result<(), AxumStatusCode> {
        let authorization = headers
            .get("authorization")
            .and_then(|header| header.to_str().ok());
        let (expected_account_id, expected_fedramp) = match authorization {
            Some("Bearer access-token") => (Some("account-123"), Some("true")),
            Some("Bearer personal-access-token") => (None, None),
            _ => return Err(AxumStatusCode::UNAUTHORIZED),
        };
        let identity = test_plugin()
            .call("settings.describe/v1", &json!({}), &[])
            .unwrap()
            .metadata["defaults"]
            .clone();
        let common = [
            ("originator", identity["originator"].as_str().unwrap()),
            ("version", identity["client_version"].as_str().unwrap()),
        ];
        if common.iter().any(|(name, value)| {
            headers.get(*name).and_then(|header| header.to_str().ok()) != Some(*value)
        }) || headers
            .get("chatgpt-account-id")
            .and_then(|header| header.to_str().ok())
            != expected_account_id
            || headers
                .get("x-openai-fedramp")
                .and_then(|header| header.to_str().ok())
                != expected_fedramp
            || headers
                .get(USER_AGENT)
                .and_then(|header| header.to_str().ok())
                != identity["user_agent"].as_str()
        {
            return Err(AxumStatusCode::UNAUTHORIZED);
        }
        Ok(())
    }

    #[tokio::test]
    async fn models_and_quota_clients_use_codex_paths_headers_and_response_shapes() {
        let plugin = test_plugin();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/backend-api/codex/models", get(mock_models))
            .route("/backend-api/wham/usage", get(mock_quota))
            .route(
                "/backend-api/wham/rate-limit-reset-credits/consume",
                post(mock_quota_reset),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let endpoints = CodexEndpoints {
            issuer: Url::parse(&format!("http://{address}")).unwrap(),
            responses_base_url: Url::parse(&format!("http://{address}/backend-api/codex")).unwrap(),
        };
        let client = Client::new();
        let models = fetch_models(
            &plugin,
            &client,
            &endpoints,
            "access-token",
            Some("account-123"),
            true,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(models, vec!["gpt-5-codex"]);
        assert_eq!(
            fetch_models(
                &plugin,
                &client,
                &endpoints,
                "personal-access-token",
                None,
                false,
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
            .await
            .unwrap(),
            vec!["gpt-5-codex"]
        );

        let quota = fetch_quota(
            &plugin,
            &client,
            &endpoints,
            "access-token",
            Some("account-123"),
            true,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(quota.allowed);
        assert!(!quota.limit_reached);
        assert_eq!(quota.primary_used_percent, Some(42));
        assert_eq!(quota.primary_window_seconds, Some(10_800));
        assert_eq!(
            quota.primary_reset_at.map(|value| value.timestamp()),
            Some(1_800_000_000)
        );
        assert_eq!(quota.reset_credits_available, Some(2));
        assert!(
            fetch_quota(
                &plugin,
                &client,
                &endpoints,
                "personal-access-token",
                None,
                false,
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
            .await
            .unwrap()
            .allowed
        );

        let request = prepare_quota_reset_request(
            &plugin,
            &client,
            &endpoints,
            "access-token",
            Some("account-123"),
            true,
            "redeem-123",
        )
        .unwrap();
        let reset = consume_quota_reset_credit(
            &plugin,
            &client,
            request,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(reset.outcome, CodexQuotaResetOutcome::Reset);
        assert_eq!(reset.windows_reset, 2);

        server.abort();
    }

    #[tokio::test]
    async fn host_limits_response_bytes_and_idle_time() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/large", get(|| async { "123456" }))
            .route(
                "/slow",
                get(|| async {
                    let stream = futures_util::stream::once(async {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        Ok::<_, std::io::Error>(Bytes::from_static(b"x"))
                    });
                    axum::body::Body::from_stream(stream)
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::new();
        let large = client
            .get(format!("http://{address}/large"))
            .send()
            .await
            .unwrap();
        assert!(matches!(
            read_body(large, Duration::from_secs(1), 4).await,
            Err(CodexConnectorError::UpstreamResponseTooLarge)
        ));
        let slow = client
            .get(format!("http://{address}/slow"))
            .send()
            .await
            .unwrap();
        assert!(matches!(
            read_body(slow, Duration::from_millis(10), 10).await,
            Err(CodexConnectorError::UpstreamTimeout)
        ));
        server.abort();
    }
    #[test]
    fn host_rejects_plan_target_or_authorization_scope_changes() {
        let plugin = test_plugin();
        let endpoints = CodexEndpoints::from_plugin(&plugin).unwrap();
        let client = Client::new();
        let metadata = backend_metadata(&endpoints, "access", None, false);
        assert!(matches!(
            prepare(
                &client,
                &plugin,
                "models_plan",
                &metadata,
                &endpoints.issuer,
                "GET",
                Some("access")
            ),
            Err(CodexConnectorError::InvalidEndpoint)
        ));
        assert!(matches!(
            prepare(
                &client,
                &plugin,
                "models_plan",
                &metadata,
                &endpoints.responses_base_url,
                "GET",
                Some("different")
            ),
            Err(CodexConnectorError::InvalidCredential)
        ));
    }
}
