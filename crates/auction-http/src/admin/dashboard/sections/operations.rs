//! Admin dashboard 履约、投递和 recovery 水位 section。

use auction_application::operations::{
    OperationsAlertCollectionOverdueItem, OperationsAlertDeliverySnapshot,
    OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertReconciliationSnapshot, OperationsAlertRecoverySnapshot, assess_delivery,
    assess_fulfillment, assess_recovery,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::fallback::unknown_section;
use crate::admin::dashboard::projection::DashboardProjection;
use crate::mapping::timestamp;

/// 履约分区：缺省响应保持已发布的字段闭集与口径；新页面显式请求的等待领取投影单列等待领取。
pub(crate) fn map_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
    collection_overdue_items: &[OperationsAlertCollectionOverdueItem],
    policy: Option<&OperationsAlertPolicy>,
    projection: DashboardProjection,
) -> AdminDashboardSectionResponse {
    match projection {
        DashboardProjection::Baseline => map_fulfillment_baseline(snapshot, policy),
        DashboardProjection::CollectionWait => {
            map_fulfillment_collection_wait(snapshot, collection_overdue_items, policy)
        }
    }
}

/// 已发布的缺省口径：已寄出等待领取的义务仍计入 `pending`，积压年龄按积压时刻把它们并回，
/// 分区状态按这一口径与告警阈值判定，不评估长期未领取；只输出基线字段闭集，已打开的旧页面
/// 照常解码。计数合并溢出说明水位损坏，分区按 unknown 失败关闭。
fn map_fulfillment_baseline(
    snapshot: OperationsAlertFulfillmentSnapshot,
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let Some(pending) = snapshot.pending.checked_add(snapshot.awaiting_collection) else {
        return unknown_section();
    };
    let baseline = OperationsAlertFulfillmentSnapshot {
        pending,
        awaiting_collection: 0,
        awaiting_collection_unreadable: 0,
        oldest_backlog_age_seconds: snapshot
            .oldest_backlog_age_seconds
            .max(snapshot.oldest_awaiting_collection_backlog_age_seconds),
        oldest_awaiting_collection_age_seconds: 0,
        oldest_awaiting_collection_backlog_age_seconds: 0,
        collection_overdue: 0,
        ..snapshot
    };
    let observed_at = timestamp(auction_time::Timestamp::now());
    let degraded =
        policy.map(|policy| assess_fulfillment(baseline, policy)).is_some_and(|assessment| {
            assessment.manual_reconcile || assessment.failed || assessment.backlog
        });
    AdminDashboardSectionResponse {
        status: if degraded {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "pending": baseline.pending,
            "blocked": baseline.blocked,
            "failed": baseline.failed,
            "manual_reconcile": baseline.manual_reconcile,
            "waiting_payment": baseline.waiting_payment,
            "waiting_prerequisite": baseline.waiting_prerequisite,
            "oldest_backlog_age_seconds": baseline.oldest_backlog_age_seconds,
            "oldest_prerequisite_wait_age_seconds": baseline.oldest_prerequisite_wait_age_seconds,
        })),
    }
}

