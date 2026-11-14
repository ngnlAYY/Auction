//! 告警台 HTTP 契约：活动/历史列表、确认、静默与导航待办计数。

use super::super::shared::deserialize_null_default;
use serde::{Deserialize, Serialize};

/// 单条告警的安全投影；不含原始错误正文、收件人或凭据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertEntryResponse {
    /// 64 位小写十六进制 fingerprint。
    pub id: String,
    pub kind: String,
    pub scope: String,
    pub source: String,
    /// `urgent` 或 `digest`（决策 66）。
    pub severity: String,
    pub active: bool,
    pub count: i64,
    pub age_seconds: i64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub resolved_at: Option<String>,
    pub last_notified_at: Option<String>,
    pub next_notify_at: Option<String>,
    pub acknowledged_at: Option<String>,
    pub acknowledged_by: Option<String>,
    pub silenced_until: Option<String>,
    /// `ops/README.md` 处置手册锚点（不含 `#`）。
    pub runbook_anchor: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertListResponse {
    pub alerts: Vec<AdminAlertEntryResponse>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub active_total: i64,
    pub resolved_total: i64,
}

/// webhook 投递的安全运营投影；不含冻结 JSON、收件人或凭据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertWebhookEntryResponse {
    pub id: String,
    pub status: String,
    pub kind: String,
    pub scope: String,
    pub source: String,
    pub attempt_count: u32,
    pub next_attempt_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertWebhookListResponse {
    pub entries: Vec<AdminAlertWebhookEntryResponse>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertAcknowledgeRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertSilenceRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub reason: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub silence_seconds: i64,
}

/// 确认/静默的稳定结果；`replayed` 表示同 ID 同参重放。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertCommandResultResponse {
    pub id: String,
    pub action: String,
    pub replayed: bool,
    pub silenced_until: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAlertCommandResponse {
    pub alert: AdminAlertCommandResultResponse,
}

/// 导航待办计数：人工对账、待确认恢复、活动告警。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminPendingCountsResponse {
    pub delivery_manual_reconcile: i64,
    pub recovery_confirmation: i64,
    pub active_alerts: i64,
}
