//! 安全类别闭集；不读取 Display、Debug、source 或错误负载中的文本。

/// 日志和 Worker 指标共用的低基数分类；未知字符串统一归为 other。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SafeErrorKind(&'static str);

impl SafeErrorKind {
    /// 其他 HTTP 5xx 响应；不读取响应正文。
    pub const HTTP_SERVER_ERROR: Self = Self("http_server_error");

    /// 远端认证拒绝。
    pub const REMOTE_AUTHENTICATION_ERROR: Self = Self("remote_authentication_error");

    /// 远端收件人无效。
    pub const REMOTE_RECIPIENT_INVALID: Self = Self("remote_recipient_invalid");

    /// 远端时间与本地观测偏差。
    pub const CLOCK_SKEW: Self = Self("clock_skew");

    /// 影子候选集合不同。
    pub const SHADOW_MISMATCH: Self = Self("shadow_mismatch");

    /// 装备来源闭集原因：equipment_name_invalid。
    pub const EQUIPMENT_NAME_INVALID: Self = Self("equipment_name_invalid");

    /// 装备来源闭集原因：equipment_quality_unknown。
    pub const EQUIPMENT_QUALITY_UNKNOWN: Self = Self("equipment_quality_unknown");

    /// 装备来源闭集原因：equipment_kind_unknown。
    pub const EQUIPMENT_KIND_UNKNOWN: Self = Self("equipment_kind_unknown");

    /// 装备来源闭集原因：equipment_category_unknown。
    pub const EQUIPMENT_CATEGORY_UNKNOWN: Self = Self("equipment_category_unknown");

    /// 装备来源闭集原因：equipment_category_mismatch。
    pub const EQUIPMENT_CATEGORY_MISMATCH: Self = Self("equipment_category_mismatch");

    /// 装备来源闭集原因：equipment_level_invalid。
    pub const EQUIPMENT_LEVEL_INVALID: Self = Self("equipment_level_invalid");

    /// 装备来源闭集原因：equipment_page_structure_invalid。
    pub const EQUIPMENT_PAGE_STRUCTURE_INVALID: Self = Self("equipment_page_structure_invalid");

    /// 装备来源闭集原因：equipment_attributes_invalid。
    pub const EQUIPMENT_ATTRIBUTES_INVALID: Self = Self("equipment_attributes_invalid");

    /// 装备来源闭集原因：equipment_canonical_mismatch。
    pub const EQUIPMENT_CANONICAL_MISMATCH: Self = Self("equipment_canonical_mismatch");

    /// 运行时开关事实损坏。
    pub const RUNTIME_SWITCH_CORRUPT: Self = Self("runtime_switch_corrupt");

    /// 运行时开关提交结果未知。
    pub const RUNTIME_SWITCH_OUTCOME_UNKNOWN: Self = Self("runtime_switch_outcome_unknown");

    /// Sent 基线闭集理由 InvalidMid。
    pub const SENT_BASELINE_INVALID_MID: Self = Self("sent_baseline_invalid_mid");

    /// Sent 基线闭集理由 OrderViolation。
    pub const SENT_BASELINE_ORDER_VIOLATION: Self = Self("sent_baseline_order_violation");

    /// Sent 基线闭集理由 PageDiscontinuity。
    pub const SENT_BASELINE_PAGE_DISCONTINUITY: Self = Self("sent_baseline_page_discontinuity");

    /// Sent 基线闭集理由 AnchorMissing。
    pub const SENT_BASELINE_ANCHOR_MISSING: Self = Self("sent_baseline_anchor_missing");

    /// Sent 基线闭集理由 AnchorChanged。
    pub const SENT_BASELINE_ANCHOR_CHANGED: Self = Self("sent_baseline_anchor_changed");

    /// Sent 基线闭集理由 PageLimit。
    pub const SENT_BASELINE_PAGE_LIMIT: Self = Self("sent_baseline_page_limit");

    /// Sent 基线闭集理由 Timeout。
    pub const SENT_BASELINE_TIMEOUT: Self = Self("sent_baseline_timeout");

