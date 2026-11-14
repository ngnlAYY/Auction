//! Admin 交付处理器共用的筛选解析与错误映射。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use auction_application::core::ApplicationError;
use auction_application::delivery::DeliveryAdminError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(crate) fn feature_unavailable() -> Response {
    super::super::feature_unavailable("delivery Admin 仅支持 PostgreSQL 完整档")
}

pub(crate) fn map_delivery_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "交付管理参数无效",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::NotFound) => {
            super::api_error(StatusCode::NOT_FOUND, "not_found", "交付不存在").into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale) => {
            super::api_error(StatusCode::CONFLICT, "stale_delivery_state", "交付状态或版本已变化")
                .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::SentSighted) => super::api_error(
            StatusCode::CONFLICT,
            "delivery_sent_sighted",
            "Sent 已出现该结算义务的邮件，须用人工 Sent 证明收口",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "服务暂时不可用",
        )
        .into_response(),
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
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ()> {
    let Some(value) = query.get(key) else { return Ok(None) };
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(drop)?;
    Ok(Some(parsed.with_timezone(&chrono::Utc)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn query(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs.iter().map(|(key, value)| (*key, (*value).to_owned())).collect()
    }

    #[test]
    fn filter_text_is_bounded_and_trimmed() {
        assert_eq!(parse_optional_filter(&query(&[]), "recipient", 8), Ok(None));
        assert_eq!(parse_optional_filter(&query(&[("recipient", "")]), "recipient", 8), Ok(None));
        assert_eq!(
            parse_optional_filter(&query(&[("recipient", "Restia")]), "recipient", 8),
            Ok(Some("Restia".to_owned()))
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", " Restia")]), "recipient", 8).is_err()
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", "Res\ntia")]), "recipient", 8).is_err()
        );
        assert!(
            parse_optional_filter(&query(&[("recipient", "RestiaLonger")]), "recipient", 8)
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
