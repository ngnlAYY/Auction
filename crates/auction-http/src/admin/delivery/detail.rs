//! Admin 投递详情读取：聚合尝试、Sent 证据、领取、处置与来源义务的安全投影。
//!
//! 处置种类是浏览器按闭集解码的值。部署窗口内升级前打开的页面仍会读取新服务端，缺省响应多出一个
//! 它不认识的种类，整页详情就解码失败，直到刷新。因此缺省响应只输出已发布的种类闭集，之后新增的
//! 种类只在新页面以 `include=` 显式请求时输出（见 `crates/auction-contracts/README.md` 的变更规则）。

use std::net::SocketAddr;

use auction_contracts::{AdminDeliveryDispositionResponse, ApiResponse};
use auction_domain::DeliveryDispositionKind;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{feature_unavailable, map_delivery_error, map_detail};
use crate::errors::{api_error, invalid_query};
use crate::query::parse_query;
use crate::session::{admin_session_guard, authenticated_admin_session, no_store_headers};
use crate::state::AdminApiState;

/// 新页面显式请求装备收回处置（迁移 `000113` 新增的种类）时的 `include` 取值。
const INCLUDE_RECOVERY_EQUIPMENT_RECOVERED: &str = "recovery_equipment_recovered";

/// 缺省响应可以出现的处置种类：Admin 命令账本的三种动作、人工 Sent 证明命令与统一处置表在
/// `000113` 之前的八种，即已发布页面解码时认可的闭集。
const PUBLISHED_DISPOSITION_KINDS: [&str; 12] = [
    "release",
    "retry_failed",
    "reconcile",
    "sent_operator_confirmed",
    "operator_acknowledged",
    "auto_sent_proof",
    "operator_sent_confirmation",
    "admin_failed",
    "equipment_loss",
    "cod_returned",
    "legacy_self",
    "preflight_retired",
];

/// 读取单条 delivery 的完整证据详情；不返回 payload、正文或凭据秘密。
pub(crate) async fn detail(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let include_recovered = match detail_projection(&uri) {
        Ok(include_recovered) => include_recovered,
        Err(response) => return *response,
    };
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let detail = match port.detail(delivery_id).await {
        Ok(detail) => detail,
        Err(error) => return map_delivery_error(error),
    };
    let mut response = map_detail(detail);
    project_dispositions(&mut response.dispositions, include_recovered);
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("交付详情已加载", response)))
        .into_response()
}

/// 详情的响应投影：缺省是已发布的处置闭集；只接受可选的 `include=recovery_equipment_recovered`
/// 追加装备收回处置。未登记的键或重复出现为“投递详情查询参数无效”，`include` 的其他取值（含空值、
/// 大小写不同与逗号列表）为“附加字段无效”，都不静默退回缺省投影。
fn detail_projection(uri: &Uri) -> Result<bool, Box<Response>> {
    let mut query = parse_query(uri, &["include"])
        .map_err(|()| Box::new(invalid_query("投递详情查询参数无效")))?;
    match query.remove("include").as_deref() {
        None => Ok(false),
        Some(INCLUDE_RECOVERY_EQUIPMENT_RECOVERED) => Ok(true),
        Some(_) => Err(Box::new(invalid_query("附加字段无效"))),
    }
}

/// 只保留投影允许的处置种类；被省略的事实仍在库中，新页面显式请求即可看到。
fn project_dispositions(
    dispositions: &mut Vec<AdminDeliveryDispositionResponse>,
    include_recovered: bool,
) {
    dispositions.retain(|item| {
        PUBLISHED_DISPOSITION_KINDS.contains(&item.kind.as_str())
            || (include_recovered
                && item.kind == DeliveryDispositionKind::RecoveryEquipmentRecovered.as_str())
    });
}

#[cfg(test)]
mod tests {
    use auction_contracts::AdminDeliveryDispositionResponse;
    use auction_domain::DeliveryDispositionKind;
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::{detail_projection, project_dispositions};

    /// 升级前已发布的 Admin 页面解码投递详情时认可的处置种类闭集（`valid_disposition`）；这里
    /// 独立抄录，不引用实现常量，代表仍在浏览器里运行的旧 decoder。
    const RELEASED_DECODER_KINDS: [&str; 12] = [
        "release",
        "retry_failed",
        "reconcile",
        "sent_operator_confirmed",
        "operator_acknowledged",
        "auto_sent_proof",
        "operator_sent_confirmation",
        "admin_failed",
        "equipment_loss",
        "cod_returned",
        "legacy_self",
        "preflight_retired",
    ];

    fn disposition(kind: &str) -> AdminDeliveryDispositionResponse {
        AdminDeliveryDispositionResponse {
            command_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            kind: kind.to_owned(),
            occurred_at: "2026-11-16T00:00:00Z".to_owned(),
            status: None,
            fencing_version: Some(2),
            failure_code: None,
            external_reference: None,
            actor: Some("admin".to_owned()),
            reason: None,
        }
    }

    /// 统一处置表的全部种类加上命令账本与人工 Sent 证明的种类。
    fn every_kind() -> Vec<AdminDeliveryDispositionResponse> {
        DeliveryDispositionKind::ALL
            .into_iter()
            .map(DeliveryDispositionKind::as_str)
            .chain(["release", "retry_failed", "reconcile", "sent_operator_confirmed"])
            .map(disposition)
            .collect()
    }

    /// 缺省响应里的每个处置种类都能被升级前的旧 decoder 解码；装备收回处置被省略。显式
    /// `include=recovery_equipment_recovered` 时追加它，其余种类不变。
    #[test]
    fn default_detail_stays_decodable_by_the_released_page() {
        let mut default = every_kind();
        project_dispositions(&mut default, false);
        assert!(
            default.iter().all(|item| RELEASED_DECODER_KINDS.contains(&item.kind.as_str())),
            "{default:?}"
        );
        assert_eq!(default.len(), RELEASED_DECODER_KINDS.len());

        let mut extended = every_kind();
        project_dispositions(&mut extended, true);
        assert_eq!(extended.len(), RELEASED_DECODER_KINDS.len() + 1);
        assert!(extended.iter().any(|item| item.kind == "recovery_equipment_recovered"));
    }

    async fn parse(query: &str) -> Result<bool, (StatusCode, Value)> {
        let uri: Uri = format!("/api/v1/admin/deliveries/x{query}").parse().expect("测试 URI");
        match detail_projection(&uri) {
            Ok(include) => Ok(include),
            Err(response) => {
                let status = response.status();
                let bytes =
                    to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
                Err((status, serde_json::from_slice(&bytes).expect("错误响应是 JSON")))
            }
        }
    }

    /// 缺省投影不带参数；只有精确的 `include=recovery_equipment_recovered` 追加装备收回处置，
    /// 其他取值与未登记的键都以 400 `invalid_query` 拒绝，不静默退回缺省投影。
    #[tokio::test]
    async fn include_is_opt_in_and_fails_closed() {
        assert_eq!(parse("").await, Ok(false));
        assert_eq!(parse("?").await, Ok(false));
        assert_eq!(parse("?include=recovery_equipment_recovered").await, Ok(true));
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=RECOVERY_EQUIPMENT_RECOVERED", "附加字段无效"),
            ("?include=recovery_equipment_recovered,cod_returned", "附加字段无效"),
            (
                "?include=recovery_equipment_recovered&include=recovery_equipment_recovered",
                "投递详情查询参数无效",
            ),
            ("?fields=all", "投递详情查询参数无效"),
        ] {
            let (status, body) = parse(query).await.expect_err(query);
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }
}
