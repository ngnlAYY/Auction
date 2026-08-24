//! MoogleMail 业务处理端口。
//!
//! 该模块承接已通过 MoogleMail 协议解析的短生命周期业务 envelope。正文和装备 key 只在
//! Worker 调用应用用例期间存在；进入持久化端口前必须转换为装备事实、密钥信封和无密文
//! delivery payload。

use std::{fmt, future::Future};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope};

use super::{
    AdminIntakeSourceSnapshot, ApplicationFuture, CompetitionTransactionError, RequestHash,
};

/// 装备过滤策略返回的稳定拒绝原因。
pub const EQUIPMENT_QUALITY_NOT_ALLOWED: &str = "quality_not_allowed";
pub const EQUIPMENT_PREFIX_NOT_ALLOWED: &str = "prefix_not_allowed";
pub const EQUIPMENT_SUFFIX_NOT_ALLOWED: &str = "suffix_not_allowed";
pub const EQUIPMENT_CATEGORY_NOT_ALLOWED: &str = "category_not_allowed";
pub const EQUIPMENT_TYPE_NOT_ALLOWED: &str = "equipment_type_not_allowed";
pub const EQUIPMENT_LEVEL_NOT_ALLOWED: &str = "level_not_allowed";
pub const EQUIPMENT_ATTRIBUTES_INVALID: &str = "attributes_invalid";
pub const EQUIPMENT_TRADEABLE_REQUIRED: &str = "tradeable_required";

/// MoogleMail 入站装备过滤策略。
///
/// 空的 allowlist 表示不限制对应维度；启用策略时至少要配置一个维度或等级范围。
/// 策略只消费已由装备来源适配器生成的安全快照，不接触外部页面正文或装备 key。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MailEquipmentFilter {
    pub enabled: bool,
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level_min: i32,
    pub level_max: i32,
}

impl MailEquipmentFilter {
    /// 校验配置边界，避免运行时用未审计的 token 或空策略拦截所有邮件。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if self.level_min < 0 || self.level_max < self.level_min || self.level_max > 1000 {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        let values = [
            &self.qualities,
            &self.prefixes,
            &self.suffixes,
            &self.categories,
            &self.equipment_types,
        ];
        let mut filter_count = 0;
        for list in values {
            if list.len() > 256 {
                return Err(MailProcessingError::InvalidConfiguration);
            }
            filter_count += list.len();
            for (index, value) in list.iter().enumerate() {
                if value.is_empty()
                    || value.len() > 128
                    || value != value.trim()
                    || value.chars().any(char::is_control)
                    || list[..index].iter().any(|previous| previous == value)
                {
                    return Err(MailProcessingError::InvalidConfiguration);
                }
            }
        }
        if self.enabled && filter_count == 0 && self.level_min == 0 && self.level_max == 0 {
            return Err(MailProcessingError::InvalidConfiguration);
        }
        Ok(())
    }

    /// 返回当前装备的稳定拒绝原因；空字符串表示通过。
    pub fn evaluate(&self, snapshot: &AdminIntakeSourceSnapshot) -> &'static str {
        if !snapshot.tradeable {
            return EQUIPMENT_TRADEABLE_REQUIRED;
        }
        let Some(attributes) = snapshot.attributes.as_object() else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let text = |name: &str| match attributes.get(name) {
            None => Some(""),
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => None,
        };
        let Some(equipment_type) = text("type") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(quality) = text("quality") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(prefix) = text("prefix") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        let Some(suffix) = text("suffix") else {
            return EQUIPMENT_ATTRIBUTES_INVALID;
        };
        match attributes.get("tradeable") {
            Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) | None | Some(Value::Null) => {
                return EQUIPMENT_TRADEABLE_REQUIRED;
            }
            Some(_) => return EQUIPMENT_ATTRIBUTES_INVALID,
        }
        let level = match attributes.get("level") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => {
                match value.as_i64().and_then(|value| i32::try_from(value).ok()) {
                    Some(value) => Some(value),
                    None => return EQUIPMENT_ATTRIBUTES_INVALID,
                }
            }
            Some(_) => return EQUIPMENT_ATTRIBUTES_INVALID,
        };
        if !self.enabled {
            return "";
        }
        if !allowed_value(&self.qualities, quality) {
            return EQUIPMENT_QUALITY_NOT_ALLOWED;
        }
        if !allowed_value(&self.prefixes, prefix) {
            return EQUIPMENT_PREFIX_NOT_ALLOWED;
        }
        if !allowed_value(&self.suffixes, suffix) {
            return EQUIPMENT_SUFFIX_NOT_ALLOWED;
        }
        if !allowed_value(&self.categories, &snapshot.category) {
            return EQUIPMENT_CATEGORY_NOT_ALLOWED;
        }
        if !allowed_value(&self.equipment_types, equipment_type) {
            return EQUIPMENT_TYPE_NOT_ALLOWED;
        }
        if (self.level_min != 0 || self.level_max != 0)
            && level.is_none_or(|value| value < self.level_min || value > self.level_max)
        {
            return EQUIPMENT_LEVEL_NOT_ALLOWED;
        }
        ""
    }
}

