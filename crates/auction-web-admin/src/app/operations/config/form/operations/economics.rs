//! 后续新拍卖经济参数编辑控件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{ManagedSection, NumberField, RuntimeConfigForm, SelectField, SwitchField};

#[component]
pub(super) fn EconomicsFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    rsx! {
        ManagedSection { form, disabled, path: "operations.economics.managed", title: t!("admin-runtime-op-section-economics"), note: t!("admin-runtime-op-note-economics"), on_dirty,
            div { class: "admin-runtime-field-grid",
                NumberField { form, disabled, path: "operations.economics.start_price", label: t!("admin-runtime-op-start-price"), minimum: "1000", maximum: "1000000000", on_dirty }
                NumberField { form, disabled, path: "operations.economics.bid_increment_ppm", label: t!("admin-runtime-op-bid-increment-ppm"), minimum: "0", maximum: "1000000", on_dirty }
                NumberField { form, disabled, path: "operations.economics.soft_close_window_seconds", label: t!("admin-runtime-op-soft-close-window-seconds"), minimum: "0", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.economics.soft_close_extension_seconds", label: t!("admin-runtime-op-soft-close-extension-seconds"), minimum: "1", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.economics.cancel_cutoff_seconds", label: t!("admin-runtime-op-cancel-cutoff-seconds"), minimum: "0", maximum: "604800", on_dirty }
                SwitchField { form, disabled, path: "operations.economics.proxy_enabled", label: t!("admin-runtime-op-proxy-enabled"), on_dirty }
                SelectField { form, disabled, path: "operations.economics.proxy_default_mode", label: t!("admin-runtime-op-proxy-default-mode"), options: vec![("fast".to_owned(), t!("admin-runtime-op-mode-fast")), ("slow".to_owned(), t!("admin-runtime-op-mode-slow"))], on_dirty }
                NumberField { form, disabled, path: "operations.economics.proxy_slow_interval_seconds", label: t!("admin-runtime-op-proxy-slow-interval-seconds"), minimum: "1", maximum: "604800", on_dirty }
                NumberField { form, disabled, path: "operations.economics.proxy_max_outbid_uses", label: t!("admin-runtime-op-proxy-max-outbid-uses"), minimum: "0", maximum: "1000", on_dirty }
                NumberField { form, disabled, path: "operations.economics.final_phase_lead_seconds", label: t!("admin-runtime-op-final-phase-lead-seconds"), minimum: "1", maximum: "604800", on_dirty }
            }
        }
    }
}
