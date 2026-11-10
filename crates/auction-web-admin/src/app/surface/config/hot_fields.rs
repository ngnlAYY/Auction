// 参数页的热读取标注路径。
//
// 与 `auction_application::admin::WORKER_HOT_FIELD_PATHS` 保持同一闭集：Worker 每个读取周期
// 检测 runtime 文件变化并在下一个循环边界使用这些字段，其余 Worker 参数仍需重启。
// 前端不能依赖后端 crate，因此这里镜像路径；新增热字段必须同时更新两处与配置参考。

pub(super) const HOT_FIELDS: &[&str] = &[
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

/// 字段是否在 Worker 下个读取周期生效。
pub(super) fn is_hot(path: &str) -> bool {
    HOT_FIELDS.contains(&path)
}
