//! Worker 运行参数热读取的分类与快照。
//!
//! 热读取只搬动节奏、预算与退避参数：它们决定 Worker 何时、以多大批次访问外部账号，
//! 不参与金额、资格、幂等、租约或审计决定（不变量 5、9）。其余 Worker 参数仍只能
//! 通过重启生效；本模块集中声明热字段，供 Worker 周期重载与 Admin 参数页的镜像表对照。
//!
//! 候选文档必须先经过完整校验才能进入本模块；这里只提取已校验字段，并在未接管分组上
//! 保留当前生效值，绝不把“未接管”解释成“回到部署默认值”。

use crate::ports::admin::{AdminRuntimeDocument, AdminRuntimeOperationsConfig};
use crate::ports::delivery::DeliverySendIntervals;
use crate::ports::mail::{MailReplyPolicy, MailSendClasses};

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
    "operations.mail.account_reply_window_seconds",
    "operations.mail.explanation_mail_window_seconds",
    "operations.mail.explanation_mail_limit",
    "operations.mail.send_classes.account_reply",
    "operations.mail.send_classes.intake_reply",
    "operations.mail.send_classes.settlement_notice",
    "operations.mail.send_classes.outbid_reminder",
    "operations.mail.send_classes.player_summary",
    "operations.mail.send_classes.operations_alert",
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
    /// 同一账号注册/轮换回信窗口秒数。
    pub account_reply_window_seconds: u64,
    /// 说明信窗口秒数。
    pub explanation_mail_window_seconds: u64,
    /// 说明信窗口内最大封数；0 表示不限。
    pub explanation_mail_limit: u32,
    /// 六类出站通知的发送开关；关闭只抑制通知，不改变命令、履约或审计。
    pub send_classes: MailSendClasses,
    /// 七类 Send 的最小开始间距；由持久化 delivery 调度读取。
    pub send_intervals: DeliverySendIntervals,
}

impl WorkerMailHotConfig {
    /// 投影为邮件处理器共享的预算策略。
    pub fn reply_policy(self) -> MailReplyPolicy {
        MailReplyPolicy {
            account_reply_window_seconds: self.account_reply_window_seconds,
            explanation_mail_window_seconds: self.explanation_mail_window_seconds,
            explanation_mail_limit: self.explanation_mail_limit,
        }
    }
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
                account_reply_window_seconds: mail.account_reply_window_seconds,
                explanation_mail_window_seconds: mail.explanation_mail_window_seconds,
                explanation_mail_limit: mail.explanation_mail_limit,
                send_classes: mail.send_classes,
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
    use super::*;

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
                account_reply_window_seconds: 86_400,
                explanation_mail_window_seconds: 86_400,
                explanation_mail_limit: 10,
                send_classes: MailSendClasses::default(),
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
    fn managed_groups_replace_only_hot_fields_and_keep_everything_else() {
        let current = baseline();
        let mut operations = AdminRuntimeOperationsConfig::default();
        let mail = operations.mail.get_or_insert_with(Default::default);
        mail.managed = true;
        mail.batch_size = 8;
        mail.batch_timeout_seconds = 900;
        mail.retry_maximum_seconds = 45;
        mail.account_reply_window_seconds = 120;
        mail.explanation_mail_window_seconds = 60;
        mail.explanation_mail_limit = 0;
        mail.send_classes.account_reply = false;
        mail.send_intervals.outbid_seconds = 9;
        mail.send_intervals.manual_seconds = 7;
        // 租约与日程是重启字段；即使候选变化，热快照也不应携带它们。
        mail.lease_duration_seconds = 90;
        mail.schedule = "@every 1m".to_owned();

        let next = WorkerHotConfig::apply_document(&current, &document_with(operations));
        assert_eq!(next.mail.batch_size, 8);
        assert_eq!(next.mail.batch_timeout_seconds, 900);
        assert_eq!(next.mail.retry_maximum_seconds, 45);
        assert_eq!(next.mail.account_reply_window_seconds, 120);
        assert_eq!(next.mail.explanation_mail_window_seconds, 60);
        assert_eq!(next.mail.explanation_mail_limit, 0);
        assert!(!next.mail.send_classes.account_reply);
        assert!(next.mail.send_classes.intake_reply);
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
