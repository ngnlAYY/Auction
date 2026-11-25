//! 寄售第 5 期迁移 000135 的 PG18 测试：合同图与来源约束、金额守恒、授权重放、升级与季末终态。

mod acl;
mod alerts;
mod claims;
mod constraints;
pub(crate) mod fixture;
mod graph;
mod graph_write;
mod inspection;
mod inventory;
mod season_end;
mod solvency;
pub(in crate::consignment) mod source_guard;
pub(crate) mod upgrade;
mod verdict_parity;
