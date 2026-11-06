//! 写入结果到“可安全重试 / 结果未知”的本地证据映射；只改写错误形状，不改变远端事实。

use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};

pub(super) fn write_unknown(
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> MoogleMailError {
    MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
}

pub(super) fn write_error(
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}

pub(super) fn staged_proof_error(
    mut error: MoogleMailError,
    operation: MoogleMailOperation,
) -> MoogleMailError {
    error.operation = operation;
    preserve_write_unknown(error, true)
}

/// 已发生外写后，读失败或后续写前失败都不能重新变成可安全重发；保留失效类别和原证据。
pub(super) fn preserve_write_unknown(
    error: MoogleMailError,
    write_started: bool,
) -> MoogleMailError {
    if !write_started {
        return error;
    }
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown = MoogleMailError::after_write(error.operation, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
