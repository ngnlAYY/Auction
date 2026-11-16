//! 结算退件路由共用的错误映射与路径解析。
//!
//! 业务拒绝一律映射为类型化的 400/404/409，不经资金或存储错误退化为泛化 503：资格、版本与
//! 来源问题需要操作者重读或改选动作，依赖暂不可用才是 503。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::settlement::SettlementReturnError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error};
use crate::query::parse_canonical_uuid;

/// 端口未装配（未配置数据库）时的稳定 501。
pub(super) fn feature_unavailable() -> Response {
    crate::errors::feature_unavailable(DATABASE_NOT_CONFIGURED)
}

/// 路径中的案件 ID 只接受规范小写、非零 UUID，避免把自由文本扩展成全表扫描。
pub(super) fn parse_return_id(value: &str) -> Result<Uuid, Box<Response>> {
    parse_canonical_uuid(value).ok_or_else(|| {
        Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_settlement_return_id", "结算退件 ID 无效")
                .into_response(),
        )
    })
}

/// 命令字段（`command_id`、案件版本、动作）不合法时的 400。
pub(super) fn invalid_command() -> Response {
    business_error(SettlementReturnError::Invalid)
}

/// 应用错误到稳定错误外壳；`operation` 只进入诊断，不进入响应。
pub(super) fn map_settlement_return_error(
    operation: &'static str,
    error: ApplicationError,
) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let response = match error {
        ApplicationError::CapabilityUnavailable(_) => {
            crate::errors::feature_unavailable(CAPABILITY_NOT_PROVIDED)
        }
        ApplicationError::SettlementReturn(error) => business_error(error),
        _ => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
                .into_response()
        }
    };
    diagnostics::mapped_response("admin_settlement_returns", operation, diagnostic_kind, response)
}

/// 结算退件业务错误闭集的 HTTP 映射；每个 409 有独立错误码，浏览器据此给出可操作提示。
fn business_error(error: SettlementReturnError) -> Response {
    let (status, code, message) = match error {
        SettlementReturnError::Invalid => {
            (StatusCode::BAD_REQUEST, "invalid_settlement_return_command", "结算退件命令参数无效")
        }
        SettlementReturnError::NotFound => {
            (StatusCode::NOT_FOUND, "settlement_return_not_found", "结算退件案件不存在")
        }
        SettlementReturnError::Stale => (
            StatusCode::CONFLICT,
            "stale_settlement_return_state",
            "结算退件案件版本已变化，请刷新后再操作",
        ),
        SettlementReturnError::CommandConflict => {
            (StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
        }
        SettlementReturnError::Untrusted => {
            (StatusCode::CONFLICT, "settlement_return_untrusted", "退件来源未证明，只能标记已核对")
        }
        SettlementReturnError::Unbound => (
            StatusCode::CONFLICT,
            "settlement_return_unbound",
            "退件未绑定原派发，只能标记已核对或重新核验",
        ),
        SettlementReturnError::AssetsMismatch => (
            StatusCode::CONFLICT,
            "settlement_return_assets_mismatch",
            "退回资产与原派发冻结资产不符",
        ),
        SettlementReturnError::ActionConflict => {
            (StatusCode::CONFLICT, "settlement_return_action_conflict", "案件当前状态不允许该动作")
        }
        SettlementReturnError::ProofRequired => {
            (StatusCode::CONFLICT, "settlement_return_proof_required", "缺少已持久化的领回证明")
        }
        SettlementReturnError::ResendLimit => (
            StatusCode::CONFLICT,
            "settlement_return_resend_limit",
            "补寄次数已达案件冻结上限，只能代管",
        ),
        SettlementReturnError::ObligationIneligible => (
            StatusCode::CONFLICT,
            "settlement_return_obligation_ineligible",
            "原结算义务已不可补寄",
        ),
        SettlementReturnError::StoredResultCorrupt | SettlementReturnError::Unavailable => {
            (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        }
    };
    api_error(status, code, message).into_response()
}

#[cfg(test)]
mod tests {
    use auction_application::core::{ApplicationError, RepositoryError};
    use auction_application::settlement::SettlementReturnError;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    use super::{map_settlement_return_error, parse_return_id};

    async fn mapped(error: ApplicationError) -> (StatusCode, serde_json::Value) {
        let response = map_settlement_return_error("test", error);
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
        (status, serde_json::from_slice(&body).expect("错误响应是 JSON"))
    }

    /// 业务资格、版本与来源拒绝都是类型化的 400/404/409，各有独立错误码；只有依赖暂不可用与
    /// 持久化结果损坏才是 503，资格错误不会退化为泛化不可用。
    #[tokio::test]
    async fn business_errors_map_to_typed_status_codes() {
        for (error, status, code) in [
            (
                SettlementReturnError::Invalid,
                StatusCode::BAD_REQUEST,
                "invalid_settlement_return_command",
            ),
            (SettlementReturnError::NotFound, StatusCode::NOT_FOUND, "settlement_return_not_found"),
            (SettlementReturnError::Stale, StatusCode::CONFLICT, "stale_settlement_return_state"),
            (SettlementReturnError::CommandConflict, StatusCode::CONFLICT, "command_conflict"),
            (SettlementReturnError::Untrusted, StatusCode::CONFLICT, "settlement_return_untrusted"),
            (SettlementReturnError::Unbound, StatusCode::CONFLICT, "settlement_return_unbound"),
            (
                SettlementReturnError::AssetsMismatch,
                StatusCode::CONFLICT,
                "settlement_return_assets_mismatch",
            ),
            (
                SettlementReturnError::ActionConflict,
                StatusCode::CONFLICT,
                "settlement_return_action_conflict",
            ),
            (
                SettlementReturnError::ProofRequired,
                StatusCode::CONFLICT,
                "settlement_return_proof_required",
            ),
            (
                SettlementReturnError::ResendLimit,
                StatusCode::CONFLICT,
                "settlement_return_resend_limit",
            ),
            (
                SettlementReturnError::ObligationIneligible,
                StatusCode::CONFLICT,
                "settlement_return_obligation_ineligible",
            ),
            (
                SettlementReturnError::StoredResultCorrupt,
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
            ),
            (
                SettlementReturnError::Unavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
            ),
        ] {
            let (actual, body) = mapped(ApplicationError::SettlementReturn(error)).await;
            assert_eq!((actual, body["code"].as_str()), (status, Some(code)), "{error:?}");
        }
    }

    /// 端口未提供能力是 501；其他应用错误（存储失败等）是 503，不冒充业务结论。
    #[tokio::test]
    async fn non_business_errors_stay_unavailable() {
        let (status, body) = mapped(ApplicationError::CapabilityUnavailable("test")).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::NOT_IMPLEMENTED, Some("feature_not_supported"))
        );
        let (status, body) =
            mapped(ApplicationError::Repository(RepositoryError::Unavailable)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("temporarily_unavailable"))
        );
    }

    /// 路径 ID 只接受规范小写、非零 UUID。
    #[test]
    fn return_id_requires_canonical_uuid() {
        let canonical = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert!(parse_return_id(canonical).is_ok());
        for invalid in [
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "0f8fad5bd9cb469fa16570867728950e",
            "00000000-0000-0000-0000-000000000000",
            "not-a-uuid",
            "",
        ] {
            let response = parse_return_id(invalid).expect_err(invalid);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{invalid}");
        }
    }
}
