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
    AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest, AdminWorkerRestartStateDto,
    ApiResponse,
};
use gloo_net::http::{Method, Request};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use self::document::{project_update_document, valid_runtime_document};
use self::fields::{invalid_runtime_response, valid_fingerprint};
use super::AdminWriteFailure;
use super::transport::WriteRequest;
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
    read_runtime_config(fetch_runtime_config_body).await
}

async fn read_runtime_config(
    mut get: impl AsyncFnMut(&str) -> Result<String, AdminRuntimeConfigApiError>,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let base = format!("/api/v1/admin/config?{RUNTIME_CONFIG_QUERY}");
    let extended = format!("{base}&include_worker_restart_state=true");
    let (body, include_worker) = match get(&extended).await {
        // 只对明确拒绝新投影的只读请求回退一次，保留编辑器所需的既有投影。
        Err(error) if error.status == Some(400) && error.code == "invalid_query" => {
            (get(&base).await?, false)
        }
        result => (result?, true),
    };
    let value = decode_runtime_config_read_response(&body, include_worker)?;
    decode_runtime_config(value, false).map_err(invalid_runtime_response)
}

async fn fetch_runtime_config_body(url: &str) -> Result<String, AdminRuntimeConfigApiError> {
    // 表单回归以完整响应替代 Fetch，后续仍走正常读取的解码路径。
    #[cfg(test)]
    {
        use dioxus::prelude::try_consume_context;

        let payload = if let Some(stub) = try_consume_context::<RuntimeConfigStub>() {
            Some(ApiResponse::ok("runtime 配置已加载", (stub.read)()?))
        } else {
            try_consume_context::<ApiResponse<AdminRuntimeConfig>>()
        };
        if let Some(mut payload) = payload {
            payload.data.worker_restart_state.get_or_insert(AdminWorkerRestartStateDto::Unknown);
            return Ok(serde_json::to_string(&payload).expect("运行配置测试投影可序列化"));
        }
    }

    let response = send_get(url).await.map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    response.text().await.map_err(|error| AdminApiFailure::client(invalid_response_error(error)))
}

fn decode_runtime_config_read_response(
    body: &str,
    include_worker: bool,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let projection: ApiResponse<WorkerRestartProjection> = serde_json::from_str(body)
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if projection.data.worker_restart_state.is_some() != include_worker {
        return Err(invalid_runtime_response("worker_restart_state"));
    }

    let mut payload: ApiResponse<AdminRuntimeConfig> = serde_json::from_str(body)
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if !include_worker {
        payload.data.worker_restart_state = Some(AdminWorkerRestartStateDto::Unknown);
    }
    Ok(payload.data)
}

// 只检查投影键是否如约出现，不复制配置文档。共享 DTO 的 Option 与枚举还可接收 null 和带标签的对象。
#[derive(Deserialize)]
struct WorkerRestartProjection {
    #[serde(default, deserialize_with = "deserialize_worker_restart_string")]
    worker_restart_state: Option<String>,
}

fn deserialize_worker_restart_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
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

