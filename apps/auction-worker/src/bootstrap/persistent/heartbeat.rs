//! Worker 心跳详情：Admin 据此说明停在待重新核验或待领回的结算退件为何不前进。

use auction_application::admin::{WORKER_COLD_CONFIG_HEARTBEAT_KEY, WorkerColdConfigDigest};
use auction_jobs::WorkerControlHandle;
use auction_observability::HeartbeatPublisher;
use serde_json::Value;

/// 结算退件 Take 可能发生的世界闭集，按名称排序。
const TAKE_WORLDS: [&str; 2] = ["isekai", "main"];

/// 摘要与本次心跳的观察序号一起落库。每次发布都按同一控制快照判断资格，
/// 启动失败或进入排空后不能发布新的可比较证据（不变量 5）。
pub(super) fn with_cold_config(
    publisher: HeartbeatPublisher,
    control: WorkerControlHandle,
    digest: WorkerColdConfigDigest,
    capabilities: impl Fn() -> Value + Send + Sync + 'static,
) -> HeartbeatPublisher {
    publisher.with_live_details(move || {
        let mut details = capabilities();
        // runner 完成校验后才标记 Serving；排空位不可逆。控制层降级时也保守地省略摘要。
        // 数据库就绪和 tracing 可独立降级，此处只读取控制层的进程阶段。
        if control.is_ready() {
            details[WORKER_COLD_CONFIG_HEARTBEAT_KEY] = serde_json::json!(digest);
        }
        details
    })
}

