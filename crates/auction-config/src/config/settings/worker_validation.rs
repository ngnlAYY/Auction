//! Worker 配置校验的模块装配。
//!
//! 不同 Worker owner 的边界校验分别放在子目录中；本文件只向 `settings` 父模块重新导出
//! 原有的 `pub(super)` 函数，保持配置加载与错误契约不变。

use super::*;

#[path = "worker_validation/auction.rs"]
mod auction;
#[path = "worker_validation/common.rs"]
mod common;
#[path = "worker_validation/control.rs"]
mod control;
#[path = "worker_validation/delivery.rs"]
mod delivery;
#[path = "worker_validation/forum.rs"]
mod forum;
#[path = "worker_validation/fulfillment.rs"]
mod fulfillment;
#[path = "worker_validation/mooglemail.rs"]
mod mooglemail;
#[path = "worker_validation/operations.rs"]
mod operations;
#[path = "worker_validation/recovery.rs"]
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
