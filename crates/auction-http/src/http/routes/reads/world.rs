use super::*;

pub(crate) async fn list_worlds(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<WorldsResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "世界目录不接受查询参数"));
    }

    let Some(reader) = state.worlds() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "世界目录暂不可用",
        ));
    };
    let worlds = reader.list_worlds().await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "世界目录暂不可用")
    })?;
    let worlds = worlds.into_iter().map(map_world).collect();
    Ok(Json(ApiResponse::ok("世界状态已加载", WorldsResponse { worlds })))
}

pub(crate) fn map_world(world: WorldSnapshot) -> (String, WorldStatusResponse) {
    (world.id, WorldStatusResponse { enabled: world.enabled })
}
