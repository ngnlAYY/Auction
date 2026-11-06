//! Worker 持久化运行时的装配与生命周期编排；主循环负责配置校验、任务启动、优雅排空和
//! Worker control 状态，业务处理按 owner 分开。

use std::sync::Arc;

use crate::WorkerControlHandle;
use auction_application::admin::{
    RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_PROOF_KIND,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklister, is_recovery_buyer_blacklist_reason,
};
use auction_application::auction::{
    AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, SlowProxyCommandPort, SlowProxyDueReader, WorldReader,
};
use auction_application::core::{ApplicationError, Clock, RequestHash};
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryRecipientExclusion, DeliverySendResult, DeliveryTransport,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::mail::{
    MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailNotificationTemplate,
};
use auction_application::recovery::{
    RecoveryActionOutcome, RecoveryResolutionStore, RecoveryWorkStore,
};
use auction_application::scheduling::{
    ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore,
};
use auction_application::settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentStore, SettlementWorkClaim,
    SettlementWorkStore,
};
use auction_domain::{CommandId, deterministic_uuid};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

use super::auction;
use super::forum;
use super::forum::reply as forum_reply;
use super::forum::sync as forum_sync;
use super::mail::poll as mail_poll;
use super::operations as operations_alert;
use super::player_notifications;
use super::recovery;
use super::recovery::health_runtime as recovery_health;
use super::settlement as fulfillment;

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

use common::{bind_worker_metric_modes, drain_delivery_tasks, drain_worker_tasks, retry_delay};
pub(crate) use common::{merge_worker_result, poll_delay};
pub(crate) use common::{wait_until_or_shutdown, wait_while_paused};
use delivery::run_delivery_cycle;
pub(crate) use lifecycle::process_claim;
use operations::{OperationsAlertLoopAction, run_operations_alert_loop};
pub(crate) use recovery_actions::process_recovery_action;
pub(crate) use settlement::{
    process_fulfillment_claim, process_settlement_claim, record_settlement_failure,
};