/// 等待领取投影：已寄出等待领取的义务单列，不计入 `pending` 与积压；长期未领取的超期件数与
/// 清单出自同一只读快照，阈值即 Admin 生效的告警策略，有超期件时分区降级；没有告警策略时不
/// 声称超期：计数为 0、清单为空。清单只含业务标识与展示字段。
fn map_fulfillment_collection_wait(
    snapshot: OperationsAlertFulfillmentSnapshot,
    collection_overdue_items: &[OperationsAlertCollectionOverdueItem],
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(auction_time::Timestamp::now());
    let assessment = policy.map(|policy| assess_fulfillment(snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.failed) {
        Some("fulfillment_failed")
    } else if assessment.is_some_and(|assessment| assessment.backlog) {
        Some("fulfillment_backlog")
    } else if assessment.is_some_and(|assessment| assessment.collection_overdue) {
        Some("fulfillment_collection_overdue")
    } else {
        None
    };
    // 长期未领取与告警扫描同一阈值判定；没有告警策略时不声称超期。
    let (collection_overdue_count, collection_overdue_items) = if policy.is_some() {
        (snapshot.collection_overdue, collection_overdue_items)
    } else {
        (0, &[][..])
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "pending": snapshot.pending,
            "blocked": snapshot.blocked,
            "failed": snapshot.failed,
            "manual_reconcile": snapshot.manual_reconcile,
            "waiting_payment": snapshot.waiting_payment,
            "waiting_prerequisite": snapshot.waiting_prerequisite,
            "awaiting_collection": snapshot.awaiting_collection,
            "awaiting_collection_unreadable": snapshot.awaiting_collection_unreadable,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
            "oldest_prerequisite_wait_age_seconds": snapshot.oldest_prerequisite_wait_age_seconds,
            "oldest_awaiting_collection_age_seconds":
                snapshot.oldest_awaiting_collection_age_seconds,
            "collection_overdue_count": collection_overdue_count,
            "collection_overdue_items": collection_overdue_items
                .iter()
                .map(|item| json!({
                    "obligation_id": item.obligation_id,
                    "obligation_kind": item.obligation_kind.as_str(),
                    "world": item.auction_world,
                    "auction_sequence": item.auction_sequence,
                    "lot_number": item.lot_number,
                    "equipment_category": item.equipment_category,
                    "equipment_name": item.equipment_name,
                    "recipient": item.recipient,
                    "sent_at": timestamp(item.sent_at),
                    "age_seconds": item.age_seconds,
                    "last_read_failed": item.last_read_failed,
                }))
                .collect::<Vec<_>>(),
        })),
    }
}

pub(crate) fn map_delivery(
    snapshot: OperationsAlertDeliverySnapshot,
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(auction_time::Timestamp::now());
    let assessment = policy.map(|policy| assess_delivery(snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.returned_mail) {
        Some("delivery_returned_mail")
    } else if assessment.is_some_and(|assessment| assessment.queue_latency) {
        Some("delivery_queue_latency")
    } else if assessment.is_some_and(|assessment| assessment.pending) {
        Some("delivery_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "water": {
                "pending": snapshot.pending,
                "failed": snapshot.failed,
                "manual_reconcile": snapshot.manual_reconcile,
                "oldest_pending_age_seconds": snapshot.oldest_pending_age_seconds,
                "oldest_manual_reconcile_age_seconds": snapshot.oldest_manual_reconcile_age_seconds,
                "last_update_at": snapshot.last_update_at.map(timestamp),
            },
        })),
    }
}

pub(crate) fn map_recovery(
    snapshot: OperationsAlertRecoverySnapshot,
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(auction_time::Timestamp::now());
    let assessment = policy.map(|policy| assess_recovery(&snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.failed_action) {
        Some("recovery_action_failed")
    } else if assessment.is_some_and(|assessment| assessment.payment_overdue) {
        Some("payment_overdue")
    } else if assessment.is_some_and(|assessment| assessment.payment_mail_unsent) {
        Some("payment_mail_unsent")
    } else if assessment.is_some_and(|assessment| assessment.relist_pending) {
        Some("relist_pending")
    } else if snapshot.worlds.iter().any(|world| {
        // 只记账策略将两项现金来源同时标为不适用；混合或未知状态仍需告警。
        !matches!(
            (world.balance_status.as_str(), world.evidence_status.as_str()),
            ("ok", "fresh") | ("not_applicable", "not_applicable")
        )
    }) {
        Some("recovery_source_degraded")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "worlds": snapshot.worlds.into_iter().map(|world| json!({
                "world": world.world,
                "balance_credits": world.balance_credits,
                "balance_status": world.balance_status,
                "evidence_status": world.evidence_status,
                "balance_observed_at": world.balance_observed_at.map(timestamp),
                "evidence_last_success_at": world.evidence_last_success_at.map(timestamp),
                "updated_at": timestamp(world.updated_at),
            })).collect::<Vec<_>>(),
            "payment_overdue_count": snapshot.payment_overdue_count,
            "relist_pending_count": snapshot.relist_pending_count,
            "failed_action_count": snapshot.failed_action_count,
            "manual_reconcile_count": snapshot.manual_reconcile_count,
            "oldest_overdue_age_seconds": snapshot.oldest_overdue_age_seconds,
        })),
    }
}