    /// Sent 基线闭集理由 ReadUnavailable。
    pub const SENT_BASELINE_READ_UNAVAILABLE: Self = Self("sent_baseline_read_unavailable");

    /// Sent 基线闭集理由 CacheBoundaryDelta。
    pub const SENT_BASELINE_CACHE_BOUNDARY_DELTA: Self = Self("sent_baseline_cache_boundary_delta");

    /// Sent 基线闭集理由 BoundaryUnavailable。
    pub const SENT_BASELINE_BOUNDARY_UNAVAILABLE: Self = Self("sent_baseline_boundary_unavailable");

    /// Sent 基线闭集理由 ScopeChanged。
    pub const SENT_BASELINE_SCOPE_CHANGED: Self = Self("sent_baseline_scope_changed");

    /// Sent 基线闭集理由 ResponseHintConflict。
    pub const SENT_BASELINE_RESPONSE_HINT_CONFLICT: Self =
        Self("sent_baseline_response_hint_conflict");

    /// Sent 基线闭集理由 Canceled。
    pub const SENT_BASELINE_CANCELED: Self = Self("sent_baseline_canceled");

    /// 本地 I/O 失败。
    pub const IO_ERROR: Self = Self("io_error");

    /// 密码运算失败。
    pub const CRYPTOGRAPHIC_ERROR: Self = Self("cryptographic_error");

    /// 外部响应字段解析失败。
    pub const RESPONSE_PARSE_ERROR: Self = Self("response_parse_error");

    /// 请求或受限头字段编码失败。
    pub const REQUEST_ENCODING_ERROR: Self = Self("request_encoding_error");

    /// 远端正文传输失败。
    pub const REMOTE_BODY_ERROR: Self = Self("remote_body_error");

    /// 远端传输失败。
    pub const REMOTE_TRANSPORT_ERROR: Self = Self("remote_transport_error");

    /// 凭据所有权不可用。
    pub const CREDENTIAL_UNAVAILABLE: Self = Self("credential_unavailable");

    /// 远端会话已失效。
    pub const REMOTE_SESSION_EXPIRED: Self = Self("remote_session_expired");

    /// 远端账号忙。
    pub const REMOTE_ACCOUNT_BUSY: Self = Self("remote_account_busy");

    /// 远端限流。
    pub const REMOTE_RATE_LIMITED: Self = Self("remote_rate_limited");

    /// Sent 扫描页数预算耗尽。
    pub const SENT_SCAN_PAGE_LIMIT: Self = Self("sent_scan_page_limit");

    /// Sent 扫描时间预算耗尽。
    pub const SENT_SCAN_TIMEOUT: Self = Self("sent_scan_timeout");

    /// 未确认持久化前置事实。
    pub const PERSISTENCE_UNAVAILABLE: Self = Self("persistence_unavailable");

    /// 远端响应超限。
    pub const REMOTE_RESPONSE_TOO_LARGE: Self = Self("remote_response_too_large");

    /// 远端协议结构不符。
    pub const REMOTE_PROTOCOL_ERROR: Self = Self("remote_protocol_error");

    /// 远端身份不匹配。
    pub const REMOTE_IDENTITY_MISMATCH: Self = Self("remote_identity_mismatch");

    /// 远端身份未获证明。
    pub const REMOTE_IDENTITY_UNPROVEN: Self = Self("remote_identity_unproven");

    /// 外写结果未知。
    pub const REMOTE_WRITE_UNKNOWN: Self = Self("remote_write_unknown");

    /// 远端交互验证。
    pub const REMOTE_CHALLENGE: Self = Self("remote_challenge");

    /// 远端主题已锁定。
    pub const REMOTE_TOPIC_LOCKED: Self = Self("remote_topic_locked");

    /// 远端拒绝权限。
    pub const REMOTE_PERMISSION_DENIED: Self = Self("remote_permission_denied");

    /// 隔离的任务 panic；只作观测，不代表持久化工作项计次。
    pub const TASK_PANIC: Self = Self("task_panic");

