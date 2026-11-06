// Worker 运行参数热读取的分类与快照。
//
// 热读取只搬动节奏、预算与退避参数：它们决定 Worker 何时、以多大批次访问外部账号，
// 不参与金额、资格、幂等、租约或审计决定（不变量 5、9）。其余 Worker 参数仍只能
// 通过重启生效；本模块集中声明两侧字段，供 Worker 周期重载、Admin 标注和文档使用。
//
// 候选文档必须先经过完整校验才能进入本模块；这里只提取已校验字段，并在未接管分组上
// 保留当前生效值，绝不把“未接管”解释成“回到部署默认值”。

use crate::ports::delivery::DeliverySendIntervals;

use crate::ports::admin::{AdminRuntimeDocument, AdminRuntimeOperationsConfig};

/// Worker 周期重读后可立即生效的 runtime 文档字段路径。
/// 路径与 Admin 参数页的输入路径一致；新增热字段时必须同时接上消费方。
pub const WORKER_HOT_FIELD_PATHS: &[&str] = &[
    "operations.mail.send_intervals.outbid_seconds",
    "operations.mail.send_intervals.account_seconds",
    "operations.mail.send_intervals.intake_seconds",
    "operations.mail.send_intervals.settlement_seconds",
    "operations.mail.send_intervals.equipment_seconds",
    "operations.mail.send_intervals.operations_seconds",
    "operations.mail.send_intervals.manual_seconds",
    "operations.mail.batch_size",
    "operations.mail.jitter_seconds",
    "operations.mail.fetch_attempts",
    "operations.mail.fetch_timeout_seconds",
    "operations.mail.process_attempts",
    "operations.mail.item_timeout_seconds",
    "operations.mail.batch_timeout_seconds",
    "operations.mail.retry_base_seconds",
    "operations.mail.retry_maximum_seconds",
    "operations.forum.batch_size",
    "operations.forum.poll_interval_seconds",
    "operations.forum.jitter_seconds",
    "operations.forum.item_timeout_seconds",
    "operations.worker.delivery.batch_limit",
    "operations.worker.delivery.poll_interval_seconds",
    "operations.worker.delivery.jitter_seconds",
    "operations.worker.delivery.request_timeout_seconds",
    "operations.worker.delivery.finalize_timeout_seconds",
];

/// Worker 参数页里明确仍需重启的字段代表项；用于文档、测试与界面说明，不追求穷举。
/// 这些字段或被启动期装配冻结（租约、远端传输预算、轮询开关），或参与业务模板冻结。
pub const WORKER_RESTART_ONLY_FIELD_PATHS: &[&str] = &[
    "operations.mail.schedule",
    "operations.mail.lease_duration_seconds",
    "operations.mail.renew_interval_seconds",
    "operations.mail.maximum_pages",
    "operations.mail.maximum_response_bytes",
    "operations.mail.minimum_request_interval_millis",
    "operations.mail.request_timeout_seconds",
    "operations.forum.lease_duration_seconds",
    "operations.forum.maximum_response_bytes",
    "operations.forum.maximum_topic_pages",
    "operations.forum.minimum_request_interval_millis",
    "operations.forum.request_timeout_seconds",
    "operations.forum.enabled",
    "operations.worker.delivery.lease_duration_seconds",
    "operations.worker.delivery.renew_interval_seconds",
    "operations.worker.delivery.sending_grace_seconds",
    "operations.worker.concurrency",
    "operations.worker.poll_interval_seconds",
    "operations.worker.batch_limit",
    "operations.worker.jitter_seconds",
    "operations.worker.item_timeout_seconds",
    "operations.worker.recovery_retry_delay_seconds",
    "operations.economics.start_price",
    "operations.economics.bid_increment_ppm",
    "operations.schedule.timezone",
    "operations.alerts.scan_interval_seconds",
];

/// 单个 Worker 参数在运行中的作用方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerFieldEffect {
    /// Worker 每个读取周期检测 runtime 文件并在下一个循环边界使用新值。
    Hot,
    /// 只有进程重启后生效。
    RestartOnly,
    /// 不属于 Worker 运行参数（Public/Admin 或非 runtime 文档字段）。
    NotWorkerRuntime,
}

/// 返回参数路径的生效方式；未登记的字段一律按仅重启处理。
pub fn worker_hot_field_effect(path: &str) -> WorkerFieldEffect {
    if WORKER_HOT_FIELD_PATHS.contains(&path) {
        WorkerFieldEffect::Hot
    } else if path.starts_with("operations.") {
        WorkerFieldEffect::RestartOnly
    } else {
        WorkerFieldEffect::NotWorkerRuntime
    }
}