/// 资产对账与偿付能力 section；报告缺失时明确 unknown，不用空数据冒充无差异。缺省响应每个
/// world 只给缺失件数与待付缺口（已发布闭集）；等待领取投影追加在途装备件数与在途货款笔数和
/// 金额。在途只展示，不参与降级判定：它们已随邮件离开 Bot，等待收件人领取。
pub(crate) fn map_reconciliation(
    snapshot: OperationsAlertReconciliationSnapshot,
    projection: DashboardProjection,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(auction_time::Timestamp::now());
    let (Some(custody_report_age_seconds), Some(solvency_report_age_seconds)) =
        (snapshot.custody_report_age_seconds, snapshot.solvency_report_age_seconds)
    else {
        return AdminDashboardSectionResponse {
            status: DashboardStatus::Unknown.as_str().to_owned(),
            observed_at: None,
            error_code: Some("reconciliation_report_missing".to_owned()),
            data: None,
        };
    };
    let drifted = snapshot
        .worlds
        .iter()
        .any(|world| world.custody_missing_count > 0 || world.solvency_shortfall_credits > 0);
    AdminDashboardSectionResponse {
        status: if drifted {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "custody_report_age_seconds": custody_report_age_seconds,
            "solvency_report_age_seconds": solvency_report_age_seconds,
            "worlds": snapshot.worlds.iter().map(|world| match projection {
                DashboardProjection::Baseline => json!({
                    "world": world.world,
                    "custody_missing_count": world.custody_missing_count,
                    "solvency_shortfall_credits": world.solvency_shortfall_credits,
                }),
                DashboardProjection::CollectionWait => json!({
                    "world": world.world,
                    "custody_missing_count": world.custody_missing_count,
                    "custody_in_transit_count": world.custody_in_transit_count,
                    "solvency_shortfall_credits": world.solvency_shortfall_credits,
                    "solvency_in_transit_count": world.solvency_in_transit_count,
                    "solvency_in_transit_credits": world.solvency_in_transit_credits,
                }),
            }).collect::<Vec<_>>(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auction_application::operations::{
        OperationsAlertReconciliationWorld, OperationsAlertRecoverySnapshot,
    };
    use auction_domain::SettlementObligationKind;
    use serde::Deserialize;
    use uuid::Uuid;

    use super::{
        DashboardProjection, OperationsAlertCollectionOverdueItem, OperationsAlertDeliverySnapshot,
        OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
        OperationsAlertReconciliationSnapshot, map_delivery, map_fulfillment, map_reconciliation,
        map_recovery,
    };

    /// 已发布页面（基线 b2300b46）的履约分区闭集；缺省响应必须能被它解码。
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BaselineFulfillmentData {
        pending: i64,
        blocked: i64,
        failed: i64,
        manual_reconcile: i64,
        waiting_payment: i64,
        waiting_prerequisite: i64,
        observed_at: String,
        oldest_backlog_age_seconds: i64,
        oldest_prerequisite_wait_age_seconds: i64,
    }

    /// 已发布页面（基线 b2300b46）的对账分区闭集。
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BaselineReconciliationData {
        observed_at: String,
        custody_report_age_seconds: i64,
        solvency_report_age_seconds: i64,
        worlds: Vec<BaselineReconciliationWorld>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BaselineReconciliationWorld {
        world: String,
        custody_missing_count: i64,
        solvency_shortfall_credits: i64,
    }

    fn policy() -> OperationsAlertPolicy {
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
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_threshold: Duration::from_secs(900),
            fulfillment_collection_overdue_age_threshold: Duration::from_secs(259_200),
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_threshold: Duration::from_secs(900),
        }
    }

    fn fulfillment(
        pending: i64,
        waiting_payment: i64,
        waiting_prerequisite: i64,
        age: i64,
        prerequisite_age: i64,
    ) -> OperationsAlertFulfillmentSnapshot {
        OperationsAlertFulfillmentSnapshot {
            pending,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment,
            waiting_prerequisite,
            awaiting_collection: 0,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: age,
            oldest_prerequisite_wait_age_seconds: prerequisite_age,
            oldest_awaiting_collection_age_seconds: 0,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 0,
            oldest_settlement_return_quarantined_age_seconds: 0,
            settlement_return_take_unknown: Default::default(),
        }
    }

    /// 没有等待领取时两种投影的积压判定相同，都与告警扫描共用阈值。
    #[test]
    fn fulfillment_uses_shared_scanner_thresholds_and_ignores_payment_waiting() {
        let policy = policy();
        for projection in [DashboardProjection::Baseline, DashboardProjection::CollectionWait] {
            let map = |snapshot| map_fulfillment(snapshot, &[], Some(&policy), projection);
            assert_eq!(map(fulfillment(4, 4, 0, 10, 0)).status, "degraded", "{projection:?}");
            assert_eq!(map(fulfillment(0, 0, 0, 900, 0)).status, "degraded", "{projection:?}");
            assert_eq!(map(fulfillment(0, 100, 0, 0, 0)).status, "ok", "{projection:?}");
            assert_eq!(map(fulfillment(0, 0, 1, 0, 900)).status, "degraded", "{projection:?}");
        }
    }

    /// 缺省响应保持已发布的口径与字段闭集：已寄出等待领取的义务仍计入 `pending`，积压年龄按
    /// 积压时刻并回，分区按这一口径降级，不输出等待领取字段与清单，基线页面的闭集 DTO 能解码；
    /// 扩展投影单列等待领取，等待领取本身不降级，基线页面的闭集 DTO 解码它会失败。
    #[test]
    fn baseline_projection_keeps_the_published_fulfillment_contract() {
        let policy = policy();
        let snapshot = OperationsAlertFulfillmentSnapshot {
            awaiting_collection: 3,
            awaiting_collection_unreadable: 1,
            oldest_awaiting_collection_age_seconds: 86_400,
            oldest_awaiting_collection_backlog_age_seconds: 90_000,
            ..fulfillment(0, 2, 1, 0, 300)
        };
        let baseline = map_fulfillment(snapshot, &[], Some(&policy), DashboardProjection::Baseline);
        assert_eq!(baseline.status, "degraded", "缺省口径下等待领取仍是积压");
        let value = baseline.data.expect("履约数据");
        let mut keys =
            value.as_object().expect("履约数据是对象").keys().cloned().collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "blocked",
                "failed",
                "manual_reconcile",
                "observed_at",
                "oldest_backlog_age_seconds",
                "oldest_prerequisite_wait_age_seconds",
                "pending",
                "waiting_payment",
                "waiting_prerequisite",
            ],
            "缺省响应的键集合等于基线"
        );
        let data: BaselineFulfillmentData =
            serde_json::from_value(value).expect("基线页面的闭集 DTO 必须能解码缺省响应");
        assert_eq!(
            (
                data.pending,
                data.blocked,
                data.failed,
                data.manual_reconcile,
                data.waiting_payment,
                data.waiting_prerequisite,
                data.oldest_backlog_age_seconds,
                data.oldest_prerequisite_wait_age_seconds,
            ),
            (3, 0, 0, 0, 2, 1, 90_000, 300),
            "等待领取并回 pending 与积压年龄"
        );
        assert_eq!(Some(data.observed_at), baseline.observed_at);
        let without_policy = map_fulfillment(snapshot, &[], None, DashboardProjection::Baseline);
        assert_eq!(without_policy.status, "ok", "没有告警策略时不评估");

        let extended =
            map_fulfillment(snapshot, &[], Some(&policy), DashboardProjection::CollectionWait);
        assert_eq!(extended.status, "ok", "等待领取投影下等待领取不是积压");
        let value = extended.data.expect("履约数据");
        assert_eq!(value["pending"], 0);
        assert_eq!(value["awaiting_collection"], 3);
        assert_eq!(value["oldest_backlog_age_seconds"], 0);
        assert!(
            serde_json::from_value::<BaselineFulfillmentData>(value).is_err(),
            "扩展字段只能经显式 include 输出，基线页面按闭集解码会失败"
        );

        // 合并计数溢出说明水位损坏：缺省分区按 unknown 失败关闭，不截断。
        let corrupt = OperationsAlertFulfillmentSnapshot { pending: i64::MAX, ..snapshot };
        let corrupt = map_fulfillment(corrupt, &[], Some(&policy), DashboardProjection::Baseline);
        assert_eq!((corrupt.status.as_str(), corrupt.data), ("unknown", None));
    }

    /// 等待领取单列为独立分组，计数、读取失败数与最老等待年龄原样投影；主计数是达到长期未领取
    /// 阈值的件数（超期件数），总等待数只作背景。有超期件时降级，并原样投影同一快照里的清单；
    /// 没有超期件不降级；没有告警策略时不声称超期。
    #[test]
    fn awaiting_collection_is_projected_separately_without_backlog() {
        let policy = policy();
        let sent_at = auction_time::Timestamp::from_timestamp(1_700_000_000, 0).expect("寄出时刻");
        let item = OperationsAlertCollectionOverdueItem {
            obligation_id: Uuid::from_u128(0x10),
            obligation_kind: SettlementObligationKind::SellerPayout,
            auction_world: "main".to_owned(),
            auction_sequence: 4,
            lot_number: 12,
            equipment_category: "Shield".to_owned(),
            equipment_name: "Radiant Buckler".to_owned(),
            recipient: "collection-seller".to_owned(),
            sent_at,
            age_seconds: 691_200,
            last_read_failed: true,
        };
        let overdue = OperationsAlertFulfillmentSnapshot {
            awaiting_collection: 36,
            awaiting_collection_unreadable: 12,
            oldest_awaiting_collection_age_seconds: 691_200,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 1,
            ..fulfillment(0, 0, 0, 0, 0)
        };
        let section = map_fulfillment(
            overdue,
            std::slice::from_ref(&item),
            Some(&policy),
            DashboardProjection::CollectionWait,
        );
        assert_eq!(section.status, "degraded", "有寄出时长达到 72 小时阈值的等待领取");
        let data = section.data.expect("履约数据");
        assert_eq!(data["pending"], 0);
        assert_eq!(data["awaiting_collection"], 36);
        assert_eq!(data["awaiting_collection_unreadable"], 12);
        assert_eq!(data["oldest_awaiting_collection_age_seconds"], 691_200);
        assert_eq!(data["collection_overdue_count"], 1, "主计数是超期件数，不是 36");
        assert_eq!(
            data["collection_overdue_items"],
            serde_json::json!([{
                "obligation_id": "00000000-0000-0000-0000-000000000010",
                "obligation_kind": "seller_payout",
                "world": "main",
                "auction_sequence": 4,
                "lot_number": 12,
                "equipment_category": "Shield",
                "equipment_name": "Radiant Buckler",
                "recipient": "collection-seller",
                "sent_at": "2023-11-14T22:13:20.000000Z",
                "age_seconds": 691_200,
                "last_read_failed": true
            }])
        );

        // 没有超期件只单列，不降级；没有告警策略时不声称超期，也不透出清单。
        let fresh = OperationsAlertFulfillmentSnapshot {
            oldest_awaiting_collection_age_seconds: 259_199,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            ..overdue
        };
        let section =
            map_fulfillment(fresh, &[], Some(&policy), DashboardProjection::CollectionWait);
        assert_eq!(section.status, "ok");
        let data = section.data.expect("履约数据");
        assert_eq!(data["collection_overdue_count"], 0);
        assert_eq!(data["collection_overdue_items"], serde_json::json!([]));
        let without_policy = map_fulfillment(
            overdue,
            std::slice::from_ref(&item),
            None,
            DashboardProjection::CollectionWait,
        );
        assert_eq!(without_policy.status, "ok");
        let data = without_policy.data.expect("履约数据");
        assert_eq!(data["collection_overdue_count"], 0);
        assert_eq!(data["collection_overdue_items"], serde_json::json!([]));
    }

    #[test]
    fn delivery_and_recovery_use_the_same_scanner_thresholds() {
        let policy = policy();
        let fresh_delivery = map_delivery(
            OperationsAlertDeliverySnapshot {
                pending: 5,
                failed: 0,
                manual_reconcile: 0,
                returned_mail_manual_reconcile: 0,
                oldest_pending_age_seconds: 10,
                oldest_manual_reconcile_age_seconds: 0,
                oldest_returned_mail_age_seconds: 0,
                claimed: 0,
                oldest_claimed_age_seconds: 0,
                last_update_at: None,
            },
            Some(&policy),
        );
        assert_eq!(fresh_delivery.status, "degraded");
        let old_delivery = map_delivery(
            OperationsAlertDeliverySnapshot {
                pending: 5,
                failed: 0,
                manual_reconcile: 0,
                returned_mail_manual_reconcile: 0,
                oldest_pending_age_seconds: 900,
                oldest_manual_reconcile_age_seconds: 0,
                oldest_returned_mail_age_seconds: 0,
                claimed: 0,
                oldest_claimed_age_seconds: 0,
                last_update_at: None,
            },
            Some(&policy),
        );
        assert_eq!(old_delivery.status, "degraded");
        let fresh_recovery = map_recovery(
            OperationsAlertRecoverySnapshot {
                payment_overdue_count: 4,
                payment_mail_unsent_count: 0,
                relist_pending_count: 0,
                failed_action_count: 0,
                manual_reconcile_count: 0,
                oldest_overdue_age_seconds: 10,
                oldest_payment_mail_unsent_age_seconds: 0,
                worlds: Vec::new(),
            },
            Some(&policy),
        );
        assert_eq!(fresh_recovery.status, "degraded");
        let relist_only = map_recovery(
            OperationsAlertRecoverySnapshot {
                payment_overdue_count: 0,
                payment_mail_unsent_count: 0,
                relist_pending_count: 1,
                failed_action_count: 0,
                manual_reconcile_count: 0,
                oldest_overdue_age_seconds: 0,
                oldest_payment_mail_unsent_age_seconds: 0,
                worlds: Vec::new(),
            },
            Some(&policy),
        );
        assert_eq!(relist_only.status, "degraded");
    }

    /// 在途装备与在途货款原样投影但不使对账分区降级；缺失与缺口仍按差异降级。
    #[test]
    fn reconciliation_in_transit_is_projected_without_degrading() {
        let world = |world, custody_in_transit_count, solvency_in_transit_credits| {
            OperationsAlertReconciliationWorld {
                world,
                custody_missing_count: 0,
                custody_in_transit_count,
                solvency_shortfall_credits: 0,
                solvency_in_transit_count: i64::from(solvency_in_transit_credits > 0),
                solvency_in_transit_credits,
            }
        };
        let snapshot = OperationsAlertReconciliationSnapshot {
            custody_report_age_seconds: Some(60),
            solvency_report_age_seconds: Some(60),
            worlds: vec![world("isekai", 0, 0), world("main", 5, 38_668_000)],
        };
        let section = map_reconciliation(snapshot.clone(), DashboardProjection::CollectionWait);
        assert_eq!(section.status, "ok", "在途不是差异");
        let data = section.data.expect("对账数据");
        assert_eq!(data["worlds"][1]["custody_in_transit_count"], 5);
        assert_eq!(data["worlds"][1]["solvency_in_transit_count"], 1);
        assert_eq!(data["worlds"][1]["solvency_in_transit_credits"], 38_668_000);
        assert!(
            serde_json::from_value::<BaselineReconciliationData>(data).is_err(),
            "在途字段只能经显式 include 输出"
        );

        // 缺省响应保持已发布闭集：每个 world 只有缺失件数与待付缺口，基线页面的 DTO 能解码。
        let baseline = map_reconciliation(snapshot.clone(), DashboardProjection::Baseline);
        assert_eq!(baseline.status, "ok");
        let data: BaselineReconciliationData =
            serde_json::from_value(baseline.data.expect("对账数据"))
                .expect("基线页面的闭集 DTO 必须能解码缺省响应");
        assert_eq!(Some(data.observed_at), baseline.observed_at);
        assert_eq!((data.custody_report_age_seconds, data.solvency_report_age_seconds), (60, 60));
        assert_eq!(
            data.worlds
                .iter()
                .map(|world| {
                    (
                        world.world.as_str(),
                        world.custody_missing_count,
                        world.solvency_shortfall_credits,
                    )
                })
                .collect::<Vec<_>>(),
            [("isekai", 0, 0), ("main", 0, 0)]
        );

        let mut drifted = snapshot;
        drifted.worlds[1].custody_missing_count = 1;
        for projection in [DashboardProjection::Baseline, DashboardProjection::CollectionWait] {
            assert_eq!(map_reconciliation(drifted.clone(), projection).status, "degraded");
        }
    }
}
