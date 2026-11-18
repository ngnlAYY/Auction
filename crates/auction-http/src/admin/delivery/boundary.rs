//! Admin 交付处理器共用的筛选解析与错误映射。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::delivery::DeliveryAdminError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::diagnostics;
use crate::errors::{CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error};

pub(crate) fn feature_unavailable() -> Response {
    crate::errors::feature_unavailable(DATABASE_NOT_CONFIGURED)
}

pub(crate) fn map_delivery_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => {
            crate::errors::feature_unavailable(CAPABILITY_NOT_PROVIDED)
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付管理参数无效")
                .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "交付不存在").into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale) => {
            api_error(StatusCode::CONFLICT, "stale_delivery_state", "交付状态或版本已变化")
                .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::SentSighted) => api_error(
            StatusCode::CONFLICT,
            "delivery_sent_sighted",
            "Sent 已出现该结算义务的邮件，须用人工 Sent 证明收口",
        )
        .into_response(),
        // 业务资格拒绝：成交已撤销或已不是当前派发，不能归入参数错误或依赖不可用。
        ApplicationError::DeliveryAdmin(DeliveryAdminError::SettlementIneligible) => api_error(
            StatusCode::CONFLICT,
            "settlement_obligation_ineligible",
            "结算义务已不可执行，不能重新激活原投递",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::RecipientRenamed) => api_error(
            StatusCode::CONFLICT,
            "delivery_recipient_renamed",
            "登录 key 回信的收件角色已换绑，不能重新排队",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::CommandSnapshotUnreadable) => {
            api_error(
                StatusCode::CONFLICT,
                "delivery_command_snapshot_unreadable",
                "这条投递的命令记录无法读取，不能执行该动作",
            )
            .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        _ => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
                .into_response()
        }
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_delivery",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

/// 解析可选筛选文本：缺省与空串都表示未设置；拒绝未裁剪、超长或含控制字符的值。
/// 等价于应用层的 `validate`：HTTP 层先失败关闭，存储层仍会再校验一次。
pub(crate) fn parse_optional_filter(
    query: &std::collections::BTreeMap<&str, String>,
    key: &str,
    maximum: usize,
) -> Result<Option<String>, ()> {
    let Some(value) = query.get(key) else { return Ok(None) };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > maximum || value != value.trim() || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value.clone()))
}

/// 解析可选参考编号；只接受规范 UUID，避免把自由文本当成 ID 扩展成全表扫描。
pub(crate) fn parse_optional_reference(
    query: &std::collections::BTreeMap<&str, String>,
    key: &str,
) -> Result<Option<uuid::Uuid>, ()> {
    let Some(value) = query.get(key) else { return Ok(None) };
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = uuid::Uuid::parse_str(value).map_err(drop)?;
    if parsed.is_nil() || parsed.hyphenated().to_string() != *value {
        return Err(());
    }
    Ok(Some(parsed))
}

/// 解析可选时间筛选；只接受带时区的 RFC3339 文本，不做本地时区猜测。
pub(crate) fn parse_optional_timestamp(
    query: &std::collections::BTreeMap<&str, String>,
    key: &str,
) -> Result<Option<auction_time::Timestamp>, ()> {
    let Some(value) = query.get(key) else { return Ok(None) };
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = auction_time::Timestamp::parse_from_rfc3339(value).map_err(drop)?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::to_bytes;

    use super::*;

    fn query(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs.iter().map(|(key, value)| (*key, (*value).to_owned())).collect()
    }

    async fn mapped(error: DeliveryAdminError) -> (StatusCode, serde_json::Value) {
        let response = map_delivery_error(ApplicationError::DeliveryAdmin(error));
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
        (status, serde_json::from_slice(&body).expect("错误响应是 JSON"))
    }

    /// 成交撤销后拒绝重新激活是类型化业务冲突：精确 409 与稳定错误码，不退化为 400 参数错误或
    /// 503 依赖不可用；既有映射保持不变。
    #[tokio::test]
    async fn settlement_ineligibility_maps_to_a_precise_conflict() {
        let (status, body) = mapped(DeliveryAdminError::SettlementIneligible).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "settlement_obligation_ineligible");

        for (error, status, code) in [
            (DeliveryAdminError::Invalid, StatusCode::BAD_REQUEST, "invalid_delivery_command"),
            (DeliveryAdminError::Stale, StatusCode::CONFLICT, "stale_delivery_state"),
            (DeliveryAdminError::SentSighted, StatusCode::CONFLICT, "delivery_sent_sighted"),
            (
                DeliveryAdminError::RecipientRenamed,
                StatusCode::CONFLICT,
                "delivery_recipient_renamed",
            ),
            (
                DeliveryAdminError::CommandSnapshotUnreadable,
                StatusCode::CONFLICT,
                "delivery_command_snapshot_unreadable",
            ),
            (
                DeliveryAdminError::Unavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
            ),
        ] {
            let (actual_status, body) = mapped(error).await;
            assert_eq!((actual_status, body["code"].as_str()), (status, Some(code)), "{error:?}");
        }
    }

    #[test]
    fn filter_text_is_bounded_and_trimmed() {
        assert_eq!(parse_optional_filter(&query(&[]), "recipient", 8), Ok(None));
        assert_eq!(parse_optional_filter(&query(&[("recipient", "")]), "recipient", 8), Ok(None));
        assert_eq!(
            parse_optional_filter(&query(&[("recipient", "Sample")]), "recipient", 8),
            Ok(Some("Sample".to_owned()))
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", " Sample")]), "recipient", 8).is_err()
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", "Sam\nple")]), "recipient", 8).is_err()
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", "SampleLonger")]), "recipient", 8)
                .is_err()
        );
    }

    #[test]
    fn reference_requires_canonical_non_nil_uuid() {
        let canonical = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(
            parse_optional_reference(&query(&[("reference", canonical)]), "reference"),
            Ok(Some(uuid::Uuid::parse_str(canonical).expect("uuid")))
        );
        for invalid in [
            "0f8fad5bd9cb469fa16570867728950e",
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "00000000-0000-0000-0000-000000000000",
            "not-a-uuid",
        ] {
            assert!(
                parse_optional_reference(&query(&[("reference", invalid)]), "reference").is_err(),
                "应拒绝非法参考编号: {invalid}"
            );
        }
        assert_eq!(parse_optional_reference(&query(&[]), "reference"), Ok(None));
    }

    #[test]
    fn timestamps_require_rfc3339() {
        assert_eq!(
            parse_optional_timestamp(&query(&[("from", "2026-11-02T01:14:00Z")]), "from")
                .expect("timestamp")
                .expect("value")
                .to_rfc3339(),
            "2026-11-02T01:14:00+00:00"
        );
        assert_eq!(
            parse_optional_timestamp(&query(&[("from", "2026-11-02T09:14:00+08:00")]), "from")
                .expect("timestamp")
                .expect("value")
                .to_rfc3339(),
            "2026-11-02T01:14:00+00:00"
        );
        for invalid in ["2026-11-02", "2026-11-02T01:14", "yesterday"] {
            assert!(
                parse_optional_timestamp(&query(&[("from", invalid)]), "from").is_err(),
                "应拒绝非法时间: {invalid}"
            );
        }
    }
}
