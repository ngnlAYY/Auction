//! Worker 各循环参数编辑控件。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{ManagedSection, NumberField, RuntimeConfigForm, RuntimeGroup};

#[component]
pub(super) fn WorkerFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.worker.managed", title: t!("admin-runtime-op-section-worker"), note: t!("admin-runtime-op-note-worker"), on_dirty,
            RuntimeGroup { title: t!("admin-runtime-op-section-lifecycle"),
            div { class: "admin-runtime-field-grid",
                NumberField { form, disabled, path: "operations.worker.concurrency", label: t!("admin-runtime-op-concurrency"), minimum: "1", maximum: "1000", on_dirty }
                NumberField { form, disabled, path: "operations.worker.poll_interval_seconds", label: t!("admin-runtime-op-poll-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.worker.batch_limit", label: t!("admin-runtime-op-batch-limit"), minimum: "1", maximum: "1000", on_dirty }
                NumberField { form, disabled, path: "operations.worker.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.worker.item_timeout_seconds", label: t!("admin-runtime-op-item-timeout-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.worker.recovery_retry_delay_seconds", label: t!("admin-runtime-op-recovery-retry-delay-seconds"), minimum: "1", maximum: "86400", on_dirty }
            }
            }
            RuntimeGroup { title: t!("admin-runtime-op-section-delivery"),
                div { class: "admin-runtime-field-grid",
                    NumberField { form, disabled, path: "operations.worker.delivery.batch_limit", label: t!("admin-runtime-op-batch-limit"), minimum: "1", maximum: "1000", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.poll_interval_seconds", label: t!("admin-runtime-op-poll-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.lease_duration_seconds", label: t!("admin-runtime-op-lease-duration-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.renew_interval_seconds", label: t!("admin-runtime-op-renew-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.sending_grace_seconds", label: t!("admin-runtime-op-sending-grace-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.request_timeout_seconds", label: t!("admin-runtime-op-request-timeout-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.delivery.finalize_timeout_seconds", label: t!("admin-runtime-op-finalize-timeout-seconds"), minimum: "1", maximum: "300", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-section-fulfillment"),
                div { class: "admin-runtime-field-grid",
                    NumberField { form, disabled, path: "operations.worker.fulfillment.batch_limit", label: t!("admin-runtime-op-batch-limit"), minimum: "1", maximum: "1000", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.concurrency", label: t!("admin-runtime-op-concurrency"), minimum: "1", maximum: "1000", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.poll_interval_seconds", label: t!("admin-runtime-op-poll-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.lease_duration_seconds", label: t!("admin-runtime-op-lease-duration-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.item_timeout_seconds", label: t!("admin-runtime-op-item-timeout-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.fulfillment.retry_delay_seconds", label: t!("admin-runtime-op-retry-delay-seconds"), minimum: "1", maximum: "86400", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-section-recovery"),
                div { class: "admin-runtime-field-grid",
                    NumberField { form, disabled, path: "operations.worker.recovery.batch_limit", label: t!("admin-runtime-op-batch-limit"), minimum: "1", maximum: "1000", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.concurrency", label: t!("admin-runtime-op-concurrency"), minimum: "1", maximum: "1000", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.poll_interval_seconds", label: t!("admin-runtime-op-poll-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.lease_duration_seconds", label: t!("admin-runtime-op-lease-duration-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.item_timeout_seconds", label: t!("admin-runtime-op-item-timeout-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.low_balance_threshold", label: t!("admin-runtime-op-low-balance-threshold"), minimum: "0", maximum: "9007199254740991", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.health_refresh_interval_seconds", label: t!("admin-runtime-op-health-refresh-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                    NumberField { form, disabled, path: "operations.worker.recovery.evidence_stale_after_seconds", label: t!("admin-runtime-op-evidence-stale-after-seconds"), minimum: "1", maximum: "86400", on_dirty }
                }
            }
        }
    }
}
