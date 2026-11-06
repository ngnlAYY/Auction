//! 定期维护读取：compose 草稿快照、受控 discard、Sent 摘要与实收探查。
//!
//! 普通列表和详情不持有 compose 排他会话；只有草稿读取与 discard 需要窄互斥。
//! 观测本身不提升为投递成功；discard 只丢弃调用方已精确匹配到本地已知投递的草稿。
//!
//! 草稿读取与 discard 位于 `compose`，入站资产探查位于 `inbox`，
//! Sent 摘要扫描与详情位于 `sent`，Read Mail 观察计数位于 `read`。

mod compose;
mod inbox;
mod read;
mod sent;

pub use compose::{MoogleMailComposeSnapshot, MoogleMailDiscardOutcome};
pub use sent::{MoogleMailSentListEntry, MoogleMailSentObservation};
