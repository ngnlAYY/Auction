//! 寄售合同义务图的写入与寄售专用领取端口；实现归存储适配器。

mod claim;
mod graph;
#[cfg(test)]
mod memory;

pub use claim::{ClaimIsolation, ConsignmentFulfillmentClaims, ConsignmentObligationClaim};
pub use graph::{ConsignmentGraphStore, StoredConsignmentGraph};
