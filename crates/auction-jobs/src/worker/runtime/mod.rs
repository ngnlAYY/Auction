//! Worker 持久化运行时的装配与生命周期编排。
//!
//! 主循环只负责配置校验、任务启动、优雅排空和 Worker control 状态；具体业务处理按
//! delivery、recovery、生命周期和结算 owner 拆到相邻模块，避免在调度器中复制领域规则。

use std::sync::Arc;

use crate::WorkerControlHandle;
use auction_application::{
    ApplicationError, AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, Clock, DeliverySendResult, DeliveryTransport,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore, MailDeliveryActivation,
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate, RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_PROOF_KIND,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON, RecoveryActionOutcome, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklister, RecoveryResolutionStore, RecoveryWorkStore, RequestHash,
    ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore, SettleAuctionCommand,
    SettlementCommandPort, SettlementFulfillmentClaim, SettlementFulfillmentCommandPort,
    SettlementFulfillmentStore, SettlementWorkClaim, SettlementWorkStore, SlowProxyCommandPort,
    SlowProxyDueReader,
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
#[path = "../realtime/consumer.rs"]
mod realtime_consumer;
#[path = "../recovery/loop/mod.rs"]
mod recovery;
#[path = "recovery.rs"]
mod recovery_actions;
#[path = "../recovery/health.rs"]
mod recovery_health;
#[path = "schedule.rs"]
mod schedule;

pub use forum_reply::ForumReplyRuntime;
pub use forum_sync::ForumSyncRuntime;
pub use mail_poll::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeConfig,
};
pub use operations_alert::{OperationsAlertRuntime, OperationsAlertRuntimeConfig};
pub use realtime_consumer::{
    PersistentRealtimeConsumer, RealtimeConsumerConfigError, RealtimeConsumerRunner,
};
pub use recovery_health::RecoveryHealthRuntime;

mod common;
mod config;
mod delivery;
mod error;
mod lifecycle;
mod model;
mod operations;
mod runner;
mod settlement;

pub use config::*;
pub use error::WorkerError;
pub use model::PersistentWorker;
pub use runner::{
    run_persistent_until_shutdown, run_persistent_with_settlement_until_shutdown,
    run_until_shutdown,
};

use common::{
    bind_worker_metric_modes, drain_delivery_tasks, drain_worker_tasks, merge_worker_result,
    poll_delay, retry_delay, wait_until_or_shutdown,
};
use delivery::run_delivery_cycle;
use lifecycle::process_claim;
use operations::{OperationsAlertLoopAction, run_operations_alert_loop};
use recovery_actions::process_recovery_action;
use settlement::{process_fulfillment_claim, process_settlement_claim, record_settlement_failure};
