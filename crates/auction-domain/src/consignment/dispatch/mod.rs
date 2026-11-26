//! 寄售派发轮次与尝试的领域规则：轮 ID、尝试身份、冻结身份与证据类别。
//!
//! 纯规则，不读时钟、不访问存储；唯一键、锁与事务由存储车道承担。
//! 轮 ID 与尝试 ID 的 v5 字节一经冻结不得改动，向量固定在
//! `crates/auction-domain/tests/consignment/dispatch_vectors.rs`。

mod attempt;
mod errors;
mod evidence;
mod round;
mod start;

pub use attempt::{AttemptKey, FrozenDispatchIdentity, collection_attempt_id, dispatch_attempt_id};
pub use errors::DispatchRuleError;
pub use evidence::EvidenceCategory;
pub use round::{
    DispatchRoundKey, RoundOrigin, dispatch_ref, dispatch_round_id, next_round_number,
};
pub use start::{
    DispatchClaimFacts, WriteStartDecision, WriteStartFacts, WriteStartRejection,
    decide_write_start,
};
