//! 装备读取失败的非权威诊断、敏感证据句柄和有界结果投影。

use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AdminIntakeSourceSnapshot, PreparedMailEvidence};
use crate::ApplicationError;

#[path = "equipment_diagnostics/reason.rs"]
mod reason;

pub use reason::EquipmentSourceDetailCode;

/// 单次装备读取的安全关联字段；附件位置从一开始，不包含装备 key 或 URL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailEquipmentDiagnosticContext {
    pub mailbox_key: String,
    pub world_id: String,
    pub external_mail_id: String,
    pub external_equipment_id: String,
    pub attachment_position: u16,
}

impl MailEquipmentDiagnosticContext {
    /// 在发送请求前核对有界身份，防止把非关联文本写入日志或证据头。
    pub fn validate(&self) -> bool {
        safe_text(&self.mailbox_key, 256)
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && safe_text(&self.external_mail_id, 512)
            && equipment_id(&self.external_equipment_id)
            && (1..=1000).contains(&self.attachment_position)
    }
}

/// 首个装备失败发生的处理阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailEquipmentDiagnosticStage {
    Request,
    Response,
    Parse,
    Validate,
}

impl MailEquipmentDiagnosticStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::Parse => "parse",
            Self::Validate => "validate",
        }
    }
}

/// 可公开给管理员的低基数失败分类，不携带驱动错误原文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailEquipmentDiagnosticCategory {
    Configuration,
    CredentialUnavailable,
    SessionExpired,
    Timeout,
    Network,
    Redirect,
    ResponseTooLarge,
    ContentEncoding,
    RateLimited,
    ServerError,
    HttpStatus,
    ContentType,
    ProtocolDrift,
    IdentityMismatch,
    SnapshotInvalid,
    FilterRejected,
}

impl MailEquipmentDiagnosticCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::SessionExpired => "session_expired",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Redirect => "redirect",
            Self::ResponseTooLarge => "response_too_large",
            Self::ContentEncoding => "content_encoding",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::HttpStatus => "http_status",
            Self::ContentType => "content_type",
            Self::ProtocolDrift => "protocol_drift",
            Self::IdentityMismatch => "identity_mismatch",
            Self::SnapshotInvalid => "snapshot_invalid",
            Self::FilterRejected => "filter_rejected",
        }
    }
}

/// 证据留存状态；Prepared 仅用于进程内，不允许进入数据库或 DTO。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailEquipmentEvidenceStatus {
    Prepared,
    Retained,
    BodyUnavailable,
    SaveFailed,
    CompletionUnconfirmed,
    PrepareFailed,
    Disabled,
}

impl MailEquipmentEvidenceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Retained => "retained",
            Self::BodyUnavailable => "body_unavailable",
            Self::SaveFailed => "save_failed",
            Self::CompletionUnconfirmed => "completion_unconfirmed",
            Self::PrepareFailed => "prepare_failed",
            Self::Disabled => "disabled",
        }
    }
}

/// 首次提交的安全失败投影；不得参与业务摘要、幂等比较或轮询游标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailEquipmentDiagnosticSnapshot {
    pub schema_version: u8,
    pub equipment_id: String,
    pub attachment_position: u16,
    pub stage: MailEquipmentDiagnosticStage,
    pub category: MailEquipmentDiagnosticCategory,
    pub detail_code: Option<String>,
    pub http_status: Option<u16>,
    pub elapsed_millis: Option<u64>,
    pub evidence_id: Option<Uuid>,
    pub evidence_status: MailEquipmentEvidenceStatus,
    pub retention_error: Option<String>,
}

