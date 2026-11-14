//! 通知指标在 delivery 终态边界的映射；只读取冻结 payload，不改变发送结果。
//!
//! 记录失败必须是纯观测：调用方在终态已落库后同步调用本模块，记录器实现不得返回错误，
//! 也不得因此重试发送、重排工作项或修改 delivery 状态。

use auction_application::delivery::DeliveryWorkClaim;
use auction_application::notification_metrics::{
    NotificationFailure, NotificationKind, NotificationRecorder, NotificationStage,
};
use chrono::{DateTime, Utc};

/// 每次发送的估算邮资消耗（Credits）。
///
/// 尚未核实 HV 的实际邮资金额与计费方式，因此保持 0：不伪造估算，指标族仍按种类
/// 预置零值。核实后在此填入真实金额，`auction_notification_postage_estimate_credits_total`
/// 即按发送次数累计，供偿付能力报告使用。
pub(super) const POSTAGE_ESTIMATE_CREDITS: u64 = 0;

/// 由 claim 的冻结载荷识别通知种类；Return/Take 与未知模板不产生指标。
pub(super) fn kind_of(claim: &DeliveryWorkClaim) -> Option<NotificationKind> {
    let operation = claim.payload.get("operation").and_then(serde_json::Value::as_str);
    let template = claim.payload.get("template").and_then(serde_json::Value::as_str);
    NotificationKind::from_delivery(&claim.origin_kind, operation, template)
}

/// 创建到终态的送达延迟；负值（宿主时钟回拨）按零处理，不写入虚假的历史分布。
pub(super) fn latency(
    created_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
) -> std::time::Duration {
    (completed_at - created_at).to_std().unwrap_or_default()
}

pub(super) fn record_sent(
    recorder: &dyn NotificationRecorder,
    kind: Option<NotificationKind>,
    latency: std::time::Duration,
) {
    let Some(kind) = kind else { return };
    recorder.observe_stage(kind, NotificationStage::Sent);
    recorder.observe_delivery_latency(kind, latency);
    recorder.observe_postage_estimate(kind, POSTAGE_ESTIMATE_CREDITS);
}

/// 记录终态失败；`metadata` 只读取其中的 `stage`、`category` 与 `reason_code` 闭集字段。
pub(super) fn record_failed(
    recorder: &dyn NotificationRecorder,
    kind: Option<NotificationKind>,
    failure_code: &str,
    metadata: &serde_json::Value,
    latency: std::time::Duration,
) {
    let Some(kind) = kind else { return };
    recorder.observe_stage(kind, NotificationStage::Failed);
    recorder.observe_delivery_latency(kind, latency);
    let field = |key: &str| metadata.get(key).and_then(serde_json::Value::as_str);
    recorder.observe_failure(
        kind,
        NotificationFailure::from_delivery_failure(
            failure_code,
            field("reason_code"),
            field("category"),
        ),
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use auction_application::notification_metrics::{
        NotificationKind, NotificationRecorder, NotificationStage,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct FakeRecorder {
        stages: Mutex<Vec<(NotificationKind, NotificationStage)>>,
        failures: Mutex<Vec<(NotificationKind, NotificationFailure)>>,
        latencies: Mutex<Vec<(NotificationKind, u64)>>,
    }

    impl NotificationRecorder for FakeRecorder {
        fn observe_stage(&self, kind: NotificationKind, stage: NotificationStage) {
            self.stages.lock().unwrap().push((kind, stage));
        }

        fn observe_failure(&self, kind: NotificationKind, failure: NotificationFailure) {
            self.failures.lock().unwrap().push((kind, failure));
        }

        fn observe_delivery_latency(&self, kind: NotificationKind, latency: std::time::Duration) {
            self.latencies.lock().unwrap().push((kind, latency.as_secs()));
        }

        fn observe_postage_estimate(&self, _kind: NotificationKind, _credits: u64) {}
    }

    fn claim(origin_kind: &str, operation: &str, template: &str) -> DeliveryWorkClaim {
        DeliveryWorkClaim {
            delivery_id: uuid::Uuid::nil(),
            command_id: uuid::Uuid::nil(),
            request_hash: auction_application::core::RequestHash::new([1; 32]),
            origin_kind: origin_kind.to_owned(),
            origin_id: "origin".to_owned(),
            recipient: "player".to_owned(),
            payload: json!({"operation": operation, "template": template}),
            lease_token: uuid::Uuid::nil(),
            lease_until: Utc.timestamp_opt(0, 0).unwrap(),
            fencing_version: 1,
            attempt_count: 1,
            worker_id: "worker".to_owned(),
            claimed_at: Utc.timestamp_opt(0, 0).unwrap(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    #[test]
    fn sent_records_stage_latency_and_postage() {
        let recorder = FakeRecorder::default();
        let claim = claim("settlement", "send", "settlement_seller_payout");
        let kind = kind_of(&claim);
        assert_eq!(kind, Some(NotificationKind::SettlementSellerPayout));
        record_sent(
            &recorder,
            kind,
            latency(Utc.timestamp_opt(100, 0).unwrap(), Utc.timestamp_opt(190, 0).unwrap()),
        );
        assert_eq!(
            recorder.stages.lock().unwrap().as_slice(),
            &[(kind.unwrap(), NotificationStage::Sent)]
        );
        assert_eq!(recorder.latencies.lock().unwrap().as_slice(), &[(kind.unwrap(), 90)]);
    }

    #[test]
    fn failure_maps_insufficient_postage_from_metadata() {
        let recorder = FakeRecorder::default();
        let claim = claim("mail_intake", "send", "register_key");
        let kind = kind_of(&claim);
        record_failed(
            &recorder,
            kind,
            "send_rejected",
            &json!({"stage": "send", "category": "write_rejected", "reason_code": "insufficient_postage"}),
            std::time::Duration::from_secs(5),
        );
        assert_eq!(
            recorder.failures.lock().unwrap().as_slice(),
            &[(NotificationKind::RegisterKey, NotificationFailure::InsufficientPostage)]
        );
    }

    #[test]
    fn non_notification_operations_are_skipped() {
        let recorder = FakeRecorder::default();
        let claim = claim("mail_intake", "return", "register_key");
        assert_eq!(kind_of(&claim), None);
        record_sent(&recorder, None, std::time::Duration::from_secs(1));
        record_failed(&recorder, None, "send_rejected", &json!({}), std::time::Duration::ZERO);
        assert!(recorder.stages.lock().unwrap().is_empty());
        assert!(recorder.failures.lock().unwrap().is_empty());
        assert!(recorder.latencies.lock().unwrap().is_empty());
    }

    #[test]
    fn negative_clock_skew_becomes_zero_latency() {
        assert_eq!(
            latency(Utc.timestamp_opt(100, 0).unwrap(), Utc.timestamp_opt(90, 0).unwrap()),
            std::time::Duration::ZERO
        );
    }
}
