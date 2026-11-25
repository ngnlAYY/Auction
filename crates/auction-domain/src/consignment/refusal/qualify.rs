//! 退回事实的资格裁决：计入、不计入或待判（决定 18、26）。

use super::fact::{Deliverability, RefusalKind, ReturnFactInput};

/// 资格裁决结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualification {
    /// 合格，可参加阈值判定。
    Qualified,
    /// 确定不计。
    NotCounted(NotCounted),
    /// 证据不足，保留事实待判，证清前不处罚。
    Pending(PendingReason),
}

/// 确定不计入的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotCounted {
    /// 退回发生在有效交割关闭界点或其后：只核销本金，不入窗。
    AfterDeliveryClosed,
    /// 已付款或存在相反的实物交付证据。
    PaidOrOpposite,
    /// 退回物与原轮资产不符。
    WrongAsset,
    /// 同一 K、同一原轮重复退件。
    Duplicate,
}

/// 待判原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingReason {
    /// 远端分钟区间横跨关闭界点。
    BoundaryMinuteAmbiguous,
    /// 可交付性有争议、与界点矛盾或缺界点。
    DeliverabilityDisputed,
    /// 买家归属未证明。
    OwnershipUnproven,
}

/// 按事实本身裁决资格；先排除确定不计的情形，再处理待判，最后按季界判定。
///
/// 保护的不变量：不得用 `proven_at`、Take 时间或关季任务的执行时刻改写先后（决定 26），
/// 因此本函数只读 `returned_at` 与关闭界点；矛盾输入一律失败关闭为待判（不变量 6）。
pub fn qualify_return(fact: &ReturnFactInput) -> Qualification {
    if fact.duplicate {
        return Qualification::NotCounted(NotCounted::Duplicate);
    }
    if !fact.asset_matches {
        return Qualification::NotCounted(NotCounted::WrongAsset);
    }
    if fact.paid_or_opposite {
        return Qualification::NotCounted(NotCounted::PaidOrOpposite);
    }
    if !fact.ownership_proven {
        return Qualification::Pending(PendingReason::OwnershipUnproven);
    }
    match fact.kind {
        // main 没有换季交割界点，资格只取决于上面的证据结论。
        RefusalKind::MainCodReturn => Qualification::Qualified,
        RefusalKind::IsekaiUnpaidCarrierReturn => qualify_isekai(fact),
    }
}

fn qualify_isekai(fact: &ReturnFactInput) -> Qualification {
    let disputed = Qualification::Pending(PendingReason::DeliverabilityDisputed);
    let Some(close) = fact.delivery_close_at else {
        return disputed;
    };
    match fact.deliverability {
        Deliverability::Disputed => disputed,
        Deliverability::Closed => Qualification::NotCounted(NotCounted::AfterDeliveryClosed),
        Deliverability::Deliverable => {
            if fact.returned_at >= close {
                // 上游称可交付，但时刻已在关闭等号起：结论互相矛盾，不处罚也不丢事实。
                disputed
            } else if fact.returned_minute_end > close {
                Qualification::Pending(PendingReason::BoundaryMinuteAmbiguous)
            } else {
                Qualification::Qualified
            }
        }
    }
}
