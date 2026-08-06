//! Optional stateless MCP transport and built-in tool dispatch.

mod image;
mod search;

use std::{borrow::Cow, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{
        HeaderValue, Method, Request, Response, StatusCode,
        header::{AUTHORIZATION, COOKIE, HOST, ORIGIN, PROXY_AUTHORIZATION},
        uri::Authority,
    },
    response::IntoResponse,
    routing::post,
};
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, DiscoverResult, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::{
    application::ProxyService,
    domain::{
        ApiFormat, ApiKeyPermission, CompiledApiKey, CompiledMcpServer, CompiledRuntimeConfig,
        McpServerKind,
    },
    runtime_config::McpRuntimeConfig,
};

use self::image::{IMAGEGEN_TOOL_NAME, execute_imagegen, imagegen_tool};
use self::search::{WEB_RUN_TOOL_NAME, execute_web_run, web_run_tool};

type McpTransport = StreamableHttpService<McpHandler, LocalSessionManager>;

/// Public-listener MCP service. Every request is independently authenticated
/// and carries its immutable runtime snapshot into the RMCP handler.
#[derive(Clone)]
pub struct McpService {
    transport: McpTransport,
    image_transport: McpTransport,
    proxy: ProxyService,
    cancellation_token: CancellationToken,
    allowed_hosts: Arc<[String]>,
    allowed_origins: Arc<[String]>,
}

impl McpService {
    #[must_use]
    pub fn new(proxy: ProxyService, config: &McpRuntimeConfig) -> Self {
        let cancellation_token = CancellationToken::new();
        let handler = McpHandler {
            proxy: proxy.clone(),
            allow_legacy_2025_11_25: config.allow_legacy_2025_11_25,
            search_result_bytes: config.search_result_bytes,
            image_result_bytes: config.image_result_bytes,
        };
        let transport = build_transport(
            handler.clone(),
            config,
            config.request_body_bytes,
            cancellation_token.clone(),
        );
        let image_transport = build_transport(
            handler,
            config,
            config.image_request_body_bytes,
            cancellation_token.clone(),
        );
        Self {
            transport,
            image_transport,
            proxy,
            cancellation_token,
            allowed_hosts: Arc::from(config.allowed_hosts.clone()),
            allowed_origins: Arc::from(config.allowed_origins.clone()),
        }
    }

    /// Builds the optional public data-plane routes.
    pub fn router(self) -> Router {
        let allowed_origins = self
            .allowed_origins
            .iter()
            .map(|origin| HeaderValue::from_str(origin).expect("validated MCP origin"))
            .collect::<Vec<_>>();
        let router = Router::new()
            .route("/mcp/{slug}", post(handle_mcp_request))
            .with_state(self);
        if allowed_origins.is_empty() {
            router
        } else {
            router.layer(
                CorsLayer::new()
                    .allow_origin(AllowOrigin::list(allowed_origins))
                    .allow_methods([Method::POST])
                    .allow_headers(AllowHeaders::mirror_request()),
            )
        }
    }

    /// Cancels active RMCP work after listener shutdown has stopped acceptance.
    pub fn begin_shutdown(&self) {
        self.cancellation_token.cancel();
    }

    async fn handle(&self, slug: String, mut request: Request<Body>) -> Response<Body> {
        match optional_single_header(request.headers(), HOST) {
            Ok(Some(host)) if header_authority_is_allowed(host, &self.allowed_hosts) => {}
            Ok(None)
                if request.uri().authority().is_some_and(|authority| {
                    authority_is_allowed(authority, &self.allowed_hosts)
                }) => {}
            Ok(Some(_)) | Ok(None) | Err(()) => return StatusCode::FORBIDDEN.into_response(),
        }
        let origin = match optional_single_header(request.headers(), ORIGIN) {
            Ok(origin) => origin,
            Err(()) => return StatusCode::FORBIDDEN.into_response(),
        };
        if origin.is_some_and(|origin| !origin_is_allowed(origin, &self.allowed_origins)) {
            return StatusCode::FORBIDDEN.into_response();
        }
        if request.headers().contains_key("mcp-session-id") {
            return StatusCode::BAD_REQUEST.into_response();
        }

        let (snapshot, api_key) = match self.proxy.authenticate_api_key(request.headers()) {
            Ok(principal) => principal,
            Err(error) => return error.into_response(),
        };
        let Some(server) = snapshot.mcp_server(&slug) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let transport = match server.kind() {
            McpServerKind::WebSearch => self.transport.clone(),
            McpServerKind::Image => self.image_transport.clone(),
        };
        request.headers_mut().remove(AUTHORIZATION);
        request.headers_mut().remove(PROXY_AUTHORIZATION);
        request.headers_mut().remove(COOKIE);
        request.extensions_mut().insert(McpRequestPrincipal {
            snapshot,
            api_key,
            server,
        });

        let response = match transport.oneshot(request).await {
            Ok(response) => response,
            Err(error) => match error {},
        };
        let (parts, body) = response.into_parts();
        Response::from_parts(parts, Body::new(body))
    }
}

