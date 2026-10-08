//! Administrator-only native plugin management and bounded package uploads.

use super::*;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

struct StagedUpload {
    path: std::path::PathBuf,
    handed_off: bool,
}

impl Drop for StagedUpload {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(super) fn routes() -> Router<ConsoleState> {
    Router::new()
        .route("/console/v1/plugins", get(list))
        .route("/console/v1/plugins/reauth", post(reauthorize))
        .route("/console/v1/plugins/discover", post(discover))
        .route("/console/v1/plugins/jobs/{id}", get(job))
        .route("/console/v1/plugins/{id}", get(detail))
        .route(
            "/console/v1/plugins/{id}/state",
            axum::routing::put(save_state),
        )
        .route(
            "/console/v1/plugins/{id}/settings",
            get(settings).put(save_settings),
        )
        .route(
            "/console/v1/plugins/{id}/artifacts/{digest}",
            axum::routing::delete(delete_artifact),
        )
}

pub(super) fn upload_routes() -> Router<ConsoleState> {
    Router::new()
        .route("/console/v1/plugins/install", post(install))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(MAX_UPLOAD_BYTES))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReauthorizationInput {
    password: String,
}

async fn reauthorize(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ReauthorizationInput>,
) -> Result<Response, ConsoleError> {
    Ok(Json(
        state
            .auth
            .authorize_plugin_management(principal, input.password)
            .await?,
    )
    .into_response())
}

async fn authorize(
    state: &ConsoleState,
    principal: ConsolePrincipal,
    headers: &HeaderMap,
) -> Result<(), ConsoleError> {
    let token = headers
        .get("x-plugin-authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or(ConsoleError::Auth(AuthError::InvalidCredentials))?;
    state
        .auth
        .consume_plugin_authorization(principal, token)
        .await?;
    state
        .coordinator
        .verify_active_admin(principal.user_id())
        .await?;
    Ok(())
}

fn expected(headers: &HeaderMap) -> Result<&str, ConsoleError> {
    headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or(ConsoleError::Validation)
}

fn with_etag(value: impl Serialize, revision: i64, digest: Option<&str>) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&crate::application::plugin_etag(revision, digest))
            .expect("plugin ETag is ASCII"),
    );
    response
}

fn mutation(result: crate::persistence::MutationResult) -> Json<MutationResponse> {
    Json(MutationResponse {
        id: result.id,
        secret: None,
        correlation_id: result
            .correlation_id
            .expect("committed mutation has an audit correlation"),
    })
}

async fn list(State(state): State<ConsoleState>) -> Result<Response, ConsoleError> {
    Ok(Json(state.coordinator.managed_plugins().await?).into_response())
}

async fn detail(
    State(state): State<ConsoleState>,
    Path(id): Path<String>,
) -> Result<Response, ConsoleError> {
    let value = state
        .coordinator
        .managed_plugins()
        .await?
        .into_iter()
        .find(|plugin| plugin.id == id)
        .ok_or(ConsoleError::NotFound)?;
    Ok(with_etag(
        &value,
        value.revision,
        value.artifact_digest.as_deref(),
    ))
}

async fn settings(
    State(state): State<ConsoleState>,
    Path(id): Path<String>,
) -> Result<Response, ConsoleError> {
    let value = state.coordinator.managed_plugin_settings(&id).await?;
    Ok(with_etag(
        &value,
        value.revision,
        Some(&value.artifact_digest),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateInput {
    enabled: bool,
    artifact_digest: Option<String>,
}

async fn save_state(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<StateInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    authorize(&state, principal, &headers).await?;
    Ok(mutation(
        state
            .coordinator
            .save_plugin_state(
                principal.user_id(),
                id,
                input.enabled,
                input.artifact_digest,
                expected(&headers)?,
            )
            .await?,
    ))
}

async fn save_settings(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<crate::persistence::PluginSettingsInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    authorize(&state, principal, &headers).await?;
    Ok(mutation(
        state
            .coordinator
            .save_plugin_settings(principal.user_id(), id, input, expected(&headers)?)
            .await?,
    ))
}

async fn delete_artifact(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path((id, digest)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<MutationResponse>, ConsoleError> {
    authorize(&state, principal, &headers).await?;
    Ok(mutation(
        state
            .coordinator
            .delete_plugin_artifact(principal.user_id(), id, digest, expected(&headers)?)
            .await?,
    ))
}

async fn discover(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    headers: HeaderMap,
) -> Result<Response, ConsoleError> {
    authorize(&state, principal, &headers).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .coordinator
                .begin_plugin_job(principal.user_id(), Uuid::new_v4(), None)
                .await?,
        ),
    )
        .into_response())
}

async fn job(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    Ok(Json(state.coordinator.plugin_job(id).await?).into_response())
}

async fn install(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ConsoleError> {
    authorize(&state, principal, &headers).await?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(ConsoleError::Validation);
    }
    let id = Uuid::new_v4();
    let path = state
        .coordinator
        .plugin_catalog()?
        .root()
        .join("staging")
        .join(format!("{id}.tar.gz"));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .await
        .map_err(|_| ConsoleError::Internal)?;
    let mut staged = StagedUpload {
        path: path.clone(),
        handed_off: false,
    };
    let mut stream = body.into_data_stream();
    let mut size = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ConsoleError::Validation)?;
        size = size
            .checked_add(chunk.len())
            .filter(|size| *size <= MAX_UPLOAD_BYTES)
            .ok_or(ConsoleError::Validation)?;
        file.write_all(&chunk)
            .await
            .map_err(|_| ConsoleError::Internal)?;
    }
    if size == 0 {
        return Err(ConsoleError::Validation);
    }
    file.sync_all().await.map_err(|_| ConsoleError::Internal)?;
    drop(file);
    let job = state
        .coordinator
        .begin_plugin_job(principal.user_id(), id, Some(path))
        .await?;
    staged.handed_off = true;
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}
