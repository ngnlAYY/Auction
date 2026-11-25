//! 寄售派发轮次与尝试的窄端口：授权轮次、登记尝试、起写与追加证据。
//!
//! 命令只带业务键与摘要；轮 / 尝试 ID 与推进规则由 `auction_domain::consignment::dispatch` 给出，
//! 存储实现（第 6 期 6D4）在同一事务内落实，应用层不直接取锁。

mod collection;
mod commands;
mod store;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod work;

pub use collection::{
    CertifyCodFacts, CollectedEvidence, CollectionCursor, CollectionLeg, CollectionPage,
    PendingCollection,
};
pub use commands::{
    AttemptReceipt, AuthorizeDispatch, DispatchOperation, EvidenceReceipt, HandoffReceipt,
    MarkWriteStarted, ProveHandoff, RecordEvidence, RoundReceipt, StartAttempt,
};
pub use store::DispatchStore;
pub use work::{
    DispatchSendRequest, DispatchWorkClaim, DispatchWorkStore, PreparedRedispatch,
    QueuedRedispatch, RedispatchPrepared,
};