fn allowed_value(allowed: &[String], value: &str) -> bool {
    allowed.is_empty() || allowed.iter().any(|candidate| candidate == value)
}

/// MoogleMail 业务 envelope 中单件装备附件。
///
/// `key` 只用于读取可信装备页和生成密钥信封，禁止实现任何会输出 key 的格式化或序列化。
pub struct MailEquipmentAttachment {
    pub external_equipment_id: String,
    pub key: EquipmentKey,
}

impl fmt::Debug for MailEquipmentAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailEquipmentAttachment")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl MailEquipmentAttachment {
    /// 清零短生命周期装备 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }

    /// 为同一封邮件的有界幂等重试建立短生命周期附件副本。
    fn duplicate_for_retry(&self) -> Self {
        Self {
            external_equipment_id: self.external_equipment_id.clone(),
            key: self.key.duplicate_for_retry(),
        }
    }
}

/// 已由外部协议适配器校验的 MoogleMail 业务 envelope。
///
/// 该类型不允许进入数据库或持久化命令。`Drop` 会尽力清除正文、发件人、标题和附件 key。
pub struct MailMessage {
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub received_at: DateTime<Utc>,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: i64,
    pub attachments: Vec<MailEquipmentAttachment>,
    pub returnable: bool,
    pub claimable: bool,
    pub content_digest: [u8; 32],
}

impl fmt::Debug for MailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailMessage")
            .field("mailbox_key", &self.mailbox_key)
            .field("external_mail_id", &self.external_mail_id)
            .field("world_id", &self.world_id)
            .field("received_at", &self.received_at)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .field("content_digest", &self.content_digest)
            .finish()
    }
}

impl Drop for MailMessage {
    fn drop(&mut self) {
        self.clear_secrets();
    }
}

impl MailMessage {
    /// 校验 Worker 到应用边界的 envelope，拒绝不完整或越界的业务输入。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !valid_text(&self.mailbox_key, 256)
            || !valid_text(&self.external_mail_id, 512)
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.received_at.timestamp_nanos_opt().is_none()
            || !valid_text(&self.sender, 256)
            || !valid_text(&self.subject, 512)
            || self.body.len() > 64 * 1024
            // 邮件正文允许保留换行、回车和制表符；其他控制字符仍然拒绝，避免
            // 把协议外的二进制/终端控制内容带入后续业务解析和通知正文。
            || self
                .body
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || self.credits < 0
            || self.cod < 0
            || self.content_digest == [0; 32]
            || self.attachments.is_empty() && self.cod > 0 && self.credits == 0
            || self.attachments.len() > 1000
        {
            return Err(MailProcessingError::InvalidMessage);
        }
        for (index, attachment) in self.attachments.iter().enumerate() {
            if !decimal_identity(&attachment.external_equipment_id)
                || index > 0
                    && self.attachments[..index].iter().any(|previous| {
                        previous.external_equipment_id == attachment.external_equipment_id
                    })
            {
                return Err(MailProcessingError::InvalidMessage);
            }
        }
        Ok(())
    }

    /// 清除当前 envelope 持有的敏感短生命周期数据。
    pub fn clear_secrets(&mut self) {
        clear_string(&mut self.sender);
        clear_string(&mut self.subject);
        clear_string(&mut self.body);
        for attachment in &mut self.attachments {
            attachment.clear_secret();
        }
    }

    /// 为同一稳定命令 ID 的有界进程内重试建立显式副本。
    ///
    /// 该类型刻意不实现 `Clone`；只有 Worker 的有限次数重试可以调用此入口，并且任意时刻最多
    /// 保留原 envelope 与一个执行副本。所有副本仍通过 `Drop` 清除正文、身份和装备 key。
    pub fn duplicate_for_retry(&self) -> Self {
        Self {
            mailbox_key: self.mailbox_key.clone(),
            external_mail_id: self.external_mail_id.clone(),
            world_id: self.world_id.clone(),
            received_at: self.received_at,
            sender: self.sender.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
            credits: self.credits,
            cod: self.cod,
            attachments: self
                .attachments
                .iter()
                .map(MailEquipmentAttachment::duplicate_for_retry)
                .collect(),
            returnable: self.returnable,
            claimable: self.claimable,
            content_digest: self.content_digest,
        }
    }

    /// 返回排序后的外部装备 ID，用于稳定 delivery payload 和审计元数据。
    pub fn sorted_attachment_ids(&self) -> Vec<String> {
        let mut ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.external_equipment_id.clone())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
}

