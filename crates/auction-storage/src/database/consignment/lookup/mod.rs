//! 解析工作的事务内原语。调用方先锁资格与挂单，再锁许可单例，最后锁工作。
//! 领取路径在锁单例后只以快照读取挂单，不反向取得挂单写锁。

mod admission;
mod claim;
pub(crate) mod control;
mod lease;
pub(crate) mod protocol;
mod reclaim;
mod store;
mod terminal;
pub(crate) mod work;

#[cfg(feature = "test-db")]
mod test_access;
