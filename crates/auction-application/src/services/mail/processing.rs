//! MoogleMail 业务处理与拍卖入站用例。
//!
//! 业务处理先完成外部装备读取和安全校验，再调用拍卖 owner，最后为领取/退回/通知安排
//! delivery outbox。拍卖事实与密钥信封由单独的 owner 事务提交；重复处理依靠稳定命令 ID
//! 和请求摘要重放，不依靠内存标记。

use std::{collections::HashMap, sync::Arc};

use auction_domain::{CommandId, EquipmentId};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AdminIntakeError, ApplicationError, ApplicationFuture, Clock, CompetitionTransaction,
    EquipmentKeySealerPort, MailAuctionIntakeCommand, MailAuctionIntakePort,
    MailAuctionIntakeRecord, MailAuctionIntakeResult, MailAuctionIntakeTransaction,
    MailAuctionIntakeWrite, MailAuctionItemWrite, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOperation, MailDeliveryOrigin, MailDeliveryResult, MailEquipmentAttachment,
    MailEquipmentFilter, MailEquipmentReader, MailLoginKey, MailMessage, MailNotificationTemplate,
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
