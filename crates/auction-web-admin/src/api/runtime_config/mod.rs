//! Admin runtime 配置 HTTP 客户端。
//! 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
//! 文件、密钥和命令账本永远不进入浏览器。

#[cfg(test)]
use std::rc::Rc;

mod document;
mod fields;
mod infrastructure;
mod operations;
mod taxonomy;

pub(crate) use auction_contracts::AdminRuntimeConfigResponse as AdminRuntimeConfig;

use auction_contracts::{
    AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest, ApiResponse,
};
use gloo_net::http::Request;
use serde_json::Value;

use self::document::valid_runtime_document;
use self::fields::{invalid_runtime_response, valid_fingerprint};
use super::{
    AdminApiFailure, invalid_request_error, invalid_response_error, response_failure, send_get,
    send_request,
};

/// Runtime 配置请求失败时保留 HTTP 状态，页面可区分 fingerprint 冲突和普通错误。
pub(crate) type AdminRuntimeConfigApiError = AdminApiFailure;

/// 读取、校验与保存都显式请求的扩展投影：装备分类词典与告警区块的长期未领取阈值。缺省响应
/// 不含该阈值，只供升级前的页面；本页按含该键的闭集严格校验。
const RUNTIME_CONFIG_QUERY: &str =
    "include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";

#[cfg(test)]
#[derive(Clone)]
struct RuntimeConfigStub {
    read: Rc<dyn Fn() -> Result<AdminRuntimeConfig, AdminApiFailure>>,
    save: Rc<dyn Fn(AdminRuntimeConfigUpdateRequest) -> AdminRuntimeConfig>,
}

/// 替换宿主回归的读写传输，保留请求 DTO 和真实投影校验。
#[cfg(test)]
pub(crate) fn stub_runtime_config(
    read: impl Fn() -> Result<AdminRuntimeConfig, AdminApiFailure> + 'static,
    save: impl Fn(AdminRuntimeConfigUpdateRequest) -> AdminRuntimeConfig + 'static,
) {
    dioxus::prelude::provide_context(RuntimeConfigStub {
        read: Rc::new(read),
        save: Rc::new(save),
    });
}

pub(crate) async fn fetch_admin_runtime_config()
-> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<RuntimeConfigStub>() {
        return decode_runtime_config((stub.read)()?, false).map_err(invalid_runtime_response);
    }
    let payload = fetch_runtime_config_response().await?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

async fn fetch_runtime_config_response()
-> Result<ApiResponse<AdminRuntimeConfig>, AdminRuntimeConfigApiError> {
    // 表单回归以完整响应替代 Fetch，后续仍走正常读取的解码路径。
    #[cfg(test)]
    if let Some(payload) = dioxus::prelude::try_consume_context::<ApiResponse<AdminRuntimeConfig>>()
    {
        return Ok(payload);
    }

    let response = send_get(&format!("/api/v1/admin/config?{RUNTIME_CONFIG_QUERY}"))
        .await
        .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    response.json().await.map_err(|error| AdminApiFailure::client(invalid_response_error(error)))
}

