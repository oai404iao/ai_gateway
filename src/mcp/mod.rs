//! Optional MCP transport, legacy Session compatibility, and built-in tool dispatch.

mod image;
mod search;

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{
        HeaderName, HeaderValue, Method, Request, Response, StatusCode,
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
        StreamableHttpServerConfig, StreamableHttpService,
        session::{SessionManager, local::LocalSessionManager},
    },
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::{
    application::ProxyService,
    domain::{
        ApiFormat, ApiKeyPermission, CompiledApiKey, CompiledMcpServer, CompiledRuntimeConfig,
        McpServerKind, McpTransportSettings,
    },
    runtime_config::RuntimeConfig,
};

use self::image::{IMAGEGEN_TOOL_NAME, execute_imagegen, imagegen_tool};
use self::search::{WEB_RUN_TOOL_NAME, execute_web_run, web_run_tool};

type McpTransport = StreamableHttpService<McpHandler, LocalSessionManager>;

/// Public-listener MCP service. Every HTTP request is authenticated against one
/// immutable runtime snapshot; optional legacy protocol state remains confined
/// to the RMCP transport.
#[derive(Clone)]
pub struct McpService {
    proxy: ProxyService,
    runtime: Arc<RuntimeConfig>,
    cancellation_token: CancellationToken,
    active: Arc<Mutex<Option<ActiveMcpTransports>>>,
}

struct ActiveMcpTransports {
    settings: McpTransportSettings,
    cancellation_token: CancellationToken,
    search: McpTransport,
    image: McpTransport,
    search_sessions: Arc<LocalSessionManager>,
    image_sessions: Arc<LocalSessionManager>,
}

impl ActiveMcpTransports {
    fn close_legacy_sessions(self) {
        tokio::spawn(async move {
            close_all_sessions(self.search_sessions).await;
            close_all_sessions(self.image_sessions).await;
        });
    }
}

impl McpService {
    #[must_use]
    pub fn new(proxy: ProxyService, runtime: Arc<RuntimeConfig>) -> Self {
        let updates = runtime.subscribe();
        let cancellation_token = CancellationToken::new();
        let service = Self {
            proxy,
            runtime,
            cancellation_token,
            active: Arc::new(Mutex::new(None)),
        };
        service.reconcile(updates.borrow().system_settings().mcp());
        service.spawn_reconciler(updates);
        service
    }

    /// Builds the optional public data-plane routes.
    pub fn router(self) -> Router {
        let runtime = Arc::clone(&self.runtime);
        Router::new()
            .route(
                "/mcp/{slug}",
                post(handle_mcp_request)
                    .get(handle_mcp_request)
                    .delete(handle_mcp_request),
            )
            .with_state(self)
            .layer(
                CorsLayer::new()
                    .allow_origin(AllowOrigin::predicate(move |origin, _| {
                        let snapshot = runtime.snapshot();
                        let settings = snapshot.system_settings().mcp();
                        settings.enabled() && origin_is_allowed(origin, settings.allowed_origins())
                    }))
                    .allow_methods([Method::POST, Method::GET, Method::DELETE])
                    .allow_headers(AllowHeaders::mirror_request())
                    .expose_headers([HeaderName::from_static("mcp-session-id")]),
            )
    }

    /// Cancels active RMCP work after listener shutdown has stopped acceptance.
    pub fn begin_shutdown(&self) {
        self.cancellation_token.cancel();
        if let Some(active) = self
            .active
            .lock()
            .expect("MCP transport state lock poisoned")
            .take()
        {
            active.cancellation_token.cancel();
        }
    }

