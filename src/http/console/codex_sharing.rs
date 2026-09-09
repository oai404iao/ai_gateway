//! Console sharing management and ownership-scoped monetary usage.

use super::*;
use crate::domain::codex_sharing::{SharingGroup, SharingGroupInput};

pub(super) async fn list(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(serde_json::json!({
        "runtime_available": state.coordinator.sharing_runtime().available(),
        "groups": state.coordinator.sharing_groups(None).await?,
    })))
}

pub(super) async fn create(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<SharingGroupInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(
        &state,
        principal,
        ControlPlaneMutation::SaveCodexSharing {
            id: Uuid::new_v4(),
            input,
            expected_updated_at: None,
        },
    )
    .await
}

async fn group(state: &ConsoleState, id: Uuid) -> Result<SharingGroup, ConsoleError> {
    state
        .coordinator
        .sharing_groups(None)
        .await?
        .into_iter()
        .find(|group| group.id == id)
        .ok_or(ConsoleError::NotFound)
}

pub(super) async fn get(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    resource_response(to_json(group(&state, id).await?))
}

pub(super) async fn update(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<SharingGroupInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::SaveCodexSharing {
            id,
            input,
            expected_updated_at: Some(if_match(&headers)?),
        },
    )
    .await
}

pub(super) async fn usage(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    let group = group(&state, id).await?;
    let mut seats = Vec::new();
    for (index, user) in group.policy.seats.iter().enumerate() {
        let mut inspection = group.clone();
        let user_id = user.unwrap_or_else(Uuid::nil);
        inspection.policy.seats[index] = Some(user_id);
        let usage = state
            .coordinator
            .sharing_runtime()
            .inspect(&inspection, user_id)
            .await;
        seats.push(serde_json::json!({
            "seat_number": index + 1, "user_id": user, "usage": usage,
        }));
    }
    Ok(Json(serde_json::json!({ "seats": seats })))
}

pub(super) async fn own(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    let Some(group) = state
        .coordinator
        .sharing_groups(Some(principal.user_id()))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Json(serde_json::Value::Null));
    };
    let usage = state
        .coordinator
        .sharing_runtime()
        .inspect(&group, principal.user_id())
        .await;
    Ok(Json(serde_json::json!({
        "id": group.id, "name": group.policy.name, "enabled": group.policy.enabled,
        "seat_count": group.policy.seats.len(), "currency": "USD",
        "request_reservation_amount": group.policy.request_reservation_amount,
        "user_requests_per_minute": group.policy.user_requests_per_minute,
        "user_max_concurrent_requests": group.policy.user_max_concurrent_requests,
        "usage": usage,
    })))
}