/// 组装一次心跳的详情；每次发布心跳时调用。
///
/// `take_identity_available` 与投递领取、Take 激活用同一判定（当前能否以持久凭据身份执行该世界的
/// 结算退件 Take）。数据库凭据在运行中失效或运行 owner 被撤销后，下一次心跳就不再声明该世界，
/// Admin 详情随之显示“领回能力不可用”，而不是沿用启动时的声明。
pub(super) fn details(
    forum_sync_enabled: bool,
    recheck_worlds: &[&str],
    take_identity_available: impl Fn(&str) -> bool,
) -> Value {
    let take_worlds =
        TAKE_WORLDS.into_iter().filter(|world| take_identity_available(world)).collect::<Vec<_>>();
    serde_json::json!({
        "forum_sync_enabled": forum_sync_enabled,
        "settlement_return_recheck_worlds": recheck_worlds,
        "settlement_return_take_worlds": take_worlds,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use auction_application::admin::{WORKER_COLD_CONFIG_HEARTBEAT_KEY, WorkerColdConfigDigest};
    use auction_jobs::WorkerControlHandle;
    use auction_observability::{
        HealthFuture, Heartbeat, HeartbeatCommand, HeartbeatHealthReader, HeartbeatPublisher,
        HeartbeatPublisherConfig, HeartbeatReadinessFuture, HeartbeatStatus, HeartbeatStore,
        ProcessRole, ServiceName, TracingHealth, TracingHealthReader, TracingStatus,
    };
    use auction_time::{Duration, Timestamp};
    use tokio::sync::Notify;

    use super::{details, with_cold_config};
    use crate::bootstrap::control::worker_heartbeat_health;

    /// 能力判定每次都重新读取：失去持久凭据身份的世界不再出现在下一次详情里。
    #[test]
    fn take_worlds_follow_the_current_identity_availability() {
        let isekai_available = AtomicBool::new(true);
        let available = |world: &str| match world {
            "isekai" => isekai_available.load(Ordering::SeqCst),
            "main" => true,
            _ => false,
        };

        let before = details(true, &["isekai", "main"], available);
        assert_eq!(
            before,
            serde_json::json!({
                "forum_sync_enabled": true,
                "settlement_return_recheck_worlds": ["isekai", "main"],
                "settlement_return_take_worlds": ["isekai", "main"],
            })
        );

        isekai_available.store(false, Ordering::SeqCst);
        let after = details(true, &["isekai", "main"], available);
        assert_eq!(after["settlement_return_take_worlds"], serde_json::json!(["main"]));
        // 只读核验能力来自配置的读取 gateway，不随 Take 身份变化。
        assert_eq!(
            after["settlement_return_recheck_worlds"],
            serde_json::json!(["isekai", "main"])
        );
    }

    /// 未启用 MoogleMail 或只配置 Cookie 时没有任何世界能冻结身份。
    #[test]
    fn no_identity_declares_no_take_world() {
        let none = details(false, &[], |_| false);
        assert_eq!(none["settlement_return_take_worlds"], serde_json::json!([]));
        assert_eq!(none["settlement_return_recheck_worlds"], serde_json::json!([]));
    }

    #[derive(Default)]
    struct RecordingStore(Mutex<Vec<HeartbeatCommand>>);

    impl HeartbeatStore for RecordingStore {
        fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()> {
            Box::pin(async {
                assert!(command.validate());
                self.0.lock().unwrap().push(command);
                Ok(())
            })
        }

        fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>> {
            Box::pin(async { panic!("心跳发布不应读取存储") })
        }
    }

    #[derive(Default)]
    struct ReadinessProbe {
        entered: Notify,
        resume: Notify,
    }

    impl HeartbeatHealthReader for ReadinessProbe {
        fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
            Box::pin(async {
                self.entered.notify_one();
                self.resume.notified().await;
                false
            })
        }

        fn audit_failure_count(&self) -> i64 {
            0
        }
    }

    struct DisabledTracing;

    impl TracingHealthReader for DisabledTracing {
        fn snapshot(&self) -> TracingHealth {
            TracingHealth {
                status: TracingStatus::Disabled,
                dropped_span_count: 0,
                export_failure_count: 0,
                last_success_at: None,
                last_failure_at: None,
                last_error_code: String::new(),
            }
        }
    }

    fn publisher(
        store: Arc<RecordingStore>,
        control: &WorkerControlHandle,
        probe: Arc<ReadinessProbe>,
    ) -> HeartbeatPublisher {
        with_cold_config(
            HeartbeatPublisher::new(
                store,
                HeartbeatPublisherConfig {
                    service_name: ServiceName::Worker,
                    process_role: ProcessRole::Worker,
                    instance_id: "worker-fixture".into(),
                    version: "test".into(),
                    started_at: Timestamp::from_timestamp(1, 0).unwrap(),
                    ttl: Duration::seconds(30),
                },
                worker_heartbeat_health(probe, control.clone()),
                Arc::new(DisabledTracing),
            ),
            control.clone(),
            WorkerColdConfigDigest::from_projection(1, &"启动配置").unwrap(),
            || details(true, &["main"], |world| world == "main"),
        )
    }

    #[tokio::test]
    async fn heartbeats_never_include_the_digest_if_runner_does_not_reach_serving() {
        let store = Arc::new(RecordingStore::default());
        let control = WorkerControlHandle::new("test", 0);
        let probe = Arc::new(ReadinessProbe::default());
        let publisher = publisher(store.clone(), &control, probe.clone());

        publisher.publish_starting().await.unwrap();
        // 装配根的首个 current 发生在 runner 校验之前。校验拒绝时控制句柄一直未进入 Serving。
        probe.resume.notify_one();
        publisher.publish_current().await.unwrap();
        control.mark_draining();
        publisher.publish_stopping().await.unwrap();

        let observations = store.0.lock().unwrap();
        assert_eq!(
            observations.iter().map(|command| command.status).collect::<Vec<_>>(),
            [HeartbeatStatus::Starting, HeartbeatStatus::Degraded, HeartbeatStatus::Stopping],
        );
        assert!(
            observations
                .iter()
                .all(|command| { command.details.get(WORKER_COLD_CONFIG_HEARTBEAT_KEY).is_none() })
        );
    }

    #[tokio::test]
    async fn drain_during_a_readiness_probe_removes_the_digest_from_the_next_heartbeat() {
        let store = Arc::new(RecordingStore::default());
        let control = WorkerControlHandle::new("test", 0);
        let probe = Arc::new(ReadinessProbe::default());
        let publisher = publisher(store.clone(), &control, probe.clone());

        control.mark_serving();
        probe.resume.notify_one();
        publisher.publish_current().await.unwrap();
        probe.entered.notified().await;

        let (published, ()) = tokio::join!(publisher.publish_current(), async {
            probe.entered.notified().await;
            control.mark_draining();
            probe.resume.notify_one();
        },);
        published.unwrap();

        let observations = store.0.lock().unwrap();
        let [serving, draining] = observations.as_slice() else {
            panic!("应记录两次持久发布");
        };
        assert_eq!(serving.status, HeartbeatStatus::Degraded);
        assert_eq!(draining.status, serving.status);
        assert_eq!(draining.ttl, serving.ttl);
        assert_eq!(draining.observation_sequence, serving.observation_sequence + 1);
        assert!(serving.details.get(WORKER_COLD_CONFIG_HEARTBEAT_KEY).is_some());
        assert!(draining.details.get(WORKER_COLD_CONFIG_HEARTBEAT_KEY).is_none());
        assert_eq!(
            draining.details["settlement_return_take_worlds"],
            serving.details["settlement_return_take_worlds"],
        );
    }
}
