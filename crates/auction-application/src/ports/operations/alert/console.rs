//! 告警台的只读投影、操作者确认/静默命令与待办计数。
//!
//! 告警台不是第二套状态机：活动/历史状态仍由 `operations_alert_signals` 的水位拥有，
//! 确认与静默只追加操作者事实。静默只把该 signal 的 `next_notify_at` 推到到期时间，
//! 不改变 kind/scope/source/count/age/active，也不跳过阈值评估；到期后重复通知自然恢复。

use auction_domain::{valid_text, valid_text_allow_empty};
use auction_time::Timestamp;
use uuid::Uuid;

use super::signals::{
    OperationsAlertError, OperationsAlertKind, OperationsAlertSeverity, OperationsAlertSource,
};
use crate::ports::core::ApplicationFuture;
use crate::ports::core::RequestHash;

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
    /// 同一指纹恢复后再出现时递增，用于绑定处置所针对的事件。
    pub incident_generation: i64,
    pub count: i64,
    pub age_seconds: i64,
    pub first_seen_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
    pub last_notified_at: Option<Timestamp>,
    pub next_notify_at: Option<Timestamp>,
    /// 当前活动周期内最近一次操作者确认；signal 恢复后再次出现时由存储层清空。
    pub acknowledged_at: Option<Timestamp>,
    pub acknowledged_by: Option<String>,
    /// 当前活动周期内最近一次静默到期时间；到期后字段仍保留到下一个周期。
    pub silenced_until: Option<Timestamp>,
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
            || self.incident_generation < 0
            || self.count < 0
            || self.age_seconds < 0
            || self.last_seen_at < self.first_seen_at
            || self.acknowledged_by.as_ref().is_some_and(|actor| !valid_alert_actor(actor))
            || self.acknowledged_at.is_some() != self.acknowledged_by.is_some()
            || self.acknowledged_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || (!self.active && self.silenced_until.is_some())
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
        // 两个分类计数不随状态筛选变化，`total` 只计所选类别；条目必须属于所选类别。
        let selected = query.filter.active();
        let expected_total = match selected {
            Some(true) => Some(self.active_total),
            Some(false) => Some(self.resolved_total),
            None => self.active_total.checked_add(self.resolved_total),
        };
        if self.page != query.page
            || self.page_size != query.page_size
            || self.total < 0
            || self.active_total < 0
            || self.resolved_total < 0
            || expected_total != Some(self.total)
            || i64::try_from(self.entries.len()).map_or(true, |count| count > self.page_size)
            || i64::try_from(self.entries.len()).map_or(true, |count| count > self.total)
            || self.entries.iter().any(|entry| {
                entry.validate().is_err() || selected.is_some_and(|active| entry.active != active)
            })
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 导航待办计数：人工对账、待确认恢复、活动告警与未结案结算退件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdminPendingCounts {
    pub delivery_manual_reconcile: i64,
    pub recovery_confirmation: i64,
    pub active_alerts: i64,
    /// 未结案（`closed_at` 为空）的结算退件案件数；含隔离与 Take 结果未知的案件。
    pub settlement_returns: i64,
}

impl AdminPendingCounts {
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.delivery_manual_reconcile < 0
            || self.recovery_confirmation < 0
            || self.active_alerts < 0
            || self.settlement_returns < 0
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
    pub incident_generation: i64,
    pub actor_id: String,
    pub reason: String,
    pub request_hash: RequestHash,
}

impl OperationsAlertAcknowledgeCommand {
    /// 同 ID 同参重放的稳定摘要，绑定操作员看到的事件代数。
    pub fn request_hash(
        fingerprint: &[u8; 32],
        incident_generation: i64,
        actor_id: &str,
        reason: &str,
    ) -> RequestHash {
        request_digest("acknowledge", fingerprint, incident_generation, 0, actor_id, reason)
    }

    pub fn new(
        command_id: Uuid,
        fingerprint: [u8; 32],
        incident_generation: i64,
        actor_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, OperationsAlertError> {
        let actor_id = actor_id.into();
        let reason = reason.into();
        let command = Self {
            command_id,
            fingerprint,
            incident_generation,
            request_hash: Self::request_hash(&fingerprint, incident_generation, &actor_id, &reason),
            actor_id,
            reason,
        };
        command.validate()?;
        Ok(command)
    }

    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.command_id.is_nil()
            || self.fingerprint == [0; 32]
            || self.incident_generation < 0
            || !valid_alert_actor(&self.actor_id)
            || !valid_alert_reason(&self.reason)
            || self.request_hash
                != Self::request_hash(
                    &self.fingerprint,
                    self.incident_generation,
                    &self.actor_id,
                    &self.reason,
                )
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
    pub incident_generation: i64,
    pub silence_seconds: i64,
    pub actor_id: String,
    pub reason: String,
    pub request_hash: RequestHash,
}

impl OperationsAlertSilenceCommand {
    /// 同 ID 同参重放的稳定摘要，绑定操作员看到的事件代数。
    pub fn request_hash(
        fingerprint: &[u8; 32],
        incident_generation: i64,
        silence_seconds: i64,
        actor_id: &str,
        reason: &str,
    ) -> RequestHash {
        request_digest(
            "silence",
            fingerprint,
            incident_generation,
            silence_seconds,
            actor_id,
            reason,
        )
    }

