use std::{collections::HashSet, sync::OnceLock};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use chrono::NaiveDate;
use rmcp::{
    ErrorData,
    handler::server::tool::schema_for_input,
    model::{CallToolResult, ContentBlock, Tool, ToolAnnotations},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    application::ProxyService,
    domain::{
        ApiOperation, McpSearchContextSize, McpSearchExternalWebAccess, RequestLogSource,
        WebSearchMcpSettings,
    },
};

use super::McpRequestPrincipal;

pub(super) const WEB_RUN_TOOL_NAME: &str = "web.run";
static WEB_RUN_TOOL: OnceLock<Tool> = OnceLock::new();
const WEB_RUN_DESCRIPTION: &str = "Access internet search, page navigation, PDF inspection, \
and supported live data lookups. Send only the command families needed for the current call. \
Search batches support up to four queries and image search up to two. Reuse the returned \
search_session_id when opening, clicking, finding, or screenshotting prior result references; \
direct HTTPS URLs do not require a prior search session.";

const MAX_TEXT_BYTES: usize = 8 * 1024;
const MAX_REF_BYTES: usize = 2 * 1024;
const MAX_SEARCH_QUERIES: usize = 4;
const MAX_IMAGE_QUERIES: usize = 2;
const MAX_OPERATIONS: usize = 10;
const MAX_QUERY_DOMAINS: usize = 20;

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebRunArguments {
    /// Opaque stateless continuation handle returned by an earlier call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        length(equal = 36),
        regex(
            pattern = r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
        )
    )]
    pub search_session_id: Option<String>,
    /// Query the internet search engine for a given list of queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 4))]
    pub search_query: Option<Vec<SearchQuery>>,
    /// Query the image search engine for a given list of queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 2))]
    pub image_query: Option<Vec<SearchQuery>>,
    /// Open pages by reference id or a complete HTTPS URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub open: Option<Vec<OpenOperation>>,
    /// Open numbered links from previously opened pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub click: Option<Vec<ClickOperation>>,
    /// Find text patterns in pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub find: Option<Vec<FindOperation>>,
    /// Take screenshots of PDF pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub screenshot: Option<Vec<ScreenshotOperation>>,
    /// Look up market prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub finance: Option<Vec<FinanceOperation>>,
    /// Look up weather forecasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub weather: Option<Vec<WeatherOperation>>,
    /// Look up sports schedules or standings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub sports: Option<Vec<SportsOperation>>,
    /// Get the current time for UTC offsets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 10))]
    pub time: Option<Vec<TimeOperation>>,
    /// Controls the bounded amount of search output requested upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_length: Option<SearchResponseLength>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchQuery {
    /// Search query.
    #[schemars(length(min = 1, max = 8192))]
    pub q: String,
    /// Filter results to this number of recent days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<u64>,
    /// Restrict this query to specific domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 20), inner(length(min = 1, max = 253)))]
    pub domains: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenOperation {
    /// Search reference id or a complete HTTPS URL.
    #[schemars(length(min = 1, max = 2048))]
    pub ref_id: String,
    /// Optional line number to position the opened page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineno: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClickOperation {
    /// Search reference id containing the numbered link.
    #[schemars(length(min = 1, max = 2048))]
    pub ref_id: String,
    /// Numbered link id to open.
    pub id: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindOperation {
    /// Search reference id or a complete HTTPS URL.
    #[schemars(length(min = 1, max = 2048))]
    pub ref_id: String,
    /// Text pattern to find.
    #[schemars(length(min = 1, max = 8192))]
    pub pattern: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScreenshotOperation {
    /// Search reference id or a complete HTTPS PDF URL.
    #[schemars(length(min = 1, max = 2048))]
    pub ref_id: String,
    /// Zero-indexed PDF page number.
    pub pageno: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinanceOperation {
    /// Ticker symbol.
    #[schemars(length(min = 1, max = 64))]
    pub ticker: String,
    /// Asset type.
    pub r#type: FinanceAssetType,
    /// ISO 3166-1 alpha-3 market, `OTC`, or an empty crypto market.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 16))]
    pub market: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FinanceAssetType {
    Equity,
    Fund,
    Crypto,
    Index,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WeatherOperation {
    /// Location in `Country, Area, City` form.
    #[schemars(length(min = 1, max = 512))]
    pub location: String,
    /// Start date in `YYYY-MM-DD` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$"))]
    pub start: Option<String>,
    /// Number of forecast days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub duration: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SportsOperation {
    /// Compatibility discriminator used by Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<SportsToolName>,
    /// Sports operation.
    pub r#fn: SportsFunction,
    /// League.
    pub league: SportsLeague,
    /// Common team alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 64))]
    pub team: Option<String>,
    /// Opponent alias used together with `team`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 64))]
    pub opponent: Option<String>,
    /// Start date in `YYYY-MM-DD` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$"))]
    pub date_from: Option<String>,
    /// End date in `YYYY-MM-DD` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$"))]
    pub date_to: Option<String>,
    /// Maximum games to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub num_games: Option<u64>,
    /// Optional locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 64))]
    pub locale: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsToolName {
    Sports,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsFunction {
    Schedule,
    Standings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsLeague {
    Nba,
    Wnba,
    Nfl,
    Nhl,
    Mlb,
    Epl,
    Ncaamb,
    Ncaawb,
    Ipl,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimeOperation {
    /// UTC offset formatted like `+03:00`.
    #[schemars(regex(pattern = r"^[+-][0-9]{2}:[0-9]{2}$"))]
    pub utc_offset: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchResponseLength {
    Short,
    #[default]
    Medium,
    Long,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WebRunOutput {
    output: String,
    results: Vec<Value>,
    search_session_id: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    output: String,
    #[serde(default)]
    results: Option<Vec<Value>>,
}

#[must_use]
pub(super) fn web_run_tool() -> Tool {
    WEB_RUN_TOOL.get_or_init(build_web_run_tool).clone()
}

fn build_web_run_tool() -> Tool {
    Tool::new(
        WEB_RUN_TOOL_NAME,
        WEB_RUN_DESCRIPTION,
        schema_for_input::<WebRunArguments>().expect("web.run input schema is an object"),
    )
    .with_title("Web search")
    .with_output_schema::<WebRunOutput>()
    .with_annotations(
        ToolAnnotations::with_title("Web search")
            .read_only(true)
            .destructive(false)
            .idempotent(false)
            .open_world(true),
    )
}

pub(super) async fn execute_web_run(
    proxy: &ProxyService,
    principal: McpRequestPrincipal,
    mut arguments: WebRunArguments,
    result_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    let settings = principal
        .server
        .web_search_settings()
        .ok_or_else(|| ErrorData::internal_error("MCP search kind mismatch", None))?;
    let continuation_scope = principal
        .server
        .continuation_scope()
        .ok_or_else(|| ErrorData::internal_error("MCP search scope is unavailable", None))?;
    if let Err(error) = validate_arguments(&arguments) {
        return Ok(tool_error(error.message.into_owned()));
    }
    if let Err(error) = apply_domain_policy(&mut arguments, settings) {
        return Ok(tool_error(error.message.into_owned()));
    }

    let search_session_id = match arguments.search_session_id.as_deref() {
        Some(value) => match Uuid::parse_str(value) {
            Ok(value) => value,
            Err(_) => return Ok(tool_error("search_session_id must be a UUID")),
        },
        None => Uuid::new_v4(),
    };
    if requires_search_session(&arguments) && arguments.search_session_id.is_none() {
        return Ok(tool_error(
            "search_session_id is required when using a prior search reference id",
        ));
    }

    let response_length = arguments.response_length.unwrap_or_default();
    let max_output_tokens = output_tokens(response_length, settings);
    let provider_id = provider_search_id(
        principal.api_key.id(),
        principal.server.id(),
        continuation_scope,
        search_session_id,
    );
    let commands = command_value(&arguments)?;
    let body = json!({
        "id": provider_id,
        "model": principal.server.model_rule().client_model(),
        "input": command_summary(&arguments),
        "commands": commands,
        "settings": search_settings(settings),
        "max_output_tokens": max_output_tokens,
    });
    let body = serde_json::to_vec(&body)
        .map_err(|_| ErrorData::internal_error("failed to encode search request", None))?;
    let request = Request::post("/v1/alpha/search")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|_| ErrorData::internal_error("failed to build search request", None))?;

    let response = match proxy
        .proxy_authenticated(
            ApiOperation::StandaloneWebSearch,
            request,
            principal.snapshot,
            principal.api_key,
            RequestLogSource::Mcp,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(tool_error(format!(
                "Search request failed ({}): {}",
                error.status().as_u16(),
                error.message()
            )));
        }
    };
    let status = response.status();
    let bytes = match to_bytes(response.into_body(), result_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(tool_error(
                "Search response exceeded the configured MCP result limit.",
            ));
        }
    };
    if !status.is_success() {
        return Ok(tool_error(search_error_message(status)));
    }
    let parsed = match serde_json::from_slice::<SearchResponse>(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Ok(tool_error(
                "Search upstream returned an invalid JSON response.",
            ));
        }
    };
    let results = parsed.results.unwrap_or_default();
    let session = search_session_id.to_string();
    let structured = serde_json::to_value(WebRunOutput {
        output: parsed.output.clone(),
        results,
        search_session_id: session,
    })
    .map_err(|_| ErrorData::internal_error("failed to encode MCP search result", None))?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(parsed.output)]);
    result.structured_content = Some(structured);
    Ok(result)
}

fn command_value(arguments: &WebRunArguments) -> Result<Value, ErrorData> {
    let mut value = serde_json::to_value(arguments)
        .map_err(|_| ErrorData::internal_error("failed to encode search commands", None))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ErrorData::internal_error("search commands are not an object", None))?;
    object.remove("search_session_id");
    Ok(value)
}

fn search_settings(settings: &WebSearchMcpSettings) -> Value {
    let mut value = Map::new();
    value.insert(
        "search_context_size".into(),
        Value::String(
            match settings.search_context_size {
                McpSearchContextSize::Low => "low",
                McpSearchContextSize::Medium => "medium",
                McpSearchContextSize::High => "high",
            }
            .into(),
        ),
    );
    value.insert("allowed_callers".into(), json!(["direct"]));
    value.insert(
        "external_web_access".into(),
        match settings.external_web_access {
            McpSearchExternalWebAccess::Cached => Value::Bool(false),
            McpSearchExternalWebAccess::Indexed => Value::String("indexed".into()),
            McpSearchExternalWebAccess::Live => Value::Bool(true),
        },
    );
    if !settings.allowed_domains.is_empty() || !settings.blocked_domains.is_empty() {
        let mut filters = Map::new();
        if !settings.allowed_domains.is_empty() {
            filters.insert("allowed_domains".into(), json!(settings.allowed_domains));
        }
        if !settings.blocked_domains.is_empty() {
            filters.insert("blocked_domains".into(), json!(settings.blocked_domains));
        }
        value.insert("filters".into(), Value::Object(filters));
    }
    Value::Object(value)
}

fn output_tokens(response_length: SearchResponseLength, settings: &WebSearchMcpSettings) -> u64 {
    match response_length {
        SearchResponseLength::Short => settings.max_output_tokens.short,
        SearchResponseLength::Medium => settings.max_output_tokens.medium,
        SearchResponseLength::Long => settings.max_output_tokens.long,
    }
}

fn provider_search_id(
    api_key_id: Uuid,
    mcp_server_id: Uuid,
    continuation_scope: &[u8; 32],
    search_session_id: Uuid,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway/mcp/web-search-session/v2\0");
    hasher.update(api_key_id.as_bytes());
    hasher.update(mcp_server_id.as_bytes());
    hasher.update(continuation_scope);
    hasher.update(search_session_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn command_summary(arguments: &WebRunArguments) -> String {
    let mut commands = Vec::new();
    push_command(&mut commands, "search_query", &arguments.search_query);
    push_command(&mut commands, "image_query", &arguments.image_query);
    push_command(&mut commands, "open", &arguments.open);
    push_command(&mut commands, "click", &arguments.click);
    push_command(&mut commands, "find", &arguments.find);
    push_command(&mut commands, "screenshot", &arguments.screenshot);
    push_command(&mut commands, "finance", &arguments.finance);
    push_command(&mut commands, "weather", &arguments.weather);
    push_command(&mut commands, "sports", &arguments.sports);
    push_command(&mut commands, "time", &arguments.time);
    format!("Execute web.run commands: {}.", commands.join(", "))
}

fn push_command<T>(target: &mut Vec<String>, name: &str, values: &Option<Vec<T>>) {
    if let Some(values) = values
        && !values.is_empty()
    {
        target.push(format!("{name}={}", values.len()));
    }
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn search_error_message(status: StatusCode) -> String {
    format!(
        "Search request failed ({}): The search upstream rejected the request.",
        status.as_u16()
    )
}

fn validate_arguments(arguments: &WebRunArguments) -> Result<(), ErrorData> {
    let mut command_count = 0;
    command_count += validate_list(
        "search_query",
        arguments.search_query.as_deref(),
        MAX_SEARCH_QUERIES,
    )?;
    command_count += validate_list(
        "image_query",
        arguments.image_query.as_deref(),
        MAX_IMAGE_QUERIES,
    )?;
    command_count += validate_list("open", arguments.open.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list("click", arguments.click.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list("find", arguments.find.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list(
        "screenshot",
        arguments.screenshot.as_deref(),
        MAX_OPERATIONS,
    )?;
    command_count += validate_list("finance", arguments.finance.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list("weather", arguments.weather.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list("sports", arguments.sports.as_deref(), MAX_OPERATIONS)?;
    command_count += validate_list("time", arguments.time.as_deref(), MAX_OPERATIONS)?;
    if command_count == 0 {
        return Err(ErrorData::invalid_params(
            "web.run requires at least one command",
            None,
        ));
    }

    for query in arguments
        .search_query
        .iter()
        .flatten()
        .chain(arguments.image_query.iter().flatten())
    {
        validate_text("query", &query.q, MAX_TEXT_BYTES)?;
        if let Some(domains) = query.domains.as_deref() {
            if domains.is_empty() || domains.len() > MAX_QUERY_DOMAINS {
                return Err(ErrorData::invalid_params(
                    format!("query domains must contain 1 to {MAX_QUERY_DOMAINS} items"),
                    None,
                ));
            }
            let mut seen = HashSet::new();
            for domain in domains {
                let canonical = canonical_domain(domain)?;
                if !seen.insert(canonical) {
                    return Err(ErrorData::invalid_params(
                        "query domains must not contain duplicates",
                        None,
                    ));
                }
            }
        }
    }
    for operation in arguments.open.iter().flatten() {
        validate_ref_id(&operation.ref_id)?;
    }
    for operation in arguments.click.iter().flatten() {
        validate_ref_id(&operation.ref_id)?;
    }
    for operation in arguments.find.iter().flatten() {
        validate_ref_id(&operation.ref_id)?;
        validate_text("find pattern", &operation.pattern, MAX_TEXT_BYTES)?;
    }
    for operation in arguments.screenshot.iter().flatten() {
        validate_ref_id(&operation.ref_id)?;
    }
    for operation in arguments.finance.iter().flatten() {
        validate_text("ticker", &operation.ticker, 64)?;
        if let Some(market) = operation.market.as_deref() {
            validate_optional_text("market", market, 16)?;
        }
    }
    for operation in arguments.weather.iter().flatten() {
        validate_text("weather location", &operation.location, 512)?;
        if let Some(start) = operation.start.as_deref() {
            validate_date("weather start", start)?;
        }
        if operation.duration == Some(0) {
            return Err(ErrorData::invalid_params(
                "weather duration must be greater than zero",
                None,
            ));
        }
    }
    for operation in arguments.sports.iter().flatten() {
        if let Some(team) = operation.team.as_deref() {
            validate_optional_text("sports team", team, 64)?;
        }
        if let Some(opponent) = operation.opponent.as_deref() {
            validate_optional_text("sports opponent", opponent, 64)?;
        }
        if let Some(date) = operation.date_from.as_deref() {
            validate_date("sports date_from", date)?;
        }
        if let Some(date) = operation.date_to.as_deref() {
            validate_date("sports date_to", date)?;
        }
        if operation.num_games == Some(0) {
            return Err(ErrorData::invalid_params(
                "sports num_games must be greater than zero",
                None,
            ));
        }
        if let Some(locale) = operation.locale.as_deref() {
            validate_optional_text("sports locale", locale, 64)?;
        }
    }
    for operation in arguments.time.iter().flatten() {
        if !valid_utc_offset(&operation.utc_offset) {
            return Err(ErrorData::invalid_params(
                "utc_offset must use the form +HH:MM or -HH:MM",
                None,
            ));
        }
    }
    Ok(())
}

fn validate_list<T>(name: &str, values: Option<&[T]>, maximum: usize) -> Result<usize, ErrorData> {
    let Some(values) = values else {
        return Ok(0);
    };
    if values.is_empty() || values.len() > maximum {
        return Err(ErrorData::invalid_params(
            format!("{name} must contain 1 to {maximum} items"),
            None,
        ));
    }
    Ok(values.len())
}

fn validate_ref_id(value: &str) -> Result<(), ErrorData> {
    validate_text("ref_id", value, MAX_REF_BYTES)?;
    if value.starts_with("http://") {
        return Err(ErrorData::invalid_params(
            "direct page URLs must use HTTPS",
            None,
        ));
    }
    if value.contains("://") && !is_direct_https_url(value) {
        return Err(ErrorData::invalid_params(
            "ref_id must be a search reference id or a valid HTTPS URL",
            None,
        ));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, maximum: usize) -> Result<(), ErrorData> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(ErrorData::invalid_params(
            format!("{name} must be non-empty and at most {maximum} bytes"),
            None,
        ));
    }
    Ok(())
}

fn validate_optional_text(name: &str, value: &str, maximum: usize) -> Result<(), ErrorData> {
    if value.len() > maximum || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(ErrorData::invalid_params(
            format!("{name} must be at most {maximum} bytes and contain no control characters"),
            None,
        ));
    }
    Ok(())
}

fn validate_date(name: &str, value: &str) -> Result<(), ErrorData> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
        && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok();
    if !valid {
        return Err(ErrorData::invalid_params(
            format!("{name} must use YYYY-MM-DD"),
            None,
        ));
    }
    Ok(())
}

fn valid_utc_offset(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 6
        || !matches!(bytes[0], b'+' | b'-')
        || bytes[3] != b':'
        || !bytes[1..3].iter().all(u8::is_ascii_digit)
        || !bytes[4..6].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let hours = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
    let minutes = (bytes[4] - b'0') * 10 + bytes[5] - b'0';
    hours <= 23 && minutes <= 59
}

fn canonical_domain(value: &str) -> Result<String, ErrorData> {
    if value.is_empty() || value.len() > 253 || !value.is_ascii() || value.ends_with('.') {
        return Err(ErrorData::invalid_params("invalid search domain", None));
    }
    let canonical = value.to_ascii_lowercase();
    if !canonical.split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    }) {
        return Err(ErrorData::invalid_params("invalid search domain", None));
    }
    Ok(canonical)
}

fn is_direct_https_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn requires_search_session(arguments: &WebRunArguments) -> bool {
    arguments
        .open
        .iter()
        .flatten()
        .any(|operation| !is_direct_https_url(&operation.ref_id))
        || arguments.click.iter().flatten().next().is_some()
        || arguments
            .find
            .iter()
            .flatten()
            .any(|operation| !is_direct_https_url(&operation.ref_id))
        || arguments
            .screenshot
            .iter()
            .flatten()
            .any(|operation| !is_direct_https_url(&operation.ref_id))
}

fn apply_domain_policy(
    arguments: &mut WebRunArguments,
    settings: &WebSearchMcpSettings,
) -> Result<(), ErrorData> {
    let allowed = settings
        .allowed_domains
        .iter()
        .map(|domain| domain.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let blocked = settings
        .blocked_domains
        .iter()
        .map(|domain| domain.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for query in arguments
        .search_query
        .iter_mut()
        .flatten()
        .chain(arguments.image_query.iter_mut().flatten())
    {
        let Some(domains) = query.domains.take() else {
            continue;
        };
        let supplied = !domains.is_empty();
        let mut normalized = Vec::with_capacity(domains.len());
        let mut seen = HashSet::new();
        for domain in domains {
            let domain = canonical_domain(&domain)?;
            if blocked.contains(&domain)
                || (!allowed.is_empty() && !allowed.contains(&domain))
                || !seen.insert(domain.clone())
            {
                continue;
            }
            normalized.push(domain);
        }
        if supplied && normalized.is_empty() {
            return Err(ErrorData::invalid_params(
                "query domains are excluded by this MCP server's domain policy",
                None,
            ));
        }
        query.domains = Some(normalized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{McpSearchTokenLimits, WebSearchMcpSettings};

    fn query_arguments() -> WebRunArguments {
        WebRunArguments {
            search_query: Some(vec![SearchQuery {
                q: "Rust MCP".into(),
                recency: None,
                domains: None,
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn prior_reference_requires_search_session() {
        let arguments = WebRunArguments {
            open: Some(vec![OpenOperation {
                ref_id: "turn0search0".into(),
                lineno: None,
            }]),
            ..Default::default()
        };
        assert!(requires_search_session(&arguments));

        let direct = WebRunArguments {
            open: Some(vec![OpenOperation {
                ref_id: "https://example.com/docs".into(),
                lineno: None,
            }]),
            ..Default::default()
        };
        assert!(!requires_search_session(&direct));
    }

    #[test]
    fn domain_policy_intersects_and_blocks_query_domains() {
        let mut arguments = query_arguments();
        arguments.search_query.as_mut().unwrap()[0].domains = Some(vec![
            "Allowed.Example".into(),
            "blocked.example".into(),
            "other.example".into(),
        ]);
        let settings = WebSearchMcpSettings {
            allowed_domains: vec!["allowed.example".into(), "blocked.example".into()],
            blocked_domains: vec!["blocked.example".into()],
            max_output_tokens: McpSearchTokenLimits::default(),
            ..Default::default()
        };

        apply_domain_policy(&mut arguments, &settings).unwrap();

        assert_eq!(
            arguments.search_query.unwrap()[0].domains.as_deref(),
            Some(["allowed.example".to_owned()].as_slice())
        );
    }

    #[test]
    fn validates_command_presence_and_limits() {
        assert!(validate_arguments(&WebRunArguments::default()).is_err());
        assert!(validate_arguments(&query_arguments()).is_ok());

        let arguments = WebRunArguments {
            image_query: Some(
                (0..=MAX_IMAGE_QUERIES)
                    .map(|index| SearchQuery {
                        q: format!("image {index}"),
                        recency: None,
                        domains: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        assert!(validate_arguments(&arguments).is_err());

        let invalid_date = WebRunArguments {
            weather: Some(vec![WeatherOperation {
                location: "United States, CA, San Francisco".into(),
                start: Some("2026-02-30".into()),
                duration: Some(7),
            }]),
            ..Default::default()
        };
        assert!(validate_arguments(&invalid_date).is_err());
    }

    #[test]
    fn provider_id_is_stable_and_principal_scoped() {
        let session = Uuid::parse_str("a9d9239b-8faa-45c8-8933-bf47d0e74062").unwrap();
        let first_key = Uuid::parse_str("187a5fed-da61-440d-b037-1c26f79f33c8").unwrap();
        let second_key = Uuid::parse_str("419dbf28-4b82-4594-954f-484c5242fb25").unwrap();
        let first_server = Uuid::parse_str("a6e931b6-e926-4f78-a30f-f3fd849c8920").unwrap();
        let second_server = Uuid::parse_str("de9e6d23-d420-44c6-a1fa-3449a229f19f").unwrap();
        let first_scope = [1_u8; 32];
        let second_scope = [2_u8; 32];

        assert_eq!(
            provider_search_id(first_key, first_server, &first_scope, session),
            provider_search_id(first_key, first_server, &first_scope, session)
        );
        assert_ne!(
            provider_search_id(first_key, first_server, &first_scope, session),
            provider_search_id(second_key, first_server, &first_scope, session)
        );
        assert_ne!(
            provider_search_id(first_key, first_server, &first_scope, session),
            provider_search_id(first_key, second_server, &first_scope, session)
        );
        assert_ne!(
            provider_search_id(first_key, first_server, &first_scope, session),
            provider_search_id(first_key, first_server, &second_scope, session)
        );
    }
}
