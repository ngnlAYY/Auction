//! 代理命令成功后的安全反馈面板。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::super::super::format_credit_amount;
use super::super::super::proxy::{proxy_status_label, proxy_transition_label};
use crate::api::ProxyCommandAuthorization;

pub(super) fn proxy_success_panel(
    authorization: &ProxyCommandAuthorization,
    on_close: EventHandler<()>,
) -> Element {
    let success_message = match authorization.bid.as_ref() {
        Some(bid) if bid.leading => {
            t!("proxy-leading-now", price: format_credit_amount(bid.current_price))
        }
        _ => t!("proxy-saved"),
    };
    let quota = if authorization.quota.limit == 0 {
        t!("account-quota-unlimited", used: authorization.quota.used)
    } else {
        t!(
            "account-quota-bounded",
            used: authorization.quota.used,
            limit: authorization.quota.limit,
            remaining: authorization.quota.limit.saturating_sub(authorization.quota.used)
        )
    };
    let transitions = if authorization.transitions.is_empty() {
        t!("value-empty")
    } else {
        authorization
            .transitions
            .iter()
            .map(|transition| {
                format!(
                    "{}: {} · {}",
                    transition.transition_number,
                    proxy_transition_label(&transition.kind),
                    format_credit_amount(transition.new_maximum_amount),
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let status = proxy_status_label(&authorization.status);

    rsx! {
        section { class: "public-auction-bid-form",
            p { class: "action-feedback action-feedback--success", role: "status",
                "{success_message}"
            }
            p { class: "public-auction-bid-form__meta",
                {t!("proxy-status", status: status)}
                " · "
                "{quota}"
            }
            p { class: "public-auction-bid-form__meta public-auction-bid-form__history",
                {t!("account-transition-history")}
                ": "
                "{transitions}"
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {t!("catalog-close")}
                }
            }
        }
    }
}