    pub fn new(
        command_id: Uuid,
        fingerprint: [u8; 32],
        incident_generation: i64,
        silence_seconds: i64,
        actor_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, OperationsAlertError> {
        let actor_id = actor_id.into();
        let reason = reason.into();
        let command = Self {
            command_id,
            fingerprint,
            incident_generation,
            silence_seconds,
            request_hash: Self::request_hash(
                &fingerprint,
                incident_generation,
                silence_seconds,
                &actor_id,
                &reason,
            ),
            actor_id,
            reason,
        };
        command.validate()?;
        Ok(command)
    }

    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.command_id.is_nil()
            || self.fingerprint == [0; 32]
            || self.incident_generation < 0
            || !(MIN_OPERATIONS_ALERT_SILENCE_SECONDS..=MAX_OPERATIONS_ALERT_SILENCE_SECONDS)
                .contains(&self.silence_seconds)
            || !valid_alert_actor(&self.actor_id)
            || !valid_alert_reason(&self.reason)
            || self.request_hash
                != Self::request_hash(
                    &self.fingerprint,
                    self.incident_generation,
                    self.silence_seconds,
                    &self.actor_id,
                    &self.reason,
                )
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
    pub silenced_until: Option<Timestamp>,
    pub replayed: bool,
}

/// 告警台读写窄端口；写侧只有确认与静默两个只追加人员事实。
pub trait OperationsAlertConsole: Send + Sync {
    fn list_alerts(
        &self,
        query: OperationsAlertConsoleQuery,
        now: Timestamp,
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
    pub next_attempt_at: Option<Timestamp>,
    pub sending_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub failure_code: Option<String>,
    pub created_at: Timestamp,
}

/// 单次运营视图最多返回的 webhook 投递数。
pub const MAX_OPERATIONS_ALERT_WEBHOOK_VIEW: usize = 100;

impl OperationsAlertWebhookEntry {
    /// 校验 webhook 投递的闭集状态、失败码和安全展示边界。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        let failure_matches_status = matches!(
            (self.status.as_str(), self.failure_code.as_deref()),
            ("pending" | "sending" | "succeeded", None)
                | ("failed", Some("permanent" | "retry_exhausted"))
                | ("manual_reconcile", Some("outcome_unknown")),
        );
        let timestamps_match_status = match self.status.as_str() {
            "pending" => {
                self.next_attempt_at.is_some()
                    && self.sending_at.is_none()
                    && self.completed_at.is_none()
            }
            "sending" => {
                self.next_attempt_at.is_none()
                    && self.sending_at.is_some()
                    && self.completed_at.is_none()
            }
            "succeeded" | "failed" => {
                self.next_attempt_at.is_none()
                    && self.sending_at.is_none()
                    && self.completed_at.is_some()
            }
            // 租约过期保留发送时间，显式完成为未知结果时会清除它。
            "manual_reconcile" => self.next_attempt_at.is_none() && self.completed_at.is_some(),
            _ => false,
        };
        if self.delivery_id.is_nil()
            || !failure_matches_status
            || !timestamps_match_status
            || self.attempt_count > i32::MAX as u32
            || self.created_at.timestamp_nanos_opt().is_none()
            || self.next_attempt_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || self.sending_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || self.completed_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

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
    incident_generation: i64,
    silence_seconds: i64,
    actor_id: &str,
    reason: &str,
) -> RequestHash {
    let mut payload = Vec::with_capacity(192 + actor_id.len() + reason.len());
    payload.extend_from_slice(b"{\"action\":\"");
    payload.extend_from_slice(action.as_bytes());
    payload.extend_from_slice(b"\",\"fingerprint\":\"");
    payload.extend_from_slice(crate::ports::core::lower_hex(fingerprint).as_bytes());
    // 同 ID 改换事件属于换参，即使指纹、操作与备注都相同。
    payload.extend_from_slice(b"\",\"incident_generation\":");
    payload.extend_from_slice(incident_generation.to_string().as_bytes());
    payload.extend_from_slice(b",\"silence_seconds\":");
    payload.extend_from_slice(silence_seconds.to_string().as_bytes());
    // 只转义 JSON 分隔符；普通 actor 的摘要字节保持不变，危险字符不能跨字段拼接。
    payload.extend_from_slice(b",\"actor_id\":\"");
    for byte in actor_id.bytes() {
        if matches!(byte, b'"' | b'\\') {
            payload.push(b'\\');
        }
        payload.push(byte);
    }
    payload.extend_from_slice(b"\",\"reason\":");
    payload.extend_from_slice(reason.len().to_string().as_bytes());
    payload.push(b':');
    payload.extend_from_slice(reason.as_bytes());
    payload.push(b'}');
    RequestHash::from_canonical_bytes(&payload)
}

/// 操作者标识沿用领域规范单行文本，上限 256 字节。
pub(crate) fn valid_alert_actor(value: &str) -> bool {
    valid_text(value, 256)
}

/// 确认/静默理由可以留空；非空时沿用领域规范单行文本，上限 2048 字节。
pub(crate) fn valid_alert_reason(value: &str) -> bool {
    valid_text_allow_empty(value, 2048)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

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
    fn webhook_manual_reconcile_allows_optional_sending_time_and_requires_terminal_fields() {
        let sending = Timestamp::from_timestamp(1_700_000_000, 0).expect("有效时刻");
        let completed = Timestamp::from_timestamp(1_700_000_120, 0).expect("有效时刻");
        let entry = OperationsAlertWebhookEntry {
            delivery_id: Uuid::new_v4(),
            status: "manual_reconcile".to_owned(),
            kind: OperationsAlertKind::ServiceMissing,
            scope: "worker".to_owned(),
            source: OperationsAlertSource::Process,
            attempt_count: 1,
            next_attempt_at: None,
            sending_at: Some(sending),
            completed_at: Some(completed),
            failure_code: Some("outcome_unknown".to_owned()),
            created_at: sending,
        };
        assert_eq!(entry.validate(), Ok(()));
        let explicit = OperationsAlertWebhookEntry { sending_at: None, ..entry.clone() };
        assert_eq!(explicit.validate(), Ok(()));

        for invalid in [
            OperationsAlertWebhookEntry { next_attempt_at: Some(completed), ..entry.clone() },
            OperationsAlertWebhookEntry { completed_at: None, ..entry.clone() },
            OperationsAlertWebhookEntry { failure_code: None, ..entry.clone() },
            OperationsAlertWebhookEntry { sending_at: Some(Timestamp::MAX_UTC), ..entry },
        ] {
            assert_eq!(invalid.validate(), Err(OperationsAlertError::StoredResultCorrupt));
        }
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
            1,
            3_600,
            "admin",
            "维护窗口",
        )
        .expect("valid command");
        assert_eq!(
            command.request_hash,
            OperationsAlertSilenceCommand::request_hash(
                &fingerprint,
                1,
                3_600,
                "admin",
                "维护窗口"
            )
        );
        assert_matches!(
            OperationsAlertSilenceCommand::new(
                Uuid::new_v4(),
                fingerprint,
                1,
                MIN_OPERATIONS_ALERT_SILENCE_SECONDS - 1,
                "admin",
                ""
            ),
            Err(OperationsAlertError::Invalid)
        );
        assert_matches!(
            OperationsAlertSilenceCommand::new(
                Uuid::new_v4(),
                fingerprint,
                1,
                MAX_OPERATIONS_ALERT_SILENCE_SECONDS + 1,
                "admin",
                ""
            ),
            Err(OperationsAlertError::Invalid)
        );
    }

