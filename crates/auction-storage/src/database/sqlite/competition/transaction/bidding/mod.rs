//! SQLite 竞价事务子模块装配。
//!
//! 命令账本、锁内快照、计划应用、代理/lot 写入、审计和 realtime 事件分别由子模块拥有；
//! `transaction.rs` 仍是唯一的 `BEGIN IMMEDIATE` 事务边界。

mod audit;
mod eligibility;
mod facts;
mod ledger;
mod plan;
mod proxy;
mod realtime;
mod snapshot;

pub(super) use audit::{insert_audit_for_public_command, insert_audit_for_slow_proxy};
pub(super) use facts::{insert_bid, update_lot};
pub(super) use proxy::apply_proxy_changes;
pub(super) use realtime::insert_realtime_event;