    /// HTTP 内部错误响应。
    pub const HTTP_INTERNAL_ERROR: Self = Self("http_internal_error");

    /// HTTP 上游错误响应。
    pub const HTTP_BAD_GATEWAY: Self = Self("http_bad_gateway");

    /// HTTP 服务暂不可用响应。
    pub const HTTP_SERVICE_UNAVAILABLE: Self = Self("http_service_unavailable");

    /// 固定安全类别：`admin_audit_error`。
    pub const ADMIN_AUDIT_ERROR: Self = Self("admin_audit_error");

    /// 固定安全类别：`admin_credential_error`。
    pub const ADMIN_CREDENTIAL_ERROR: Self = Self("admin_credential_error");

    /// 固定安全类别：`admin_forum_preview_error`。
    pub const ADMIN_FORUM_PREVIEW_ERROR: Self = Self("admin_forum_preview_error");

    /// 固定安全类别：`admin_intake_error`。
    pub const ADMIN_INTAKE_ERROR: Self = Self("admin_intake_error");

    /// 固定安全类别：`admin_lot_update_error`。
    pub const ADMIN_LOT_UPDATE_ERROR: Self = Self("admin_lot_update_error");

    /// 固定安全类别：`admin_mail_error`。
    pub const ADMIN_MAIL_ERROR: Self = Self("admin_mail_error");

    /// 固定安全类别：`admin_rules_error`。
    pub const ADMIN_RULES_ERROR: Self = Self("admin_rules_error");

    /// 固定安全类别：`admin_runtime_error`。
    pub const ADMIN_RUNTIME_ERROR: Self = Self("admin_runtime_error");

    /// 固定安全类别：`admin_user_error`。
    pub const ADMIN_USER_ERROR: Self = Self("admin_user_error");

    /// 固定安全类别：`admin_world_error`。
    pub const ADMIN_WORLD_ERROR: Self = Self("admin_world_error");

    /// 固定安全类别：`capability_unavailable`。
    pub const CAPABILITY_UNAVAILABLE: Self = Self("capability_unavailable");

    /// 固定安全类别：`command_conflict`。
    pub const COMMAND_CONFLICT: Self = Self("command_conflict");

    /// 固定安全类别：`command_ledger_invalid`。
    pub const COMMAND_LEDGER_INVALID: Self = Self("command_ledger_invalid");

    /// 固定安全类别：`competition_rejected`。
    pub const COMPETITION_REJECTED: Self = Self("competition_rejected");

    /// 固定安全类别：`configuration_invalid`。
    pub const CONFIGURATION_INVALID: Self = Self("configuration_invalid");

    /// 固定安全类别：`delivery_admin_error`。
    pub const DELIVERY_ADMIN_ERROR: Self = Self("delivery_admin_error");

    /// 固定安全类别：`dependency_mismatch`。
    pub const DEPENDENCY_MISMATCH: Self = Self("dependency_mismatch");

    /// 固定安全类别：`dependency_unavailable`。
    pub const DEPENDENCY_UNAVAILABLE: Self = Self("dependency_unavailable");

    /// 固定安全类别：`domain_rejected`。
    pub const DOMAIN_REJECTED: Self = Self("domain_rejected");

    /// 固定安全类别：`equipment_secret_not_found`。
    pub const EQUIPMENT_SECRET_NOT_FOUND: Self = Self("equipment_secret_not_found");

    /// 固定安全类别：`evidence_expired`。
    pub const EVIDENCE_EXPIRED: Self = Self("evidence_expired");

    /// 固定安全类别：`evidence_not_observed`。
    pub const EVIDENCE_NOT_OBSERVED: Self = Self("evidence_not_observed");

    /// 固定安全类别：`evidence_request_invalid`。
    pub const EVIDENCE_REQUEST_INVALID: Self = Self("evidence_request_invalid");

    /// 固定安全类别：`evidence_revoked`。
    pub const EVIDENCE_REVOKED: Self = Self("evidence_revoked");

    /// 固定安全类别：`evidence_unavailable`。
    pub const EVIDENCE_UNAVAILABLE: Self = Self("evidence_unavailable");