/// 提交冻结的配置候选，并核对本次保存回执的完整文档。
pub(crate) async fn update_admin_runtime_config(
    command_id: uuid::Uuid,
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, AdminWriteFailure> {
    let command = AdminRuntimeConfigUpdateRequest { command_id, expected_fingerprint, document };
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<RuntimeConfigStub>() {
        return decode_runtime_config_update((stub.save)(command.clone()), command.document);
    }
    let response =
        WriteRequest::general(&format!("/api/v1/admin/config?{RUNTIME_CONFIG_QUERY}"), Method::PUT)
            .send_json(&command)
            .await?;
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    decode_runtime_config_update(payload.data, command.document)
}

fn decode_runtime_config_update(
    value: AdminRuntimeConfig,
    mut candidate: Value,
) -> Result<AdminRuntimeConfig, AdminWriteFailure> {
    let value = decode_runtime_config(value, true)
        .map_err(|_error| AdminWriteFailure::outcome_unknown("invalid_response"))?;
    // 只有与冻结候选对应的回执才能推进编辑基线。投影只作用于比较用的候选，重试复用原请求。
    project_update_document(&mut candidate);
    if value.document != candidate {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
    }
    Ok(value)
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
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use auction_contracts::AdminWorkerRestartStateDto;
    use auction_contracts::ApiResponse;
    use serde_json::{Value, json};

    use super::{
        AdminRuntimeConfig, RUNTIME_CONFIG_QUERY, decode_runtime_config,
        decode_runtime_config_read_response, read_runtime_config,
    };
    use crate::api::AdminApiFailure;

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
    const WORKER_PROJECTION: &str = include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/runtime-config-worker-restart-state.json"
    );
    const BASE_URL: &str = "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";
    const EXTENDED_URL: &str = "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds&include_worker_restart_state=true";

    fn response(body: &str) -> AdminRuntimeConfig {
        serde_json::from_str::<ApiResponse<AdminRuntimeConfig>>(body).unwrap().data
    }

    pub(super) fn default_response() -> AdminRuntimeConfig {
        response(DEFAULT_PROJECTION)
    }

    #[test]
    fn config_update_receipt_matches_the_frozen_candidate_projection() {
        use super::decode_runtime_config_update;
        use crate::api::AdminWriteFailure;

        let mut receipt = default_response();
        receipt.restart_required = true;
        let early_bird = json!({
            "enabled": false, "timezone": "UTC", "worlds": ["isekai"], "season_starts": ["05-01", "11-01"],
            "season_start_time": "09:00", "window_days": 7, "session_days": [1, 3, 5, 7],
            "open_time": "18:00", "preview_lead_minutes": 120, "live_minutes": 1440,
            "qualities": [], "pause_intake_before_season": true
        });
        receipt.document["operations"]["schedule"]["early_bird"] = early_bird;
        let mut candidate = receipt.document.clone();
        candidate["bot_players"] = json!([]);
        candidate["equipment_filter"]["qualities"] = json!([]);
        let schedule = &mut candidate["operations"]["schedule"]["early_bird"];
        schedule["pre_season_session"] = json!(true);
        schedule["pre_season_days_before"] = json!(3);

        let accepted = decode_runtime_config_update(receipt.clone(), candidate.clone()).unwrap();
        assert_eq!(accepted.document, receipt.document);
        for (path, value) in [
            ("/metrics/enabled", json!(true)),
            ("/operations/schedule/early_bird/season_starts", json!(["11-01", "05-01"])),
        ] {
            let mut invalid = receipt.clone();
            *invalid.document.pointer_mut(path).unwrap() = value;
            assert!(
                matches!(
                    decode_runtime_config_update(invalid, candidate.clone()),
                    Err(AdminWriteFailure::OutcomeUnknown { code, response_status: None })
                        if code == "invalid_response"
                ),
                "{path}"
            );
        }
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
    fn decodes_only_known_worker_restart_states_from_the_backend_projection() {
        assert_eq!(
            decode_runtime_config_read_response(WORKER_PROJECTION, true)
                .unwrap()
                .worker_restart_state,
            Some(AdminWorkerRestartStateDto::Unknown)
        );
        let mut body: Value = serde_json::from_str(WORKER_PROJECTION).unwrap();
        for (wire, state) in [
            ("restart_required", AdminWorkerRestartStateDto::RestartRequired),
            ("consistent", AdminWorkerRestartStateDto::Consistent),
        ] {
            body["data"]["worker_restart_state"] = json!(wire);
            assert_eq!(
                decode_runtime_config_read_response(&body.to_string(), true)
                    .unwrap()
                    .worker_restart_state,
                Some(state)
            );
        }

        body["data"].as_object_mut().unwrap().remove("worker_restart_state");
        let missing = decode_runtime_config_read_response(&body.to_string(), true).unwrap_err();
        assert_eq!(missing.code, "invalid_response");
        for invalid in [json!("outdated"), json!(false), json!({"unknown": null}), json!(null)] {
            body["data"]["worker_restart_state"] = invalid;
            let failure = decode_runtime_config_read_response(&body.to_string(), true).unwrap_err();
            assert_eq!(failure.code, "invalid_response");
        }
        assert!(decode_runtime_config_read_response(&body.to_string(), false).is_err());
    }

    fn read_with_replies<'a>(
        replies: impl IntoIterator<Item = (&'a str, Result<&'a str, AdminApiFailure>)>,
    ) -> Result<AdminRuntimeConfig, AdminApiFailure> {
        let mut replies = replies.into_iter();
        let result = {
            let mut read = pin!(read_runtime_config(async |url| {
                let (expected_url, result) = replies.next().expect("收到意外的配置 GET");
                assert_eq!(url, expected_url);
                result.map(str::to_owned)
            }));
            let Poll::Ready(result) = read.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            else {
                panic!("替身 GET 应立即完成");
            };
            result
        };
        assert!(replies.next().is_none(), "配置 GET 次数不足");
        result
    }

    #[test]
    fn editor_reads_worker_state_alongside_the_existing_projections() {
        let worker: Value = serde_json::from_str(WORKER_PROJECTION).unwrap();
        let mut combined: Value = serde_json::from_str(DEFAULT_PROJECTION).unwrap();
        combined["data"]["worker_restart_state"] = worker["data"]["worker_restart_state"].clone();
        let value = read_with_replies([(EXTENDED_URL, Ok(combined.to_string().as_str()))]).unwrap();
        assert!(value.equipment_taxonomy.is_some());
        assert_eq!(value.worker_restart_state, Some(AdminWorkerRestartStateDto::Unknown));
        let missing_state = read_with_replies([(EXTENDED_URL, Ok(DEFAULT_PROJECTION))]);
        assert_eq!(missing_state.unwrap_err().code, "invalid_response");
        let missing_editor_projections = read_with_replies([(EXTENDED_URL, Ok(WORKER_PROJECTION))]);
        assert_eq!(missing_editor_projections.unwrap_err().code, "invalid_response");
    }

    #[test]
    fn editor_falls_back_once_only_on_http_400_invalid_query() {
        const INVALID_QUERY: &str =
            r#"{"code":"invalid_query","message":"不支持查询参数","data":null,"request_id":null}"#;
        let invalid_query = || AdminApiFailure::from_http_body(400, INVALID_QUERY);
        let value = read_with_replies([
            (EXTENDED_URL, Err(invalid_query())),
            (BASE_URL, Ok(DEFAULT_PROJECTION)),
        ])
        .unwrap();
        assert!(value.equipment_taxonomy.is_some());
        assert_eq!(value.worker_restart_state, Some(AdminWorkerRestartStateDto::Unknown));
        let rejected = read_with_replies([
            (EXTENDED_URL, Err(invalid_query())),
            (BASE_URL, Err(invalid_query())),
        ])
        .unwrap_err();
        assert_eq!((rejected.status, rejected.code.as_str()), (Some(400), "invalid_query"));

        for error in [
            AdminApiFailure::from_http_body(400, ""),
            AdminApiFailure::from_http_body(503, INVALID_QUERY),
            AdminApiFailure::client("invalid_query"),
        ] {
            let actual = read_with_replies([(EXTENDED_URL, Err(error.clone()))]).unwrap_err();
            assert_eq!((actual.status, actual.code), (error.status, error.code));
        }
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
    #[test]
    fn runtime_config_validate_contract_fixtures() {
        use crate::api::contract_fixtures::{admin_fixture, decode};

        for (body, taxonomy, threshold) in [
            (admin_fixture!("config-validate-default"), false, false),
            (admin_fixture!("config-validate-taxonomy"), true, false),
            (admin_fixture!("config-validate-collection-threshold"), false, true),
            (admin_fixture!("config-validate"), true, true),
        ] {
            let payload: ApiResponse<AdminRuntimeConfig> = decode(body, "/data", "fingerprint");
            assert_eq!(payload.data.equipment_taxonomy.is_some(), taxonomy);
            assert_eq!(
                payload.data.document["operations"]["alerts"]
                    .get("fulfillment_collection_overdue_age_seconds")
                    .is_some(),
                threshold
            );
            let result = decode_runtime_config(payload.data, false);
            if taxonomy && threshold {
                let config = result.expect("浏览器请求的完整预校验投影");
                assert_eq!(config.document["allow_manual_early_open"], true);
            } else {
                assert!(result.is_err(), "浏览器请求的扩展字段缺失");
            }
        }

        let body = admin_fixture!("config-validate-invalid");
        let failure = AdminApiFailure::from_http_body(400, body);
        assert_eq!(failure.code, "invalid_runtime_config");
        let mut invalid: serde_json::Value = serde_json::from_str(body).expect("拒绝回执");
        invalid.as_object_mut().unwrap().remove("code").expect("稳定错误码");
        assert_eq!(AdminApiFailure::from_http_body(400, &invalid.to_string()).code, "http_400");
    }
}
