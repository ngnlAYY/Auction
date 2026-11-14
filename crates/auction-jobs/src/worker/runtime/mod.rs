//! Worker 持久化运行时的装配与生命周期编排；主循环负责配置校验、任务启动、优雅排空和
//! Worker control 状态，业务处理按 owner 分开。

use auction_observability::WorkerResult;

use super::forum::reply as forum_reply;
use super::forum::sync as forum_sync;
use super::mail::poll as mail_poll;
use super::operations as operations_alert;
use super::player_notifications;
use super::recovery::health_runtime as recovery_health;

mod asset_reconciliation;
pub(crate) mod recovery_actions;
pub(crate) mod schedule;

pub use asset_reconciliation::{
    ASSET_RECONCILIATION_INTERVAL, ASSET_RECONCILIATION_TIMEOUT, AssetReconciliationRuntime,
};
pub use forum_reply::{ForumReplyPorts, ForumReplyRuntime};
pub use forum_sync::{ForumSyncPorts, ForumSyncRuntime};
pub use mail_poll::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeBindings, MailPollRuntimeConfig,
    MailPollSchedules, PhasedMailSchedule, mail_world_phase,
};
pub use operations_alert::{OperationsAlertRuntime, OperationsAlertRuntimeConfig};
pub use player_notifications::{PlayerNotificationRuntime, PlayerNotificationRuntimeConfig};
pub use recovery_health::RecoveryHealthRuntime;

mod common;
mod config;
mod delivery;
mod error;
mod hot;
mod lifecycle;
mod model;
mod operations;
mod quota;
mod runner;
mod settlement;

pub use config::*;
pub use error::WorkerError;
pub use hot::WorkerHotRuntime;
pub use model::PersistentWorker;
pub use quota::WorkerConnectionQuota;
pub use runner::{run_persistent_with_settlement_until_shutdown, run_until_shutdown};

pub(crate) use common::{merge_worker_result, poll_delay};
pub(crate) use common::{wait_until_or_shutdown, wait_while_paused};
pub(crate) use lifecycle::process_claim;
pub(crate) use recovery_actions::process_recovery_action;
pub(crate) use settlement::{
    process_fulfillment_claim, process_settlement_claim, record_settlement_failure,
};
