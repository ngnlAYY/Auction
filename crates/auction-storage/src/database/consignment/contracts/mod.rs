//! 寄售合同义务图的存储适配：单事务图写入、来源加载与寄售专用领取。
//!
//! 与拍卖义务共用 `settlement_obligations` 与履约表，但来源是 `consignment_contracts`；通用拍卖
//! 领取只认 `contract_id IS NULL`，寄售义务只由本模块领取。本期没有生产调用方，调用方是 PG 测试。

mod claims;
pub(crate) mod graph;
mod policy;
mod source;
