//! operations alert 的领域安全投影、阈值规则和窄存储端口。
//! 告警不是拍卖事实，也不能改变竞价、结算或履约结果。该模块只把已经由各 owner 提供的
//! 健康摘要转换成稳定 signal，并通过独立端口交给 PostgreSQL 完整档保存。通知入队仍然
//! 发生在扫描事务之后，避免外部 MoogleMail 结果反向决定告警事实。

use std::{fmt, time::Duration};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};
use crate::ApplicationError;

/// 单轮最多允许冻结的告警 signal 数量。
pub const MAX_OPERATIONS_ALERT_SIGNALS: usize = 64;
/// 单个告警批次最多包含的 signal 数量。
pub const MAX_OPERATIONS_ALERT_BATCH_ITEMS: usize = 64;
/// 告警批次正文的最大字节数。
pub const MAX_OPERATIONS_ALERT_BODY_BYTES: usize = 16 << 10;

#[path = "alert/delivery.rs"]
mod delivery;
#[path = "alert/digest.rs"]
mod digest;
#[path = "alert/evaluate.rs"]
mod evaluate;
#[path = "alert/health.rs"]
mod health;
#[path = "alert/model.rs"]
mod model;
#[path = "alert/signals.rs"]
mod signals;

pub use delivery::{
    OperationsAlertBatchClaim, OperationsAlertBatchItem, OperationsAlertClaimRequest,
    OperationsAlertDeliveryStatus, OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch,
    OperationsAlertObserveCommand,
};
pub use digest::{body_digest, canonical_summary, canonical_summary_prefix, request_hash};
pub use evaluate::evaluate_snapshot;
pub use health::{
    OperationsAlertHealth, OperationsAlertRuntimeStatus, OperationsAlertSourceReader,
    OperationsAlertStore,
};
pub use model::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertProcessSnapshot, OperationsAlertProcessState, OperationsAlertRecoverySnapshot,
    OperationsAlertRecoveryWorld, OperationsAlertSnapshot, OperationsAlertSourceObservation,
    OperationsAlertTarget,
};
pub use signals::{
    OPERATIONS_ALERT_DIGEST_INTERVAL, OperationsAlertError, OperationsAlertKind,
    OperationsAlertSeverity, OperationsAlertSignal, OperationsAlertSource,
};