    /// 固定安全类别：`evidence_unproven`。
    pub const EVIDENCE_UNPROVEN: Self = Self("evidence_unproven");

    /// 固定安全类别：`evidence_unsupported`。
    pub const EVIDENCE_UNSUPPORTED: Self = Self("evidence_unsupported");

    /// 固定安全类别：`forum_identity_rejected`。
    pub const FORUM_IDENTITY_REJECTED: Self = Self("forum_identity_rejected");

    /// 固定安全类别：`funding_conflict`。
    pub const FUNDING_CONFLICT: Self = Self("funding_conflict");

    /// 固定安全类别：`funding_invalid`。
    pub const FUNDING_INVALID: Self = Self("funding_invalid");

    /// 固定安全类别：`funding_serialization_conflict`。
    pub const FUNDING_SERIALIZATION_CONFLICT: Self = Self("funding_serialization_conflict");

    /// 固定安全类别：`funding_storage_error`。
    pub const FUNDING_STORAGE_ERROR: Self = Self("funding_storage_error");

    /// 固定安全类别：`funding_unavailable`。
    pub const FUNDING_UNAVAILABLE: Self = Self("funding_unavailable");

    /// 固定安全类别：`funding_unproven`。
    pub const FUNDING_UNPROVEN: Self = Self("funding_unproven");

    /// 固定安全类别：`funding_unsupported`。
    pub const FUNDING_UNSUPPORTED: Self = Self("funding_unsupported");

    /// 固定安全类别：`lease_expired`。
    pub const LEASE_EXPIRED: Self = Self("lease_expired");

    /// 固定安全类别：`mail_poll_state_not_found`。
    pub const MAIL_POLL_STATE_NOT_FOUND: Self = Self("mail_poll_state_not_found");

    /// 固定安全类别：`mail_processing_error`。
    pub const MAIL_PROCESSING_ERROR: Self = Self("mail_processing_error");

    /// 固定安全类别：`mail_state_conflict`。
    pub const MAIL_STATE_CONFLICT: Self = Self("mail_state_conflict");

    /// 固定安全类别：`operations_alert_error`。
    pub const OPERATIONS_ALERT_ERROR: Self = Self("operations_alert_error");

    /// 固定安全类别：`other`。
    pub const OTHER: Self = Self("other");

    /// 固定安全类别：`realtime_admin_error`。
    pub const REALTIME_ADMIN_ERROR: Self = Self("realtime_admin_error");

    /// 固定安全类别：`recovery_rejected`。
    pub const RECOVERY_REJECTED: Self = Self("recovery_rejected");

    /// 固定安全类别：`remote_busy`。
    pub const REMOTE_BUSY: Self = Self("remote_busy");

    /// 固定安全类别：`remote_rejected`。
    pub const REMOTE_REJECTED: Self = Self("remote_rejected");

    /// 固定安全类别：`remote_retryable`。
    pub const REMOTE_RETRYABLE: Self = Self("remote_retryable");

    /// 固定安全类别：`remote_write_started`。
    pub const REMOTE_WRITE_STARTED: Self = Self("remote_write_started");

    /// 固定安全类别：`repository_conflict`。
    pub const REPOSITORY_CONFLICT: Self = Self("repository_conflict");

    /// 固定安全类别：`repository_failed`。
    pub const REPOSITORY_FAILED: Self = Self("repository_failed");

    /// 固定安全类别：`repository_serialization_conflict`。
    pub const REPOSITORY_SERIALIZATION_CONFLICT: Self = Self("repository_serialization_conflict");

    /// 固定安全类别：`rules_revision_conflict`。
    pub const RULES_REVISION_CONFLICT: Self = Self("rules_revision_conflict");

    /// 固定安全类别：`runtime_command_error`。
    pub const RUNTIME_COMMAND_ERROR: Self = Self("runtime_command_error");

    /// 固定安全类别：`runtime_file_error`。
    pub const RUNTIME_FILE_ERROR: Self = Self("runtime_file_error");

