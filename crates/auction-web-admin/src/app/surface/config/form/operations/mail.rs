//! MoogleMail 调度与处理预算编辑器；保存候选不会触发邮件轮询。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{ManagedSection, NumberField, RuntimeConfigForm, RuntimeGroup, SwitchField, TextField};

#[component]
pub(super) fn MailFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.mail.managed", title: t!("admin-runtime-op-section-mail"), note: t!("admin-runtime-op-note-mail"), on_dirty,
            RuntimeGroup { title: t!("admin-runtime-op-mail-intervals"),
                p { class: "admin-runtime-section-note", {t!("admin-runtime-op-mail-intervals-note")} }
                div { class: "admin-runtime-field-grid",
                NumberField { form, disabled, path: "operations.mail.send_intervals.outbid_seconds", label: t!("admin-runtime-op-mail-interval-outbid"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.account_seconds", label: t!("admin-runtime-op-mail-interval-account"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.intake_seconds", label: t!("admin-runtime-op-mail-interval-intake"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.settlement_seconds", label: t!("admin-runtime-op-mail-interval-settlement"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.equipment_seconds", label: t!("admin-runtime-op-mail-interval-equipment"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.operations_seconds", label: t!("admin-runtime-op-mail-interval-operations"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.send_intervals.manual_seconds", label: t!("admin-runtime-op-mail-interval-manual"), minimum: "0", maximum: "86400", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-mail-scheduling"),
                div { class: "admin-runtime-field-grid",
                TextField { form, disabled, path: "operations.mail.schedule", label: t!("admin-runtime-op-mail-schedule"), on_dirty }
                NumberField { form, disabled, path: "operations.mail.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.lease_duration_seconds", label: t!("admin-runtime-op-lease-duration-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.renew_interval_seconds", label: t!("admin-runtime-op-renew-interval-seconds"), minimum: "1", maximum: "86399", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-mail-fetch"),
                div { class: "admin-runtime-field-grid",
                NumberField { form, disabled, path: "operations.mail.batch_size", label: t!("admin-runtime-op-mail-batch-size"), minimum: "1", maximum: "1000", on_dirty }
                NumberField { form, disabled, path: "operations.mail.maximum_pages", label: t!("admin-runtime-op-maximum-pages"), minimum: "1", maximum: "100", on_dirty }
                NumberField { form, disabled, path: "operations.mail.maximum_response_bytes", label: t!("admin-runtime-op-maximum-response-bytes"), minimum: "1", maximum: "8388608", on_dirty }
                NumberField { form, disabled, path: "operations.mail.minimum_request_interval_millis", label: t!("admin-runtime-op-minimum-request-interval-millis"), minimum: "1", maximum: "60000", on_dirty }
                NumberField { form, disabled, path: "operations.mail.request_timeout_seconds", label: t!("admin-runtime-op-request-timeout-seconds"), minimum: "1", maximum: "300", on_dirty }
                NumberField { form, disabled, path: "operations.mail.fetch_attempts", label: t!("admin-runtime-op-fetch-attempts"), minimum: "1", maximum: "10", on_dirty }
                NumberField { form, disabled, path: "operations.mail.fetch_timeout_seconds", label: t!("admin-runtime-op-fetch-timeout-seconds"), minimum: "1", maximum: "300", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-mail-processing"),
                div { class: "admin-runtime-field-grid",
                NumberField { form, disabled, path: "operations.mail.process_attempts", label: t!("admin-runtime-op-process-attempts"), minimum: "1", maximum: "10", on_dirty }
                NumberField { form, disabled, path: "operations.mail.item_timeout_seconds", label: t!("admin-runtime-op-item-timeout-seconds"), minimum: "1", maximum: "300", on_dirty }
                NumberField { form, disabled, path: "operations.mail.batch_timeout_seconds", label: t!("admin-runtime-op-mail-batch-timeout-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.mail.retry_base_seconds", label: t!("admin-runtime-op-retry-base-seconds"), minimum: "1", maximum: "300", on_dirty }
                NumberField { form, disabled, path: "operations.mail.retry_maximum_seconds", label: t!("admin-runtime-op-retry-maximum-seconds"), minimum: "1", maximum: "300", on_dirty }
                }
            }
            RuntimeGroup { title: t!("admin-runtime-op-mail-replies"),
                div { class: "admin-runtime-field-grid",
                SwitchField { form, disabled, path: "operations.mail.rekey_enabled", label: t!("admin-runtime-op-mail-rekey-enabled"), on_dirty }
                NumberField { form, disabled, path: "operations.mail.account_reply_window_seconds", label: t!("admin-runtime-op-mail-account-reply-window-seconds"), minimum: "60", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.mail.explanation_mail_window_seconds", label: t!("admin-runtime-op-mail-explanation-window-seconds"), minimum: "60", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.mail.explanation_mail_limit", label: t!("admin-runtime-op-mail-explanation-limit"), minimum: "0", maximum: "100", on_dirty }
                }
            }
        }
    }
}