/// MoogleMail 附件解析得到的可信装备读取器。
pub trait MailEquipmentReader: Send + Sync {
    fn read(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;
}

/// 单件进入拍卖 owner 的已验证事实和短生命周期 key。
pub struct MailAuctionItem {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub attributes: Value,
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

/// MoogleMail 拍卖入站命令。
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
    pub attributes: Value,
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
            || i64::from(self.added_count + self.existing_count) != self.lot_ids.len() as i64
        {
            return Err(MailProcessingError::OwnerResultInvalid);
        }
        Ok(())
    }
}

/// MoogleMail 拍卖入站事务能力。
pub trait MailAuctionIntakeTransaction: Send {
    fn add_mail_auction_intake(
        &mut self,
        write: MailAuctionIntakeWrite,
    ) -> impl Future<Output = Result<MailAuctionIntakeResult, CompetitionTransactionError>> + Send;

    fn append_mail_auction_record(
        &mut self,
        record: MailAuctionIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// MoogleMail 拍卖入站用例端口。
pub trait MailAuctionIntakePort: Send + Sync {
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

/// Delivery 操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOperation {
    Send,
    Return,
    Take,
}

impl MailDeliveryOperation {
    /// 返回稳定的 outbox 操作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Return => "return",
            Self::Take => "take",
        }
    }
}

/// MoogleMail delivery 的业务 owner 闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOrigin {
    MailIntake,
    SettlementRecoveryAction,
    OperationsAlert,
}

impl MailDeliveryOrigin {
    /// 返回 `deliveries.origin_kind` 使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MailIntake => "mail_intake",
            Self::SettlementRecoveryAction => "settlement_recovery_action",
            Self::OperationsAlert => "operations_alert",
        }
    }
}

/// MoogleMail 通知模板闭集；正文不能由入站邮件任意注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNotificationTemplate {
    AuctionAssetsRejected,
    AuctionNoAttachments,
    AuctionEquipmentRejected,
    UserAssetsRejected,
    RegisterKey,
    RegisterResult,
    RebindResult,
    ForumAuctionNotice,
    RecoveryPaymentReminder,
    OperationsAlert,
}

impl MailNotificationTemplate {
    /// 返回稳定的模板名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuctionAssetsRejected => "auction_assets_rejected",
            Self::AuctionNoAttachments => "auction_no_attachments",
            Self::AuctionEquipmentRejected => "auction_equipment_rejected",
            Self::UserAssetsRejected => "user_assets_rejected",
            Self::RegisterKey => "register_key",
            Self::RegisterResult => "register_result",
            Self::RebindResult => "rebind_result",
            Self::ForumAuctionNotice => "forum_auction_notice",
            Self::RecoveryPaymentReminder => "recovery_payment_reminder",
            Self::OperationsAlert => "operations_alert",
        }
    }
}

