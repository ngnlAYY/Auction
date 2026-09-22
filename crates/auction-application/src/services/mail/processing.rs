//! MoogleMail 业务处理与拍卖入站用例。
//!
//! 业务处理先恢复已提交的拍卖接受决定；新邮件才进行外部装备读取、安全校验和拍卖录入，
//! 最后为领取/退回/通知安排 delivery outbox。拍卖事实与密钥信封由单独的 owner 事务提交；
//! 重复处理恢复原投稿资产并补齐同一领取意图，不根据变化后的远端页面反向退回。

use std::{collections::HashMap, sync::Arc};

use auction_domain::{CommandId, EquipmentId};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AdminIntakeError, ApplicationError, ApplicationFuture, Clock, CompetitionTransaction,
    EquipmentKeySealerPort, MailAuctionAcceptance, MailAuctionIntakeCommand,
    MailAuctionIntakeLookup, MailAuctionIntakePort, MailAuctionIntakeRecord,
    MailAuctionIntakeResult, MailAuctionIntakeTransaction, MailAuctionIntakeWrite,
    MailAuctionItemWrite, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailDeliveryResult, MailEquipmentAttachment, MailEquipmentFilter,
    MailEquipmentReader, MailLoginKey, MailMessage, MailNotificationTemplate,
    MailProcessingDecision, MailProcessingError, MailProcessor, MailUserCommandKind, MailUserPort,
    MailUserRebindCommand, MailUserRegisterCommand, MailUserResultCode, StoredCommand,
    TransactionPolicy, TransactionRunner, mail_auction_aggregate_id,
};

#[path = "processing/commands.rs"]
mod commands;
#[path = "processing/intake.rs"]
mod intake;
#[path = "processing/pricing.rs"]
mod pricing;
#[path = "processing/processor.rs"]
mod processor;

pub use intake::MailAuctionIntakeService;
pub use processor::{MailProcessorConfig, MailProcessorService};