    /// 固定安全类别：`serialization_failed`。
    pub const SERIALIZATION_FAILED: Self = Self("serialization_failed");

    /// 固定安全类别：`session_error`。
    pub const SESSION_ERROR: Self = Self("session_error");

    /// 固定安全类别：`settlement_not_found`。
    pub const SETTLEMENT_NOT_FOUND: Self = Self("settlement_not_found");

    /// 固定安全类别：`task_join`。
    pub const TASK_JOIN: Self = Self("task_join");

    /// 固定安全类别：`timeout`。
    pub const TIMEOUT: Self = Self("timeout");

    /// 固定安全类别：`transaction_policy_invalid`。
    pub const TRANSACTION_POLICY_INVALID: Self = Self("transaction_policy_invalid");

    /// 只接受已登记类别，不把调用方字符串直接放进日志或指标。
    pub fn from_label(label: &str) -> Self {
        match label {
            "http_server_error" => Self::HTTP_SERVER_ERROR,
            "remote_authentication_error" => Self::REMOTE_AUTHENTICATION_ERROR,
            "remote_recipient_invalid" => Self::REMOTE_RECIPIENT_INVALID,
            "clock_skew" => Self::CLOCK_SKEW,
            "shadow_mismatch" => Self::SHADOW_MISMATCH,

            "equipment_name_invalid" => Self::EQUIPMENT_NAME_INVALID,
            "equipment_quality_unknown" => Self::EQUIPMENT_QUALITY_UNKNOWN,
            "equipment_kind_unknown" => Self::EQUIPMENT_KIND_UNKNOWN,
            "equipment_category_unknown" => Self::EQUIPMENT_CATEGORY_UNKNOWN,
            "equipment_category_mismatch" => Self::EQUIPMENT_CATEGORY_MISMATCH,
            "equipment_level_invalid" => Self::EQUIPMENT_LEVEL_INVALID,
            "equipment_page_structure_invalid" => Self::EQUIPMENT_PAGE_STRUCTURE_INVALID,
            "equipment_attributes_invalid" => Self::EQUIPMENT_ATTRIBUTES_INVALID,
            "equipment_canonical_mismatch" => Self::EQUIPMENT_CANONICAL_MISMATCH,

            "runtime_switch_corrupt" => Self::RUNTIME_SWITCH_CORRUPT,
            "runtime_switch_outcome_unknown" => Self::RUNTIME_SWITCH_OUTCOME_UNKNOWN,
            "sent_baseline_invalid_mid" => Self::SENT_BASELINE_INVALID_MID,
            "sent_baseline_order_violation" => Self::SENT_BASELINE_ORDER_VIOLATION,
            "sent_baseline_page_discontinuity" => Self::SENT_BASELINE_PAGE_DISCONTINUITY,
            "sent_baseline_anchor_missing" => Self::SENT_BASELINE_ANCHOR_MISSING,
            "sent_baseline_anchor_changed" => Self::SENT_BASELINE_ANCHOR_CHANGED,
            "sent_baseline_page_limit" => Self::SENT_BASELINE_PAGE_LIMIT,
            "sent_baseline_timeout" => Self::SENT_BASELINE_TIMEOUT,
            "sent_baseline_read_unavailable" => Self::SENT_BASELINE_READ_UNAVAILABLE,
            "sent_baseline_cache_boundary_delta" => Self::SENT_BASELINE_CACHE_BOUNDARY_DELTA,
            "sent_baseline_boundary_unavailable" => Self::SENT_BASELINE_BOUNDARY_UNAVAILABLE,
            "sent_baseline_scope_changed" => Self::SENT_BASELINE_SCOPE_CHANGED,
            "sent_baseline_response_hint_conflict" => Self::SENT_BASELINE_RESPONSE_HINT_CONFLICT,
            "sent_baseline_canceled" => Self::SENT_BASELINE_CANCELED,

            "io_error" => Self::IO_ERROR,
            "cryptographic_error" => Self::CRYPTOGRAPHIC_ERROR,
            "response_parse_error" => Self::RESPONSE_PARSE_ERROR,
            "request_encoding_error" => Self::REQUEST_ENCODING_ERROR,
            "remote_body_error" => Self::REMOTE_BODY_ERROR,
            "remote_transport_error" => Self::REMOTE_TRANSPORT_ERROR,
            "credential_unavailable" => Self::CREDENTIAL_UNAVAILABLE,
            "remote_session_expired" => Self::REMOTE_SESSION_EXPIRED,
            "remote_account_busy" => Self::REMOTE_ACCOUNT_BUSY,
            "remote_rate_limited" => Self::REMOTE_RATE_LIMITED,
            "sent_scan_page_limit" => Self::SENT_SCAN_PAGE_LIMIT,
            "sent_scan_timeout" => Self::SENT_SCAN_TIMEOUT,
            "persistence_unavailable" => Self::PERSISTENCE_UNAVAILABLE,
            "remote_response_too_large" => Self::REMOTE_RESPONSE_TOO_LARGE,
            "remote_protocol_error" => Self::REMOTE_PROTOCOL_ERROR,
            "remote_identity_mismatch" => Self::REMOTE_IDENTITY_MISMATCH,
            "remote_identity_unproven" => Self::REMOTE_IDENTITY_UNPROVEN,
            "remote_write_unknown" => Self::REMOTE_WRITE_UNKNOWN,
            "remote_challenge" => Self::REMOTE_CHALLENGE,
            "remote_topic_locked" => Self::REMOTE_TOPIC_LOCKED,
            "remote_permission_denied" => Self::REMOTE_PERMISSION_DENIED,
            "task_panic" => Self::TASK_PANIC,

            "http_internal_error" => Self::HTTP_INTERNAL_ERROR,
            "http_bad_gateway" => Self::HTTP_BAD_GATEWAY,
            "http_service_unavailable" => Self::HTTP_SERVICE_UNAVAILABLE,

            "admin_audit_error" => Self::ADMIN_AUDIT_ERROR,
            "admin_credential_error" => Self::ADMIN_CREDENTIAL_ERROR,
            "admin_forum_preview_error" => Self::ADMIN_FORUM_PREVIEW_ERROR,
            "admin_intake_error" => Self::ADMIN_INTAKE_ERROR,
            "admin_lot_update_error" => Self::ADMIN_LOT_UPDATE_ERROR,
            "admin_mail_error" => Self::ADMIN_MAIL_ERROR,
            "admin_rules_error" => Self::ADMIN_RULES_ERROR,
            "admin_runtime_error" => Self::ADMIN_RUNTIME_ERROR,
            "admin_user_error" => Self::ADMIN_USER_ERROR,
            "admin_world_error" => Self::ADMIN_WORLD_ERROR,
            "capability_unavailable" => Self::CAPABILITY_UNAVAILABLE,
            "command_conflict" => Self::COMMAND_CONFLICT,
            "command_ledger_invalid" => Self::COMMAND_LEDGER_INVALID,
            "competition_rejected" => Self::COMPETITION_REJECTED,
            "configuration_invalid" => Self::CONFIGURATION_INVALID,
            "delivery_admin_error" => Self::DELIVERY_ADMIN_ERROR,
            "dependency_mismatch" => Self::DEPENDENCY_MISMATCH,
            "dependency_unavailable" => Self::DEPENDENCY_UNAVAILABLE,
            "domain_rejected" => Self::DOMAIN_REJECTED,
            "equipment_secret_not_found" => Self::EQUIPMENT_SECRET_NOT_FOUND,
            "evidence_expired" => Self::EVIDENCE_EXPIRED,
            "evidence_not_observed" => Self::EVIDENCE_NOT_OBSERVED,
            "evidence_request_invalid" => Self::EVIDENCE_REQUEST_INVALID,
            "evidence_revoked" => Self::EVIDENCE_REVOKED,
            "evidence_unavailable" => Self::EVIDENCE_UNAVAILABLE,
            "evidence_unproven" => Self::EVIDENCE_UNPROVEN,
            "evidence_unsupported" => Self::EVIDENCE_UNSUPPORTED,
            "forum_identity_rejected" => Self::FORUM_IDENTITY_REJECTED,
            "funding_conflict" => Self::FUNDING_CONFLICT,
            "funding_invalid" => Self::FUNDING_INVALID,
            "funding_serialization_conflict" => Self::FUNDING_SERIALIZATION_CONFLICT,
            "funding_storage_error" => Self::FUNDING_STORAGE_ERROR,
            "funding_unavailable" => Self::FUNDING_UNAVAILABLE,
            "funding_unproven" => Self::FUNDING_UNPROVEN,
            "funding_unsupported" => Self::FUNDING_UNSUPPORTED,
            "lease_expired" => Self::LEASE_EXPIRED,
            "mail_poll_state_not_found" => Self::MAIL_POLL_STATE_NOT_FOUND,
            "mail_processing_error" => Self::MAIL_PROCESSING_ERROR,
            "mail_state_conflict" => Self::MAIL_STATE_CONFLICT,
            "operations_alert_error" => Self::OPERATIONS_ALERT_ERROR,
            "other" => Self::OTHER,
            "realtime_admin_error" => Self::REALTIME_ADMIN_ERROR,
            "recovery_rejected" => Self::RECOVERY_REJECTED,
            "remote_busy" => Self::REMOTE_BUSY,
            "remote_rejected" => Self::REMOTE_REJECTED,
            "remote_retryable" => Self::REMOTE_RETRYABLE,
            "remote_write_started" => Self::REMOTE_WRITE_STARTED,
            "repository_conflict" => Self::REPOSITORY_CONFLICT,
            "repository_failed" => Self::REPOSITORY_FAILED,
            "repository_serialization_conflict" => Self::REPOSITORY_SERIALIZATION_CONFLICT,
            "rules_revision_conflict" => Self::RULES_REVISION_CONFLICT,
            "runtime_command_error" => Self::RUNTIME_COMMAND_ERROR,
            "runtime_file_error" => Self::RUNTIME_FILE_ERROR,
            "serialization_failed" => Self::SERIALIZATION_FAILED,
            "session_error" => Self::SESSION_ERROR,
            "settlement_not_found" => Self::SETTLEMENT_NOT_FOUND,
            "task_join" => Self::TASK_JOIN,
            "timeout" => Self::TIMEOUT,
            "transaction_policy_invalid" => Self::TRANSACTION_POLICY_INVALID,
            _ => Self::OTHER,
        }
    }

