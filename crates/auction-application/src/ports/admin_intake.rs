//! Admin 装备来源与拍品录入端口。
//!
//! 外部来源必须先在数据库锁之外被规范化；进入 owner 事务的结构不包含原始 URL、canonical
//! JSON 或明文装备 key，只携带公开装备事实、来源摘要和加密信封。

use std::{collections::BTreeMap, fmt, future::Future};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{
    CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, EquipmentKeyEnvelopeError,
};

use super::{ApplicationFuture, CompetitionTransactionError, RequestHash};

/// Admin 装备来源的闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminIntakeSourceKind {
    Url,
    Json,
}

impl AdminIntakeSourceKind {
    /// 返回冻结的外部契约名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Json => "json",
        }
    }
}

/// canonical JSON 中单个属性的公开值。
#[derive(Clone, Deserialize, Serialize)]
pub struct CanonicalEquipmentStat {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
}

impl fmt::Debug for CanonicalEquipmentStat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalEquipmentStat")
            .field("value", &self.value)
            .field("base", &self.base)
            .finish()
    }
}

/// Admin canonical 装备输入。
///
/// `key` 只用于来源适配器即时生成密文信封，禁止进入持久化命令、日志或 HTTP 响应。
#[derive(Clone, Deserialize, Serialize)]
pub struct CanonicalEquipment {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub equipment_type: String,
    pub quality: String,
    pub prefix: String,
    pub suffix: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub stats: BTreeMap<String, CanonicalEquipmentStat>,
    #[serde(skip_serializing)]
    pub key: String,
}

impl fmt::Debug for CanonicalEquipment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalEquipment")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("equipment_type", &self.equipment_type)
            .field("quality", &self.quality)
            .field("prefix", &self.prefix)
            .field("suffix", &self.suffix)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("stats", &self.stats)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl Drop for CanonicalEquipment {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

impl CanonicalEquipment {
    /// 清零 key 的 UTF-8 缓冲区，供请求生命周期结束前调用。
    pub fn clear_secret(&mut self) {
        let mut bytes = std::mem::take(&mut self.key).into_bytes();
        bytes.fill(0);
    }

    /// 校验 canonical 输入的边界和封闭字段。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.name, 512)
            || !valid_text(&self.category, 128)
            || !valid_text(&self.equipment_type, 128)
            || !valid_optional_text(&self.quality, 128)
            || !valid_optional_text(&self.prefix, 128)
            || !valid_optional_text(&self.suffix, 128)
            || self.level.is_some_and(|level| !(1..=100_000).contains(&level))
            || !self.tradeable
            || self.stats.is_empty()
            || self.stats.len() > 256
            || EquipmentKey::new(&self.key).is_err()
        {
            return Err(AdminIntakeError::InvalidCommand);
        }
        for (name, stat) in &self.stats {
            if !valid_text(name, 128)
                || !valid_text(&stat.value, 256)
                || stat
                    .base
                    .is_some_and(|base| !base.is_finite() || !(0.0..=1_000_000.0).contains(&base))
            {
                return Err(AdminIntakeError::InvalidCommand);
            }
        }
        Ok(())
    }

    fn digest_view(&self) -> CanonicalEquipmentDigest<'_> {
        let mut hasher = Sha256::new();
        hasher.update(self.key.as_bytes());
        let mut key_digest = [0_u8; 32];
        key_digest.copy_from_slice(&hasher.finalize());
        CanonicalEquipmentDigest {
            external_equipment_id: &self.external_equipment_id,
            name: &self.name,
            category: &self.category,
            equipment_type: &self.equipment_type,
            quality: &self.quality,
            prefix: &self.prefix,
            suffix: &self.suffix,
            level: self.level,
            tradeable: self.tradeable,
            stats: &self.stats,
            key_digest,
        }
    }
}

#[derive(Serialize)]
struct CanonicalEquipmentDigest<'a> {
    external_equipment_id: &'a str,
    name: &'a str,
    category: &'a str,
    equipment_type: &'a str,
    quality: &'a str,
    prefix: &'a str,
    suffix: &'a str,
    level: Option<i32>,
    tradeable: bool,
    stats: &'a BTreeMap<String, CanonicalEquipmentStat>,
    key_digest: [u8; 32],
}

