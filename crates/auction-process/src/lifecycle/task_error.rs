//! Tokio join 及顶层错误链的只读分类；不格式化 panic payload，不转换原错误。

use std::error::Error;

use tokio::task::JoinError;

/// 只检查 join 原因；日志不能使用 JoinError 的 Display 或 Debug。
pub fn join_error_kind(error: &JoinError) -> &'static str {
    if error.is_panic() {
        "panic"
    } else if error.is_cancelled() {
        "cancelled"
    } else {
        "other"
    }
}

/// 在格式化顶层错误前检查整条 source 链；非 join 错误保留原诊断。
pub fn task_error_kind(error: &(dyn Error + 'static)) -> Option<&'static str> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<JoinError>() {
            return Some(join_error_kind(error));
        }
        current = error.source();
    }
    None
}