    /// 返回日志和指标共享的稳定闭集标签。
    pub const fn label(self) -> &'static str {
        self.0
    }
}

/// 只从类型化变体投影，不格式化错误；记录 helper 只接收投影结果。
pub trait DiagnosticError {
    fn diagnostic_kind(&self) -> SafeErrorKind;
}

impl DiagnosticError for crate::ApplicationError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for crate::InternalEvidenceError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for crate::FundingError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for serde_json::Error {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::SERIALIZATION_FAILED
    }
}

impl DiagnosticError for auction_domain::DomainError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for std::num::TryFromIntError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for std::io::Error {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for chrono::OutOfRangeError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::CONFIGURATION_INVALID
    }
}

impl DiagnosticError for chrono_tz::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for url::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::CONFIGURATION_INVALID
    }
}

impl DiagnosticError for crate::SessionError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::SESSION_ERROR
    }
}

impl DiagnosticError for crate::LotWatchError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Transaction(error) => error.diagnostic_kind(),
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            Self::Invalid | Self::NotFound | Self::TooMany => SafeErrorKind::DOMAIN_REJECTED,
        }
    }
}

impl DiagnosticError for crate::ForumPmError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::RateLimited => SafeErrorKind::REMOTE_RETRYABLE,
            Self::OutcomeUnknown => SafeErrorKind::REMOTE_WRITE_STARTED,
            Self::Authentication | Self::InvalidRecipient | Self::Protocol => {
                SafeErrorKind::REMOTE_REJECTED
            }
        }
    }
}

