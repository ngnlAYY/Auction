use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{AuctionId, CommandId, LotId, SettlementId};

use super::{ApplicationFuture, AuctionLifecycleActor, RequestHash};

/// Admin 可以确认的 recovery 结果闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryConfirmationAction {
    Reclaimed,
    Cancelled,
    EquipmentRecovered,
}

impl RecoveryConfirmationAction {
    /// 返回冻结在 HTTP/数据库契约中的命令值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reclaimed => "reclaimed",
            Self::Cancelled => "cancelled",
            Self::EquipmentRecovered => "equipment_recovered",
        }
    }

    /// 从外部命令值解析闭集；未知值必须失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "reclaimed" => Some(Self::Reclaimed),
            "cancelled" => Some(Self::Cancelled),
            "equipment_recovered" => Some(Self::EquipmentRecovered),
            _ => None,
        }
    }
}

/// Admin recovery 确认命令。
///
/// `expected_state_version` 是版本 fence；存储适配器必须在锁定 recovery 后重新校验它，
/// 不能把报告中的允许按钮当作写入授权。`request_hash` 由 HTTP/Worker 入口按同一规则计算，
/// 用于 recovery 专用命令账本的冲突检测。
#[derive(Debug, Clone)]
pub struct RecoveryConfirmationCommand {
    pub command_id: CommandId,
    pub recovery_id: SettlementId,
    pub expected_state_version: i64,
    pub action: RecoveryConfirmationAction,
    pub reason: String,
    pub request_hash: RequestHash,
    /// 入口请求关联 ID；旧版只把它写入审计 metadata，不占用 audit_records.request_id 列。
    pub request_id: Option<String>,
    pub actor: AuctionLifecycleActor,
}

impl RecoveryConfirmationCommand {
    /// 校验 HTTP 与 Worker 共用的最小命令边界。
    pub fn validate(&self) -> Result<(), RecoveryCommandError> {
        if self.command_id.is_nil()
            || self.recovery_id.is_nil()
            || self.expected_state_version <= 0
            || self.actor.actor_id.trim().is_empty()
            || self.actor.actor_id.chars().count() > 256
            || self.request_id.as_ref().is_some_and(|request_id| {
                request_id.is_empty() || request_id != request_id.trim() || request_id.len() > 256
            })
        {
            return Err(RecoveryCommandError::InvalidCommand);
        }

        if self.action == RecoveryConfirmationAction::Reclaimed {
            if !self.reason.trim().is_empty() {
                return Err(RecoveryCommandError::InvalidCommand);
            }
        } else if self.reason.is_empty()
            || self.reason != self.reason.trim()
            || self.reason.len() > 512
        {
            return Err(RecoveryCommandError::InvalidCommand);
        }
        Ok(())
    }

    /// 按旧版 Go recovery ledger 的 canonical `{RecoveryID,Version,Action,Reason}` 计算摘要。
    /// `command_id` 是账本索引，不属于旧请求摘要；Reason 只在摘要中做 TrimSpace。
    pub fn legacy_request_hash(
        recovery_id: Uuid,
        expected_state_version: i64,
        action: RecoveryConfirmationAction,
        reason: &str,
    ) -> Result<RequestHash, serde_json::Error> {
        let mut payload = Vec::with_capacity(128);
        payload.extend_from_slice(b"{\"RecoveryID\":[");
        for (index, byte) in recovery_id.as_bytes().iter().enumerate() {
            if index != 0 {
                payload.push(b',');
            }
            payload.extend_from_slice(byte.to_string().as_bytes());
        }
        payload.extend_from_slice(b"],\"Version\":");
        payload.extend_from_slice(expected_state_version.to_string().as_bytes());
        payload.extend_from_slice(b",\"Action\":");
        append_go_json_string(&mut payload, action.as_str())?;
        payload.extend_from_slice(b",\"Reason\":");
        append_go_json_string(&mut payload, reason.trim())?;
        payload.push(b'}');
        Ok(RequestHash::from_canonical_bytes(&payload))
    }
}

