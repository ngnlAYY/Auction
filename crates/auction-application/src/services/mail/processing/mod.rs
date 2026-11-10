//! MoogleMail 业务处理与拍卖入站用例。
//! 业务处理先恢复已提交的拍卖接受决定；新邮件才进行外部装备读取、安全校验和拍卖录入，
//! 最后为领取/退回/通知安排 delivery outbox。拍卖事实与密钥信封由单独的 owner 事务提交；
//! 重复处理恢复原投稿资产并补齐同一领取意图，不根据变化后的远端页面反向退回。

use std::{collections::HashMap, sync::Arc};

use auction_domain::{CommandId, EquipmentId};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::admin::{AdminIntakeError, EquipmentKeySealerPort};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{
    Clock, CompetitionTransaction, StoredCommand, TransactionPolicy, TransactionRunner,
};
use crate::ports::mail::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeRecord, MailAuctionIntakeResult,
    MailAuctionIntakeTransaction, MailAuctionIntakeWrite, MailAuctionItemWrite,
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailDeliveryResult, MailEquipmentAttachment, MailEquipmentFilter, MailEquipmentReader,
    MailLoginKey, MailMessage, MailNoticeEnqueue, MailNoticeLimit, MailNotificationTemplate,
    MailProcessingDecision, MailProcessingError, MailProcessor, MailUserCommandKind, MailUserPort,
    MailUserRebindCommand, MailUserRegisterCommand, MailUserRekeyCommand, MailUserResultCode,
    SubmissionReceiptFacts, mail_auction_aggregate_id,
};
use crate::ports::observability::external_metrics::{MailNoticeClass, MailWorld};

mod commands;
mod intake;
mod pricing;
mod processor;
mod receipt;

pub use intake::MailAuctionIntakeService;
pub use processor::{MailProcessorConfig, MailProcessorService};
pub use receipt::{
    MAX_SUBMISSION_RECEIPT_BODY_BYTES, SUBMISSION_RECEIPT_SUBJECT, submission_receipt_body,
    submission_receipt_invalid_reason,
};