fn build_transport(
    handler: McpHandler,
    config: &McpRuntimeConfig,
    request_body_bytes: usize,
    cancellation_token: CancellationToken,
) -> McpTransport {
    let transport_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_sse_retry(None)
        .with_allowed_hosts(config.allowed_hosts.clone())
        .with_allowed_origins(config.allowed_origins.clone())
        .with_max_request_body_bytes(request_body_bytes)
        .with_stateless_protocol_metadata_required(!config.allow_legacy_2025_11_25)
        .with_cancellation_token(cancellation_token);
    StreamableHttpService::new(
        move || Ok(handler.clone()),
        Default::default(),
        transport_config,
    )
}

fn optional_single_header(
    headers: &axum::http::HeaderMap,
    name: axum::http::header::HeaderName,
) -> Result<Option<&HeaderValue>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        Err(())
    } else {
        Ok(value)
    }
}

fn header_authority_is_allowed(value: &HeaderValue, allowed_hosts: &[String]) -> bool {
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<Authority>().ok())
        .is_some_and(|authority| authority_is_allowed(&authority, allowed_hosts))
}

fn authority_is_allowed(authority: &Authority, allowed_hosts: &[String]) -> bool {
    allowed_hosts
        .iter()
        .any(|allowed| authority.as_str().eq_ignore_ascii_case(allowed))
}

fn origin_is_allowed(value: &HeaderValue, allowed_origins: &[String]) -> bool {
    value
        .to_str()
        .ok()
        .and_then(|value| reqwest::Url::parse(value).ok())
        .filter(|origin| {
            matches!(origin.scheme(), "https" | "http")
                && origin.host().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none()
        })
        .map(|origin| origin.origin().ascii_serialization())
        .is_some_and(|origin| allowed_origins.iter().any(|allowed| allowed == &origin))
}

async fn handle_mcp_request(
    State(service): State<McpService>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    service.handle(slug, request).await
}

#[derive(Clone)]
struct McpRequestPrincipal {
    snapshot: Arc<CompiledRuntimeConfig>,
    api_key: Arc<CompiledApiKey>,
    server: Arc<CompiledMcpServer>,
}

impl McpRequestPrincipal {
    fn permits_web_search(&self) -> bool {
        matches!(self.server.kind(), McpServerKind::WebSearch)
            && self
                .api_key
                .permits(ApiFormat::OpenAiResponses, ApiKeyPermission::Proxy)
            && self.server.model_rule().tiers().iter().any(|tier| {
                tier.candidates().iter().any(|candidate| {
                    candidate.channel().supports_standalone_web_search()
                        && self
                            .api_key
                            .permits_route_candidate(candidate.channel_slot())
                })
            })
    }

    fn permits_image_tool(&self) -> bool {
        matches!(self.server.kind(), McpServerKind::Image)
            && self
                .api_key
                .permits(ApiFormat::OpenAiImages, ApiKeyPermission::Proxy)
            && self.server.model_rule().tiers().iter().any(|tier| {
                tier.candidates().iter().any(|candidate| {
                    self.api_key
                        .permits_route_candidate(candidate.channel_slot())
                })
            })
    }
}

#[derive(Clone)]
struct McpHandler {
    proxy: ProxyService,
    allow_legacy_2025_11_25: bool,
    search_result_bytes: usize,
    image_result_bytes: usize,
}

