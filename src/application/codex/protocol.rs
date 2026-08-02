use std::time::Duration;

use base64::Engine;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rand::RngCore;
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::time::timeout;

use crate::persistence::{CodexQuotaResetOutcome, CodexQuotaUpdate};

use super::CodexConnectorError;

pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const CODEX_OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub const CODEX_CLIENT_VERSION: &str = "0.146.0";

const MAX_TOKEN_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MODELS_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_QUOTA_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_QUOTA_RESET_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CodexEndpoints {
    pub issuer: Url,
    pub responses_base_url: Url,
}

impl Default for CodexEndpoints {
    fn default() -> Self {
        Self {
            issuer: Url::parse("https://auth.openai.com").expect("Codex issuer URL is valid"),
            responses_base_url: Url::parse("https://chatgpt.com/backend-api/codex")
                .expect("Codex Responses base URL is valid"),
        }
    }
}

impl CodexEndpoints {
    pub fn token_url(&self) -> Result<Url, CodexConnectorError> {
        self.issuer
            .join("/oauth/token")
            .map_err(|_| CodexConnectorError::InvalidEndpoint)
    }

    pub fn models_url(&self) -> Result<Url, CodexConnectorError> {
        let mut url = Url::parse(&format!(
            "{}/models",
            self.responses_base_url.as_str().trim_end_matches('/')
        ))
        .map_err(|_| CodexConnectorError::InvalidEndpoint)?;
        url.query_pairs_mut()
            .append_pair("client_version", CODEX_CLIENT_VERSION);
        Ok(url)
    }

    pub fn quota_url(&self) -> Result<Url, CodexConnectorError> {
        let base = self.responses_base_url.as_str().trim_end_matches('/');
        let backend = base
            .strip_suffix("/codex")
            .ok_or(CodexConnectorError::InvalidEndpoint)?;
        Url::parse(&format!("{backend}/wham/usage"))
            .map_err(|_| CodexConnectorError::InvalidEndpoint)
    }

    pub fn quota_reset_url(&self) -> Result<Url, CodexConnectorError> {
        let base = self.responses_base_url.as_str().trim_end_matches('/');
        let backend = base
            .strip_suffix("/codex")
            .ok_or(CodexConnectorError::InvalidEndpoint)?;
        Url::parse(&format!("{backend}/wham/rate-limit-reset-credits/consume"))
            .map_err(|_| CodexConnectorError::InvalidEndpoint)
    }
}

#[derive(Clone)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Clone, Debug)]
pub struct CodexIdentity {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
}

#[derive(Clone)]
pub struct ExchangedTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Clone)]
pub struct RefreshedTokens {
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Clone)]
pub struct CallbackCode {
    pub code: String,
    pub state: String,
}

#[derive(Deserialize)]
struct QuotaUsageResponse {
    rate_limit: Option<QuotaRateLimit>,
    #[serde(default)]
    rate_limit_reset_credits: Option<QuotaResetCreditsSummary>,
}

#[derive(Deserialize)]
struct QuotaRateLimit {
    allowed: bool,
    limit_reached: bool,
    primary_window: Option<QuotaWindow>,
    secondary_window: Option<QuotaWindow>,
}

#[derive(Deserialize)]
struct QuotaWindow {
    used_percent: i32,
    limit_window_seconds: i32,
    reset_at: i64,
}

#[derive(Deserialize)]
struct QuotaResetCreditsSummary {
    available_count: i64,
}

#[derive(Serialize)]
struct ConsumeQuotaResetCreditRequest {
    redeem_request_id: String,
}

