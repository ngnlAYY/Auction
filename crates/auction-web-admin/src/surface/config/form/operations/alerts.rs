//! 运维告警参数编辑控件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{ManagedSection, NumberField, RuntimeConfigForm, SelectField, SwitchField, TextField};

#[component]
pub(super) fn AlertsFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.alerts.managed", title: t!("admin-runtime-op-section-alerts"), note: t!("admin-runtime-op-note-alerts"), on_dirty,
            div { class: "admin-runtime-field-grid",
                SwitchField { form, disabled, path: "operations.alerts.enabled", label: t!("admin-runtime-op-enabled"), on_dirty }
                NumberField { form, disabled, path: "operations.alerts.scan_interval_seconds", label: t!("admin-runtime-op-scan-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.source_timeout_seconds", label: t!("admin-runtime-op-source-timeout-seconds"), minimum: "1", maximum: "30", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.batch_window_seconds", label: t!("admin-runtime-op-batch-window-seconds"), minimum: "1", maximum: "3600", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.cooldown_seconds", label: t!("admin-runtime-op-cooldown-seconds"), minimum: "1", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.claim_lease_seconds", label: t!("admin-runtime-op-claim-lease-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.retry_delay_seconds", label: t!("admin-runtime-op-retry-delay-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.max_signals_per_scan", label: t!("admin-runtime-op-max-signals-per-scan"), minimum: "1", maximum: "64", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.max_batch_items", label: t!("admin-runtime-op-max-batch-items"), minimum: "1", maximum: "64", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.max_body_bytes", label: t!("admin-runtime-op-max-body-bytes"), minimum: "256", maximum: "16384", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.delivery_pending_count_threshold", label: t!("admin-runtime-op-delivery-pending-count-threshold"), minimum: "1", maximum: "9007199254740991", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.delivery_pending_age_seconds", label: t!("admin-runtime-op-delivery-pending-age-seconds"), minimum: "1", maximum: "2592000", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.delivery_manual_reconcile_count_threshold", label: t!("admin-runtime-op-delivery-manual-reconcile-count-threshold"), minimum: "1", maximum: "9007199254740991", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.fulfillment_backlog_count_threshold", label: t!("admin-runtime-op-fulfillment-backlog-count-threshold"), minimum: "1", maximum: "9007199254740991", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.fulfillment_backlog_age_seconds", label: t!("admin-runtime-op-fulfillment-backlog-age-seconds"), minimum: "1", maximum: "2592000", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.recovery_overdue_count_threshold", label: t!("admin-runtime-op-recovery-overdue-count-threshold"), minimum: "1", maximum: "9007199254740991", on_dirty }
                NumberField { form, disabled, path: "operations.alerts.recovery_overdue_age_seconds", label: t!("admin-runtime-op-recovery-overdue-age-seconds"), minimum: "1", maximum: "2592000", on_dirty }
                SelectField { form, disabled, path: "operations.alerts.world_id", label: t!("admin-runtime-op-world-id"), options: vec![("main".to_owned(), t!("admin-runtime-mailbox-main")), ("isekai".to_owned(), t!("admin-runtime-mailbox-isekai"))], on_dirty }
                TextField { form, disabled, path: "operations.alerts.recipient", label: t!("admin-runtime-op-recipient"), on_dirty }
            }
        }
    }
}
