//! 寄售处罚账号工作的执行端口：列出待执行工作并按原处罚业务键逐项执行。
//!
//! 第 6 期在获证事务里固定处罚、构成项与唯一账号工作；本端口只执行其账号效果，入参不含窗口、阈值与计数
//! （D9 §5.2，决定 18、23）。执行必须幂等：重放、重启与并发领取对同一处罚至多产生一次账号效果（不变量 1、5）。

use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::scan::{ScanCursor, ScanPage};

/// 一项待处理的账号工作；只含固定业务键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SanctionWorkRef {
    /// 待执行账号工作 ID。
    pub work_id: Uuid,
    /// 所属处罚。
    pub sanction_id: Uuid,
    /// 被处罚买家；执行时与工作行核对，不符失败关闭。
    pub buyer_user_id: Uuid,
}

/// 一项账号工作的执行结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanctionWorkOutcome {
    /// 本次落下账号效果；`account_command_id` 是拉黑命令账本里的命令。
    Applied {
        /// Users owner 的拉黑命令 ID。
        account_command_id: Uuid,
    },
    /// 账号因别的途径已被禁：只记录“已禁”，不引用任何拉黑命令、不追加黑名单历史。
    AlreadyBlacklisted,
    /// 周期已解禁：旧工作收口为 `cycle_closed`，没有禁人。
    CycleClosed,
    /// 效果早已落库：只返回原结论，没有任何新写入。
    Replayed,
    /// 持久事实自相矛盾（如已完成却无效果回执）：失败关闭，转人工对账，不写、不补回执。
    Inconsistent,
}

/// 处罚账号工作的持久仓库。
pub trait SanctionWorkStore: Send + Sync {
    /// 游标之后尚未收口的账号工作（待执行、执行中，或已取消但没有结果行），按 `(创建时间, 工作 ID)` 升序，
    /// 至多 `limit` 项；读满一页才给出下一页游标。
    fn open_sanction_work(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<SanctionWorkRef>>;

    /// 在买家互斥下执行一项工作；账号命令外部不可见地先于效果提交，重复调用复用原命令。
    fn execute_sanction_work(
        &self,
        work: SanctionWorkRef,
    ) -> ConsignmentFuture<'_, SanctionWorkOutcome>;
}