impl MailEquipmentDiagnosticSnapshot {
    /// 校验版本、枚举和留存状态矩阵，禁止任意说明文本或敏感内容进入投影。
    pub fn validate(&self) -> bool {
        let detail_valid = match (self.category, self.detail_code.as_deref()) {
            (MailEquipmentDiagnosticCategory::ProtocolDrift, Some(value))
                if self.stage == MailEquipmentDiagnosticStage::Parse =>
            {
                EquipmentSourceDetailCode::from_code(value).is_some()
            }
            (MailEquipmentDiagnosticCategory::SnapshotInvalid, Some(value)) => matches!(
                value,
                "adapter_invalid"
                    | "fingerprint_invalid"
                    | "equipment_id_invalid"
                    | "name_invalid"
                    | "category_invalid"
                    | "level_invalid"
                    | "collected_at_invalid"
                    | "collected_at_future"
                    | "attributes_invalid"
            ),
            (MailEquipmentDiagnosticCategory::FilterRejected, Some(value)) => matches!(
                value,
                "quality_not_allowed"
                    | "prefix_not_allowed"
                    | "suffix_not_allowed"
                    | "category_not_allowed"
                    | "equipment_type_not_allowed"
                    | "level_not_allowed"
                    | "attributes_invalid"
                    | "tradeable_required"
            ),
            (_, None) => true,
            _ => false,
        };
        let evidence_valid = match self.evidence_status {
            MailEquipmentEvidenceStatus::Prepared => false,
            MailEquipmentEvidenceStatus::Retained
            | MailEquipmentEvidenceStatus::BodyUnavailable => {
                self.evidence_id.is_some_and(|id| !id.is_nil()) && self.retention_error.is_none()
            }
            MailEquipmentEvidenceStatus::SaveFailed
            | MailEquipmentEvidenceStatus::CompletionUnconfirmed => {
                self.evidence_id.is_some_and(|id| !id.is_nil()) && self.retention_error.is_some()
            }
            MailEquipmentEvidenceStatus::PrepareFailed => {
                self.evidence_id.is_none() && self.retention_error.is_some()
            }
            MailEquipmentEvidenceStatus::Disabled => {
                self.evidence_id.is_none() && self.retention_error.is_none()
            }
        };
        self.schema_version == 1
            && equipment_id(&self.equipment_id)
            && (1..=1000).contains(&self.attachment_position)
            && self.http_status.is_none_or(|status| (100..=599).contains(&status))
            && self.elapsed_millis.is_none_or(|elapsed| elapsed <= 300_000)
            && detail_valid
            && evidence_valid
            && self.retention_error.as_deref().is_none_or(safe_error_code)
            && serde_json::to_vec(self).is_ok_and(|value| value.len() <= 4096)
    }
}

/// 读取期间的诊断上下文；只持有已加密响应，成功读取后可直接丢弃。
pub struct MailEquipmentObservation {
    pub context: MailEquipmentDiagnosticContext,
    pub stage: MailEquipmentDiagnosticStage,
    pub category: MailEquipmentDiagnosticCategory,
    pub source_detail_code: Option<EquipmentSourceDetailCode>,
    pub http_status: Option<u16>,
    pub elapsed_millis: Option<u64>,
    pub prepared: Option<PreparedMailEvidence>,
    pub preparation_status: MailEquipmentEvidenceStatus,
    pub preparation_error: Option<String>,
}

impl MailEquipmentObservation {
    /// 为未接入诊断扩展的既有读取器提供无证据观察，不改变其读取结果。
    pub fn unavailable(context: MailEquipmentDiagnosticContext) -> Self {
        Self {
            context,
            stage: MailEquipmentDiagnosticStage::Request,
            category: MailEquipmentDiagnosticCategory::ProtocolDrift,
            source_detail_code: None,
            http_status: None,
            elapsed_millis: None,
            prepared: None,
            preparation_status: MailEquipmentEvidenceStatus::Disabled,
            preparation_error: None,
        }
    }
}

/// 读取结果与诊断句柄共同返回，读取错误不能被日志格式化丢失或输出原文。
pub enum MailEquipmentReadOutcome {
    Loaded { snapshot: AdminIntakeSourceSnapshot, observation: MailEquipmentObservation },
    Failed { error: ApplicationError, observation: MailEquipmentObservation },
}

pub type MailEquipmentReadFuture<'a> =
    Pin<Box<dyn Future<Output = MailEquipmentReadOutcome> + Send + 'a>>;
pub type MailEquipmentRetentionFuture<'a> =
    Pin<Box<dyn Future<Output = MailEquipmentDiagnosticSnapshot> + Send + 'a>>;

/// 已冻结业务结果后的留证任务；由 Worker 在业务超时边界外执行。
pub struct PendingMailEquipmentDiagnostic {
    future: MailEquipmentRetentionFuture<'static>,
}

impl PendingMailEquipmentDiagnostic {
    pub(crate) fn new(future: MailEquipmentRetentionFuture<'static>) -> Self {
        Self { future }
    }

    /// 消费唯一句柄，完成有界留证并返回安全结果，不重新执行业务命令。
    pub async fn resolve(self) -> MailEquipmentDiagnosticSnapshot {
        self.future.await
    }
}

fn equipment_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn safe_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn safe_error_code(value: &str) -> bool {
    matches!(
        value,
        "invalid_context"
            | "invalid_outcome"
            | "invalid_message"
            | "payload_too_large"
            | "body_unavailable"
            | "encryption"
            | "serialization"
            | "io"
            | "capacity"
            | "not_found"
            | "authentication"
            | "unsupported_platform"
            | "retention_timeout"
            | "retention_cancelled"
            | "recorder_unavailable"
    )
}
