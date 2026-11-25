//! 订单证据的恢复段：Sale 封闭事实、Admin 处置记录与复核工作到 wire DTO 的映射。
//!
//! 只含 ID、动作码与时刻（不变量 7：不含装备 key、预约码、原信正文与凭据）；未知码值失败关闭，
//! 不合成默认值。

use auction_application::consignment::{ConsignmentError, OrderEvidence};
use auction_contracts::OrderEvidenceItemV2;

use super::wire_code;

/// 把恢复段的三类应用证据映射为 DTO；其他类别不属于本段，按依赖异常失败关闭。
pub(super) fn recovery_dto(item: OrderEvidence) -> Result<OrderEvidenceItemV2, ConsignmentError> {
    match item {
        OrderEvidence::Closure { closure_kind, closed_at } => {
            Ok(OrderEvidenceItemV2::Closure { closure_kind: wire_code(closure_kind)?, closed_at })
        }
        OrderEvidence::Resolution { resolution_id, action, evidence_id, resolved_at } => {
            Ok(OrderEvidenceItemV2::Resolution {
                resolution_id,
                action: wire_code(action)?,
                evidence_id,
                resolved_at,
            })
        }
        OrderEvidence::Recheck { work_id, requested_at } => {
            Ok(OrderEvidenceItemV2::Recheck { work_id, requested_at })
        }
        _ => Err(ConsignmentError::Unavailable),
    }
}
