//! 告警台的只读投影、操作者确认/静默命令与待办计数。
//!
//! 告警台不是第二套状态机：活动/历史状态仍由 `operations_alert_signals` 的水位拥有，
//! 确认与静默只追加操作者事实。静默只把该 signal 的 `next_notify_at` 推到到期时间，
//! 不改变 kind/scope/source/count/age/active，也不跳过阈值评估；到期后重复通知自然恢复。

use self::parent::*;
use crate::ports::operations::alert as parent;

/// 告警台单页最大条目数；与 Admin 其它列表保持同一上限。
pub const MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE: i64 = 100;
/// 告警台允许跳过的最大条目数；超过即拒绝，避免深分页扫描整张水位表。
pub const MAX_OPERATIONS_ALERT_CONSOLE_OFFSET: i64 = 100_000;
/// 静默的最短秒数：少于该值属于误触而不是运维静默。
pub const MIN_OPERATIONS_ALERT_SILENCE_SECONDS: i64 = 60;
/// 静默的最长秒数：7 天；更长会掩盖下一轮正常运维节奏。
pub const MAX_OPERATIONS_ALERT_SILENCE_SECONDS: i64 = 7 * 24 * 3_600;

/// 告警台列表筛选闭集。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OperationsAlertFilter {
    #[default]
    Active,
    Resolved,
    All,
}

impl OperationsAlertFilter {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Resolved => "resolved",
            Self::All => "all",
        }
    }

    /// 只接受闭集；未知值由 HTTP 边界映射为参数错误，不回退到“全部”。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "resolved" => Some(Self::Resolved),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// 返回 SQL 需要的活动筛选；`All` 为 `None`。
    pub const fn active(self) -> Option<bool> {
        match self {
            Self::Active => Some(true),
            Self::Resolved => Some(false),
            Self::All => None,
        }
    }
}

/// 告警台列表查询；分页为数字页语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertConsoleQuery {
    pub filter: OperationsAlertFilter,
    pub source: Option<OperationsAlertSource>,
    pub page: i64,
    pub page_size: i64,
}

impl OperationsAlertConsoleQuery {
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.page < 1
            || self.page_size < 1
            || self.page_size > MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE
        {
            return Err(OperationsAlertError::Invalid);
        }
        let offset = self
            .page
            .checked_sub(1)
            .and_then(|page| page.checked_mul(self.page_size))
            .ok_or(OperationsAlertError::Invalid)?;
        if offset > MAX_OPERATIONS_ALERT_CONSOLE_OFFSET {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }

    pub fn offset(&self) -> Result<i64, OperationsAlertError> {
        self.validate()?;
        Ok((self.page - 1) * self.page_size)
    }
}

/// 一条告警台条目：signal 水位 + 当前确认/静默投影 + 处置手册锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertConsoleEntry {
    pub fingerprint: [u8; 32],
    pub kind: OperationsAlertKind,
    pub scope: String,
    pub source: OperationsAlertSource,
    pub active: bool,
    pub count: i64,
    pub age_seconds: i64,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub last_notified_at: Option<DateTime<Utc>>,
    pub next_notify_at: Option<DateTime<Utc>>,
    /// 当前活动周期内最近一次操作者确认；signal 恢复后再次出现时由存储层清空。
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<String>,
    /// 当前活动周期内最近一次静默到期时间；到期后字段仍保留到下一个周期。
    pub silenced_until: Option<DateTime<Utc>>,
}

impl OperationsAlertConsoleEntry {
    /// 稳定告警标识：fingerprint 的小写十六进制。
    pub fn alert_id(&self) -> String {
        crate::ports::core::lower_hex(&self.fingerprint)
    }

    pub const fn severity(&self) -> OperationsAlertSeverity {
        self.kind.severity()
    }

    pub const fn runbook_anchor(&self) -> &'static str {
        self.source.runbook_anchor()
    }

    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if OperationsAlertSource::parse(self.source.as_str()).is_none()
            || !auction_domain::valid_text(&self.scope, 128)
            || self.count < 0
            || self.age_seconds < 0
            || self.last_seen_at < self.first_seen_at
            || self.acknowledged_by.as_ref().is_some_and(|actor| !valid_alert_actor(actor))
            || (self.active == self.resolved_at.is_some())
            || (self.active && self.next_notify_at.is_none())
            || (!self.active && self.next_notify_at.is_some())
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 告警台一页；`total` 是当前筛选下的总数，两个计数用于页头与导航提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertConsolePage {
    pub entries: Vec<OperationsAlertConsoleEntry>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub active_total: i64,
    pub resolved_total: i64,
}

