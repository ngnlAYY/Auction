//! 可替换的 UTC 时钟端口。

use auction_time::Timestamp;

/// 为领域用例提供可替换的 UTC 时钟，避免业务层直接读取系统时间。
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}
