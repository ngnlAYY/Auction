//! 安全类别闭集；不读取 Display、Debug、source 或错误负载中的文本。

/// 日志和 Worker 指标共用的低基数分类；只能取下列关联常量，调用方字符串无法构造新类别。
/// 各错误类型直接投影为常量，不经字符串往返，新增变体不会静默落入 other。
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

    // 装备来源闭集原因：标签即常量值。
    pub const EQUIPMENT_NAME_INVALID: Self = Self("equipment_name_invalid");
    pub const EQUIPMENT_QUALITY_UNKNOWN: Self = Self("equipment_quality_unknown");
    pub const EQUIPMENT_KIND_UNKNOWN: Self = Self("equipment_kind_unknown");
    pub const EQUIPMENT_CATEGORY_UNKNOWN: Self = Self("equipment_category_unknown");
    pub const EQUIPMENT_CATEGORY_MISMATCH: Self = Self("equipment_category_mismatch");
    pub const EQUIPMENT_LEVEL_INVALID: Self = Self("equipment_level_invalid");
    pub const EQUIPMENT_PAGE_STRUCTURE_INVALID: Self = Self("equipment_page_structure_invalid");
    pub const EQUIPMENT_ATTRIBUTES_INVALID: Self = Self("equipment_attributes_invalid");
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

    // 固定安全类别：标签即常量值。
    pub const ADMIN_AUDIT_ERROR: Self = Self("admin_audit_error");
    pub const ADMIN_CREDENTIAL_ERROR: Self = Self("admin_credential_error");
    pub const ADMIN_FORUM_PREVIEW_ERROR: Self = Self("admin_forum_preview_error");
    pub const ADMIN_INTAKE_ERROR: Self = Self("admin_intake_error");
    pub const ADMIN_LOT_UPDATE_ERROR: Self = Self("admin_lot_update_error");
    pub const ADMIN_MAIL_ERROR: Self = Self("admin_mail_error");
    pub const ADMIN_RULES_ERROR: Self = Self("admin_rules_error");
    pub const ADMIN_RUNTIME_ERROR: Self = Self("admin_runtime_error");
    pub const ADMIN_USER_ERROR: Self = Self("admin_user_error");
    pub const ADMIN_WORLD_ERROR: Self = Self("admin_world_error");
    pub const CAPABILITY_UNAVAILABLE: Self = Self("capability_unavailable");
    pub const COMMAND_CONFLICT: Self = Self("command_conflict");
    pub const COMMAND_LEDGER_INVALID: Self = Self("command_ledger_invalid");
    pub const COMPETITION_REJECTED: Self = Self("competition_rejected");
    pub const COMPETITION_QUEUE_TIMEOUT: Self = Self("competition_queue_timeout");
    pub const CONFIGURATION_INVALID: Self = Self("configuration_invalid");
    pub const DELIVERY_ADMIN_ERROR: Self = Self("delivery_admin_error");
    pub const DELIVERY_ALREADY_DISPOSED: Self = Self("delivery_already_disposed");
    pub const DELIVERY_DISPOSITION_INVALID: Self = Self("delivery_disposition_invalid");
    pub const DELIVERY_NOT_ACKNOWLEDGEABLE: Self = Self("delivery_not_acknowledgeable");
    pub const DELIVERY_NOT_FOUND: Self = Self("delivery_not_found");
    pub const DELIVERY_STALE: Self = Self("delivery_stale");
    pub const DEPENDENCY_MISMATCH: Self = Self("dependency_mismatch");
    pub const DEPENDENCY_UNAVAILABLE: Self = Self("dependency_unavailable");
    pub const DOMAIN_REJECTED: Self = Self("domain_rejected");
    pub const EQUIPMENT_SECRET_NOT_FOUND: Self = Self("equipment_secret_not_found");
    pub const EVIDENCE_EXPIRED: Self = Self("evidence_expired");
    pub const EVIDENCE_NOT_OBSERVED: Self = Self("evidence_not_observed");
    pub const EVIDENCE_REQUEST_INVALID: Self = Self("evidence_request_invalid");
    pub const EVIDENCE_REVOKED: Self = Self("evidence_revoked");
    pub const EVIDENCE_UNAVAILABLE: Self = Self("evidence_unavailable");
    pub const EVIDENCE_UNPROVEN: Self = Self("evidence_unproven");
    pub const EVIDENCE_UNSUPPORTED: Self = Self("evidence_unsupported");
    pub const FORUM_IDENTITY_REJECTED: Self = Self("forum_identity_rejected");
    pub const FUNDING_CONFLICT: Self = Self("funding_conflict");
    pub const FUNDING_INVALID: Self = Self("funding_invalid");
    pub const FUNDING_SERIALIZATION_CONFLICT: Self = Self("funding_serialization_conflict");
    pub const FUNDING_STORAGE_ERROR: Self = Self("funding_storage_error");
    pub const FUNDING_UNAVAILABLE: Self = Self("funding_unavailable");
    pub const FUNDING_UNPROVEN: Self = Self("funding_unproven");
    pub const FUNDING_UNSUPPORTED: Self = Self("funding_unsupported");
    pub const LEASE_EXPIRED: Self = Self("lease_expired");
    pub const MAIL_POLL_STATE_NOT_FOUND: Self = Self("mail_poll_state_not_found");
    pub const MAIL_PROCESSING_ERROR: Self = Self("mail_processing_error");
    pub const MAIL_STATE_CONFLICT: Self = Self("mail_state_conflict");
    pub const OPERATIONS_ALERT_ERROR: Self = Self("operations_alert_error");
    /// 当前没有生产来源：各错误类型直接投影为具体常量，不经字符串回退。
    /// 新增类别须登记新常量，不得以 other 兜底。
    pub const OTHER: Self = Self("other");
    pub const PLAYER_NOTIFICATION_ERROR: Self = Self("player_notification_error");
    pub const REALTIME_ADMIN_ERROR: Self = Self("realtime_admin_error");
    pub const RECOVERY_REJECTED: Self = Self("recovery_rejected");
    pub const REMOTE_BUSY: Self = Self("remote_busy");
    pub const REMOTE_REJECTED: Self = Self("remote_rejected");
    pub const REMOTE_RETRYABLE: Self = Self("remote_retryable");
    pub const REMOTE_WRITE_STARTED: Self = Self("remote_write_started");
    pub const REPOSITORY_CONFLICT: Self = Self("repository_conflict");
    pub const REPOSITORY_FAILED: Self = Self("repository_failed");
    pub const REPOSITORY_SERIALIZATION_CONFLICT: Self = Self("repository_serialization_conflict");
    pub const RULES_REVISION_CONFLICT: Self = Self("rules_revision_conflict");
    pub const RUNTIME_COMMAND_ERROR: Self = Self("runtime_command_error");
    pub const RUNTIME_FILE_ERROR: Self = Self("runtime_file_error");
    pub const SERIALIZATION_FAILED: Self = Self("serialization_failed");
    pub const SESSION_ERROR: Self = Self("session_error");
    pub const SETTLEMENT_NOT_FOUND: Self = Self("settlement_not_found");
    pub const SETTLEMENT_RETURN_REJECTED: Self = Self("settlement_return_rejected");
    pub const TASK_JOIN: Self = Self("task_join");
    pub const TIMEOUT: Self = Self("timeout");
    pub const TRANSACTION_POLICY_INVALID: Self = Self("transaction_policy_invalid");

    /// 返回日志和指标共享的稳定闭集标签。
    pub const fn label(self) -> &'static str {
        self.0
    }
}

mod implementations;

pub use implementations::DiagnosticError;