impl McpHandler {
    fn principal(context: &RequestContext<RoleServer>) -> Result<&McpRequestPrincipal, ErrorData> {
        context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<McpRequestPrincipal>())
            .ok_or_else(|| ErrorData::internal_error("MCP request context is unavailable", None))
    }

    fn server_info(principal: Option<&McpRequestPrincipal>) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().build();
        let (name, description) = principal.map_or_else(
            || ("ai-gateway-mcp".to_owned(), None),
            |principal| {
                (
                    principal.server.slug().to_owned(),
                    principal.server.description().map(ToOwned::to_owned),
                )
            },
        );
        let mut implementation = Implementation::new(name, env!("CARGO_PKG_VERSION"));
        implementation.title = principal.map(|principal| principal.server.name().to_owned());
        implementation.description = description.clone();
        let mut info = ServerInfo::new(capabilities).with_server_info(implementation);
        if let Some(description) = description {
            info = info.with_instructions(description);
        }
        info
    }

    fn versions(&self) -> Cow<'static, [ProtocolVersion]> {
        if self.allow_legacy_2025_11_25 {
            Cow::Borrowed(&[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25])
        } else {
            Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
        }
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        Self::server_info(None)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.versions()
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        if !self.allow_legacy_2025_11_25
            || request.protocol_version != ProtocolVersion::V_2025_11_25
        {
            return Err(ErrorData::invalid_request(
                "initialize is not supported by the stateless MCP 2026-07-28 transport",
                None,
            ));
        }
        context.peer.set_peer_info(request.clone());
        let principal = Self::principal(&context)?;
        let mut info = Self::server_info(Some(principal));
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        Ok(info)
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        let principal = Self::principal(&context)?;
        Ok(DiscoverResult::from_server_info(
            self.versions().into_owned(),
            Self::server_info(Some(principal)),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        match name {
            WEB_RUN_TOOL_NAME => Some(web_run_tool()),
            IMAGEGEN_TOOL_NAME => Some(imagegen_tool()),
            _ => None,
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let principal = Self::principal(&context)?;
        let tools = match principal.server.kind() {
            McpServerKind::WebSearch => principal
                .permits_web_search()
                .then(web_run_tool)
                .into_iter()
                .collect(),
            McpServerKind::Image => principal
                .permits_image_tool()
                .then(imagegen_tool)
                .into_iter()
                .collect(),
        };
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let principal = Self::principal(&context)?.clone();
        match (principal.server.kind(), request.name.as_ref()) {
            (McpServerKind::WebSearch, WEB_RUN_TOOL_NAME) => {
                if !principal.permits_web_search() {
                    return Ok(rmcp::model::CallToolResult::error(vec![
                        rmcp::model::ContentBlock::text(
                            "This API key cannot access the configured MCP tool.",
                        ),
                    ])
                    .into());
                }
                let arguments = serde_json::from_value(
                    request
                        .arguments
                        .map(serde_json::Value::Object)
                        .unwrap_or_else(|| serde_json::json!({})),
                )
                .map_err(|error| {
                    rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
                        format!("Invalid web.run arguments: {error}"),
                    )])
                });
                let arguments = match arguments {
                    Ok(arguments) => arguments,
                    Err(result) => return Ok(result.into()),
                };
                execute_web_run(&self.proxy, principal, arguments, self.search_result_bytes)
                    .await
                    .map(Into::into)
            }
            (McpServerKind::Image, IMAGEGEN_TOOL_NAME) => {
                if !principal.permits_image_tool() {
                    return Ok(rmcp::model::CallToolResult::error(vec![
                        rmcp::model::ContentBlock::text(
                            "This API key cannot access the configured MCP tool.",
                        ),
                    ])
                    .into());
                }
                let arguments = serde_json::from_value(
                    request
                        .arguments
                        .map(serde_json::Value::Object)
                        .unwrap_or_else(|| serde_json::json!({})),
                );
                let arguments = match arguments {
                    Ok(arguments) => arguments,
                    Err(_) => {
                        return Ok(rmcp::model::CallToolResult::error(vec![
                            rmcp::model::ContentBlock::text(
                                "Invalid image_gen.imagegen arguments.",
                            ),
                        ])
                        .into());
                    }
                };
                execute_imagegen(&self.proxy, principal, arguments, self.image_result_bytes)
                    .await
                    .map(Into::into)
            }
            _ => Err(ErrorData::invalid_params("unknown MCP tool", None)),
        }
    }
}