/// MoogleMail 轮询/处理的热参数；租约、日程与远端传输预算不在其中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerMailHotConfig {
    /// 每轮最多读取邮件数。
    pub batch_size: u16,
    /// 计划触发后的随机抖动上限，单位秒。
    pub jitter_seconds: u64,
    /// 单次读取内联尝试次数。
    pub fetch_attempts: u16,
    /// 单次读取总超时，单位秒。
    pub fetch_timeout_seconds: u64,
    /// 单封邮件业务处理内联尝试次数。
    pub process_attempts: u16,
    /// 单次邮件处理总超时，单位秒。
    pub item_timeout_seconds: u64,
    /// 单次邮箱 claim 的总处理预算，单位秒。
    pub batch_timeout_seconds: u64,
    /// 指数退避基础秒数。
    pub retry_base_seconds: u64,
    /// 指数退避上限秒数。
    pub retry_maximum_seconds: u64,
    /// 七类 Send 的最小开始间距；由持久化 delivery 调度读取。
    pub send_intervals: DeliverySendIntervals,
}

/// 论坛同步与回帖导入的热参数；租约与远端传输预算不在其中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerForumHotConfig {
    /// 每轮领取的论坛任务上限。
    pub batch_size: u16,
    pub poll_interval_seconds: u64,
    pub jitter_seconds: u64,
    /// 单个论坛任务的总处理超时，必须小于当前生效租约。
    pub item_timeout_seconds: u64,
}

/// Delivery outbox 热参数；租约、续租、发送宽限与并发不在其中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerDeliveryHotConfig {
    pub batch_limit: u16,
    pub poll_interval_seconds: u64,
    pub jitter_seconds: u64,
    /// 单次完整投递请求的总超时。
    pub request_timeout_seconds: u64,
    /// 结果写回的超时。
    pub finalize_timeout_seconds: u64,
}

/// Worker 热读取快照；每个字段都已通过完整文档校验，可直接在循环边界使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerHotConfig {
    pub mail: WorkerMailHotConfig,
    pub forum: WorkerForumHotConfig,
    pub delivery: WorkerDeliveryHotConfig,
}

impl WorkerHotConfig {
    /// 从完整校验后的 runtime 文档提取热字段；未接管的参数组保留 `current`。
    pub fn apply_document(current: &Self, document: &AdminRuntimeDocument) -> Self {
        let Some(operations) = document.operations.as_ref() else {
            return *current;
        };
        let mut next = *current;
        next.apply_operations(operations);
        next
    }