    #[test]
    fn acknowledge_rejects_empty_actor_and_zero_fingerprint() {
        assert_matches!(
            OperationsAlertAcknowledgeCommand::new(Uuid::new_v4(), [0; 32], 1, "admin", ""),
            Err(OperationsAlertError::Invalid)
        );
        assert_matches!(
            OperationsAlertAcknowledgeCommand::new(Uuid::new_v4(), [2; 32], 1, "", ""),
            Err(OperationsAlertError::Invalid)
        );
    }

    #[test]
    fn confirmation_and_silence_hashes_bind_the_incident_generation() {
        let fingerprint = [2; 32];
        let command_id = Uuid::new_v4();
        let hashes = |incident_generation| {
            (
                OperationsAlertAcknowledgeCommand::new(
                    command_id,
                    fingerprint,
                    incident_generation,
                    "admin",
                    "核对中",
                )
                .map(|command| command.request_hash),
                OperationsAlertSilenceCommand::new(
                    command_id,
                    fingerprint,
                    incident_generation,
                    3_600,
                    "admin",
                    "核对中",
                )
                .map(|command| command.request_hash),
            )
        };
        let original = hashes(0);
        assert!(original.0.is_ok() && original.1.is_ok());
        assert_eq!(original, hashes(0));
        let next = hashes(1);
        assert!(next.0.is_ok() && next.1.is_ok());
        assert_ne!(original.0, next.0);
        assert_ne!(original.1, next.1);
        assert_eq!(
            hashes(-1),
            (Err(OperationsAlertError::Invalid), Err(OperationsAlertError::Invalid))
        );
    }
}