/// world-scoped 的 Admin 装备录入命令。
#[derive(Clone)]
pub struct AdminIntakeCommand {
    pub command_id: CommandId,
    pub world_id: String,
    pub source_kind: Option<AdminIntakeSourceKind>,
    pub equipment_url: Option<String>,
    pub equipment: Option<CanonicalEquipment>,
    pub start_price: i64,
    pub player_id: String,
    pub actor_id: String,
    pub request_id: Option<String>,
}

impl fmt::Debug for AdminIntakeCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminIntakeCommand")
            .field("command_id", &self.command_id)
            .field("world_id", &self.world_id)
            .field("source_kind", &self.source_kind)
            .field("equipment_url", &self.equipment_url)
            .field("equipment", &self.equipment)
            .field("start_price", &self.start_price)
            .field("player_id", &self.player_id)
            .field("actor_id", &self.actor_id)
            .field("request_id", &self.request_id)
            .finish()
    }
}

impl AdminIntakeCommand {
    /// 空 source kind 沿用旧契约的 URL 默认值。
    pub const fn normalized_source_kind(&self) -> AdminIntakeSourceKind {
        match self.source_kind {
            Some(value) => value,
            None => AdminIntakeSourceKind::Url,
        }
    }

    /// 校验不依赖数据库状态的命令字段。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.command_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.start_price <= 0
            || !valid_text(&self.player_id, 256)
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
        {
            return Err(AdminIntakeError::InvalidCommand);
        }
        match self.normalized_source_kind() {
            AdminIntakeSourceKind::Url => {
                if self.equipment.is_some()
                    || self.equipment_url.as_deref().is_none_or(|value| !valid_text(value, 2048))
                {
                    return Err(AdminIntakeError::InvalidCommand);
                }
            }
            AdminIntakeSourceKind::Json => {
                if self.equipment_url.is_some()
                    || self.equipment.as_ref().is_none_or(|value| value.validate().is_err())
                {
                    return Err(AdminIntakeError::InvalidCommand);
                }
            }
        }
        Ok(())
    }

    /// 计算不包含明文 key 的稳定请求摘要。
    pub fn request_hash(&self, aggregate_id: Uuid) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            source_kind: &'static str,
            world_id: &'a str,
            equipment_url: Option<&'a str>,
            equipment: Option<CanonicalEquipmentDigest<'a>>,
            start_price: i64,
            player_id: &'a str,
            actor_id: &'a str,
            request_id: Option<&'a str>,
        }

        let equipment = self.equipment.as_ref().map(CanonicalEquipment::digest_view);
        RequestHash::from_request(
            "admin_auction_intake_item",
            aggregate_id,
            &Request {
                source_kind: self.normalized_source_kind().as_str(),
                world_id: &self.world_id,
                equipment_url: self.equipment_url.as_deref(),
                equipment,
                start_price: self.start_price,
                player_id: &self.player_id,
                actor_id: &self.actor_id,
                request_id: self.request_id.as_deref(),
            },
        )
    }
}

/// 外部来源适配器交给应用 owner 的规范化快照。
pub struct AdminIntakeSourceSnapshot {
    pub adapter: String,
    pub source_fingerprint: [u8; 32],
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub attributes: Value,
    pub collected_at: DateTime<Utc>,
    pub key: EquipmentKey,
}

impl fmt::Debug for AdminIntakeSourceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminIntakeSourceSnapshot")
            .field("adapter", &self.adapter)
            .field("source_fingerprint", &self.source_fingerprint)
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("attributes", &self.attributes)
            .field("collected_at", &self.collected_at)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl AdminIntakeSourceSnapshot {
    /// 校验适配器输出且拒绝未来时间和非 object attributes。
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), AdminIntakeError> {
        if !valid_text(&self.adapter, 128)
            || self.source_fingerprint == [0; 32]
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.name, 512)
            || !valid_text(&self.category, 128)
            || self.level.is_some_and(|level| !(0..=100_000).contains(&level))
            || !self.tradeable
            || self.collected_at.timestamp_micros() == 0
            || self.collected_at > now
            || !self.attributes.is_object()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }

    /// 清零短生命周期 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }
}

