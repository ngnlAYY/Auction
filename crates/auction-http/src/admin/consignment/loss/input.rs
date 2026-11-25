//! 损失案件路径身份解码：只接受规范小写 UUID，拒绝查询参数与百分号编码。

use auction_application::consignment::ConsignmentError;
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::http::Uri;
use uuid::Uuid;

use crate::query::parse_canonical_uuid;

/// position 路径身份；读与写都不接受查询参数，解码失败只返回固定的收件请求错误。
pub(super) fn position_target(
    uri: &Uri,
    path: Result<Path<String>, PathRejection>,
) -> Result<Uuid, ConsignmentError> {
    if uri.query().is_some_and(|query| !query.is_empty()) || uri.path().contains('%') {
        return Err(ConsignmentError::InvalidDepositRequest);
    }
    let Ok(Path(value)) = path else {
        return Err(ConsignmentError::InvalidDepositRequest);
    };
    parse_canonical_uuid(&value).ok_or(ConsignmentError::InvalidDepositRequest)
}