#[derive(Deserialize)]
struct ConsumeQuotaResetCreditResponse {
    code: ConsumeQuotaResetCreditCode,
    #[serde(default)]
    windows_reset: i32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConsumeQuotaResetCreditCode {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
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

pub fn state_hash(state: &str) -> [u8; 32] {
    Sha256::digest(state.as_bytes()).into()
}

pub fn state_matches(expected_hash: &[u8], state: &str) -> bool {
    let actual = state_hash(state);
    expected_hash.len() == actual.len() && expected_hash.ct_eq(&actual).into()
}

pub fn build_authorize_url(
    endpoints: &CodexEndpoints,
    pkce: &PkceCodes,
    state: &str,
) -> Result<String, CodexConnectorError> {
    let mut url = endpoints
        .issuer
        .join("/oauth/authorize")
        .map_err(|_| CodexConnectorError::InvalidEndpoint)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CODEX_OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", CODEX_OAUTH_REDIRECT_URI)
        .append_pair("scope", CODEX_OAUTH_SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", CODEX_ORIGINATOR);
    Ok(url.into())
}

pub fn parse_callback_url(value: &str) -> Result<CallbackCode, CodexConnectorError> {
    let url = Url::parse(value).map_err(|_| CodexConnectorError::InvalidCallback)?;
    if url.scheme() != "http"
        || url.host_str() != Some("localhost")
        || url.port_or_known_default() != Some(1455)
        || url.path() != "/auth/callback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(CodexConnectorError::InvalidCallback);
    }
    let values = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if values.contains_key("error") {
        return Err(CodexConnectorError::OauthDenied);
    }
    let code = values
        .get("code")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or(CodexConnectorError::InvalidCallback)?;
    let state = values
        .get("state")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or(CodexConnectorError::InvalidCallback)?;
    Ok(CallbackCode {
        code: code.to_owned(),
        state: state.to_owned(),
    })
}

pub async fn exchange_code(
    client: &Client,
    endpoints: &CodexEndpoints,
    code: &str,
    verifier: &str,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<ExchangedTokens, CodexConnectorError> {
    #[derive(Deserialize)]
    struct TokenResponse {
        id_token: String,
        access_token: String,
        refresh_token: String,
    }

    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding::encode(code),
        urlencoding::encode(CODEX_OAUTH_REDIRECT_URI),
        urlencoding::encode(CODEX_OAUTH_CLIENT_ID),
        urlencoding::encode(verifier),
    );
    let response = timeout(
        response_header_timeout,
        client
            .post(endpoints.token_url()?)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send(),
    )
    .await
    .map_err(|_| CodexConnectorError::UpstreamTimeout)?
    .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status();
    let body = read_body(response, stream_idle_timeout, MAX_TOKEN_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(token_endpoint_error(status, &body, false));
    }
    let response: TokenResponse =
        serde_json::from_slice(&body).map_err(|_| CodexConnectorError::InvalidTokenResponse)?;
    if response.id_token.is_empty()
        || response.access_token.is_empty()
        || response.refresh_token.is_empty()
    {
        return Err(CodexConnectorError::InvalidTokenResponse);
    }
    Ok(ExchangedTokens {
        id_token: response.id_token,
        access_token: response.access_token,
        refresh_token: response.refresh_token,
    })
}

pub async fn refresh_tokens(
    client: &Client,
    endpoints: &CodexEndpoints,
    refresh_token: &str,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<RefreshedTokens, CodexConnectorError> {
    #[derive(serde::Serialize)]
    struct RefreshRequest<'a> {
        client_id: &'static str,
        grant_type: &'static str,
        refresh_token: &'a str,
    }
    #[derive(Deserialize)]
    struct RefreshResponse {
        id_token: Option<String>,
        access_token: Option<String>,
        refresh_token: Option<String>,
    }

    let request_body = serde_json::to_vec(&RefreshRequest {
        client_id: CODEX_OAUTH_CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
    })
    .map_err(|_| CodexConnectorError::InvalidTokenResponse)?;
    let response = timeout(
        response_header_timeout,
        client
            .post(endpoints.token_url()?)
            .header(CONTENT_TYPE, "application/json")
            .body(request_body)
            .send(),
    )
    .await
    .map_err(|_| CodexConnectorError::UpstreamTimeout)?
    .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status();
    let body = read_body(response, stream_idle_timeout, MAX_TOKEN_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(token_endpoint_error(status, &body, true));
    }
    let response: RefreshResponse =
        serde_json::from_slice(&body).map_err(|_| CodexConnectorError::InvalidTokenResponse)?;
    if response.id_token.is_none()
        && response.access_token.is_none()
        && response.refresh_token.is_none()
    {
        return Err(CodexConnectorError::InvalidTokenResponse);
    }
    Ok(RefreshedTokens {
        id_token: response.id_token.filter(|value| !value.is_empty()),
        access_token: response.access_token.filter(|value| !value.is_empty()),
        refresh_token: response.refresh_token.filter(|value| !value.is_empty()),
    })
}

pub async fn fetch_models(
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<Vec<String>, CodexConnectorError> {
    #[derive(Deserialize)]
    struct ModelsEnvelope {
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        slug: String,
        #[serde(default)]
        supported_in_api: Option<bool>,
    }

    let response = timeout(
        response_header_timeout,
        client
            .get(endpoints.models_url()?)
            .headers(codex_headers(access_token, account_id, is_fedramp)?)
            .send(),
    )
    .await
    .map_err(|_| CodexConnectorError::UpstreamTimeout)?
    .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status();
    let body = read_body(response, stream_idle_timeout, MAX_MODELS_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(CodexConnectorError::CodexBackendStatus(status.as_u16()));
    }
    let response: ModelsEnvelope =
        serde_json::from_slice(&body).map_err(|_| CodexConnectorError::InvalidModelsResponse)?;
    let mut models = response
        .models
        .into_iter()
        .filter(|model| model.supported_in_api != Some(false))
        .map(|model| model.slug.trim().to_owned())
        .filter(|model| !model.is_empty() && model.len() <= 300)
        .collect::<Vec<_>>();
    models.sort_unstable();
    models.dedup();
    if models.is_empty() {
        return Err(CodexConnectorError::NoModels);
    }
    Ok(models)
}

pub async fn fetch_quota(
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<CodexQuotaUpdate, CodexConnectorError> {
    let checked_at = Utc::now();
    let response = timeout(
        response_header_timeout,
        client
            .get(endpoints.quota_url()?)
            .headers(codex_headers(access_token, account_id, is_fedramp)?)
            .send(),
    )
    .await
    .map_err(|_| CodexConnectorError::UpstreamTimeout)?
    .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status();
    let body = read_body(response, stream_idle_timeout, MAX_QUOTA_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(CodexConnectorError::CodexBackendStatus(status.as_u16()));
    }
    let response: QuotaUsageResponse =
        serde_json::from_slice(&body).map_err(|_| CodexConnectorError::InvalidQuotaResponse)?;
    let rate_limit = response
        .rate_limit
        .ok_or(CodexConnectorError::InvalidQuotaResponse)?;
    let primary = validate_window(rate_limit.primary_window)?;
    let secondary = validate_window(rate_limit.secondary_window)?;
    let reset_credits_available = response
        .rate_limit_reset_credits
        .map(|credits| credits.available_count)
        .map(|count| {
            if count < 0 {
                Err(CodexConnectorError::InvalidQuotaResponse)
            } else {
                Ok(count)
            }
        })
        .transpose()?;
    Ok(CodexQuotaUpdate {
        allowed: rate_limit.allowed,
        limit_reached: rate_limit.limit_reached,
        primary_used_percent: primary.as_ref().map(|window| window.used_percent),
        primary_window_seconds: primary.as_ref().map(|window| window.window_seconds),
        primary_reset_at: primary.as_ref().and_then(|window| window.reset_at),
        secondary_used_percent: secondary.as_ref().map(|window| window.used_percent),
        secondary_window_seconds: secondary.as_ref().map(|window| window.window_seconds),
        secondary_reset_at: secondary.as_ref().and_then(|window| window.reset_at),
        reset_credits_available,
        checked_at,
    })
}

#[allow(clippy::too_many_arguments)] // mirrors the authenticated quota-fetch request surface
pub async fn consume_quota_reset_credit(
    client: &Client,
    endpoints: &CodexEndpoints,
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
    redeem_request_id: &str,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
) -> Result<CodexQuotaResetResult, CodexConnectorError> {
    let request_body = serde_json::to_vec(&ConsumeQuotaResetCreditRequest {
        redeem_request_id: redeem_request_id.to_owned(),
    })
    .map_err(|_| CodexConnectorError::InvalidQuotaResponse)?;
    let response = timeout(
        response_header_timeout,
        client
            .post(endpoints.quota_reset_url()?)
            .headers(codex_headers(access_token, account_id, is_fedramp)?)
            .header(CONTENT_TYPE, "application/json")
            .body(request_body)
            .send(),
    )
    .await
    .map_err(|_| CodexConnectorError::UpstreamTimeout)?
    .map_err(|_| CodexConnectorError::UpstreamUnavailable)?;
    let status = response.status();
    let body = read_body(
        response,
        stream_idle_timeout,
        MAX_QUOTA_RESET_RESPONSE_BYTES,
    )
    .await?;
    if !status.is_success() {
        return Err(CodexConnectorError::CodexBackendStatus(status.as_u16()));
    }
    let response: ConsumeQuotaResetCreditResponse =
        serde_json::from_slice(&body).map_err(|_| CodexConnectorError::InvalidQuotaResponse)?;
    if !(0..=2).contains(&response.windows_reset) {
        return Err(CodexConnectorError::InvalidQuotaResponse);
    }
    let outcome = match response.code {
        ConsumeQuotaResetCreditCode::Reset => CodexQuotaResetOutcome::Reset,
        ConsumeQuotaResetCreditCode::NothingToReset => CodexQuotaResetOutcome::NothingToReset,
        ConsumeQuotaResetCreditCode::NoCredit => CodexQuotaResetOutcome::NoCredit,
        ConsumeQuotaResetCreditCode::AlreadyRedeemed => CodexQuotaResetOutcome::AlreadyRedeemed,
    };
    Ok(CodexQuotaResetResult {
        outcome,
        windows_reset: response.windows_reset,
    })
}

struct ValidWindow {
    used_percent: i32,
    window_seconds: i32,
    reset_at: Option<DateTime<Utc>>,
}

fn validate_window(
    window: Option<QuotaWindow>,
) -> Result<Option<ValidWindow>, CodexConnectorError> {
    let Some(window) = window else {
        return Ok(None);
    };
    if !(0..=100).contains(&window.used_percent) || window.limit_window_seconds <= 0 {
        return Err(CodexConnectorError::InvalidQuotaResponse);
    }
    let reset_at = DateTime::from_timestamp(window.reset_at, 0)
        .ok_or(CodexConnectorError::InvalidQuotaResponse)?;
    Ok(Some(ValidWindow {
        used_percent: window.used_percent,
        window_seconds: window.limit_window_seconds,
        reset_at: Some(reset_at),
    }))
}

pub fn parse_identity(jwt: &str) -> Result<CodexIdentity, CodexConnectorError> {
    #[derive(Deserialize)]
    struct Claims {
        #[serde(default)]
        email: Option<String>,
        #[serde(rename = "https://api.openai.com/profile", default)]
        profile: Option<Profile>,
        #[serde(rename = "https://api.openai.com/auth", default)]
        auth: Option<Auth>,
    }
    #[derive(Deserialize)]
    struct Profile {
        #[serde(default)]
        email: Option<String>,
    }
    #[derive(Deserialize)]
    struct Auth {
        #[serde(default)]
        chatgpt_plan_type: Option<Value>,
        #[serde(default)]
        chatgpt_account_id: Option<String>,
        #[serde(default)]
        chatgpt_user_id: Option<String>,
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        chatgpt_account_is_fedramp: bool,
    }

    let claims: Claims = decode_jwt(jwt)?;
    let email = normalize_claim(
        claims
            .email
            .or_else(|| claims.profile.and_then(|profile| profile.email)),
        320,
    )?;
    let auth = claims.auth;
    let account_id = normalize_claim(
        auth.as_ref()
            .and_then(|auth| auth.chatgpt_account_id.clone()),
        300,
    )?;
    let user_id = normalize_claim(
        auth.as_ref().and_then(|auth| {
            auth.chatgpt_user_id
                .clone()
                .or_else(|| auth.user_id.clone())
        }),
        300,
    )?;
    let plan_type = match auth
        .as_ref()
        .and_then(|auth| auth.chatgpt_plan_type.as_ref())
    {
        Some(Value::String(value)) => normalize_claim(Some(value.clone()), 100)?,
        Some(Value::Null) | None => None,
        Some(_) => return Err(CodexConnectorError::InvalidJwt),
    };
    Ok(CodexIdentity {
        email,
        account_id,
        user_id,
        plan_type,
        is_fedramp: auth
            .as_ref()
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
    })
}

pub fn parse_jwt_expiration(jwt: &str) -> Result<Option<DateTime<Utc>>, CodexConnectorError> {
    #[derive(Deserialize)]
    struct Claims {
        #[serde(default)]
        exp: Option<i64>,
    }
    let claims: Claims = decode_jwt(jwt)?;
    claims.exp.map_or(Ok(None), |value| {
        DateTime::from_timestamp(value, 0)
            .map(Some)
            .ok_or(CodexConnectorError::InvalidJwt)
    })
}

fn decode_jwt<T: for<'de> Deserialize<'de>>(jwt: &str) -> Result<T, CodexConnectorError> {
    let mut parts = jwt.split('.');
    let (_, payload, _) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(signature), None)
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty() =>
        {
            (header, payload, signature)
        }
        _ => return Err(CodexConnectorError::InvalidJwt),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .map_err(|_| CodexConnectorError::InvalidJwt)?;
    serde_json::from_slice(&bytes).map_err(|_| CodexConnectorError::InvalidJwt)
}

fn normalize_claim(
    value: Option<String>,
    maximum_bytes: usize,
) -> Result<Option<String>, CodexConnectorError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > maximum_bytes {
        return Err(CodexConnectorError::InvalidJwt);
    }
    Ok(Some(value.to_owned()))
}

fn codex_headers(
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
) -> Result<HeaderMap, CodexConnectorError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| CodexConnectorError::InvalidCredential)?,
    );
    headers.insert(
        "ChatGPT-Account-ID",
        HeaderValue::from_str(account_id).map_err(|_| CodexConnectorError::InvalidCredential)?,
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&codex_user_agent())
            .map_err(|_| CodexConnectorError::InvalidCredential)?,
    );
    headers.insert("originator", HeaderValue::from_static(CODEX_ORIGINATOR));
    headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));
    if is_fedramp {
        headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
    }
    Ok(headers)
}