/// 外部 URL/canonical JSON 读取器端口。
pub trait AdminIntakeSourceReader: Send + Sync {
    fn read_url(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;

    fn read_json(
        &self,
        world_id: String,
        equipment: CanonicalEquipment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;
}

/// 装备 key 信封加密端口。
pub trait EquipmentKeySealerPort: Send + Sync {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError>;
}

/// 进入 owner 事务的脱敏事实集合。
#[derive(Clone)]
pub struct AdminIntakeWrite {
    pub command_id: CommandId,
    pub aggregate_id: Uuid,
    pub world_id: String,
    pub source_kind: AdminIntakeSourceKind,
    pub source_reference: String,
    pub source_fingerprint: [u8; 32],
    pub actor_id: String,
    pub request_id: Option<String>,
    pub player_id: String,
    pub start_price: i64,
    pub candidate_equipment_id: EquipmentId,
    pub candidate_submission_id: Uuid,
    pub candidate_lot_id: Uuid,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub equipment_attributes: Value,
    pub collected_at: DateTime<Utc>,
    pub key_envelope: EquipmentKeyEnvelope,
    pub request_hash: RequestHash,
}

impl AdminIntakeWrite {
    /// 校验写入结构不含 raw source/key，并确保候选标识互不相同。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.command_id.is_nil()
            || self.aggregate_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || !valid_text(&self.source_reference, 512)
            || self.source_fingerprint == [0; 32]
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
            || !valid_text(&self.player_id, 256)
            || self.start_price <= 0
            || self.candidate_equipment_id.is_nil()
            || self.candidate_submission_id.is_nil()
            || self.candidate_lot_id.is_nil()
            || self.candidate_equipment_id.as_uuid() == self.candidate_submission_id
            || self.candidate_equipment_id.as_uuid() == self.candidate_lot_id
            || self.candidate_submission_id == self.candidate_lot_id
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.equipment_name, 512)
            || !valid_text(&self.equipment_category, 128)
            || self.equipment_level.is_some_and(|level| !(0..=100_000).contains(&level))
            || !self.equipment_attributes.is_object()
            || self.collected_at.timestamp_micros() == 0
            || self.key_envelope.validate().is_err()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }
}

/// Admin 录入完成后可安全重放的结果。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminIntakeResult {
    pub auction_id: Uuid,
    pub auction_sequence: i64,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub submission_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i32,
    pub lot_state_version: i64,
    pub equipment_id: Uuid,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}

impl AdminIntakeResult {
    /// 校验命令账本中恢复出的安全结果。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.auction_id.is_nil()
            || self.auction_sequence <= 0
            || self.auction_state_version <= 0
            || self.preview_revision < 0
            || self.submission_id.is_nil()
            || self.lot_id.is_nil()
            || self.lot_number <= 0
            || self.lot_state_version <= 0
            || self.equipment_id.is_nil()
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.equipment_name, 512)
            || !valid_text(&self.equipment_category, 128)
            || self.equipment_level.is_some_and(|level| !(0..=100_000).contains(&level))
            || self.start_price <= 0
            || !valid_text(&self.seller_player_id, 256)
        {
            return Err(AdminIntakeError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 进入同一事务的安全审计信息。
#[derive(Debug, Clone)]
pub struct AdminIntakeRecord {
    pub command_id: CommandId,
    pub world_id: String,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub result: AdminIntakeResult,
}

/// Admin 录入事务的窄存储适配器能力。
pub trait AdminIntakeTransaction: Send {
    fn add_admin_intake(
        &mut self,
        write: AdminIntakeWrite,
    ) -> impl Future<Output = Result<AdminIntakeResult, CompetitionTransactionError>> + Send;

    fn append_admin_intake_record(
        &mut self,
        record: AdminIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// Admin 装备录入用例端口。
pub trait AdminIntakePort: Send + Sync {
    fn add_item(&self, command: AdminIntakeCommand) -> ApplicationFuture<'_, AdminIntakeResult>;
}

/// Admin 装备录入应用错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminIntakeError {
    #[error("Admin 装备录入命令无效")]
    InvalidCommand,
    #[error("Admin 装备来源格式或协议不受支持")]
    InvalidSource,
    #[error("Admin 装备来源暂时不可用")]
    SourceUnavailable,
    #[error("Admin 装备来源能力未配置")]
    SourceUnconfigured,
    #[error("装备 key 加密能力未配置或失败")]
    KeyUnavailable,
    #[error("Admin 装备录入命令与已有请求冲突")]
    CommandConflict,
    #[error("Admin 装备录入结果损坏")]
    StoredResultCorrupt,
}

impl From<EquipmentKeyEnvelopeError> for AdminIntakeError {
    fn from(_: EquipmentKeyEnvelopeError) -> Self {
        Self::KeyUnavailable
    }
}

/// 生成 world-scoped 命令账本聚合标识。
pub fn aggregate_id(world_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/admin-intake/{world_id}").as_bytes())
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

fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}
