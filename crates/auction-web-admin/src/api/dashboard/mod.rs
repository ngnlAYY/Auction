//! Admin dashboard 聚合投影客户端。
//! 页面只读取各 owner 提供的安全摘要；数据库行、delivery payload、Cookie 和外部错误正文
//! 不会进入浏览器。分区 DTO 与后端 dashboard 响应保持字段闭集，可空字段也必须显式出现
//! （值可为 `null`）；类型表达不了的跨字段语义由 `valid` 子模块在解码后校验。

pub(crate) use auction_contracts::{
    AdminDashboardHealthResponse as AdminDashboardHealth,
    AdminDashboardWorkerResponse as AdminDashboardWorker,
};
use auction_contracts::{AdminDashboardResponse, AdminDashboardSectionResponse, ApiResponse};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use super::mail::AdminMailPolling;
use super::{invalid_response_error, response_error, send_get};

mod valid;

use valid::valid_dashboard_semantics;

/// Dashboard section 的浏览器类型化投影；wire 层仍保留 owner 构造的有界 JSON。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminDashboardSection<T> {
    pub status: String,
    pub observed_at: Option<String>,
    pub error_code: Option<String>,
    pub data: Option<T>,
}

/// 进程 tracing 安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminTracingData {
    pub status: String,
    pub dropped_span_count: i64,
    pub export_failure_count: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_success_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_failure_at: Option<String>,
    pub last_error_code: String,
}

/// 单个进程的健康安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminProcessData {
    pub service_name: String,
    pub process_role: String,
    pub instance_id: String,
    pub version: String,
    pub started_at: String,
    pub status: String,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub tracing: AdminTracingData,
    pub observed_at: String,
    pub expires_at: String,
}

/// 进程 dashboard section 的数据。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminProcessSectionData {
    pub processes: Vec<AdminProcessData>,
    pub summary: AdminDashboardHealth,
    pub observed_at: String,
}

/// Realtime dashboard 的 lag 条目。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeLag {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_advanced_at: Option<String>,
}

/// Realtime 未解决失败的安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeFailure {
    pub count: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub oldest_at: Option<String>,
    pub latest_code: String,
}

/// Realtime dashboard section 的数据。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeData {
    pub total_lag: i64,
    pub lags: Vec<AdminRealtimeLag>,
    pub failure: AdminRealtimeFailure,
    pub observed_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminAlertingData {
    pub enabled: bool,
    pub active_signal_count: i64,
    pub due_signal_count: i64,
    pub claimed_batch_count: i64,
    pub succeeded_batch_count: i64,
    pub failed_batch_count: i64,
    pub manual_reconcile_count: i64,
    pub webhook_pending_count: i64,
    pub webhook_sending_count: i64,
    pub webhook_failed_count: i64,
    pub webhook_manual_reconcile_count: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_observed_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_success_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_failure_at: Option<String>,
    pub last_error_code: String,
}

