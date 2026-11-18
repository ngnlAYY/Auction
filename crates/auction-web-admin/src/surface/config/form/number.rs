//! 配置数值控件的回填、离焦反馈与纠错时机。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::surface::config::field_announcement::FieldAnnouncer;
use crate::surface::config::hot_fields::is_hot;
use crate::surface::config::validation::{FormIssues, NumberIssue, field_id, number_issue};

#[derive(Clone, Copy)]
struct NumberEdit {
    issue: Option<NumberIssue>,
    reported: bool,
}

#[component]
pub(super) fn NumberInput(
    path: String,
    label: String,
    minimum: &'static str,
    maximum: &'static str,
    original: Option<String>,
    value: String,
    disabled: bool,
    on_input: EventHandler<String>,
) -> Element {
    let issues = use_context::<FormIssues>();
    let announcer = use_context::<FieldAnnouncer>();
    let mut edit = use_signal(|| None::<NumberEdit>);
    let issue = number_issue(&value, minimum, maximum);
    let historic_error = issue.is_some() && original.as_deref() == Some(value.as_str());
    let in_summary = issues.0.read().iter().any(|issue| issue.path == path);
    let message = issue
        .filter(|_| edit().map_or(historic_error, |edit| edit.reported) || in_summary)
        .map(|issue| issue.message(minimum, maximum));
    let was_invalid = message.is_some();
    let id = field_id(&path);
    let error_id = format!("{id}-error");
    let hot = is_hot(&path);

    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}"
                if hot {
                    span { class: "admin-runtime-hot-badge", title: t!("admin-runtime-hot-field-title"), {t!("admin-runtime-hot-field-badge")} }
                }
            }
            input {
                id,
                name: path.clone(),
                autocomplete: "off",
                inputmode: "numeric",
                "data-runtime-number": path.clone(),
                "data-runtime-label": label,
                aria_invalid: was_invalid.to_string(),
                aria_describedby: was_invalid.then_some(error_id),
                r#type: "number",
                disabled,
                min: minimum,
                max: maximum,
                step: "1",
                value,
                oninput: {
                    let path = path.clone();
                    move |event: FormEvent| {
                        let value = event.value();
                        let issue = number_issue(&value, minimum, maximum);
                        // 输入只延续或清除已展示的错误；新错误在离焦或提交后展示。
                        let reported = edit.peek().map_or(historic_error, |edit| edit.reported)
                            || issues.0.peek().iter().any(|issue| issue.path == path);
                        edit.set(Some(NumberEdit { issue, reported: reported && issue.is_some() }));
                        if reported && issue.is_none() {
                            announcer.clear_field(&field_id(&path));
                        }
                        on_input.call(value);
                    }
                },
                onblur: move |_| {
                    let pending_edit = *edit.peek();
                    if let Some(NumberEdit { issue: Some(issue), reported: false }) = pending_edit
                        && !issues.0.peek().iter().any(|issue| issue.path == path)
                    {
                        edit.set(Some(NumberEdit { issue: Some(issue), reported: true }));
                        announcer.announce_on_blur(&field_id(&path), issue.message(minimum, maximum));
                    }
                },
            }
            if let Some(message) = message.as_ref() {
                small { id: error_id.clone(), class: "admin-config-field-error", "{message}" }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::rc::Rc;

    use dioxus::core::Mutation;
    use dioxus::prelude::*;

    use super::NumberInput;
    use crate::surface::auctions::draft_page_tests::init_i18n;
    use crate::surface::config::field_announcement::{FieldLiveRegion, use_field_announcer};
    use crate::surface::config::test_form::{FormBrowser, dispatch};
    use crate::surface::config::validation::FormIssues;
    use crate::surface::realtime::text_attributes;

    fn number_pair() -> Element {
        init_i18n();
        let mut first = use_signal(|| "11".to_owned());
        let mut second = use_signal(|| "0".to_owned());
        let issues = use_signal(Vec::new);
        use_context_provider(|| FormIssues(issues));
        let announcer = use_field_announcer();
        use_context_provider(|| announcer);
        rsx! {
            NumberInput {
                path: "first", label: "数字一", minimum: "0", maximum: "10",
                original: Some("11".to_owned()), value: first(), disabled: false,
                on_input: move |value| first.set(value),
            }
            NumberInput {
                path: "second", label: "数字二", minimum: "0", maximum: "20",
                original: Some("0".to_owned()), value: second(), disabled: false,
                on_input: move |value| second.set(value),
            }
            FieldLiveRegion { announcer }
        }
    }

    #[test]
    fn correcting_a_number_only_cancels_its_own_pending_announcement() {
        for corrected in ["first", "second"] {
            let browser = Rc::new(FormBrowser::default());
            let mut dom = VirtualDom::new(number_pair).with_root_context(browser.clone());
            let edits = dom.rebuild_to_vec().edits;
            let field = |path| {
                text_attributes(&edits, "data-runtime-number")
                    .find_map(|(id, name)| (name == path).then_some(id))
                    .unwrap()
            };
            let second = field("second");
            dispatch(&dom, second, "input", "21");
            dom.render_immediate_to_vec();
            dispatch(&dom, second, "blur", "");
            let invalid = dom.render_immediate_to_vec().edits;
            assert!(
                text_attributes(&invalid, "aria-invalid").any(|entry| entry == (second, "true"))
            );
            dispatch(&dom, field(corrected), "input", "1");
            dom.render_immediate_to_vec();

            browser.fire_timer();
            let announcement = dom.render_immediate_to_vec().edits;
            assert_eq!(
                announcement.iter().any(|edit| matches!(edit,
                    Mutation::SetText { value, .. } if value.contains("20")
                )),
                corrected == "first"
            );
        }
    }
}
