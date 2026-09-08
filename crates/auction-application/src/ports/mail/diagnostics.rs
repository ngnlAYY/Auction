//! MoogleMail 敏感诊断证据的窄端口。

use std::{future::Future, pin::Pin};

use uuid::Uuid;

use super::mail_processing::MailMessage;

/// 诊断记录所处阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDiagnosticStage {
    Transport,
    Parse,
    Business,
}

/// 诊断记录上下文；字段只能包含关联标识，不能包含 Cookie 或 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailDiagnosticContext {
    pub stage: MailDiagnosticStage,
    pub operation: String,
    pub world_id: String,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
}

/// 业务决定及安全失败分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailDiagnosticOutcome {
    pub failure_code: String,
    pub decision_code: Option<String>,
}

/// 已经加密、可异步持久化的证据；此类型不持有正文明文。
#[derive(PartialEq, Eq)]
pub struct PreparedMailEvidence {
    pub(crate) evidence_id: Uuid,
    pub(crate) context: MailDiagnosticContext,
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) plaintext_bytes: usize,
    pub(crate) sha256: String,
    pub(crate) payload_kind: String,
    pub(crate) status: Option<u16>,
    pub(crate) body_available: bool,
}

impl PreparedMailEvidence {
    /// 封装已由适配器加密的业务邮件；归档时仍须认证所有元数据和载荷摘要。
    pub fn new(
        evidence_id: Uuid,
        context: MailDiagnosticContext,
        ciphertext: Vec<u8>,
        plaintext_bytes: usize,
        sha256: String,
    ) -> Self {
        Self {
            evidence_id,
            context,
            ciphertext,
            plaintext_bytes,
            sha256,
            payload_kind: "parsed_message".to_owned(),
            status: None,
            body_available: true,
        }
    }

    /// 将密文标记为原始响应；空响应与未取得正文的区别必须进入认证数据。
    pub fn with_response_metadata(mut self, status: Option<u16>, body_available: bool) -> Self {
        self.payload_kind = "raw_response".to_owned();
        self.status = status;
        self.body_available = body_available;
        self
    }

    /// 返回安全的证据关联编号。
    pub fn evidence_id(&self) -> Uuid {
        self.evidence_id
    }
    /// 返回不含正文的处理上下文。
    pub fn context(&self) -> &MailDiagnosticContext {
        &self.context
    }
    /// 仅供归档适配器认证和重新封装，不允许写入普通日志。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
    /// 返回原始载荷字节数。
    pub fn plaintext_bytes(&self) -> usize {
        self.plaintext_bytes
    }
    /// 返回原始载荷的稳定 SHA-256。
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// 区分原始响应和已经提取的邮件字段。
    pub fn payload_kind(&self) -> &str {
        &self.payload_kind
    }
    /// 返回实际观察到的 HTTP 状态；未知时为 None。
    pub fn status(&self) -> Option<u16> {
        self.status
    }
    /// 区分未取得响应正文与实际取得零字节正文。
    pub fn body_available(&self) -> bool {
        self.body_available
    }
}

/// 归档后的安全回执；不携带证据正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailDiagnosticReceipt {
    pub evidence_id: Uuid,
    pub plaintext_bytes: usize,
    pub sha256: String,
    pub body_available: bool,
}

/// 诊断错误只允许输出静态分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDiagnosticError {
    InvalidContext,
    InvalidOutcome,
    InvalidMessage,
    PayloadTooLarge,
    BodyUnavailable,
    Encryption,
    Serialization,
    Io,
    Capacity,
    NotFound,
    Authentication,
    UnsupportedPlatform,
}

impl MailDiagnosticError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidContext => "invalid_context",
            Self::InvalidOutcome => "invalid_outcome",
            Self::InvalidMessage => "invalid_message",
            Self::PayloadTooLarge => "payload_too_large",
            Self::BodyUnavailable => "body_unavailable",
            Self::Encryption => "encryption",
            Self::Serialization => "serialization",
            Self::Io => "io",
            Self::Capacity => "capacity",
            Self::NotFound => "not_found",
            Self::Authentication => "authentication",
            Self::UnsupportedPlatform => "unsupported_platform",
        }
    }
}

pub type MailDiagnosticFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, MailDiagnosticError>> + Send + 'a>>;

/// 敏感邮件诊断证据记录器。
pub trait MailDiagnosticRecorder: Send + Sync {
    fn prepare_message(
        &self,
        context: MailDiagnosticContext,
        message: &MailMessage,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError>;

    fn retain(
        &self,
        prepared: PreparedMailEvidence,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt>;

    fn retain_response(
        &self,
        context: MailDiagnosticContext,
        status: Option<u16>,
        body: Option<&[u8]>,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt>;
}
