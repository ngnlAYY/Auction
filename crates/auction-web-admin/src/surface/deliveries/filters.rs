//! Delivery 检索筛选器组件：状态 + 收件人、参考编号、模板、类别、世界与时间。

use auction_time::{SecondsFormat, Timestamp};
use auction_web_shared::{a11y::focus_element_by_id_after_render, t};
use dioxus::prelude::*;

use super::labels::{DELIVERY_STATUS_VALUES, delivery_category_label, delivery_status_label};
use crate::api::{LocalTimeError, datetime_local_value, parse_local_datetime};
use crate::surface::admin_world_label;

/// 提交前的筛选草稿；只在「应用筛选」时写入地址栏，输入过程不触发网络请求。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DeliveryFiltersDraft {
    pub recipient: String,
    pub reference: String,
    pub template: String,
    pub category: String,
    pub world: String,
    pub created_from: String,
    pub created_to: String,
}

const DELIVERY_CATEGORY_VALUES: [&str; 7] =
    ["outbid", "account", "intake", "settlement", "equipment", "operations", "manual"];
const CREATED_FROM_FIELD: &str = "delivery-created-from";
const CREATED_TO_FIELD: &str = "delivery-created-to";

/// 只转换显示值，草稿在实际编辑前保留地址栏的精确时刻。
fn timestamp_to_local_input(value: &str) -> String {
    if Timestamp::parse_from_rfc3339(value).is_ok() {
        datetime_local_value(value).unwrap_or_default()
    } else {
        value.to_owned()
    }
}

