//! 读取路径共用的错误映射；只投影闭集枚举，不携带远端文本或凭据。

use super::super::*;

pub(super) fn read_journal_unavailable(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::PersistenceUnavailable,
        true,
    )
}

pub(super) fn fetch_budget_exhausted(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
        true,
    )
}
