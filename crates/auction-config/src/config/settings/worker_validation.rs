//! Worker 配置校验的模块装配；边界校验在子目录按 owner 持有，此处只重新导出 `pub(super)` 函数。

mod auction;
mod common;
mod control;
mod delivery;
mod forum;
mod fulfillment;
mod mooglemail;
mod operations;
mod recovery;

pub(super) use auction::validate_auction_worker;
pub(super) use common::valid_config_text;
pub(super) use control::validate_worker_control;
pub(super) use delivery::validate_delivery_worker;
pub(super) use forum::validate_forum_worker;
pub(super) use fulfillment::validate_fulfillment_worker;
pub(super) use mooglemail::validate_mooglemail_worker;
pub(super) use operations::validate_operations_alert_worker;
pub(super) use recovery::validate_recovery_worker;