pub(crate) async fn validate_admin_runtime_config(
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let command = AdminRuntimeConfigValidateRequest { document };
    let response = send_request(|abort_signal| {
        Request::post(&format!("/api/v1/admin/config:validate?{RUNTIME_CONFIG_QUERY}"))
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

pub(crate) async fn update_admin_runtime_config(
    command_id: uuid::Uuid,
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let command = AdminRuntimeConfigUpdateRequest { command_id, expected_fingerprint, document };
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<RuntimeConfigStub>() {
        return decode_runtime_config((stub.save)(command), true).map_err(invalid_runtime_response);
    }
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let response = send_request(|abort_signal| {
        Request::put(&format!("/api/v1/admin/config?{RUNTIME_CONFIG_QUERY}"))
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, true).map_err(invalid_runtime_response)
}

fn decode_runtime_config(
    value: AdminRuntimeConfig,
    expected_restart_required: bool,
) -> Result<AdminRuntimeConfig, &'static str> {
    if value.restart_required != expected_restart_required
        || !valid_fingerprint(&value.fingerprint)
        || !valid_runtime_document(&value.document)
        || !value.equipment_taxonomy.as_ref().is_some_and(taxonomy::valid_equipment_taxonomy)
    {
        return Err("runtime_config");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use auction_contracts::ApiResponse;
    use serde_json::json;

    use super::{AdminRuntimeConfig, RUNTIME_CONFIG_QUERY, decode_runtime_config};

    // 夹具来自带 RUNTIME_CONFIG_QUERY 的 Admin GET 响应，由 HTTP 测试
    // runtime_config_get_matches_browser_fixtures 用相同输入逐字段核对。
    // 改动运行配置投影时，三份夹具和 HTTP、浏览器两边的测试都要一起更新。
    const DEFAULT_PROJECTION: &str = include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
    );
    const LONG_VALUES_PROJECTION: &str = include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/runtime-config-long-values.json"
    );
    const DISABLED_ORIGINS_PROJECTION: &str = include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/runtime-config-disabled-origins.json"
    );

    fn response(body: &str) -> AdminRuntimeConfig {
        serde_json::from_str::<ApiResponse<AdminRuntimeConfig>>(body).unwrap().data
    }

    pub(super) fn default_response() -> AdminRuntimeConfig {
        response(DEFAULT_PROJECTION)
    }

    /// 读取、校验与保存都带两个扩展参数：装备分类词典与长期未领取阈值。
    #[test]
    fn runtime_requests_include_both_projections() {
        assert_eq!(
            RUNTIME_CONFIG_QUERY,
            "include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds"
        );
    }

    #[test]
    fn decodes_default_backend_projection() {
        let response = decode_runtime_config(default_response(), false).unwrap();
        assert!(response.equipment_taxonomy.is_some());
        assert_eq!(
            response.document["operations"]["alerts"]["fulfillment_collection_overdue_age_seconds"],
            259_200
        );
    }

    #[test]
    fn decodes_backend_projection_with_long_path_and_tracing_endpoint() {
        let response = decode_runtime_config(response(LONG_VALUES_PROJECTION), false).unwrap();
        let prefix =
            response.document["equipment_source"]["allowed_path_prefixes"][0].as_str().unwrap();
        let endpoint = response.document["tracing"]["endpoint"].as_str().unwrap();
        assert_eq!(prefix.len(), 602);
        assert_eq!(endpoint.len(), 2134);
    }

    #[test]
    fn decodes_backend_projection_with_disabled_origin_entries_verbatim() {
        let response = decode_runtime_config(response(DISABLED_ORIGINS_PROJECTION), false).unwrap();
        assert_eq!(response.document["security"]["allowed_origins_enabled"], false);
        assert_eq!(
            response.document["security"]["allowed_origins"],
            json!(["", " https://example.invalid ", ""])
        );
    }

    #[test]
    fn value_format_errors_do_not_prevent_reading_runtime_config() {
        let mut response = default_response();
        response.document["security"]["trusted_proxy_cidrs"] = json!(["bad-cidr"]);
        response.document["security"]["allowed_origins"] =
            json!(["https://example.invalid", "https://example.invalid/"]);
        response.document["equipment_source"]["allowed_path_prefixes"] =
            json!(["/equip/../", "/equip%2F/"]);
        response.document["tracing"]["enabled"] = json!(true);
        response.document["tracing"]["endpoint"] = json!("https://example.invalid/?query");
        assert!(decode_runtime_config(response, false).is_ok());
    }

    #[test]
    fn simultaneous_old_and_new_color_selections_are_retained() {
        let mut response = default_response();
        response.document["equipment_colors"]["rules"] = json!([{
            "category": "Cloth", "categories": ["Light"],
            "equipment_type": "Phase", "equipment_types": ["Cotton"],
            "quality": "Legendary", "qualities": ["Peerless"],
            "level": {"min": 0, "max": 0}, "prefixes": ["Radiant"], "suffixes": [],
            "prefix_color": "#ff0000", "suffix_color": "#00aa00", "combined_color": "#aa00aa",
        }]);
        let decoded = decode_runtime_config(response.clone(), false).unwrap();
        assert_eq!(decoded.document, response.document);
        response.document["equipment_colors"]["rules"][0]["qualities"] = json!([false]);
        assert!(decode_runtime_config(response, false).is_err());
    }

    #[test]
    fn extra_dashboard_mailboxes_are_retained() {
        let mut response = default_response();
        response.document["dashboard_mailboxes"] = json!([
            {"world_id": "main", "mailbox_key": "mail-1"},
            {"world_id": "isekai", "mailbox_key": "mail-2"},
            {"world_id": "main", "mailbox_key": "mail-3"},
        ]);
        let decoded = decode_runtime_config(response.clone(), false).unwrap();
        assert_eq!(decoded.document, response.document);
        response.document["dashboard_mailboxes"][0]["mailbox_key"] = json!([]);
        assert!(decode_runtime_config(response, false).is_err());
    }

    #[test]
    fn decodes_negative_safe_values_for_signed_document_fields() {
        let signed_fields = [
            "/operations/economics/start_price",
            "/operations/economics/bid_increment_ppm",
            "/operations/economics/soft_close_window_seconds",
            "/operations/economics/soft_close_extension_seconds",
            "/operations/economics/cancel_cutoff_seconds",
            "/operations/economics/proxy_slow_interval_seconds",
            "/operations/economics/final_phase_lead_seconds",
            "/operations/alerts/delivery_pending_count_threshold",
            "/operations/alerts/delivery_manual_reconcile_count_threshold",
            "/operations/alerts/fulfillment_backlog_count_threshold",
            "/operations/alerts/recovery_overdue_count_threshold",
            "/operations/worker/recovery/low_balance_threshold",
            "/equipment_filter/level/min",
            "/equipment_filter/level/max",
        ];
        for path in signed_fields {
            let mut response = default_response();
            *response.document.pointer_mut(path).unwrap() = json!(-1);
            let decoded = decode_runtime_config(response.clone(), false).expect(path);
            assert_eq!(decoded.document.pointer(path), Some(&json!(-1)));
            *response.document.pointer_mut(path).unwrap() = json!(-9_007_199_254_740_992_i64);
            assert!(decode_runtime_config(response, false).is_err(), "{path}");
        }
        for path in [
            "/operations/economics/proxy_max_outbid_uses",
            "/operations/alerts/delivery_pending_age_seconds",
            "/operations/worker/recovery/batch_limit",
        ] {
            let mut response = default_response();
            *response.document.pointer_mut(path).unwrap() = json!(-1);
            assert!(decode_runtime_config(response, false).is_err(), "{path}");
        }

        let mut response = default_response();
        response.document["equipment_colors"]["rules"] = json!([{
            "category": "", "equipment_type": "", "quality": "",
            "level": {"min": -1, "max": -1}, "prefixes": [], "suffixes": [],
            "prefix_color": "#ff0000", "suffix_color": "#00aa00", "combined_color": "#aa00aa",
        }]);
        let decoded = decode_runtime_config(response, false).unwrap();
        assert_eq!(
            decoded.document["equipment_colors"]["rules"][0]["level"],
            json!({"min": -1, "max": -1})
        );
    }

    #[test]
    fn decode_rejects_wrong_shapes_and_unsafe_integers() {
        for (path, invalid) in [
            ("/security/trusted_proxy_cidrs", json!([{}])),
            ("/equipment_source/allowed_path_prefixes", json!([null])),
            ("/tracing/endpoint", json!({"url": "https://example.invalid/"})),
            (
                "/operations/alerts/fulfillment_collection_overdue_age_seconds",
                json!(9_007_199_254_740_992_u64),
            ),
            ("/listener/mode", json!("unknown")),
        ] {
            let mut response = default_response();
            *response.document.pointer_mut(path).unwrap() = invalid;
            assert!(decode_runtime_config(response, false).is_err(), "{path}");
        }
    }

    #[test]
    fn runtime_config_read_projections_require_both_opt_ins_in_the_editor() {
        for (body, taxonomy, threshold) in [
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/admin/runtime-config-unextended.json"
                ),
                false,
                false,
            ),
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/admin/runtime-config-taxonomy.json"
                ),
                true,
                false,
            ),
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/admin/runtime-config-collection-threshold.json"
                ),
                false,
                true,
            ),
        ] {
            let data = response(body);
            assert_eq!(data.equipment_taxonomy.is_some(), taxonomy);
            assert_eq!(
                data.document["operations"]["alerts"]
                    .get("fulfillment_collection_overdue_age_seconds")
                    .is_some(),
                threshold,
            );
            assert!(decode_runtime_config(data, false).is_err());
        }
    }
}