impl DiagnosticError for crate::MailDiagnosticError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for crate::RepositoryError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for crate::AdminRuntimeConfigError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::ADMIN_RUNTIME_ERROR
    }
}

impl DiagnosticError for crate::AdminRuntimeFileError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::RUNTIME_FILE_ERROR
    }
}

impl DiagnosticError for crate::AdminRuntimeCommandError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::RUNTIME_COMMAND_ERROR
    }
}

impl<T> DiagnosticError for std::sync::PoisonError<T> {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for chrono::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for crate::AdminIntakeError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::ADMIN_INTAKE_ERROR
    }
}

impl DiagnosticError for crate::CompetitionTransactionError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        use crate::CompetitionTransactionError as Error;
        match self {
            Error::Failed(_) => SafeErrorKind::REPOSITORY_FAILED,
            Error::SerializationConflict => SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT,
            Error::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            _ => SafeErrorKind::COMPETITION_REJECTED,
        }
    }
}

impl DiagnosticError for crate::OperationsAlertError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::OPERATIONS_ALERT_ERROR
    }
}

impl DiagnosticError for crate::RuntimeSwitchError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Invalid => SafeErrorKind::DOMAIN_REJECTED,
            Self::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            Self::Corrupt => SafeErrorKind::RUNTIME_SWITCH_CORRUPT,
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::OutcomeUnknown => SafeErrorKind::RUNTIME_SWITCH_OUTCOME_UNKNOWN,
        }
    }
}