pub fn codex_user_agent() -> String {
    format!("{CODEX_ORIGINATOR}/{CODEX_CLIENT_VERSION}")
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

fn token_endpoint_error(status: StatusCode, body: &[u8], refreshing: bool) -> CodexConnectorError {
    let code = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| {
                    error
                        .get("code")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                })
                .or_else(|| value.get("code").and_then(Value::as_str))
                .map(str::to_ascii_lowercase)
        });
    if refreshing
        && (status == StatusCode::UNAUTHORIZED
            || matches!(
                code.as_deref(),
                Some(
                    "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated"
                )
            ))
    {
        CodexConnectorError::RefreshTokenInvalid
    } else {
        CodexConnectorError::TokenEndpointStatus(status.as_u16())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap as AxumHeaderMap, StatusCode as AxumStatusCode},
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::net::TcpListener;

    fn jwt(payload: Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn derives_codex_models_and_quota_endpoints_without_url_join_truncation() {
        let endpoints = CodexEndpoints::default();

        assert_eq!(
            endpoints.models_url().unwrap().path(),
            "/backend-api/codex/models"
        );
        assert_eq!(
            endpoints
                .models_url()
                .unwrap()
                .query_pairs()
                .find(|(name, _)| name == "client_version")
                .map(|(_, value)| value.into_owned()),
            Some(CODEX_CLIENT_VERSION.to_owned())
        );
        assert_eq!(
            endpoints.quota_url().unwrap().path(),
            "/backend-api/wham/usage"
        );
        assert_eq!(
            endpoints.quota_reset_url().unwrap().path(),
            "/backend-api/wham/rate-limit-reset-credits/consume"
        );
    }

    #[test]
    fn builds_authorize_url_with_pkce_state_and_fixed_public_client() {
        let endpoints = CodexEndpoints::default();
        let url = Url::parse(
            &build_authorize_url(
                &endpoints,
                &PkceCodes {
                    verifier: "verifier".into(),
                    challenge: "challenge".into(),
                },
                "state-value",
            )
            .unwrap(),
        )
        .unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(url.path(), "/oauth/authorize");
        assert_eq!(
            query.get("client_id").map(|value| value.as_ref()),
            Some(CODEX_OAUTH_CLIENT_ID)
        );
        assert_eq!(
            query.get("redirect_uri").map(|value| value.as_ref()),
            Some(CODEX_OAUTH_REDIRECT_URI)
        );
        assert_eq!(
            query.get("code_challenge").map(|value| value.as_ref()),
            Some("challenge")
        );
        assert_eq!(
            query
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some("state-value")
        );
        assert_eq!(
            query.get("originator").map(|value| value.as_ref()),
            Some(CODEX_ORIGINATOR)
        );
    }

    #[test]
    fn reports_the_pinned_codex_client_identity() {
        assert_eq!(CODEX_ORIGINATOR, "codex_cli_rs");
        assert_eq!(
            codex_user_agent(),
            format!("{CODEX_ORIGINATOR}/{CODEX_CLIENT_VERSION}")
        );
    }

    #[test]
    fn callback_parser_requires_the_exact_loopback_redirect_and_state() {
        let callback = parse_callback_url(
            "http://localhost:1455/auth/callback?code=code-value&state=state-value",
        )
        .unwrap();
        assert_eq!(callback.code, "code-value");
        assert_eq!(callback.state, "state-value");

        assert!(matches!(
            parse_callback_url(
                "https://localhost:1455/auth/callback?code=code-value&state=state-value"
            ),
            Err(CodexConnectorError::InvalidCallback)
        ));
        assert!(matches!(
            parse_callback_url(
                "http://localhost.evil.test:1455/auth/callback?code=code-value&state=state-value"
            ),
            Err(CodexConnectorError::InvalidCallback)
        ));
        assert!(matches!(
            parse_callback_url(
                "http://user@localhost:1455/auth/callback?code=code-value&state=state-value"
            ),
            Err(CodexConnectorError::InvalidCallback)
        ));
        assert!(matches!(
            parse_callback_url(
                "http://localhost:1455/auth/callback?code=code-value&state=state-value#fragment"
            ),
            Err(CodexConnectorError::InvalidCallback)
        ));
        assert!(matches!(
            parse_callback_url(
                "http://localhost:1455/auth/callback?error=access_denied&state=state-value"
            ),
            Err(CodexConnectorError::OauthDenied)
        ));
    }

    #[test]
    fn parses_codex_identity_and_access_token_expiration_without_exposing_tokens() {
        let identity = parse_identity(&jwt(json!({
            "https://api.openai.com/profile": {"email": "codex@example.test"},
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123",
                "chatgpt_user_id": "user-456",
                "chatgpt_plan_type": "plus",
                "chatgpt_account_is_fedramp": true
            }
        })))
        .unwrap();
        assert_eq!(identity.email.as_deref(), Some("codex@example.test"));
        assert_eq!(identity.account_id.as_deref(), Some("account-123"));
        assert_eq!(identity.user_id.as_deref(), Some("user-456"));
        assert_eq!(identity.plan_type.as_deref(), Some("plus"));
        assert!(identity.is_fedramp);

        let fallback_identity = parse_identity(&jwt(json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "business-workspace",
                "user_id": "fallback-user"
            }
        })))
        .unwrap();
        assert_eq!(fallback_identity.user_id.as_deref(), Some("fallback-user"));

        let expires_at = parse_jwt_expiration(&jwt(json!({"exp": 1_800_000_000})))
            .unwrap()
            .unwrap();
        assert_eq!(expires_at.timestamp(), 1_800_000_000);
        assert!(matches!(
            parse_identity(&format!("{}.extra", jwt(json!({})))),
            Err(CodexConnectorError::InvalidJwt)
        ));
        assert!(matches!(
            parse_jwt_expiration(&jwt(json!({"exp": i64::MAX}))),
            Err(CodexConnectorError::InvalidJwt)
        ));
    }

    #[test]
    fn quota_windows_reject_malformed_usage_snapshots() {
        assert!(matches!(
            validate_window(Some(QuotaWindow {
                used_percent: 101,
                limit_window_seconds: 10_800,
                reset_at: 1_800_000_000,
            })),
            Err(CodexConnectorError::InvalidQuotaResponse)
        ));
        assert!(matches!(
            validate_window(Some(QuotaWindow {
                used_percent: 50,
                limit_window_seconds: 0,
                reset_at: 1_800_000_000,
            })),
            Err(CodexConnectorError::InvalidQuotaResponse)
        ));
    }

    #[test]
    fn refresh_token_reuse_is_classified_as_a_permanent_credential_failure() {
        let error = token_endpoint_error(
            StatusCode::BAD_REQUEST,
            br#"{"error":{"code":"refresh_token_reused"}}"#,
            true,
        );
        assert!(matches!(error, CodexConnectorError::RefreshTokenInvalid));
        assert!(error.permanent_refresh_failure());
    }

    #[test]
    fn oauth_state_comparison_accepts_only_the_original_value() {
        let hash = state_hash("original");
        assert!(state_matches(&hash, "original"));
        assert!(!state_matches(&hash, "different"));
        assert!(!state_matches(&hash[..16], "original"));
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
        let expected = [
            ("authorization", "Bearer access-token"),
            ("chatgpt-account-id", "account-123"),
            ("originator", CODEX_ORIGINATOR),
            ("version", CODEX_CLIENT_VERSION),
            ("x-openai-fedramp", "true"),
        ];
        if expected.iter().any(|(name, value)| {
            headers.get(*name).and_then(|header| header.to_str().ok()) != Some(*value)
        }) || headers.get(USER_AGENT).is_none()
        {
            return Err(AxumStatusCode::UNAUTHORIZED);
        }
        Ok(())
    }

    #[tokio::test]
    async fn models_and_quota_clients_use_codex_paths_headers_and_response_shapes() {
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
            &client,
            &endpoints,
            "access-token",
            "account-123",
            true,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(models, vec!["gpt-5-codex"]);

        let quota = fetch_quota(
            &client,
            &endpoints,
            "access-token",
            "account-123",
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

        let reset = consume_quota_reset_credit(
            &client,
            &endpoints,
            "access-token",
            "account-123",
            true,
            "redeem-123",
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(reset.outcome, CodexQuotaResetOutcome::Reset);
        assert_eq!(reset.windows_reset, 2);

        server.abort();
    }
}