    fn spawn_reconciler(
        &self,
        mut updates: tokio::sync::watch::Receiver<Arc<CompiledRuntimeConfig>>,
    ) {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = service.cancellation_token.cancelled() => break,
                    changed = updates.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let snapshot = updates.borrow_and_update().clone();
                        service.reconcile(snapshot.system_settings().mcp());
                    }
                }
            }
        });
    }

    fn reconcile(&self, settings: &McpTransportSettings) {
        let mut active = self
            .active
            .lock()
            .expect("MCP transport state lock poisoned");
        if active
            .as_ref()
            .is_some_and(|active| active.settings == *settings)
        {
            return;
        }
        if let Some(previous) = active.take() {
            previous.close_legacy_sessions();
        }
        if !settings.enabled() || self.cancellation_token.is_cancelled() {
            return;
        }
        let cancellation_token = self.cancellation_token.child_token();
        let handler = McpHandler {
            proxy: self.proxy.clone(),
            allow_legacy_2025_11_25: settings.allow_legacy_2025_11_25(),
            search_result_bytes: settings.search_result_bytes(),
            image_result_bytes: settings.image_result_bytes(),
        };
        let search_sessions = Arc::new(LocalSessionManager::default());
        let image_sessions = Arc::new(LocalSessionManager::default());
        let search = build_transport(
            handler.clone(),
            settings,
            settings.request_body_bytes(),
            Arc::clone(&search_sessions),
            cancellation_token.clone(),
        );
        let image = build_transport(
            handler,
            settings,
            settings.image_request_body_bytes(),
            Arc::clone(&image_sessions),
            cancellation_token.clone(),
        );
        *active = Some(ActiveMcpTransports {
            settings: settings.clone(),
            cancellation_token,
            search,
            image,
            search_sessions,
            image_sessions,
        });
    }

    fn transport(
        &self,
        snapshot: &Arc<CompiledRuntimeConfig>,
        kind: McpServerKind,
    ) -> Option<McpTransport> {
        let latest = self.runtime.snapshot();
        if !Arc::ptr_eq(snapshot, &latest) {
            return None;
        }
        let settings = snapshot.system_settings().mcp();
        self.reconcile(settings);
        let active = self
            .active
            .lock()
            .expect("MCP transport state lock poisoned");
        active.as_ref().map(|active| match kind {
            McpServerKind::WebSearch => active.search.clone(),
            McpServerKind::Image => active.image.clone(),
        })
    }

    async fn handle(&self, slug: String, mut request: Request<Body>) -> Response<Body> {
        loop {
            let snapshot = self.runtime.snapshot();
            let settings = snapshot.system_settings().mcp();
            if !settings.enabled() {
                self.reconcile(settings);
                return StatusCode::NOT_FOUND.into_response();
            }
            match optional_single_header(request.headers(), HOST) {
                Ok(Some(host)) if header_authority_is_allowed(host, settings.allowed_hosts()) => {}
                Ok(None)
                    if request.uri().authority().is_some_and(|authority| {
                        authority_is_allowed(authority, settings.allowed_hosts())
                    }) => {}
                Ok(Some(_)) | Ok(None) | Err(()) => return StatusCode::FORBIDDEN.into_response(),
            }
            let origin = match optional_single_header(request.headers(), ORIGIN) {
                Ok(origin) => origin,
                Err(()) => return StatusCode::FORBIDDEN.into_response(),
            };
            if origin.is_some_and(|origin| !origin_is_allowed(origin, settings.allowed_origins())) {
                return StatusCode::FORBIDDEN.into_response();
            }
            let session_id = match optional_single_header(
                request.headers(),
                HeaderName::from_static("mcp-session-id"),
            ) {
                Ok(session_id) => session_id,
                Err(()) => return StatusCode::BAD_REQUEST.into_response(),
            };
            if session_id.is_some()
                && (!settings.allow_legacy_2025_11_25()
                    || request
                        .headers()
                        .get("mcp-protocol-version")
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|version| !supported_legacy_protocol_version_str(version)))
            {
                return StatusCode::BAD_REQUEST.into_response();
            }

            let api_key = match self
                .proxy
                .authenticate_api_key_in_snapshot(request.headers(), &snapshot)
            {
                Ok(principal) => principal,
                Err(error) => return error.into_response(),
            };
            let Some(server) = snapshot.mcp_server(&slug) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let Some(transport) = self.transport(&snapshot, server.kind()) else {
                if !Arc::ptr_eq(&snapshot, &self.runtime.snapshot()) {
                    continue;
                }
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
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
            return Response::from_parts(parts, Body::new(body));
        }
    }
}

fn build_transport(
    handler: McpHandler,
    settings: &McpTransportSettings,
    request_body_bytes: usize,
    session_manager: Arc<LocalSessionManager>,
    cancellation_token: CancellationToken,
) -> McpTransport {
    let transport_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(settings.allow_legacy_2025_11_25())
        .with_json_response(true)
        .with_allowed_hosts(settings.allowed_hosts().iter().cloned())
        .with_allowed_origins(settings.allowed_origins().iter().cloned())
        .with_max_request_body_bytes(request_body_bytes)
        .with_stateless_protocol_metadata_required(true)
        .with_cancellation_token(cancellation_token);
    StreamableHttpService::new(
        move || Ok(handler.clone()),
        session_manager,
        transport_config,
    )
}

async fn close_all_sessions(manager: Arc<LocalSessionManager>) {
    let sessions = manager
        .sessions
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for session_id in sessions {
        let _ = manager.close_session(&session_id).await;
    }
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

fn supported_legacy_protocol_version(version: &ProtocolVersion) -> bool {
    version == &ProtocolVersion::V_2025_11_25 || version == &ProtocolVersion::V_2025_06_18
}

fn supported_legacy_protocol_version_str(version: &str) -> bool {
    version == ProtocolVersion::V_2025_11_25.as_str()
        || version == ProtocolVersion::V_2025_06_18.as_str()
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
            Cow::Borrowed(&[
                ProtocolVersion::V_2026_07_28,
                ProtocolVersion::V_2025_11_25,
                ProtocolVersion::V_2025_06_18,
            ])
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
            || !supported_legacy_protocol_version(&request.protocol_version)
        {
            return Err(ErrorData::invalid_request(
                "initialize is not supported for this MCP protocol version",
                None,
            ));
        }
        let protocol_version = request.protocol_version.clone();
        context.peer.set_peer_info(request.clone());
        let principal = Self::principal(&context)?;
        let mut info = Self::server_info(Some(principal));
        info.protocol_version = protocol_version;
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
