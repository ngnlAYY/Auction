//! 把 Admin 管理的公开参数投影为各角色启动配置；接管必须显式启用，部署专属字段不在此覆盖。

use auction_application::{AdminRuntimeDocument, AdminRuntimeOperationsConfig};

use super::{
    AuctionConfig, AuctionEconomicsConfig, AuctionScheduleConfig, DeliveryWorkerConfig,
    FulfillmentWorkerConfig, RecoveryWorkerConfig, Role, WorkerConfig,
};

pub(super) fn apply_runtime_operations(
    auction: &mut AuctionConfig,
    worker: &mut WorkerConfig,
    role: Role,
    document: &AdminRuntimeDocument,
) {
    let Some(operations) = &document.operations else {
        return;
    };
    if operations.economics.managed {
        let candidate = &operations.economics;
        // 仅修改创建用的模板；既有拍卖规则仍由数据库 policy_snapshot 决定。
        auction.economics = AuctionEconomicsConfig {
            start_price: candidate.start_price,
            bid_increment_ppm: candidate.bid_increment_ppm,
            soft_close_window_seconds: candidate.soft_close_window_seconds,
            soft_close_extension_seconds: candidate.soft_close_extension_seconds,
            cancel_cutoff_seconds: candidate.cancel_cutoff_seconds,
            proxy_enabled: candidate.proxy_enabled,
            proxy_default_mode: candidate.proxy_default_mode.clone(),
            proxy_slow_interval_seconds: candidate.proxy_slow_interval_seconds,
            proxy_max_outbid_uses: candidate.proxy_max_outbid_uses,
            final_phase_lead_seconds: candidate.final_phase_lead_seconds,
        };
    }
    if operations.schedule.managed {
        let candidate = &operations.schedule;
        auction.schedule = AuctionScheduleConfig {
            enabled: candidate.enabled,
            timezone: candidate.timezone.clone(),
            preview_day: candidate.preview_day.clone(),
            preview_time: candidate.preview_time.clone(),
            open_day: candidate.open_day.clone(),
            open_time: candidate.open_time.clone(),
            nominal_end_day: candidate.nominal_end_day.clone(),
            nominal_end_time: candidate.nominal_end_time.clone(),
            worlds: candidate.worlds.clone(),
        };
    }
    if role == Role::Worker {
        apply_mail(worker, operations);
        apply_forum(worker, operations);
        apply_alerts(worker, operations);
        apply_worker(worker, operations);
    }
}

fn apply_mail(worker: &mut WorkerConfig, operations: &AdminRuntimeOperationsConfig) {
    let Some(candidate) = operations.mail.as_ref().filter(|mail| mail.managed) else {
        return;
    };
    let mail = &mut worker.mooglemail;
    // 不覆盖 endpoint、Cookie、全局 enabled 或邮箱身份，持久化 due、游标和租约也不在此改写。
    mail.schedule = candidate.schedule.clone();
    mail.maximum_response_bytes = candidate.maximum_response_bytes;
    mail.minimum_request_interval_millis = candidate.minimum_request_interval_millis;
    mail.request_timeout_seconds = candidate.request_timeout_seconds;
    mail.batch_size = candidate.batch_size;
    mail.maximum_pages = candidate.maximum_pages;
    mail.jitter_seconds = candidate.jitter_seconds;
    mail.lease_duration_seconds = candidate.lease_duration_seconds;
    mail.renew_interval_seconds = candidate.renew_interval_seconds;
    mail.fetch_attempts = candidate.fetch_attempts;
    mail.fetch_timeout_seconds = candidate.fetch_timeout_seconds;
    mail.process_attempts = candidate.process_attempts;
    mail.item_timeout_seconds = candidate.item_timeout_seconds;
    mail.batch_timeout_seconds = candidate.batch_timeout_seconds;
    mail.retry_base_seconds = candidate.retry_base_seconds;
    mail.retry_maximum_seconds = candidate.retry_maximum_seconds;
}

fn apply_forum(worker: &mut WorkerConfig, operations: &AdminRuntimeOperationsConfig) {
    let candidate = &operations.forum;
    if !candidate.managed {
        return;
    }
    worker.forum.enabled = candidate.enabled;
    worker.forum.batch_size = candidate.batch_size;
    worker.forum.poll_interval_seconds = candidate.poll_interval_seconds;
    worker.forum.jitter_seconds = candidate.jitter_seconds;
    worker.forum.maximum_response_bytes = candidate.maximum_response_bytes;
    worker.forum.maximum_topic_pages = candidate.maximum_topic_pages;
    worker.forum.minimum_request_interval_millis = candidate.minimum_request_interval_millis;
    worker.forum.request_timeout_seconds = candidate.request_timeout_seconds;
    worker.forum.item_timeout_seconds = candidate.item_timeout_seconds;
    worker.forum.lease_duration_seconds = candidate.lease_duration_seconds;
}