fn time_bound(value: &str) -> Result<String, LocalTimeError> {
    if value.is_empty() || Timestamp::parse_from_rfc3339(value).is_ok() {
        return Ok(value.to_owned());
    }
    // 无效边界不能降级成无界查询。
    parse_local_datetime(value).map(|value| value.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn time_bound_error(issue: LocalTimeError) -> String {
    match issue {
        LocalTimeError::Invalid => t!("admin-auctions-invalid-local-time"),
        LocalTimeError::Nonexistent => t!("admin-auctions-nonexistent-local-time"),
        LocalTimeError::Ambiguous => t!("admin-auctions-ambiguous-local-time"),
    }
}

#[component]
pub(super) fn DeliveryFilters(
    /// 当前状态筛选（地址栏恢复后的值）。
    status: String,
    recipient: String,
    reference: String,
    template: String,
    category: String,
    world: String,
    created_from: String,
    created_to: String,
    on_status_change: EventHandler<String>,
    on_filters_change: EventHandler<DeliveryFiltersDraft>,
) -> Element {
    let applied = DeliveryFiltersDraft {
        recipient,
        reference,
        template,
        category,
        world,
        created_from,
        created_to,
    };
    let mut draft = use_signal(|| applied.clone());
    let mut from_error = use_signal(|| None);
    let mut to_error = use_signal(|| None);
    // 已应用筛选随前进/后退或提交变化时整体重建草稿；否则旧草稿会在下一次“应用”时把过期
    // 条件写回地址栏。已应用筛选不变时保留操作员尚未提交的编辑。
    use_effect(use_reactive((&applied,), move |(applied,)| {
        if *draft.peek() != applied {
            draft.set(applied);
        }
        from_error.set(None);
        to_error.set(None);
    }));
    rsx! {
        div { class: "admin-delivery-filter",
            label { r#for: "delivery-status",
                {t!("delivery-filter")}
                select {
                    id: "delivery-status",
                    value: status,
                    onchange: move |event| on_status_change.call(event.value()),
                    option { value: "", {t!("delivery-all")} }
                    for status in DELIVERY_STATUS_VALUES {
                        option {
                            value: status,
                            {delivery_status_label(status)}
                        }
                    }
                }
            }
            form {
                class: "admin-delivery-search",
                onsubmit: move |event| {
                    event.prevent_default();
                    let current = draft();
                    let from = time_bound(&current.created_from);
                    let to = time_bound(&current.created_to);
                    from_error.set(from.as_ref().err().copied());
                    to_error.set(to.as_ref().err().copied());
                    let first_invalid = if from.is_err() { CREATED_FROM_FIELD } else { CREATED_TO_FIELD };
                    let (Ok(created_from), Ok(created_to)) = (from, to) else {
                        focus_element_by_id_after_render(first_invalid);
                        return;
                    };
                    on_filters_change.call(DeliveryFiltersDraft {
                        recipient: current.recipient.trim().to_owned(),
                        reference: current.reference.trim().to_owned(),
                        template: current.template.trim().to_owned(),
                        category: current.category,
                        world: current.world,
                        created_from,
                        created_to,
                    });
                },
                label { r#for: "delivery-recipient",
                    span { {t!("delivery-search-recipient")} }
                    input {
                        id: "delivery-recipient",
                        r#type: "text",
                        maxlength: "256",
                        value: "{draft().recipient}",
                        oninput: move |event| draft.write().recipient = event.value(),
                    }
                }
                label { r#for: "delivery-reference",
                    span { {t!("delivery-search-reference")} }
                    input {
                        id: "delivery-reference",
                        r#type: "text",
                        maxlength: "36",
                        value: "{draft().reference}",
                        oninput: move |event| draft.write().reference = event.value(),
                    }
                }
                label { r#for: "delivery-template",
                    span { {t!("delivery-template")} }
                    input {
                        id: "delivery-template",
                        r#type: "text",
                        maxlength: "64",
                        value: "{draft().template}",
                        oninput: move |event| draft.write().template = event.value(),
                    }
                }
                label { r#for: "delivery-category",
                    span { {t!("delivery-category")} }
                    select {
                        id: "delivery-category",
                        value: "{draft().category}",
                        onchange: move |event| draft.write().category = event.value(),
                        option { value: "", {t!("delivery-all")} }
                        for value in DELIVERY_CATEGORY_VALUES {
                            option { value: value, {delivery_category_label(value)} }
                        }
                    }
                }
                label { r#for: "delivery-world",
                    span { {t!("delivery-world")} }
                    select {
                        id: "delivery-world",
                        value: "{draft().world}",
                        onchange: move |event| draft.write().world = event.value(),
                        option { value: "", {t!("delivery-all")} }
                        option { value: "main", {admin_world_label("main")} }
                        option { value: "isekai", {admin_world_label("isekai")} }
                    }
                }
                label { r#for: CREATED_FROM_FIELD,
                    span { {t!("delivery-created-from")} }
                    input {
                        id: CREATED_FROM_FIELD,
                        r#type: "datetime-local",
                        aria_invalid: from_error().is_some().to_string(),
                        aria_describedby: from_error().is_some().then_some("delivery-created-from-error"),
                        value: timestamp_to_local_input(&draft.read().created_from),
                        oninput: move |event| {
                            let value = event.value();
                            if from_error.peek().is_some() && time_bound(&value).is_ok() {
                                from_error.set(None);
                            }
                            draft.write().created_from = value;
                        },
                    }
                    if let Some(issue) = from_error() {
                        small { id: "delivery-created-from-error", class: "admin-field-error", {time_bound_error(issue)} }
                    }
                }
                label { r#for: CREATED_TO_FIELD,
                    span { {t!("delivery-created-to")} }
                    input {
                        id: CREATED_TO_FIELD,
                        r#type: "datetime-local",
                        aria_invalid: to_error().is_some().to_string(),
                        aria_describedby: to_error().is_some().then_some("delivery-created-to-error"),
                        value: timestamp_to_local_input(&draft.read().created_to),
                        oninput: move |event| {
                            let value = event.value();
                            if to_error.peek().is_some() && time_bound(&value).is_ok() {
                                to_error.set(None);
                            }
                            draft.write().created_to = value;
                        },
                    }
                    if let Some(issue) = to_error() {
                        small { id: "delivery-created-to-error", class: "admin-field-error", {time_bound_error(issue)} }
                    }
                }
                button { class: "admin-btn-primary", r#type: "submit", {t!("delivery-search-apply")} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::{DeliveryFilters, DeliveryFiltersDraft};
    use crate::surface::auctions::editor::tests::{mount_writer, writer_form};
    use crate::surface::realtime::text_attributes;

    #[test]
    fn invalid_delivery_time_blocks_submit_and_unedited_prefill_keeps_precision() {
        let applied = Rc::new(RefCell::new(Vec::<DeliveryFiltersDraft>::new()));
        let from = "2026-11-18T09:05:37.500123456+08:00";
        let to = "2026-11-19T01:05:00Z";
        let (mut page, edits) = mount_writer({
            let applied = applied.clone();
            move || {
                let applied = applied.clone();
                rsx! {
                    DeliveryFilters {
                        status: "", recipient: "", reference: "", template: "", category: "",
                        world: "", created_from: from, created_to: to, on_status_change: |_| {},
                        on_filters_change: move |filters| applied.borrow_mut().push(filters),
                    }
                }
            }
        });
        let form = writer_form(&edits);
        let to_input = text_attributes(&edits, "value")
            .find_map(|(id, value)| (value == "2026-11-19T01:05").then_some(id))
            .unwrap();

        page.dispatch("input", to_input, "2026-11-31T01:05");
        let invalid = page.dispatch("submit", form, "");
        assert!(applied.borrow().is_empty());
        assert!(text_attributes(&invalid, "aria-invalid").any(|entry| entry == (to_input, "true")));
        assert!(
            text_attributes(&invalid, "aria-describedby")
                .any(|entry| entry == (to_input, "delivery-created-to-error"))
        );

        page.dispatch("input", to_input, "2026-11-20T01:05");
        page.dispatch("submit", form, "");
        let applied = applied.borrow();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].created_from, from);
        assert_eq!(applied[0].created_to, "2026-11-20T01:05:00Z");
    }
}