pub(crate) type AdminAlertingSection = AdminDashboardSection<AdminAlertingData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminCredentialAvailability {
    pub world: String,
    pub available: i64,
    pub invalid: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub updated_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub restart_required: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminCredentialData {
    pub availability: Vec<AdminCredentialAvailability>,
    pub observed_at: String,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_admin_change_at: Option<String>,
}

pub(crate) type AdminCredentialSection = AdminDashboardSection<AdminCredentialData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminMailData {
    pub observed_at: String,
    pub pollers: Vec<AdminMailPolling>,
}

pub(crate) type AdminMailSection = AdminDashboardSection<AdminMailData>;

/// 外部账号分区中由既有 owner 投影的子列表状态；`unavailable` 不表示空列表。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalPoller {
    pub mailbox_key: String,
    pub paused: bool,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub paused_since: Option<String>,
    pub consecutive_failures: i64,
    pub last_poll_at: String,
    pub next_poll_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalPollerList {
    pub status: String,
    pub items: Vec<AdminExternalPoller>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalMailData {
    pub busy: bool,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub busy_age_seconds: Option<i64>,
    pub gate_wait_count: i64,
    pub gate_wait_seconds_total: i64,
    pub request_count: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_request_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_success_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_failure_at: Option<String>,
    pub pollers: AdminExternalPollerList,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalForumData {
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_request_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_success_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_failure_at: Option<String>,
    pub write_unknown_count: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalMaintenanceData {
    pub state: String,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_started_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_success_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_failure_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_canceled_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub running_age_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalCredential {
    pub world: String,
    pub kind: String,
    pub available: i64,
    pub invalid: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalCredentialList {
    pub status: String,
    pub items: Vec<AdminExternalCredential>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalAccountsData {
    pub observed_at: String,
    pub mail: AdminExternalMailData,
    pub forum: AdminExternalForumData,
    pub maintenance: AdminExternalMaintenanceData,
    pub credentials: AdminExternalCredentialList,
}

pub(crate) type AdminExternalAccountsSection = AdminDashboardSection<AdminExternalAccountsData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminDeliveryWater {
    pub pending: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub last_update_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminDeliveryData {
    pub observed_at: String,
    pub water: AdminDeliveryWater,
}

pub(crate) type AdminDeliverySection = AdminDashboardSection<AdminDeliveryData>;

/// 单个 world 的资产对账差异摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminReconciliationWorld {
    pub world: String,
    pub custody_missing_count: i64,
    /// 已寄出、等待收件人领取的在途装备件数；只展示，不是差异。
    pub custody_in_transit_count: i64,
    pub solvency_shortfall_credits: i64,
    /// 已寄出且资金已支出的在途货款与退款笔数及金额；不计入缺口。
    pub solvency_in_transit_count: i64,
    pub solvency_in_transit_credits: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminReconciliationData {
    pub observed_at: String,
    pub custody_report_age_seconds: i64,
    pub solvency_report_age_seconds: i64,
    pub worlds: Vec<AdminReconciliationWorld>,
}

pub(crate) type AdminReconciliationSection = AdminDashboardSection<AdminReconciliationData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminFulfillmentData {
    pub pending: i64,
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub waiting_payment: i64,
    pub waiting_prerequisite: i64,
    /// 已寄出、只等收件人领取的义务；不计入 `pending`。
    pub awaiting_collection: i64,
    /// 等待领取中最近一次 Sent 读取失败的数量；是 `awaiting_collection` 的子集。
    pub awaiting_collection_unreadable: i64,
    pub observed_at: String,
    pub oldest_backlog_age_seconds: i64,
    pub oldest_prerequisite_wait_age_seconds: i64,
    /// 最早寄出的等待领取义务自寄出起的秒数；页面按天展示。
    pub oldest_awaiting_collection_age_seconds: i64,
    /// 服务端按告警策略阈值计的长期未领取件数（超期件数），是 `awaiting_collection` 的子集；
    /// 页面不自建阈值。
    pub collection_overdue_count: i64,
    /// 与超期件数同一快照的长期未领取清单：寄出最早优先，条数有上限，不多于超期件数。
    pub collection_overdue_items: Vec<AdminCollectionOverdueItem>,
}

/// 长期未领取清单的一项；只含业务标识与展示字段，不含投递载荷、凭据或外部错误原文。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminCollectionOverdueItem {
    pub obligation_id: String,
    /// `seller_payout`、`buyer_refund`、`unsold_return`、`withdrawn_return` 或
    /// `winner_equipment_handoff`。
    pub obligation_kind: String,
    pub world: String,
    pub auction_sequence: i64,
    /// 分类内编号；与 `equipment_category` 组成拍品代码（如 `Shd02`），不是全场编号。
    pub lot_number: i32,
    pub equipment_category: String,
    pub equipment_name: String,
    pub recipient: String,
    pub sent_at: String,
    pub age_seconds: i64,
    /// 最近一次 Sent 读取失败；`false` 只表示没有记录读取失败，不代表已读到或已确认未领取。
    pub last_read_failed: bool,
}

pub(crate) type AdminFulfillmentSection = AdminDashboardSection<AdminFulfillmentData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub balance_observed_at: Option<String>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    pub evidence_last_success_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRecoveryData {
    pub observed_at: String,
    pub worlds: Vec<AdminRecoveryWorld>,
    pub payment_overdue_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
}

pub(crate) type AdminRecoverySection = AdminDashboardSection<AdminRecoveryData>;

/// Admin dashboard 全量安全快照；前端只消费这里声明的稳定字段。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminDashboardSnapshot {
    pub health: AdminDashboardHealth,
    pub worker: AdminDashboardWorker,
    pub process: AdminDashboardSection<AdminProcessSectionData>,
    pub mail: AdminMailSection,
    pub realtime: AdminDashboardSection<AdminRealtimeData>,
    pub credentials: AdminCredentialSection,
    pub external_accounts: AdminExternalAccountsSection,
    pub delivery: AdminDeliverySection,
    pub fulfillment: AdminFulfillmentSection,
    pub recovery: AdminRecoverySection,
    pub alerting: AdminAlertingSection,
    pub reconciliation: AdminReconciliationSection,
}

/// 看板读取地址：始终显式请求等待领取扩展投影（等待领取分组、长期未领取清单与在途字段）；
/// 缺省响应保持已发布闭集，只供升级前的页面解码。
const DASHBOARD_URL: &str = "/api/v1/admin/dashboard?include=collection_wait";

/// 读取 Admin dashboard 的完整安全快照。
pub(crate) async fn fetch_dashboard() -> Result<AdminDashboardSnapshot, String> {
    let response = send_get(DASHBOARD_URL).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<Value> = response.json().await.map_err(invalid_response_error)?;
    decode_dashboard_response(payload.data)
}

fn decode_dashboard_response(value: Value) -> Result<AdminDashboardSnapshot, String> {
    let snapshot: AdminDashboardResponse =
        serde_json::from_value(value).map_err(invalid_response_error)?;
    let process = decode_section::<AdminProcessSectionData>(snapshot.process, false)?;
    // 心跳读取成功但没有存活进程时，后端返回降级分区，此时没有可报告的观测时刻。
    if process.observed_at.is_none()
        && process
            .data
            .as_ref()
            .is_some_and(|data| process.status != "degraded" || !data.processes.is_empty())
    {
        return Err(invalid_response_error("dashboard_process_observed_at"));
    }
    let snapshot = AdminDashboardSnapshot {
        health: snapshot.health,
        worker: snapshot.worker,
        process,
        mail: decode_section(snapshot.mail, true)?,
        realtime: decode_section(snapshot.realtime, true)?,
        credentials: decode_section(snapshot.credentials, true)?,
        external_accounts: decode_section(snapshot.external_accounts, true)?,
        delivery: decode_section(snapshot.delivery, true)?,
        fulfillment: decode_section(snapshot.fulfillment, true)?,
        recovery: decode_section(snapshot.recovery, true)?,
        alerting: decode_section(snapshot.alerting, false)?,
        reconciliation: decode_section(snapshot.reconciliation, true)?,
    };
    if !valid_dashboard_semantics(&snapshot) {
        return Err(invalid_response_error("dashboard_semantics"));
    }
    Ok(snapshot)
}

fn decode_section<T: DeserializeOwned>(
    section: AdminDashboardSectionResponse,
    observed_at_required: bool,
) -> Result<AdminDashboardSection<T>, String> {
    let data =
        section.data.map(serde_json::from_value).transpose().map_err(invalid_response_error)?;
    let valid = match section.status.as_str() {
        "unknown" => {
            section.observed_at.is_none()
                && data.is_none()
                && section.error_code.as_deref().is_some_and(|code| !code.is_empty())
        }
        "ok" | "degraded" => {
            section.error_code.is_none()
                && data.is_some()
                && (!observed_at_required || section.observed_at.is_some())
        }
        _ => false,
    };
    if !valid {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminDashboardSection {
        status: section.status,
        observed_at: section.observed_at,
        error_code: section.error_code,
        data,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{AdminDashboardSnapshot, decode_dashboard_response};

    impl AdminDashboardSnapshot {
        pub(crate) fn test_fixture(update: impl FnOnce(&mut Value)) -> Self {
            let mut payload = complete_payload();
            update(&mut payload);
            decode_dashboard_response(payload).expect("Dashboard 渲染夹具必须通过现有解码校验")
        }
    }

    fn unknown_section() -> Value {
        json!({"status": "unknown", "error_code": "source_unavailable"})
    }

    fn complete_payload() -> Value {
        json!({
            "health": {
                "status": "ok",
                "readiness": true,
                "audit_failure_count": 0,
                "missing_services": []
            },
            "worker": {
                "condition": "available",
                "state": "serving",
                "ready": true,
                "version": "0.1.0",
                "mail_pollers": 1
            },
            "process": unknown_section(),
            "mail": unknown_section(),
            "realtime": unknown_section(),
            "credentials": unknown_section(),
            "external_accounts": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "observed_at": "2026-01-01T00:00:00Z",
                    "mail": {
                        "busy": true,
                        "busy_age_seconds": 12,
                        "gate_wait_count": 2,
                        "gate_wait_seconds_total": 3,
                        "request_count": 5,
                        "last_request_at": "2026-01-01T00:00:00Z",
                        "last_success_at": null,
                        "last_failure_at": null,
                        "pollers": {
                            "status": "available",
                            "items": [{
                                "mailbox_key": "main",
                                "paused": true,
                                "paused_since": "2026-01-01T00:00:00Z",
                                "consecutive_failures": 0,
                                "last_poll_at": "2026-01-01T00:00:00Z",
                                "next_poll_at": "2026-01-01T00:01:00Z"
                            }]
                        }
                    },
                    "forum": {
                        "last_request_at": null,
                        "last_success_at": null,
                        "last_failure_at": null,
                        "write_unknown_count": 1
                    },
                    "maintenance": {
                        "state": "running",
                        "last_started_at": "2026-01-01T00:00:00Z",
                        "last_success_at": null,
                        "last_failure_at": null,
                        "last_canceled_at": null,
                        "running_age_seconds": 4
                    },
                    "credentials": {
                        "status": "available",
                        "items": [
                            {
                                "world": "main",
                                "kind": "mail_cookie",
                                "available": 1,
                                "invalid": 0
                            }
                        ]
                    }
                }
            },
            "delivery": unknown_section(),
            "recovery": unknown_section(),
            "fulfillment": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "pending": 2,
                    "blocked": 1,
                    "failed": 0,
                    "manual_reconcile": 0,
                    "waiting_payment": 4,
                    "waiting_prerequisite": 3,
                    "awaiting_collection": 5,
                    "awaiting_collection_unreadable": 2,
                    "observed_at": "2026-01-01T00:00:00Z",
                    "oldest_backlog_age_seconds": 120,
                    "oldest_prerequisite_wait_age_seconds": 60,
                    "oldest_awaiting_collection_age_seconds": 691200,
                    "collection_overdue_count": 3,
                    "collection_overdue_items": [{
                        "obligation_id": "00000000-0000-0000-0000-000000000010",
                        "obligation_kind": "seller_payout",
                        "world": "main",
                        "auction_sequence": 4,
                        "lot_number": 12,
                        "equipment_category": "Shield",
                        "equipment_name": "Radiant Buckler",
                        "recipient": "collection-seller",
                        "sent_at": "2026-01-01T00:00:00.000000Z",
                        "age_seconds": 691200,
                        "last_read_failed": true
                    }]
                }
            },
            "alerting": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "enabled": true,
                    "active_signal_count": 1,
                    "due_signal_count": 0,
                    "claimed_batch_count": 0,
                    "succeeded_batch_count": 1,
                    "failed_batch_count": 0,
                    "manual_reconcile_count": 0,
                    "webhook_pending_count": 2,
                    "webhook_sending_count": 0,
                    "webhook_failed_count": 0,
                    "webhook_manual_reconcile_count": 1,
                    "last_observed_at": "2026-01-01T00:00:00Z",
                    "last_success_at": null,
                    "last_failure_at": null,
                    "last_error_code": ""
                }
            },
            "reconciliation": unknown_section()
        })
    }

    fn payload_without_live_heartbeats() -> Value {
        let mut payload = complete_payload();
        let summary = json!({
            "status": "degraded",
            "readiness": false,
            "audit_failure_count": 0,
            "missing_services": ["public", "admin", "worker"]
        });
        payload["health"] = summary.clone();
        payload["process"] = json!({
            "status": "degraded",
            "data": {
                "processes": [],
                "summary": summary,
                "observed_at": "0001-01-01T00:00:00Z"
            }
        });
        payload
    }

    #[test]
    fn degraded_empty_process_section_preserves_the_rest_of_the_snapshot() {
        let snapshot = decode_dashboard_response(payload_without_live_heartbeats()).unwrap();
        assert_eq!(snapshot.process.status, "degraded");
        assert!(snapshot.process.observed_at.is_none());
        let process = snapshot.process.data.unwrap();
        assert!(process.processes.is_empty());
        assert_eq!(process.summary.missing_services, ["public", "admin", "worker"]);
        assert_eq!(snapshot.fulfillment.data.unwrap().pending, 2);
    }

    #[test]
    fn other_observed_sections_still_require_an_observation_time() {
        let mut healthy = payload_without_live_heartbeats();
        healthy["process"]["status"] = json!("ok");
        assert!(decode_dashboard_response(healthy).is_err());

        let mut with_heartbeat = payload_without_live_heartbeats();
        let observed_at = "2026-01-01T00:00:00.000000Z";
        with_heartbeat["process"]["data"]["processes"] = json!([{
            "service_name": "admin",
            "process_role": "admin",
            "instance_id": "admin-1",
            "version": "0.1.0",
            "started_at": observed_at,
            "status": "ready",
            "readiness": true,
            "audit_failure_count": 0,
            "tracing": {
                "status": "disabled",
                "dropped_span_count": 0,
                "export_failure_count": 0,
                "last_success_at": null,
                "last_failure_at": null,
                "last_error_code": ""
            },
            "observed_at": observed_at,
            "expires_at": "2026-01-01T00:01:00.000000Z"
        }]);
        with_heartbeat["process"]["data"]["observed_at"] = json!(observed_at);
        assert!(decode_dashboard_response(with_heartbeat.clone()).is_err());
        with_heartbeat["process"]["observed_at"] = json!(observed_at);
        assert!(decode_dashboard_response(with_heartbeat).is_ok());

        let no_fulfillment_time = with(complete_payload(), "/fulfillment", "observed_at", None);
        assert!(decode_dashboard_response(no_fulfillment_time).is_err());
    }

    /// 在完整载荷的 `pointer` 对象上改写一个键；`None` 表示删除该键。
    fn with(mut payload: Value, pointer: &str, key: &str, value: Option<Value>) -> Value {
        let object = payload
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .expect("测试指针必须指向对象");
        match value {
            Some(value) => object.insert(key.to_owned(), value),
            None => object.remove(key),
        };
        payload
    }

    fn mail_section(poller: Value) -> Value {
        json!({
            "status": "ok",
            "observed_at": "2026-01-01T00:00:00Z",
            "data": { "observed_at": "2026-01-01T00:00:00Z", "pollers": [poller] }
        })
    }

    fn mail_poller() -> Value {
        json!({
            "mailbox_key": "main",
            "last_poll_at": "2026-01-01T00:00:00Z",
            "next_poll_at": "2026-01-01T00:01:00Z",
            "consecutive_failures": 0,
            "paused": false,
            "state_version": 1,
            "updated_at": "2026-01-01T00:00:00Z"
        })
    }

    #[test]
    fn complete_dashboard_payload_with_waiting_and_webhook_fields_decodes() {
        let snapshot =
            decode_dashboard_response(complete_payload()).expect("完整响应必须通过浏览器解码");
        let fulfillment = snapshot.fulfillment.data.expect("履约数据");
        assert_eq!(fulfillment.waiting_payment, 4);
        assert_eq!(fulfillment.waiting_prerequisite, 3);
        assert_eq!(fulfillment.oldest_prerequisite_wait_age_seconds, 60);
        assert_eq!(fulfillment.awaiting_collection, 5);
        assert_eq!(fulfillment.awaiting_collection_unreadable, 2);
        assert_eq!(fulfillment.oldest_awaiting_collection_age_seconds, 691_200);
        assert_eq!(fulfillment.collection_overdue_count, 3);
        assert_eq!(fulfillment.collection_overdue_items.len(), 1);
        assert_eq!(fulfillment.collection_overdue_items[0].lot_number, 12);
        assert!(fulfillment.collection_overdue_items[0].last_read_failed);
        let alerting = snapshot.alerting.data.expect("告警数据");
        assert_eq!(alerting.webhook_pending_count, 2);
        assert_eq!(alerting.webhook_manual_reconcile_count, 1);
        let external = snapshot.external_accounts.data.expect("外部账号数据");
        assert!(external.mail.busy);
        assert_eq!(external.mail.busy_age_seconds, Some(12));
        assert!(external.mail.pollers.items[0].paused);
        assert_eq!(external.maintenance.state, "running");
        assert_eq!(external.forum.write_unknown_count, 1);
        assert_eq!(external.credentials.items[0].available, 1);
    }

    #[test]
    fn unknown_external_accounts_section_decodes_without_data() {
        let payload = json!({
            "health": {
                "status": "ok",
                "readiness": true,
                "audit_failure_count": 0,
                "missing_services": []
            },
            "worker": {
                "condition": "unavailable",
                "state": "",
                "ready": false,
                "version": "",
                "mail_pollers": 0
            },
            "process": unknown_section(),
            "mail": unknown_section(),
            "realtime": unknown_section(),
            "credentials": unknown_section(),
            "external_accounts": unknown_section(),
            "delivery": unknown_section(),
            "fulfillment": unknown_section(),
            "recovery": unknown_section(),
            "alerting": unknown_section(),
            "reconciliation": unknown_section()
        });

        let snapshot = decode_dashboard_response(payload).expect("不可用分区仍须解码");
        assert!(snapshot.external_accounts.data.is_none());
        assert_eq!(snapshot.external_accounts.status, "unknown");
        assert_eq!(snapshot.external_accounts.error_code.as_deref(), Some("source_unavailable"));
    }

    #[test]
    fn missing_nullable_or_counter_fields_fail_closed() {
        // 可空字段缺键与显式 null 不同；告警计数器也不再有缺省值。
        for (pointer, key) in [
            ("/external_accounts/data/mail", "last_success_at"),
            ("/external_accounts/data/mail", "busy_age_seconds"),
            ("/external_accounts/data/mail/pollers/items/0", "paused_since"),
            ("/external_accounts/data/forum", "last_request_at"),
            ("/external_accounts/data/maintenance", "running_age_seconds"),
            ("/alerting/data", "last_failure_at"),
            ("/alerting/data", "webhook_pending_count"),
            ("/alerting/data", "last_error_code"),
        ] {
            let payload = with(complete_payload(), pointer, key, None);
            assert!(decode_dashboard_response(payload).is_err(), "缺少 {pointer}/{key} 必须拒绝");
        }
    }

    #[test]
    fn awaiting_collection_counts_must_be_consistent() {
        let fulfillment = "/fulfillment/data";
        for (key, value) in [
            ("awaiting_collection_unreadable", json!(6)),
            ("awaiting_collection_unreadable", json!(-1)),
            ("awaiting_collection", json!(-1)),
            ("oldest_awaiting_collection_age_seconds", json!(-1)),
            ("collection_overdue_count", json!(6)),
            ("collection_overdue_count", json!(-1)),
            // 清单不能多于超期件数。
            ("collection_overdue_count", json!(0)),
        ] {
            let payload = with(complete_payload(), fulfillment, key, Some(value));
            assert!(decode_dashboard_response(payload).is_err(), "{key} 越界必须拒绝");
        }
        // 没有等待领取时不能带等待年龄、超期件或清单；全部归零、清单为空才是安静水位。
        let empty = with(complete_payload(), fulfillment, "awaiting_collection", Some(json!(0)));
        let empty = with(empty, fulfillment, "awaiting_collection_unreadable", Some(json!(0)));
        let empty = with(empty, fulfillment, "collection_overdue_count", Some(json!(0)));
        let empty = with(empty, fulfillment, "collection_overdue_items", Some(json!([])));
        assert!(decode_dashboard_response(empty.clone()).is_err());
        let quiet =
            with(empty, fulfillment, "oldest_awaiting_collection_age_seconds", Some(json!(0)));
        assert!(decode_dashboard_response(quiet.clone()).is_ok());
        let overdue_without_awaiting =
            with(quiet, fulfillment, "collection_overdue_count", Some(json!(1)));
        assert!(decode_dashboard_response(overdue_without_awaiting).is_err());
    }

    /// 清单项字段闭集与取值：义务类别只认等待领取会出现的五类，年龄与编号不得为负或为零，
    /// 文本不得为空；未知字段同样拒绝。
    #[test]
    fn collection_overdue_items_fail_closed() {
        let item = "/fulfillment/data/collection_overdue_items/0";
        for (key, value) in [
            ("obligation_kind", json!("winner_equipment_cod")),
            ("obligation_kind", json!("seller")),
            ("world", json!("moon")),
            ("auction_sequence", json!(0)),
            ("lot_number", json!(0)),
            ("age_seconds", json!(-1)),
            ("recipient", json!("")),
            ("equipment_name", json!(" ")),
            ("sent_at", json!("")),
            ("obligation_id", json!("")),
        ] {
            let payload = with(complete_payload(), item, key, Some(value));
            assert!(decode_dashboard_response(payload).is_err(), "{key} 非法必须拒绝");
        }
        let payload = with(complete_payload(), item, "unexpected", Some(json!(1)));
        assert!(decode_dashboard_response(payload).is_err(), "清单项未知字段必须拒绝");
        for kind in
            ["buyer_refund", "unsold_return", "withdrawn_return", "winner_equipment_handoff"]
        {
            let payload = with(complete_payload(), item, "obligation_kind", Some(json!(kind)));
            assert!(decode_dashboard_response(payload).is_ok(), "{kind} 是等待领取的义务类别");
        }
    }

    #[test]
    fn unknown_fields_fail_closed() {
        for pointer in [
            "",
            "/health",
            "/worker",
            "/alerting",
            "/alerting/data",
            "/fulfillment/data",
            "/external_accounts/data/forum",
            "/external_accounts/data/credentials/items/0",
        ] {
            let payload = with(complete_payload(), pointer, "unexpected", Some(json!(1)));
            assert!(decode_dashboard_response(payload).is_err(), "{pointer} 的未知字段必须拒绝");
        }
    }

    #[test]
    fn disabled_alerting_must_be_ok_without_observed_at() {
        let disabled = with(complete_payload(), "/alerting/data", "enabled", Some(json!(false)));
        assert!(decode_dashboard_response(disabled.clone()).is_err(), "关闭时不得带 observed_at");

        let quiet = with(disabled, "/alerting", "observed_at", None);
        assert!(decode_dashboard_response(quiet.clone()).is_ok());
        let degraded = with(quiet, "/alerting", "status", Some(json!("degraded")));
        assert!(decode_dashboard_response(degraded).is_err(), "关闭时分区只能是 ok");
    }

    #[test]
    fn enabled_alerting_observed_at_must_match_last_scan() {
        let drifted = with(
            complete_payload(),
            "/alerting",
            "observed_at",
            Some(json!("2026-01-01T00:00:01Z")),
        );
        assert!(decode_dashboard_response(drifted).is_err());
        let missing = with(complete_payload(), "/alerting", "observed_at", None);
        assert!(decode_dashboard_response(missing.clone()).is_err());

        // 从未扫描：两处都没有观测时间。
        let never_scanned = with(missing, "/alerting/data", "last_observed_at", Some(Value::Null));
        assert!(decode_dashboard_response(never_scanned).is_ok());
    }

    #[test]
    fn busy_age_must_follow_mail_gate_state() {
        let mail = "/external_accounts/data/mail";
        let idle_with_age = with(complete_payload(), mail, "busy", Some(json!(false)));
        assert!(decode_dashboard_response(idle_with_age.clone()).is_err());
        let idle = with(idle_with_age, mail, "busy_age_seconds", Some(Value::Null));
        assert!(decode_dashboard_response(idle).is_ok());

        let busy_without_age =
            with(complete_payload(), mail, "busy_age_seconds", Some(Value::Null));
        assert!(decode_dashboard_response(busy_without_age).is_err());
        let negative_age = with(complete_payload(), mail, "busy_age_seconds", Some(json!(-1)));
        assert!(decode_dashboard_response(negative_age).is_err());
    }

    #[test]
    fn dashboard_mail_pollers_reject_diagnostics() {
        let plain = with(complete_payload(), "", "mail", Some(mail_section(mail_poller())));
        assert!(decode_dashboard_response(plain).is_ok());

        let diagnostics = json!({
            "observed_at": "2026-01-01T00:00:00Z",
            "cursor_status": "empty",
            "completed_external_mail_id": null,
            "worker_condition": "available",
            "worker_observed_at": null,
            "progress": null
        });
        let mut poller = mail_poller();
        poller
            .as_object_mut()
            .expect("轮询载荷必须是对象")
            .insert("diagnostics".to_owned(), diagnostics);
        let diagnosed = with(complete_payload(), "", "mail", Some(mail_section(poller)));
        assert!(decode_dashboard_response(diagnosed).is_err());
    }

    fn reconciliation_section(world_count: usize) -> Value {
        let world = json!({
            "world": "main",
            "custody_missing_count": 0,
            "custody_in_transit_count": 5,
            "solvency_shortfall_credits": 0,
            "solvency_in_transit_count": 31,
            "solvency_in_transit_credits": 38_668_000
        });
        json!({
            "status": "ok",
            "observed_at": "2026-01-01T00:00:00Z",
            "data": {
                "observed_at": "2026-01-01T00:00:00Z",
                "custody_report_age_seconds": 1,
                "solvency_report_age_seconds": 1,
                "worlds": vec![world; world_count]
            }
        })
    }

    /// 本页始终请求等待领取扩展投影。
    #[test]
    fn dashboard_url_requests_the_collection_wait_projection() {
        assert_eq!(super::DASHBOARD_URL, "/api/v1/admin/dashboard?include=collection_wait");
    }

    /// 反例：缺省响应（已发布闭集，没有等待领取字段、清单与在途字段）不是本页请求的投影，
    /// 解码失败关闭，不把缺失的扩展字段当作 0。
    #[test]
    fn baseline_shaped_sections_are_rejected() {
        let fulfillment = "/fulfillment/data";
        let mut baseline = complete_payload();
        for key in [
            "awaiting_collection",
            "awaiting_collection_unreadable",
            "oldest_awaiting_collection_age_seconds",
            "collection_overdue_count",
            "collection_overdue_items",
        ] {
            let single = with(complete_payload(), fulfillment, key, None);
            assert!(decode_dashboard_response(single).is_err(), "缺少 {key} 必须拒绝");
            baseline = with(baseline, fulfillment, key, None);
        }
        assert!(decode_dashboard_response(baseline).is_err(), "基线形状的履约分区必须拒绝");

        let reconciliation =
            with(complete_payload(), "", "reconciliation", Some(reconciliation_section(1)));
        assert!(decode_dashboard_response(reconciliation.clone()).is_ok(), "扩展投影可解码");
        for key in
            ["custody_in_transit_count", "solvency_in_transit_count", "solvency_in_transit_credits"]
        {
            let missing = with(reconciliation.clone(), "/reconciliation/data/worlds/0", key, None);
            assert!(decode_dashboard_response(missing).is_err(), "缺少 {key} 必须拒绝");
        }
    }

    #[test]
    fn reconciliation_in_transit_must_be_non_negative() {
        let payload =
            with(complete_payload(), "", "reconciliation", Some(reconciliation_section(1)));
        let snapshot = decode_dashboard_response(payload.clone()).expect("在途字段必须可解码");
        let world = &snapshot.reconciliation.data.expect("对账数据").worlds[0];
        assert_eq!(world.custody_in_transit_count, 5);
        assert_eq!(world.solvency_in_transit_credits, 38_668_000);
        for key in
            ["custody_in_transit_count", "solvency_in_transit_count", "solvency_in_transit_credits"]
        {
            let rejected =
                with(payload.clone(), "/reconciliation/data/worlds/0", key, Some(json!(-1)));
            assert!(decode_dashboard_response(rejected).is_err(), "{key} 为负必须拒绝");
        }
    }

    fn realtime_section(total_lag: i64) -> Value {
        json!({
            "status": "ok",
            "observed_at": "2026-01-01T00:00:00Z",
            "data": {
                "observed_at": "2026-01-01T00:00:00Z",
                "total_lag": total_lag,
                "lags": [],
                "failure": { "count": 0, "oldest_at": null, "latest_code": "" }
            }
        })
    }

    #[test]
    fn semantic_closed_sets_and_bounds_fail_closed() {
        let external = "/external_accounts/data";
        for (pointer, key, valid, invalid) in [
            (format!("{external}/maintenance"), "state", json!("idle"), json!("paused")),
            (format!("{external}/mail/pollers"), "status", json!("unavailable"), json!("partial")),
            (format!("{external}/credentials"), "status", json!("unavailable"), json!("stale")),
            (format!("{external}/forum"), "write_unknown_count", json!(0), json!(-1)),
        ] {
            let accepted = with(complete_payload(), &pointer, key, Some(valid));
            assert!(decode_dashboard_response(accepted).is_ok(), "{pointer}/{key} 合法值必须通过");
            let rejected = with(complete_payload(), &pointer, key, Some(invalid));
            assert!(decode_dashboard_response(rejected).is_err(), "{pointer}/{key} 非法值必须拒绝");
        }

        for (key, valid, invalid) in [
            ("reconciliation", reconciliation_section(2), reconciliation_section(3)),
            ("realtime", realtime_section(0), realtime_section(-1)),
        ] {
            let accepted = with(complete_payload(), "", key, Some(valid));
            assert!(decode_dashboard_response(accepted).is_ok(), "{key} 合法值必须通过");
            let rejected = with(complete_payload(), "", key, Some(invalid));
            assert!(decode_dashboard_response(rejected).is_err(), "{key} 越界必须拒绝");
        }
    }
}
