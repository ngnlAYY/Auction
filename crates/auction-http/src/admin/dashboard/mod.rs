//! Admin 健康 dashboard 的 HTTP 安全投影。
//! 根模块只负责认证后的请求编排和稳定路由入口；各业务 section 的安全聚合与响应字段映射
//! 分别由子模块拥有。owner 未装配或读取失败时明确返回 unknown，避免用缓存或占位数据误导运维。

mod mapping;
mod projection;
mod sections;

use std::net::SocketAddr;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};
use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, WorkerControlCondition, WorkerControlExternalAccounts,
    WorkerControlStatus,
};
use auction_contracts::{AdminDashboardSectionResponse, ApiResponse};
use auction_observability::HealthError;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use self::projection::DashboardProjection;
use crate::diagnostics;
use crate::errors::storage_unavailable;
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

pub(crate) struct AdminDashboardSections {
    pub(crate) mail: AdminDashboardSectionResponse,
    pub(crate) realtime: AdminDashboardSectionResponse,
    pub(crate) credentials: AdminDashboardSectionResponse,
    pub(crate) external_accounts: AdminDashboardSectionResponse,
    pub(crate) delivery: AdminDashboardSectionResponse,
    pub(crate) fulfillment: AdminDashboardSectionResponse,
    pub(crate) recovery: AdminDashboardSectionResponse,
    pub(crate) alerting: AdminDashboardSectionResponse,
    pub(crate) reconciliation: AdminDashboardSectionResponse,
}

/// 读取已认证 Admin dashboard。
pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    // 缺省响应保持已发布的字段闭集与口径；等待领取扩展投影只在显式 include 时输出。
    let projection = match projection::dashboard_projection(&uri) {
        Ok(projection) => projection,
        Err(response) => return *response,
    };
    let Some(reader) = state.health() else {
        return storage_unavailable("健康 dashboard 需要已配置的数据库");
    };
    let snapshot = match reader.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    // 基础健康读取成功后，各 owner 的只读 section 彼此独立；同一请求内并发等待，
    // 避免依赖延迟相加耗尽总预算。无独立 spawn，取消请求仍会一起释放这些读取。
    let worker = async {
        match state.worker_control() {
            Some(port) => {
                let status = match port.status().await {
                    Ok(status) => status,
                    Err(diagnostic_cause) => {
                        diagnostics::warn(
                            "admin_dashboard",
                            "read_worker",
                            DiagnosticError::diagnostic_kind(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable)
                    }
                };
                let external_accounts = match port.external_accounts().await {
                    Ok(accounts) => accounts,
                    Err(diagnostic_cause) => {
                        diagnostics::warn(
                            "admin_dashboard",
                            "read_external_accounts",
                            DiagnosticError::diagnostic_kind(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        WorkerControlExternalAccounts::unavailable(
                            WorkerControlCondition::Unavailable,
                        )
                    }
                };
                (status, external_accounts)
            }
            None => (
                WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
                WorkerControlExternalAccounts::unavailable(WorkerControlCondition::Unavailable),
            ),
        }
    };
    let mail = async {
        match state.admin_mail() {
            Some(port) => sections::load_mail_status(port).await.ok(),
            None => None,
        }
    };
    let realtime = async {
        match state.realtime_admin() {
            Some(port) => sections::load_realtime_section(port)
                .await
                .unwrap_or_else(|_| sections::unknown_section()),
            None => sections::unknown_section(),
        }
    };
    // 长期未领取只在等待领取投影里评估：阈值取 Admin 生效的告警策略，与分区降级判定同一策略，
    // 清单与计数在同一只读快照里读出；缺省投影与没有策略时不评估。
    let alert_policy = state.operations_alert_policy();
    let collection = match (projection, alert_policy) {
        (DashboardProjection::CollectionWait, Some(policy)) => {
            OperationsAlertCollectionOverdueRead::CountWithItems(
                policy.fulfillment_collection_overdue_age_threshold,
            )
        }
        _ => OperationsAlertCollectionOverdueRead::Skip,
    };
    let owner_snapshot = async {
        match state.operations_alert_source() {
            Some(port) => port
                .snapshot(auction_time::Timestamp::now(), collection)
                .await
                .inspect_err(|cause| {
                    diagnostics::warn(
                        "admin_dashboard",
                        "read_operations_snapshot",
                        cause.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                })
                .ok(),
            None => None,
        }
    };
    let alerting = async {
        match state.operations_alert() {
            Some(port) => match port.health().await {
                Ok(health) => sections::map_alerting(health),
                Err(diagnostic_cause) => {
                    diagnostics::warn(
                        "admin_dashboard",
                        "read_alert_health",
                        DiagnosticError::diagnostic_kind(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    sections::unknown_section_with_code("source_unavailable")
                }
            },
            None => sections::unknown_section_with_code("source_unavailable"),
        }
    };
    let credentials = async {
        match state.credential_health() {
            Some(port) => match port.health().await {
                Ok(credentials) if credentials.validate() => Some(credentials),
                diagnostic_outcome => {
                    let (phase, safe_kind) = match &diagnostic_outcome {
                        Err(cause) => ("read_credential_snapshot", cause.diagnostic_kind()),
                        Ok(_) => {
                            ("validate_credential_snapshot", SafeErrorKind::DEPENDENCY_MISMATCH)
                        }
                    };
                    diagnostics::warn(
                        "admin_dashboard",
                        phase,
                        safe_kind,
                        DiagnosticContext::default(),
                    );
                    drop(diagnostic_outcome);
                    None
                }
            },
            None => None,
        }
    };
    let (worker, mail, realtime, owner_snapshot, alerting, credentials) =
        tokio::join!(worker, mail, realtime, owner_snapshot, alerting, credentials);
    let (worker, external_accounts) = worker;
    let mail_section = mail.as_ref().map_or_else(sections::unknown_section, |snapshot| {
        sections::map_mail_section(snapshot, state.dashboard_mailbox_keys())
    });
    let credentials_section = credentials.as_ref().map_or_else(
        || sections::unknown_section_with_code("source_unavailable"),
        |credentials| {
            sections::map_credentials(
                credentials.clone(),
                &snapshot.process,
                state.dashboard_worlds(),
            )
        },
    );
    let external_accounts_section = sections::map_external_accounts(
        external_accounts,
        mail.as_ref(),
        credentials.as_ref(),
        state.dashboard_worlds(),
    );
    let fulfillment = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.fulfillment.map(|value| {
                sections::map_fulfillment(
                    value,
                    &snapshot.collection_overdue_items,
                    alert_policy,
                    projection,
                )
            })
        })
        .unwrap_or_else(sections::unknown_section);
    let delivery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.delivery.map(|value| sections::map_delivery(value, alert_policy))
        })
        .unwrap_or_else(sections::unknown_section);
    let recovery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.recovery.clone().map(|value| sections::map_recovery(value, alert_policy))
        })
        .unwrap_or_else(sections::unknown_section);
    let reconciliation = owner_snapshot
        .and_then(|snapshot| {
            snapshot.reconciliation.map(|value| sections::map_reconciliation(value, projection))
        })
        .unwrap_or_else(sections::unknown_section);
    let response = mapping::map_dashboard(
        snapshot,
        worker,
        AdminDashboardSections {
            mail: mail_section,
            realtime,
            credentials: credentials_section,
            external_accounts: external_accounts_section,
            delivery,
            fulfillment,
            recovery,
            alerting,
            reconciliation,
        },
    );
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("管理概览已加载", response)))
        .into_response()
}

