// 经 Worker control 代为执行的外部只读能力（W1.10）。
// Admin 手工装备读取与 CLI Sent 核查只描述目标；凭据、请求限速、账号熔断与 W1.9 主动暂停
// 都由 Worker 进程统一执行，Admin/CLI 不再持有第二个限速器。Worker 不可用时调用方得到明确的
// 能力不可用结果，而不是回退到本地直连。

use crate::{AdminIntakeError, AdminIntakeSourceSnapshot};
use auction_domain::EquipmentKey;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// 远端只读请求的稳定超时（秒）；server 中间件与 client 共用同一上界。
/// 单次读取仍受 Worker 自己的 fetch 预算约束，这里只防止悬挂。
pub const WORKER_REMOTE_READ_TIMEOUT_SECONDS: u64 = 180;

/// 远端只读的稳定错误分类；不携带远端正文、Cookie 或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRemoteReadError {
    /// W1.9 主动暂停外部访问；命令未发往远端。
    Paused,
    /// Worker control 未启用、socket 不存在或对端明确不可用。
    Unavailable,
    /// 超过有界超时。
    Timeout,
    /// 对端协议版本或响应形状不受支持。
    Incompatible,
    /// 目标或参数无效。
    Invalid,
}

impl WorkerRemoteReadError {
    /// 返回稳定名称，用于诊断与 CLI 输出。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Paused => "external_access_paused",
            Self::Unavailable => "worker_control_unavailable",
            Self::Timeout => "worker_control_timeout",
            Self::Incompatible => "worker_control_incompatible",
            Self::Invalid => "invalid_remote_read_request",
        }
    }
}

/// 远端只读的异步结果。
pub type WorkerRemoteReadFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WorkerRemoteReadError>> + Send + 'a>>;

/// Admin 装备 URL 读取请求；只描述世界与目标链接。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEquipmentReadRequest {
    pub world_id: String,
    pub equipment_url: String,
}

impl WorkerEquipmentReadRequest {
    /// 世界闭集与 URL 长度边界；与 control 请求体上限一起保证有界。
    pub fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && !self.equipment_url.is_empty()
            && self.equipment_url.len() <= 512
    }
}

/// Worker 解析后的装备事实；`key` 是公开装备链接中的公示数据，只在同组 peer 的
/// Unix socket 上传输，不进入日志、审计或普通 DTO。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEquipmentReadResponse {
    pub adapter: String,
    pub source_fingerprint: Vec<u8>,
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub attributes: Value,
    pub collected_at: DateTime<Utc>,
    pub key: String,
}

impl std::fmt::Debug for WorkerEquipmentReadResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerEquipmentReadResponse")
            .field("adapter", &self.adapter)
            .field("source_fingerprint", &self.source_fingerprint)
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("collected_at", &self.collected_at)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl WorkerEquipmentReadResponse {
    /// 从 Worker 侧解析结果建立传输投影。
    pub fn from_snapshot(
        snapshot: AdminIntakeSourceSnapshot,
    ) -> Result<Self, WorkerRemoteReadError> {
        let key = snapshot
            .key
            .with_bytes(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .map_err(|error| { let _ = error; WorkerRemoteReadError::Invalid })?;
        Ok(Self {
            adapter: snapshot.adapter,
            source_fingerprint: snapshot.source_fingerprint.to_vec(),
            external_equipment_id: snapshot.external_equipment_id,
            name: snapshot.name,
            category: snapshot.category,
            level: snapshot.level,
            tradeable: snapshot.tradeable,
            attributes: snapshot.attributes,
            collected_at: snapshot.collected_at,
            key,
        })
    }

    /// 还原为 Admin 录入用例所需的来源快照；所有字段再次校验。
    pub fn into_snapshot(self) -> Result<AdminIntakeSourceSnapshot, WorkerRemoteReadError> {
        let source_fingerprint: [u8; 32] = self
            .source_fingerprint
            .as_slice()
            .try_into()
            .map_err(|error| { let _ = error; WorkerRemoteReadError::Incompatible })?;
        let key = EquipmentKey::new(&self.key).map_err(|error| { let _ = error; WorkerRemoteReadError::Incompatible })?;
        Ok(AdminIntakeSourceSnapshot {
            adapter: self.adapter,
            source_fingerprint,
            external_equipment_id: self.external_equipment_id,
            name: self.name,
            category: self.category,
            level: self.level,
            tradeable: self.tradeable,
            attributes: self.attributes,
            collected_at: self.collected_at,
            key,
        })
    }
}

/// CLI Sent 核查请求；目标与标题由调用方从本地数据库取得。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupRequest {
    pub world: String,
    pub recipient: String,
    pub subject: String,
    pub external_equipment_id: String,
}

impl WorkerSentLookupRequest {
    pub fn validate(&self) -> bool {
        matches!(self.world.as_str(), "main" | "isekai")
            && auction_domain::valid_text(&self.recipient, 256)
            && auction_domain::valid_text(&self.subject, 512)
            && !self.external_equipment_id.is_empty()
            && self.external_equipment_id.len() <= 32
            && self.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
    }
}

/// 单个 Sent 候选的安全快照；Debug 不复述收件人与标题。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupCandidate {
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    pub observed_at: DateTime<Utc>,
}

impl std::fmt::Debug for WorkerSentLookupCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerSentLookupCandidate")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .field("collected", &self.collected)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