fn apply_alerts(worker: &mut WorkerConfig, operations: &AdminRuntimeOperationsConfig) {
    let candidate = &operations.alerts;
    if !candidate.managed {
        return;
    }
    worker.operations_alert.enabled = candidate.enabled;
    worker.operations_alert.scan_interval_seconds = candidate.scan_interval_seconds;
    worker.operations_alert.jitter_seconds = candidate.jitter_seconds;
    worker.operations_alert.source_timeout_seconds = candidate.source_timeout_seconds;
    worker.operations_alert.batch_window_seconds = candidate.batch_window_seconds;
    worker.operations_alert.cooldown_seconds = candidate.cooldown_seconds;
    worker.operations_alert.claim_lease_seconds = candidate.claim_lease_seconds;
    worker.operations_alert.retry_delay_seconds = candidate.retry_delay_seconds;
    worker.operations_alert.max_signals_per_scan = candidate.max_signals_per_scan;
    worker.operations_alert.max_batch_items = candidate.max_batch_items;
    worker.operations_alert.max_body_bytes = candidate.max_body_bytes;
    worker.operations_alert.delivery_pending_count_threshold =
        candidate.delivery_pending_count_threshold;
    worker.operations_alert.delivery_pending_age_seconds = candidate.delivery_pending_age_seconds;
    worker.operations_alert.delivery_manual_reconcile_count_threshold =
        candidate.delivery_manual_reconcile_count_threshold;
    worker.operations_alert.fulfillment_backlog_count_threshold =
        candidate.fulfillment_backlog_count_threshold;
    worker.operations_alert.fulfillment_backlog_age_seconds =
        candidate.fulfillment_backlog_age_seconds;
    worker.operations_alert.recovery_overdue_count_threshold =
        candidate.recovery_overdue_count_threshold;
    worker.operations_alert.recovery_overdue_age_seconds = candidate.recovery_overdue_age_seconds;
    worker.operations_alert.world_id = candidate.world_id.clone();
    worker.operations_alert.recipient = candidate.recipient.clone();
}

fn apply_worker(worker: &mut WorkerConfig, operations: &AdminRuntimeOperationsConfig) {
    let candidate = &operations.worker;
    if !candidate.managed {
        return;
    }
    worker.concurrency = Some(candidate.concurrency);
    worker.poll_interval_seconds = Some(candidate.poll_interval_seconds);
    worker.batch_limit = candidate.batch_limit;
    worker.jitter_seconds = candidate.jitter_seconds;
    worker.item_timeout_seconds = candidate.item_timeout_seconds;
    worker.recovery_retry_delay_seconds = candidate.recovery_retry_delay_seconds;
    worker.delivery = DeliveryWorkerConfig {
        batch_limit: candidate.delivery.batch_limit,
        concurrency: candidate.delivery.concurrency,
        poll_interval_seconds: candidate.delivery.poll_interval_seconds,
        jitter_seconds: candidate.delivery.jitter_seconds,
        lease_duration_seconds: candidate.delivery.lease_duration_seconds,
        renew_interval_seconds: candidate.delivery.renew_interval_seconds,
        sending_grace_seconds: candidate.delivery.sending_grace_seconds,
        request_timeout_seconds: candidate.delivery.request_timeout_seconds,
        finalize_timeout_seconds: candidate.delivery.finalize_timeout_seconds,
    };
    worker.fulfillment = FulfillmentWorkerConfig {
        batch_limit: candidate.fulfillment.batch_limit,
        concurrency: candidate.fulfillment.concurrency,
        poll_interval_seconds: candidate.fulfillment.poll_interval_seconds,
        jitter_seconds: candidate.fulfillment.jitter_seconds,
        lease_duration_seconds: candidate.fulfillment.lease_duration_seconds,
        item_timeout_seconds: candidate.fulfillment.item_timeout_seconds,
        retry_delay_seconds: candidate.fulfillment.retry_delay_seconds,
    };
    worker.recovery = RecoveryWorkerConfig {
        batch_limit: candidate.recovery.batch_limit,
        concurrency: candidate.recovery.concurrency,
        poll_interval_seconds: candidate.recovery.poll_interval_seconds,
        jitter_seconds: candidate.recovery.jitter_seconds,
        lease_duration_seconds: candidate.recovery.lease_duration_seconds,
        item_timeout_seconds: candidate.recovery.item_timeout_seconds,
        low_balance_threshold: candidate.recovery.low_balance_threshold,
        health_refresh_interval_seconds: candidate.recovery.health_refresh_interval_seconds,
        evidence_stale_after_seconds: candidate.recovery.evidence_stale_after_seconds,
    };
}