/// 由业务 owner 生成的无密文 MoogleMail delivery 命令。
#[derive(Debug, Clone)]
pub struct MailDeliveryCommand {
    pub command_id: CommandId,
    pub request_hash: RequestHash,
    pub origin: MailDeliveryOrigin,
    pub origin_id: String,
    pub operation: MailDeliveryOperation,
    pub world_id: String,
    pub external_mail_id: String,
    pub recipient: String,
    pub template: Option<MailNotificationTemplate>,
    pub result_code: Option<String>,
    /// 注册 key 通知使用的用户 ID 引用；只允许和 `register_key` 模板一起出现。
    pub user_key_user_id: Option<Uuid>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_ids: Vec<String>,
    /// operations alert 使用的已冻结安全摘要；其他 delivery 必须省略。
    pub notification_body: Option<String>,
}

/// MoogleMail delivery 入队后的安全结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryResult {
    pub delivery_id: Uuid,
}

/// recovery delivery 激活时使用的稳定身份。
///
/// `prepare_*` 属于 delivery 行本身的幂等身份；`activate_*` 属于 recovery action 的第二个
/// 本地事务边界。两者分开保存，才能在 delivery 已经进入 pending 而 action 尚未写回时安全重放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryActivation {
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    pub prepare_request_hash: RequestHash,
    pub activate_command_id: CommandId,
    pub activate_request_hash: RequestHash,
    pub activated_at: DateTime<Utc>,
}

/// MoogleMail delivery outbox 入队端口。
pub trait MailDeliveryEnqueuer: Send + Sync {
    fn enqueue(&self, command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 以 `prepared` 状态写入 recovery delivery；普通通知仍直接进入 `pending`。
    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 将已校验的 recovery delivery 从 `prepared` 激活为可领取的 `pending`。
    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;
}

/// 业务处理器返回的安全入站决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailProcessingDecision {
    pub mail_kind: String,
    pub status: String,
    pub decision_code: String,
    pub metadata: Value,
}

impl MailProcessingDecision {
    /// 校验决定只包含数据库允许的闭集和 JSON 对象。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !matches!(
            self.mail_kind.as_str(),
            "equipment" | "bid" | "delivery" | "registration" | "rebind" | "unknown"
        ) || !matches!(self.status.as_str(), "accepted" | "rejected" | "failed")
            || !valid_text(&self.decision_code, 256)
            || !self.metadata.is_object()
        {
            return Err(MailProcessingError::InvalidDecision);
        }
        Ok(())
    }
}

/// MoogleMail 业务处理器端口。
pub trait MailProcessor: Send + Sync {
    fn process(&self, message: MailMessage) -> ApplicationFuture<'_, MailProcessingDecision>;
}

/// MoogleMail 业务处理错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum MailProcessingError {
    #[error("MoogleMail 业务 envelope 无效")]
    InvalidMessage,
    #[error("MoogleMail 业务处理配置无效")]
    InvalidConfiguration,
    #[error("MoogleMail 邮件无法安全退回或领取")]
    UnsafeDisposition,
    #[error("MoogleMail 装备来源不可信")]
    InvalidEquipment,
    #[error("MoogleMail owner 返回结果无效")]
    OwnerResultInvalid,
    #[error("MoogleMail 业务决定无效")]
    InvalidDecision,
    #[error("MoogleMail 拍卖入站命令无效")]
    InvalidAuctionCommand,
    #[error("MoogleMail Users 命令无效")]
    InvalidUserCommand,
    #[error("MoogleMail 拍卖入站密钥封装失败")]
    KeyUnavailable,
    #[error("MoogleMail 拍卖入站命令与已有请求冲突")]
    CommandConflict,
    #[error("MoogleMail 拍卖入站存储结果损坏")]
    StoredResultCorrupt,
    #[error("MoogleMail 登录 key 无效")]
    InvalidLoginKey,
    #[error("MoogleMail 登录 key 不存在")]
    LoginKeyNotFound,
    #[error("MoogleMail 登录 key 信封无效")]
    InvalidLoginKeyEnvelope,
    #[error("Users owner 返回结果无效")]
    InvalidUserResult,
}

fn decimal_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn clear_string(value: &mut String) {
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

/// 生成 world-scoped MoogleMail 拍卖命令账本聚合标识。
pub fn mail_auction_aggregate_id(world_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/mail-auction-intake/{world_id}").as_bytes())
}
