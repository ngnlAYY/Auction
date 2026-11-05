//! Worker 持久化运行时的装配与生命周期编排；主循环负责配置校验、任务启动、优雅排空和
//! Worker control 状态，业务处理按 owner 分开。

use std::sync::Arc;

use crate::WorkerControlHandle;
use auction_application::{
    ApplicationError, AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, Clock, DeliveryClaimExclusions, DeliveryRecipientExclusion,
    DeliverySendResult, DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore, MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
    RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_PROOF_KIND,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON, RecoveryActionOutcome, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklister, RecoveryResolutionStore, RecoveryWorkStore, RequestHash,
    ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore, SettleAuctionCommand,
    SettlementCommandPort, SettlementFulfillmentClaim, SettlementFulfillmentCommandPort,
    SettlementFulfillmentStore, SettlementWorkClaim, SettlementWorkStore, SlowProxyCommandPort,
    SlowProxyDueReader, WorldReader, is_recovery_buyer_blacklist_reason,
};
use auction_domain::{CommandId, deterministic_uuid};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

#[path = "../auction/lifecycle.rs"]
mod auction;
#[path = "../forum/loop.rs"]
mod forum;
#[path = "../forum/reply.rs"]
mod forum_reply;
#[path = "../forum/sync.rs"]
mod forum_sync;
#[path = "../settlement/fulfillment.rs"]
mod fulfillment;
#[path = "../mail/poll.rs"]
mod mail_poll;
#[path = "../operations/alert.rs"]
mod operations_alert;
mod player_notifications {
    include!("../player_notifications.rs");
}
#[path = "../realtime/consumer.rs"]
mod realtime_consumer;
#[path = "../recovery/loop/mod.rs"]
mod recovery;
#[path = "recovery.rs"]
mod recovery_actions;
#[path = "../recovery/health.rs"]
mod recovery_health;
mod schedule;

pub use forum_reply::{ForumReplyPorts, ForumReplyRuntime};
pub use forum_sync::{ForumSyncPorts, ForumSyncRuntime};
pub use mail_poll::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeBindings, MailPollRuntimeConfig,
    MailPollSchedules, PhasedMailSchedule, mail_world_phase,
};
pub use operations_alert::{OperationsAlertRuntime, OperationsAlertRuntimeConfig};
pub use player_notifications::{PlayerNotificationRuntime, PlayerNotificationRuntimeConfig};
pub use realtime_consumer::{
    PersistentRealtimeConsumer, RealtimeConsumerConfigError, RealtimeConsumerRunner,
};
pub use recovery_health::RecoveryHealthRuntime;

mod common;
mod config;
mod delivery;
mod error;
mod hot;
mod lifecycle;
mod model;
mod operations;
mod runner;
mod settlement;

pub use config::*;
pub use error::WorkerError;
pub use hot::WorkerHotRuntime;
pub use model::PersistentWorker;
pub use runner::{run_persistent_with_settlement_until_shutdown, run_until_shutdown};

use common::{
    bind_worker_metric_modes, drain_delivery_tasks, drain_worker_tasks, merge_worker_result,
    poll_delay, retry_delay,
};
pub(super) use common::{wait_until_or_shutdown, wait_while_paused};
use delivery::run_delivery_cycle;
use lifecycle::process_claim;
use operations::{OperationsAlertLoopAction, run_operations_alert_loop};
use recovery_actions::process_recovery_action;
use settlement::{process_fulfillment_claim, process_settlement_claim, record_settlement_failure};