/// 让摘要字符串遵循 Go `encoding/json` 的 HTML 安全转义细节。
fn append_go_json_string(payload: &mut Vec<u8>, value: &str) -> Result<(), serde_json::Error> {
    let encoded = serde_json::to_vec(value)?;
    let mut index = 0;
    while index < encoded.len() {
        let remaining = &encoded[index..];
        if remaining.starts_with(b"<") {
            payload.extend_from_slice(br#"\u003c"#);
            index += 1;
        } else if remaining.starts_with(b">") {
            payload.extend_from_slice(br#"\u003e"#);
            index += 1;
        } else if remaining.starts_with(b"&") {
            payload.extend_from_slice(br#"\u0026"#);
            index += 1;
        } else if remaining.starts_with(&[0xe2, 0x80, 0xa8]) {
            payload.extend_from_slice(br#"\u2028"#);
            index += 3;
        } else if remaining.starts_with(&[0xe2, 0x80, 0xa9]) {
            payload.extend_from_slice(br#"\u2029"#);
            index += 3;
        } else {
            payload.push(encoded[index]);
            index += 1;
        }
    }
    Ok(())
}

/// recovery 确认成功后持久化并可安全重放的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfirmationResult {
    pub recovery_id: SettlementId,
    pub action: RecoveryConfirmationAction,
    pub stage: String,
    pub resolution: String,
    pub state_version: i64,
    pub relisted_auction_id: Option<AuctionId>,
    pub relisted_lot_id: Option<LotId>,
}

impl RecoveryConfirmationResult {
    /// 校验结果形状，防止损坏的命令账本内容被重新投影到 HTTP。
    pub fn validate(&self) -> Result<(), RecoveryCommandError> {
        if self.recovery_id.is_nil()
            || self.state_version <= 0
            || self.resolution != self.action.as_str()
            || self.relisted_auction_id.is_some() != self.relisted_lot_id.is_some()
            || self.relisted_auction_id.is_some_and(AuctionId::is_nil)
            || self.relisted_lot_id.is_some_and(LotId::is_nil)
        {
            return Err(RecoveryCommandError::InvalidStoredResult);
        }

        match self.stage.as_str() {
            "relist_pending" if self.relisted_auction_id.is_none() => Ok(()),
            "refunding"
                if self.action == RecoveryConfirmationAction::EquipmentRecovered
                    && self.relisted_auction_id.is_some() =>
            {
                Ok(())
            }
            "completed"
                if self.action != RecoveryConfirmationAction::EquipmentRecovered
                    && self.relisted_auction_id.is_some() =>
            {
                Ok(())
            }
            _ => Err(RecoveryCommandError::InvalidStoredResult),
        }
    }
}

/// recovery 确认过程中可被 HTTP 映射的错误分类。
#[derive(Debug, Error)]
pub enum RecoveryCommandError {
    #[error("恢复确认参数无效")]
    InvalidCommand,
    #[error("恢复记录不存在")]
    NotFound,
    #[error("恢复状态版本已变化")]
    StaleState,
    #[error("恢复状态不允许当前确认")]
    NotConfirmable,
    #[error("恢复证据尚未解决")]
    EvidenceUnresolved,
    #[error("命令 ID 与已有 recovery 请求冲突")]
    CommandConflict,
    #[error("恢复命令账本结果无效")]
    InvalidStoredResult,
    #[error("恢复确认事务发生可重试的序列化冲突")]
    SerializationConflict,
    #[error("恢复确认存储失败: {0}")]
    Failed(String),
}

/// recovery 确认的窄存储端口。
///
/// 实现必须把命令查重、事实锁定、状态更新、审计和需要的 realtime/outbox 事实放在同一
/// Serializable 事务中。
pub trait RecoveryCommandRepository: Send + Sync {
    fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> ApplicationFuture<'_, RecoveryConfirmationResult>;
}

/// Admin/Worker 共用的 recovery 确认用例端口。
pub trait RecoveryCommandPort: Send + Sync {
    fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> ApplicationFuture<'_, RecoveryConfirmationResult>;
}

/// 轻量应用服务：负责入口校验和结果校验，具体锁与 SQL 留在存储适配器。
pub struct RecoveryCommandService<R> {
    repository: std::sync::Arc<R>,
}

impl<R> RecoveryCommandService<R>
where
    R: RecoveryCommandRepository + 'static,
{
    /// 建立 recovery 确认服务。
    pub fn new(repository: std::sync::Arc<R>) -> Self {
        Self { repository }
    }

    /// 执行一次带版本 fence 和命令账本的 recovery 确认。
    pub async fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> Result<RecoveryConfirmationResult, crate::ApplicationError> {
        command.validate()?;
        let expected_state_version = command.expected_state_version;
        let recovery_id = command.recovery_id;
        let action = command.action;
        let result = self.repository.confirm_recovery(command).await?;
        result.validate()?;
        if result.recovery_id != recovery_id
            || result.action != action
            || result.state_version <= expected_state_version
        {
            return Err(RecoveryCommandError::InvalidStoredResult.into());
        }
        Ok(result)
    }
}

impl<R> RecoveryCommandPort for RecoveryCommandService<R>
where
    R: RecoveryCommandRepository + 'static,
{
    fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> ApplicationFuture<'_, RecoveryConfirmationResult> {
        Box::pin(async move { RecoveryCommandService::confirm_recovery(self, command).await })
    }
}