    fn apply_operations(&mut self, operations: &AdminRuntimeOperationsConfig) {
        // mail 区块是 Option：省略或未接管都不能把当前生效的节奏重置成默认值。
        if let Some(mail) = operations.mail.as_ref().filter(|mail| mail.managed) {
            self.mail = WorkerMailHotConfig {
                batch_size: mail.batch_size,
                jitter_seconds: mail.jitter_seconds,
                fetch_attempts: mail.fetch_attempts,
                fetch_timeout_seconds: mail.fetch_timeout_seconds,
                process_attempts: mail.process_attempts,
                item_timeout_seconds: mail.item_timeout_seconds,
                batch_timeout_seconds: mail.batch_timeout_seconds,
                retry_base_seconds: mail.retry_base_seconds,
                retry_maximum_seconds: mail.retry_maximum_seconds,
                // 与启动投影一致：接管 mail 组时连全零节奏也按候选生效。
                send_intervals: mail.send_intervals,
            };
        }
        if operations.forum.managed {
            self.forum = WorkerForumHotConfig {
                batch_size: operations.forum.batch_size,
                poll_interval_seconds: operations.forum.poll_interval_seconds,
                jitter_seconds: operations.forum.jitter_seconds,
                item_timeout_seconds: operations.forum.item_timeout_seconds,
            };
        }
        if operations.worker.managed {
            self.delivery = WorkerDeliveryHotConfig {
                batch_limit: operations.worker.delivery.batch_limit,
                poll_interval_seconds: operations.worker.delivery.poll_interval_seconds,
                jitter_seconds: operations.worker.delivery.jitter_seconds,
                request_timeout_seconds: operations.worker.delivery.request_timeout_seconds,
                finalize_timeout_seconds: operations.worker.delivery.finalize_timeout_seconds,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    fn baseline() -> WorkerHotConfig {
        WorkerHotConfig {
            mail: WorkerMailHotConfig {
                batch_size: 100,
                jitter_seconds: 5,
                fetch_attempts: 3,
                fetch_timeout_seconds: 30,
                process_attempts: 3,
                item_timeout_seconds: 30,
                batch_timeout_seconds: 600,
                retry_base_seconds: 1,
                retry_maximum_seconds: 30,
                send_intervals: DeliverySendIntervals {
                    outbid_seconds: 4,
                    manual_seconds: 6,
                    ..DeliverySendIntervals::default()
                },
            },
            forum: WorkerForumHotConfig {
                batch_size: 100,
                poll_interval_seconds: 300,
                jitter_seconds: 5,
                item_timeout_seconds: 240,
            },
            delivery: WorkerDeliveryHotConfig {
                batch_limit: 100,
                poll_interval_seconds: 60,
                jitter_seconds: 5,
                request_timeout_seconds: 180,
                finalize_timeout_seconds: 5,
            },
        }
    }

    fn document_with(operations: AdminRuntimeOperationsConfig) -> AdminRuntimeDocument {
        AdminRuntimeDocument { operations: Some(operations), ..Default::default() }
    }

    #[test]
    fn hot_paths_are_classified_and_never_overlap_restart_only() {
        for path in WORKER_HOT_FIELD_PATHS {
            assert_eq!(worker_hot_field_effect(path), WorkerFieldEffect::Hot, "{path}");
            assert!(!WORKER_RESTART_ONLY_FIELD_PATHS.contains(path), "{path}");
        }
        for path in WORKER_RESTART_ONLY_FIELD_PATHS {
            assert_eq!(worker_hot_field_effect(path), WorkerFieldEffect::RestartOnly, "{path}");
        }
        assert_eq!(
            worker_hot_field_effect("operations.mail.lease_duration_seconds"),
            WorkerFieldEffect::RestartOnly
        );
        assert_eq!(
            worker_hot_field_effect("equipment_filter.level.min"),
            WorkerFieldEffect::NotWorkerRuntime
        );
    }

    #[test]
    fn managed_groups_replace_only_hot_fields_and_keep_everything_else() {
        let current = baseline();
        let mut operations = AdminRuntimeOperationsConfig::default();
        let mail = operations.mail.get_or_insert_with(Default::default);
        mail.managed = true;
        mail.batch_size = 8;
        mail.batch_timeout_seconds = 900;
        mail.retry_maximum_seconds = 45;
        mail.send_intervals.outbid_seconds = 9;
        mail.send_intervals.manual_seconds = 7;
        // 租约与日程是重启字段；即使候选变化，热快照也不应携带它们。
        mail.lease_duration_seconds = 90;
        mail.schedule = "@every 1m".to_owned();

        let next = WorkerHotConfig::apply_document(&current, &document_with(operations));
        assert_eq!(next.mail.batch_size, 8);
        assert_eq!(next.mail.batch_timeout_seconds, 900);
        assert_eq!(next.mail.retry_maximum_seconds, 45);
        assert_eq!(next.mail.send_intervals.outbid_seconds, 9);
        assert_eq!(next.mail.send_intervals.manual_seconds, 7);
        assert_eq!(next.forum, current.forum);
        assert_eq!(next.delivery, current.delivery);
    }

    #[test]
    fn unmanaged_groups_keep_current_values_and_managed_zero_intervals_apply() {
        let current = baseline();
        let mut operations = AdminRuntimeOperationsConfig::default();
        {
            let mail = operations.mail.get_or_insert_with(Default::default);
            mail.batch_size = 4;
            mail.send_intervals = DeliverySendIntervals::default();
        }

        let next = WorkerHotConfig::apply_document(&current, &document_with(operations.clone()));
        assert_eq!(next, current);

        operations.mail.as_mut().unwrap().managed = true;
        let next = WorkerHotConfig::apply_document(&current, &document_with(operations));
        assert_eq!(next.mail.batch_size, 4);
        // 与启动投影一致：接管 mail 组后候选的全零节奏会生效。
        assert_eq!(next.mail.send_intervals, DeliverySendIntervals::default());
    }

    #[test]
    fn business_templates_in_the_same_document_are_not_hot() {
        let current = baseline();
        let mut operations = AdminRuntimeOperationsConfig::default();
        operations.economics.managed = true;
        operations.economics.start_price = 1_234_567;
        operations.schedule.managed = true;
        operations.schedule.timezone = "Asia/Tokyo".to_owned();

        let next = WorkerHotConfig::apply_document(&current, &document_with(operations));
        assert_eq!(next, current);
    }
}