impl DiagnosticError for crate::SentBaselineReason {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::InvalidMid => SafeErrorKind::SENT_BASELINE_INVALID_MID,
            Self::OrderViolation => SafeErrorKind::SENT_BASELINE_ORDER_VIOLATION,
            Self::PageDiscontinuity => SafeErrorKind::SENT_BASELINE_PAGE_DISCONTINUITY,
            Self::AnchorMissing => SafeErrorKind::SENT_BASELINE_ANCHOR_MISSING,
            Self::AnchorChanged => SafeErrorKind::SENT_BASELINE_ANCHOR_CHANGED,
            Self::PageLimit => SafeErrorKind::SENT_BASELINE_PAGE_LIMIT,
            Self::Timeout => SafeErrorKind::SENT_BASELINE_TIMEOUT,
            Self::ReadUnavailable => SafeErrorKind::SENT_BASELINE_READ_UNAVAILABLE,
            Self::CacheBoundaryDelta => SafeErrorKind::SENT_BASELINE_CACHE_BOUNDARY_DELTA,
            Self::BoundaryUnavailable => SafeErrorKind::SENT_BASELINE_BOUNDARY_UNAVAILABLE,
            Self::ScopeChanged => SafeErrorKind::SENT_BASELINE_SCOPE_CHANGED,
            Self::ResponseHintConflict => SafeErrorKind::SENT_BASELINE_RESPONSE_HINT_CONFLICT,
            Self::Canceled => SafeErrorKind::SENT_BASELINE_CANCELED,
        }
    }
}

impl DiagnosticError for crate::EquipmentSourceDetailCode {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::InvalidName => SafeErrorKind::EQUIPMENT_NAME_INVALID,
            Self::UnknownQuality => SafeErrorKind::EQUIPMENT_QUALITY_UNKNOWN,
            Self::UnknownKind => SafeErrorKind::EQUIPMENT_KIND_UNKNOWN,
            Self::UnknownCategory => SafeErrorKind::EQUIPMENT_CATEGORY_UNKNOWN,
            Self::CategoryMismatch => SafeErrorKind::EQUIPMENT_CATEGORY_MISMATCH,
            Self::InvalidLevel => SafeErrorKind::EQUIPMENT_LEVEL_INVALID,
            Self::PageStructure => SafeErrorKind::EQUIPMENT_PAGE_STRUCTURE_INVALID,
            Self::InvalidAttributes => SafeErrorKind::EQUIPMENT_ATTRIBUTES_INVALID,
            Self::CanonicalMismatch => SafeErrorKind::EQUIPMENT_CANONICAL_MISMATCH,
        }
    }
}