/// Sent 核查的有界结果；`status` 使用既有闭集名称。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupResponse {
    pub status: String,
    pub scan_complete: bool,
    pub pages_scanned: u32,
    pub observed_at: DateTime<Utc>,
    pub candidates: Vec<WorkerSentLookupCandidate>,
}

/// Admin 装备 URL 的远端来源读取（Worker 侧真实执行、Admin/CLI 侧走 control client）。
pub trait WorkerRemoteReadPort: Send + Sync {
    fn read_equipment(
        &self,
        request: WorkerEquipmentReadRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>;

    fn lookup_sent(
        &self,
        request: WorkerSentLookupRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>;
}

impl From<WorkerRemoteReadError> for AdminIntakeError {
    fn from(error: WorkerRemoteReadError) -> Self {
        match error {
            WorkerRemoteReadError::Paused => Self::ExternalAccessPaused,
            WorkerRemoteReadError::Invalid => Self::InvalidCommand,
            WorkerRemoteReadError::Unavailable
            | WorkerRemoteReadError::Timeout
            | WorkerRemoteReadError::Incompatible => Self::SourceUnavailable,
        }
    }
}

/// 供 `ApplicationFuture` 无关的调用方复用的装箱辅助。
pub(crate) fn boxed<'a, T: Send + 'a>(
    future: impl Future<Output = Result<T, WorkerRemoteReadError>> + Send + 'a,
) -> WorkerRemoteReadFuture<'a, T> {
    Box::pin(future)
}

/// Worker 侧控制不可用时 Admin 端使用的立即失败端口；不回退到本地直连。
pub struct UnavailableWorkerRemoteRead;

impl WorkerRemoteReadPort for UnavailableWorkerRemoteRead {
    fn read_equipment(
        &self,
        _request: WorkerEquipmentReadRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse> {
        boxed(async { Err(WorkerRemoteReadError::Unavailable) })
    }

    fn lookup_sent(
        &self,
        _request: WorkerSentLookupRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse> {
        boxed(async { Err(WorkerRemoteReadError::Unavailable) })
    }
}

#[cfg(test)]
mod tests {
    use super as parent;
    use self::parent::*;
    use chrono::TimeDelta;
    use serde_json::json;

    fn snapshot() -> AdminIntakeSourceSnapshot {
        AdminIntakeSourceSnapshot {
            adapter: "hentaiverse-equipment-v1".to_owned(),
            source_fingerprint: [7; 32],
            external_equipment_id: "300753957".to_owned(),
            name: "Legendary Radiant Phase Shoes".to_owned(),
            category: "Shoes".to_owned(),
            level: Some(500),
            tradeable: true,
            attributes: json!({"Primary Attributes / Strength": {"value": "+10.00"}}),
            collected_at: Utc::now() - TimeDelta::seconds(30),
            key: EquipmentKey::new("0123456789").expect("test key is ten hex characters"),
        }
    }

    #[test]
    fn equipment_projection_round_trips_without_leaking_key_in_debug() {
        let response =
            WorkerEquipmentReadResponse::from_snapshot(snapshot()).expect("key is exposable");
        assert_eq!(response.key, "0123456789");
        let restored = response.clone().into_snapshot().expect("projection is valid");
        assert_eq!(restored.source_fingerprint, [7; 32]);
        assert_eq!(restored.external_equipment_id, "300753957");
        // 传输投影的 Debug 只暴露公开结构，key 不从 Debug 输出。
        let debug = format!("{response:?}");
        assert!(!debug.contains("0123456789"));
    }

    #[test]
    fn equipment_projection_rejects_corrupt_fingerprint_or_key() {
        let mut response =
            WorkerEquipmentReadResponse::from_snapshot(snapshot()).expect("key is exposable");
        response.source_fingerprint = vec![1; 31];
        assert_eq!(
            response.clone().into_snapshot().err(),
            Some(WorkerRemoteReadError::Incompatible)
        );
        response.source_fingerprint = vec![1; 32];
        response.key = "zzzzzzzzzz".to_owned();
        assert_eq!(response.into_snapshot().err(), Some(WorkerRemoteReadError::Incompatible));
    }

    #[test]
    fn request_validation_covers_world_bounds_and_url_length() {
        let valid = WorkerEquipmentReadRequest {
            world_id: "main".to_owned(),
            equipment_url: "https://hentaiverse.org/equip/300753957/0123456789".to_owned(),
        };
        assert!(valid.validate());
        assert!(
            !WorkerEquipmentReadRequest { world_id: "shared".to_owned(), ..valid.clone() }
                .validate()
        );
        assert!(
            !WorkerEquipmentReadRequest {
                world_id: "main".to_owned(),
                equipment_url: "x".repeat(513)
            }
            .validate()
        );

        let sent = WorkerSentLookupRequest {
            world: "isekai".to_owned(),
            recipient: "Restia·Ashdoll".to_owned(),
            subject: "Withdrawn return".to_owned(),
            external_equipment_id: "317632875".to_owned(),
        };
        assert!(sent.validate());
        assert!(
            !WorkerSentLookupRequest { external_equipment_id: "12a".to_owned(), ..sent.clone() }
                .validate()
        );
        assert!(!WorkerSentLookupRequest { recipient: String::new(), ..sent }.validate());
    }

    #[test]
    fn paused_maps_to_admin_intake_external_access_paused() {
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Paused),
            AdminIntakeError::ExternalAccessPaused
        );
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Unavailable),
            AdminIntakeError::SourceUnavailable
        );
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Invalid),
            AdminIntakeError::InvalidCommand
        );
    }
}
