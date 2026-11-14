//! 论坛同步参数编辑控件。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{ManagedSection, NumberField, RuntimeConfigForm, SwitchField};

#[component]
pub(super) fn ForumFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.forum.managed", title: t!("admin-runtime-op-section-forum"), note: t!("admin-runtime-op-note-forum"), on_dirty,
            div { class: "admin-runtime-field-grid",
                SwitchField { form, disabled, path: "operations.forum.enabled", label: t!("admin-runtime-op-enabled"), on_dirty }
                NumberField { form, disabled, path: "operations.forum.batch_size", label: t!("admin-runtime-op-batch-size"), minimum: "1", maximum: "100", on_dirty }
                NumberField { form, disabled, path: "operations.forum.poll_interval_seconds", label: t!("admin-runtime-op-poll-interval-seconds"), minimum: "1", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.forum.jitter_seconds", label: t!("admin-runtime-op-jitter-seconds"), minimum: "0", maximum: "86400", on_dirty }
                NumberField { form, disabled, path: "operations.forum.maximum_response_bytes", label: t!("admin-runtime-op-maximum-response-bytes"), minimum: "1", maximum: "8388608", on_dirty }
                NumberField { form, disabled, path: "operations.forum.maximum_topic_pages", label: t!("admin-runtime-op-maximum-topic-pages"), minimum: "1", maximum: "20", on_dirty }
                NumberField { form, disabled, path: "operations.forum.minimum_request_interval_millis", label: t!("admin-runtime-op-minimum-request-interval-millis"), minimum: "1", maximum: "60000", on_dirty }
                NumberField { form, disabled, path: "operations.forum.request_timeout_seconds", label: t!("admin-runtime-op-request-timeout-seconds"), minimum: "1", maximum: "300", on_dirty }
                NumberField { form, disabled, path: "operations.forum.item_timeout_seconds", label: t!("admin-runtime-op-item-timeout-seconds"), minimum: "1", maximum: "3600", on_dirty }
                NumberField { form, disabled, path: "operations.forum.lease_duration_seconds", label: t!("admin-runtime-op-lease-duration-seconds"), minimum: "1", maximum: "86400", on_dirty }
            }
        }
    }
}