impl OperationsAlertConsolePage {
    pub fn validate(
        &self,
        query: &OperationsAlertConsoleQuery,
    ) -> Result<(), OperationsAlertError> {
        query.validate()?;
        if self.page != query.page
            || self.page_size != query.page_size
            || self.total < 0
            || self.active_total < 0
            || self.resolved_total < 0
            || self.active_total + self.resolved_total != self.total
            || i64::try_from(self.entries.len()).map_or(true, |count| count > self.page_size)
            || i64::try_from(self.entries.len()).map_or(true, |count| count > self.total)
            || self.entries.iter().any(|entry| entry.validate().is_err())
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 导航待办计数：人工对账、待确认恢复、活动告警。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdminPendingCounts {
    pub delivery_manual_reconcile: i64,
    pub recovery_confirmation: i64,
    pub active_alerts: i64,
}

impl AdminPendingCounts {
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.delivery_manual_reconcile < 0
            || self.recovery_confirmation < 0
            || self.active_alerts < 0
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 告警确认命令；只追加事实，不改变 signal 的 kind/scope/count/age/active。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertAcknowledgeCommand {
    pub command_id: Uuid,
    pub fingerprint: [u8; 32],
    pub actor_id: String,
    pub reason: String,
    pub request_hash: RequestHash,
}

impl OperationsAlertAcknowledgeCommand {
    /// 同 ID 同参重放的稳定摘要；`request_id` 不属于摘要。
    pub fn request_hash(fingerprint: &[u8; 32], actor_id: &str, reason: &str) -> RequestHash {
        request_digest("acknowledge", fingerprint, 0, actor_id, reason)
    }

    pub fn new(
        command_id: Uuid,
        fingerprint: [u8; 32],
        actor_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, OperationsAlertError> {
        let actor_id = actor_id.into();
        let reason = reason.into();
        let command = Self {
            command_id,
            fingerprint,
            request_hash: Self::request_hash(&fingerprint, &actor_id, &reason),
            actor_id,
            reason,
        };
        command.validate()?;
        Ok(command)
    }

    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.command_id.is_nil()
            || self.fingerprint == [0; 32]
            || !valid_alert_actor(&self.actor_id)
            || !valid_alert_reason(&self.reason)
            || self.request_hash.as_bytes() == [0; 32]
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// 告警静默命令；只抑制重复通知到 `silenced_until`，不改变阈值评估。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertSilenceCommand {
    pub command_id: Uuid,
    pub fingerprint: [u8; 32],
    pub silence_seconds: i64,
    pub actor_id: String,
    pub reason: String,
    pub request_hash: RequestHash,
}

impl OperationsAlertSilenceCommand {
    pub fn request_hash(
        fingerprint: &[u8; 32],
        silence_seconds: i64,
        actor_id: &str,
        reason: &str,
    ) -> RequestHash {
        request_digest("silence", fingerprint, silence_seconds, actor_id, reason)
    }

    pub fn new(
        command_id: Uuid,
        fingerprint: [u8; 32],
        silence_seconds: i64,
        actor_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, OperationsAlertError> {
        let actor_id = actor_id.into();
        let reason = reason.into();
        let command = Self {
            command_id,
            fingerprint,
            silence_seconds,
            request_hash: Self::request_hash(&fingerprint, silence_seconds, &actor_id, &reason),
            actor_id,
            reason,
        };
        command.validate()?;
        Ok(command)
    }

    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.command_id.is_nil()
            || self.fingerprint == [0; 32]
            || !(MIN_OPERATIONS_ALERT_SILENCE_SECONDS..=MAX_OPERATIONS_ALERT_SILENCE_SECONDS)
                .contains(&self.silence_seconds)
            || !valid_alert_actor(&self.actor_id)
            || !valid_alert_reason(&self.reason)
            || self.request_hash.as_bytes() == [0; 32]
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// 确认/静默命令的稳定结果；`replayed` 表示同 ID 同参重放，不重复追加。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertCommandResult {
    pub command_id: Uuid,
    pub fingerprint: [u8; 32],
    pub action: String,
    pub silenced_until: Option<DateTime<Utc>>,
    pub replayed: bool,
}

/// 告警台读写窄端口；写侧只有确认与静默两个只追加人员事实。
pub trait OperationsAlertConsole: Send + Sync {
    fn list_alerts(
        &self,
        query: OperationsAlertConsoleQuery,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, OperationsAlertConsolePage>;

    fn acknowledge_alert(
        &self,
        command: OperationsAlertAcknowledgeCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult>;

    fn silence_alert(
        &self,
        command: OperationsAlertSilenceCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult>;

    fn pending_counts(&self) -> ApplicationFuture<'_, AdminPendingCounts>;

    /// 读取最近的 webhook 投递安全投影；只读，不触发重试或状态迁移。
    fn list_webhook_deliveries(
        &self,
        limit: usize,
    ) -> ApplicationFuture<'_, Vec<OperationsAlertWebhookEntry>>;
}

/// 运维告警 webhook 投递的安全运营投影；不含 JSON 正文、收件人或凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertWebhookEntry {
    pub delivery_id: Uuid,
    pub status: String,
    pub kind: OperationsAlertKind,
    pub scope: String,
    pub source: OperationsAlertSource,
    pub attempt_count: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failure_code: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 单次运营视图最多返回的 webhook 投递数。
pub const MAX_OPERATIONS_ALERT_WEBHOOK_VIEW: usize = 100;

/// 从路径/浏览器读取告警标识；只接受 64 位小写或大写十六进制。
pub fn parse_alert_id(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let chunk = std::str::from_utf8(chunk).ok()?;
        bytes[index] = u8::from_str_radix(chunk, 16).ok()?;
    }
    Some(bytes)
}

fn request_digest(
    action: &str,
    fingerprint: &[u8; 32],
    silence_seconds: i64,
    actor_id: &str,
    reason: &str,
) -> RequestHash {
    let mut payload = Vec::with_capacity(192 + reason.len());
    payload.extend_from_slice(b"{\"action\":\"");
    payload.extend_from_slice(action.as_bytes());
    payload.extend_from_slice(b"\",\"fingerprint\":\"");
    payload.extend_from_slice(crate::ports::core::lower_hex(fingerprint).as_bytes());
    payload.extend_from_slice(b"\",\"silence_seconds\":");
    payload.extend_from_slice(silence_seconds.to_string().as_bytes());
    payload.extend_from_slice(b",\"actor_id\":\"");
    payload.extend_from_slice(actor_id.as_bytes());
    payload.extend_from_slice(b"\",\"reason\":");
    payload.extend_from_slice(reason.len().to_string().as_bytes());
    payload.push(b':');
    payload.extend_from_slice(reason.as_bytes());
    payload.push(b'}');
    RequestHash::from_canonical_bytes(&payload)
}

pub(crate) fn valid_alert_actor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_alert_reason(value: &str) -> bool {
    value.len() <= 2048 && value == value.trim() && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::operations::alert::console as parent;

    #[test]
    fn alert_id_round_trips_and_rejects_invalid_hex() {
        let fingerprint = [0xab; 32];
        let id = crate::ports::core::lower_hex(&fingerprint);
        assert_eq!(id.len(), 64);
        assert_eq!(parse_alert_id(&id), Some(fingerprint));
        assert_eq!(parse_alert_id(&id.to_uppercase()), Some(fingerprint));
        assert_eq!(parse_alert_id(&id[..63]), None);
        assert_eq!(parse_alert_id(&"g".repeat(64)), None);
        assert_eq!(parse_alert_id(""), None);
    }

    #[test]
    fn query_rejects_out_of_bounds_pagination() {
        let query = OperationsAlertConsoleQuery {
            filter: OperationsAlertFilter::Active,
            source: None,
            page: 1,
            page_size: 100,
        };
        assert!(query.validate().is_ok());
        assert_eq!(query.offset(), Ok(0));
        let oversized = OperationsAlertConsoleQuery { page_size: 101, ..query };
        assert_eq!(oversized.validate(), Err(OperationsAlertError::Invalid));
        let deep = OperationsAlertConsoleQuery { page: 100_002, page_size: 1, ..query };
        assert_eq!(deep.validate(), Err(OperationsAlertError::Invalid));
    }

    #[test]
    fn silence_command_bounds_and_request_hash_are_stable() {
        let fingerprint = [1; 32];
        let command = OperationsAlertSilenceCommand::new(
            Uuid::new_v4(),
            fingerprint,
            3_600,
            "admin",
            "维护窗口",
        )
        .expect("valid command");
        assert_eq!(
            command.request_hash,
            OperationsAlertSilenceCommand::request_hash(&fingerprint, 3_600, "admin", "维护窗口")
        );
        assert!(matches!(
            OperationsAlertSilenceCommand::new(
                Uuid::new_v4(),
                fingerprint,
                MIN_OPERATIONS_ALERT_SILENCE_SECONDS - 1,
                "admin",
                ""
            ),
            Err(OperationsAlertError::Invalid)
        ));
        assert!(matches!(
            OperationsAlertSilenceCommand::new(
                Uuid::new_v4(),
                fingerprint,
                MAX_OPERATIONS_ALERT_SILENCE_SECONDS + 1,
                "admin",
                ""
            ),
            Err(OperationsAlertError::Invalid)
        ));
    }

    #[test]
    fn acknowledge_rejects_empty_actor_and_zero_fingerprint() {
        assert!(matches!(
            OperationsAlertAcknowledgeCommand::new(Uuid::new_v4(), [0; 32], "admin", ""),
            Err(OperationsAlertError::Invalid)
        ));
        assert!(matches!(
            OperationsAlertAcknowledgeCommand::new(Uuid::new_v4(), [2; 32], "", ""),
            Err(OperationsAlertError::Invalid)
        ));
    }
}