fn map_error(error: HealthError) -> Response {
    let diagnostic_kind = diagnostics::health_kind(&error);
    let body = || match error {
        HealthError::InvalidCommand | HealthError::StaleObservation => internal_error(),
        HealthError::InvalidData | HealthError::Storage { .. } => {
            storage_unavailable("健康 dashboard 暂不可用")
        }
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_dashboard",
        "respond_health_failure",
        diagnostic_kind,
        response,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use auction_application::core::ApplicationFuture;
    use auction_application::operations::{
        OperationsAlertCollectionOverdueItem, OperationsAlertCollectionOverdueRead,
        OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
        OperationsAlertReconciliationSnapshot, OperationsAlertReconciliationWorld,
        OperationsAlertSnapshot, OperationsAlertSourceReader,
    };
    use auction_domain::SettlementObligationKind;
    use auction_observability::{
        AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthFuture, HealthReader,
        HealthSummary, HeartbeatStatus, ProcessSnapshot, ServiceName,
    };
    use auction_time::Timestamp;
    use axum::http::StatusCode;
    use serde_json::Value;
    use uuid::Uuid;

    use crate::server::contract_fixtures::{
        AdminFixturePorts, admin_get, response_json, time, wire_fixture,
    };

    const COLLECTION_THRESHOLD: Duration = Duration::from_secs(120);

    struct MissingHeartbeats;

    impl HealthReader for MissingHeartbeats {
        fn snapshot(&self) -> HealthFuture<'_, AdminDashboardSnapshot> {
            let missing_services =
                vec![ServiceName::Public, ServiceName::Admin, ServiceName::Worker];
            let health = HealthSummary {
                status: HeartbeatStatus::Degraded,
                readiness: false,
                audit_failure_count: 0,
                missing_services: missing_services.clone(),
            };
            Box::pin(async move {
                Ok(AdminDashboardSnapshot {
                    health: health.clone(),
                    process: DashboardSection {
                        status: DashboardStatus::Degraded,
                        observed_at: None,
                        error_code: None,
                        data: Some(ProcessSnapshot {
                            processes: Vec::new(),
                            summary: health,
                            observed_at: None,
                            missing_services,
                        }),
                    },
                })
            })
        }
    }

    struct FixtureOperations(OperationsAlertCollectionOverdueRead);

    impl OperationsAlertSourceReader for FixtureOperations {
        fn snapshot(
            &self,
            _now: Timestamp,
            collection: OperationsAlertCollectionOverdueRead,
        ) -> ApplicationFuture<'_, OperationsAlertSnapshot> {
            assert_eq!(collection, self.0);
            let overdue = collection.with_items();
            let collection_overdue_items = if overdue {
                vec![OperationsAlertCollectionOverdueItem {
                    obligation_id: Uuid::from_u128(0x4000_8000_0000_0000_0032),
                    obligation_kind: SettlementObligationKind::WinnerEquipmentHandoff,
                    auction_world: "main".to_owned(),
                    auction_sequence: 5,
                    lot_number: 12,
                    equipment_category: "Staff".to_owned(),
                    equipment_name: "Legendary Staff".to_owned(),
                    recipient: "ExamplePlayer".to_owned(),
                    sent_at: time(),
                    age_seconds: 180,
                    last_read_failed: true,
                }]
            } else {
                Vec::new()
            };
            Box::pin(async move {
                Ok(OperationsAlertSnapshot {
                    fulfillment: Some(OperationsAlertFulfillmentSnapshot {
                        pending: 2,
                        awaiting_collection: 3,
                        awaiting_collection_unreadable: 1,
                        oldest_backlog_age_seconds: 45,
                        oldest_awaiting_collection_age_seconds: 180,
                        oldest_awaiting_collection_backlog_age_seconds: 200,
                        blocked: 0,
                        failed: 0,
                        manual_reconcile: 0,
                        waiting_payment: 1,
                        waiting_prerequisite: 0,
                        oldest_prerequisite_wait_age_seconds: 0,
                        collection_overdue: i64::from(overdue),
                        settlement_return_pending: 0,
                        oldest_settlement_return_pending_age_seconds: 0,
                        settlement_return_quarantined: 0,
                        oldest_settlement_return_quarantined_age_seconds: 0,
                        settlement_return_take_unknown: Default::default(),
                    }),
                    collection_overdue_items,
                    reconciliation: Some(OperationsAlertReconciliationSnapshot {
                        custody_report_age_seconds: Some(15),
                        solvency_report_age_seconds: Some(25),
                        worlds: vec![OperationsAlertReconciliationWorld {
                            world: "main",
                            custody_missing_count: 0,
                            custody_in_transit_count: 3,
                            solvency_shortfall_credits: 0,
                            solvency_in_transit_count: 1,
                            solvency_in_transit_credits: 25_000,
                        }],
                    }),
                    ..Default::default()
                })
            })
        }
    }

    fn alert_policy() -> OperationsAlertPolicy {
        OperationsAlertPolicy {
            source_timeout: Duration::from_secs(5),
            batch_window: Duration::from_secs(120),
            cooldown: Duration::from_secs(1_800),
            claim_lease: Duration::from_secs(120),
            retry_delay: Duration::from_secs(300),
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4_096,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_threshold: Duration::from_secs(900),
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 100,
            fulfillment_backlog_age_threshold: Duration::from_secs(900),
            fulfillment_collection_overdue_age_threshold: COLLECTION_THRESHOLD,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_threshold: Duration::from_secs(900),
        }
    }

    #[tokio::test]
    async fn dashboard_contract_fixtures() {
        use OperationsAlertCollectionOverdueRead::{CountWithItems, Skip};

        for (name, url, collection) in [
            ("admin/dashboard", "/api/v1/admin/dashboard", Skip),
            (
                "admin/dashboard-collection-wait",
                "/api/v1/admin/dashboard?include=collection_wait",
                CountWithItems(COLLECTION_THRESHOLD),
            ),
        ] {
            let state = AdminFixturePorts {
                health: Some(Arc::new(MissingHeartbeats)),
                alert_source: Some(Arc::new(FixtureOperations(collection))),
                ..Default::default()
            }
            .state()
            .with_operations_alert_policy(alert_policy());
            let started = Timestamp::now().trunc_subsecs(6);
            let actual = response_json(StatusCode::OK, admin_get(state, url).await).await;
            let observed_window = started..=Timestamp::now();
            let mut expected = wire_fixture(name);
            for section in ["fulfillment", "reconciliation"] {
                let observed = &actual["data"][section]["observed_at"];
                let parsed = Timestamp::parse_from_rfc3339(observed.as_str().expect("观测时刻"))
                    .expect("RFC 3339 观测时刻");
                assert!(observed_window.contains(&parsed));
                assert_eq!(&actual["data"][section]["data"]["observed_at"], observed);
                // 核对生产时钟和两处字段一致后，只替换期望样本中的时间占位值。
                let expected_section = &mut expected["data"][section];
                let fixed = Value::String(crate::mapping::timestamp(time()));
                assert_eq!(expected_section["observed_at"], fixed);
                assert_eq!(expected_section["data"]["observed_at"], fixed);
                expected_section["observed_at"] = observed.clone();
                expected_section["data"]["observed_at"] = observed.clone();
            }
            assert_eq!(actual, expected, "{name}");
        }
    }
}
