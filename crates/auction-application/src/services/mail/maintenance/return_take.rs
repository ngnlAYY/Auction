//! 退件接管 Take 工作的持久领取端口：只领退件接管（`return_intake`）建立的唯一 Take 工作。
//!
//! 期 4 的处置领取（[`DispositionWorkStore`](super::DispositionWorkStore)）排除这些工作；两条领取路径
//! 互不重叠，Take 的 POST 许可仍只由退件起写（`ReturnService::start_take`）首次提交时铸造。
//!
//! 保护不变量 4、5：领取只推进持久 fence 与租约并登记 attempt，不起写、不外写；已有成功起写或已消费的
//! 接管不再被领取（之后只由恢复路径按原 attempt 只读核对），内存状态不是工作来源。

use auction_time::Timestamp;
use uuid::Uuid;

use super::frozen_manifest::DispositionWorkClaim;
use crate::ports::consignment::{ConsignmentFuture, DispositionAction};

/// 一次退件 Take 领取：接管 ID 与沿用期 4 形状的工作领取（冻结清单即 RTS 观察清单）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnTakeClaim {
    /// 退件接管 ID。
    pub intake_id: Uuid,
    /// 该接管唯一 Take 工作的当前领取。
    pub work: DispositionWorkClaim,
}

impl ReturnTakeClaim {
    /// 校验存储适配器返回的形状：接管非空、动作只能是 Take，工作领取本身合法。
    pub fn validate(&self) -> bool {
        !self.intake_id.is_nil()
            && self.work.action == DispositionAction::Take
            && self.work.validate()
    }
}

/// 重读一次领取所需的身份；存储据此核对它仍是当前领取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnTakeLease {
    /// 退件接管 ID。
    pub intake_id: Uuid,
    /// 本次 attempt。
    pub attempt_id: Uuid,
    /// 领取时的 fence。
    pub fence: u64,
    /// 领取时的 claim token。
    pub claim_token: Uuid,
}

/// 退件 Take 工作的存储端口；实现方只委托持久事实，不在内存保存领取状态。
pub trait ReturnTakeWorkStore: Send + Sync {
    /// 领取一条到期、尚未起写且接管未消费的退件 Take 工作；没有时返回 `None`。
    ///
    /// 与期 4 领取同一写法：推进 fence 与租约、绑定当前世界就绪会话登记 attempt；世界未就绪时
    /// 失败关闭（`MailUnavailable`），不推进任何持久状态。
    fn claim_due_return_take(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<ReturnTakeClaim>>;

    /// 只读重读当前领取（起写前准备用）：租约仍有效且 attempt、fence、token 都是当前值时返回，
    /// 否则返回 `None`，调用方本轮不起写。
    fn load_current_return_take(
        &self,
        lease: ReturnTakeLease,
    ) -> ConsignmentFuture<'_, Option<ReturnTakeClaim>>;
}
