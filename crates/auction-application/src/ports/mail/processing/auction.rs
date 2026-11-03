use std::{fmt, future::Future};

use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::{ApplicationFuture, CompetitionTransactionError, RequestHash};
use super::acceptance::{MailAuctionAcceptance, MailAuctionIntakeLookup};
use super::common::MailProcessingError;
use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};

/// 单件进入拍卖 owner 的已验证事实和短生命周期 key。
pub struct MailAuctionItem {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub attributes: serde_json::Value,
    pub source_fingerprint: [u8; 32],
    pub collected_at: DateTime<Utc>,
    pub start_price: i64,
    pub key: EquipmentKey,
}

impl fmt::Debug for MailAuctionItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailAuctionItem")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("attributes", &self.attributes)
            .field("source_fingerprint", &self.source_fingerprint)
            .field("collected_at", &self.collected_at)
            .field("start_price", &self.start_price)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl MailAuctionItem {
    /// 清零待封装的装备 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }
}

pub struct MailAuctionIntakeCommand {
    pub command_id: CommandId,
    pub aggregate_id: Uuid,
    pub request_hash: RequestHash,
    pub world_id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub seller: String,
    pub received_at: DateTime<Utc>,
    pub content_digest: [u8; 32],
    pub items: Vec<MailAuctionItem>,
    pub now: DateTime<Utc>,
}

/// 进入拍卖事务前已经完成密钥封装的单件写入。
#[derive(Clone)]
pub struct MailAuctionItemWrite {
    pub equipment_id: EquipmentId,
    pub lot_id: Uuid,
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub attributes: serde_json::Value,
    pub collected_at: DateTime<Utc>,
    pub start_price: i64,
    pub key_envelope: EquipmentKeyEnvelope,
}

/// 进入 PostgreSQL owner 事务的 MoogleMail 拍卖写入计划。
#[derive(Clone)]
pub struct MailAuctionIntakeWrite {
    pub command_id: CommandId,
    pub aggregate_id: Uuid,
    pub world_id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub seller: String,
    pub received_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub submission_id: Uuid,
    pub items: Vec<MailAuctionItemWrite>,
    pub request_hash: RequestHash,
}

/// MoogleMail 拍卖入站安全结果。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MailAuctionIntakeResult {
    pub auction_id: Uuid,
    pub auction_sequence: i64,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub submission_id: Uuid,
    pub added_count: i32,
    pub existing_count: i32,
    pub lot_ids: Vec<Uuid>,
}

impl MailAuctionIntakeResult {
    /// 校验数据库 owner 返回的结果，避免把损坏结果写入入站决定。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if self.auction_id.is_nil()
            || self.auction_sequence <= 0
            || self.auction_state_version <= 0
            || self.preview_revision < 0
            || self.submission_id.is_nil()
            || self.added_count < 0
            || self.existing_count < 0
            || self.lot_ids.is_empty()
            || self.lot_ids.iter().any(Uuid::is_nil)
        {
            return Err(MailProcessingError::OwnerResultInvalid);
        }
        let total_count = self
            .added_count
            .checked_add(self.existing_count)
            .ok_or(MailProcessingError::OwnerResultInvalid)?;
        let lot_count = i32::try_from(self.lot_ids.len()).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "mail_intake",
                "convert_lot_count",
                DiagnosticContext::default(),
            );
            MailProcessingError::OwnerResultInvalid
        })?;
        if total_count != lot_count {
            return Err(MailProcessingError::OwnerResultInvalid);
        }
        Ok(())
    }
}

/// MoogleMail 拍卖入站事务能力。
pub trait MailAuctionIntakeTransaction: Send {
    /// 恢复已提交投稿及其原附件；无命令返回 None，身份或账本不一致必须失败关闭。
    fn load_mail_auction_acceptance(
        &mut self,
        lookup: MailAuctionIntakeLookup,
    ) -> impl Future<Output = Result<Option<MailAuctionAcceptance>, CompetitionTransactionError>> + Send;

    fn add_mail_auction_intake(
        &mut self,
        write: MailAuctionIntakeWrite,
    ) -> impl Future<Output = Result<MailAuctionIntakeResult, CompetitionTransactionError>> + Send;

    fn append_mail_auction_record(
        &mut self,
        record: MailAuctionIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

pub trait MailAuctionIntakePort: Send + Sync {
    /// 在重新执行外部读取、过滤或退回前恢复已有接受决定。
    fn find_accepted(
        &self,
        lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>>;

    fn ingest_mail(
        &self,
        command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult>;
}

/// MoogleMail 拍卖入站审计记录。
#[derive(Debug, Clone)]
pub struct MailAuctionIntakeRecord {
    pub command_id: CommandId,
    pub world_id: String,
    pub mailbox_key: String,
    pub result: MailAuctionIntakeResult,
}
