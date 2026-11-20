//! Admin 拍卖创建表单。

use auction_time::Timestamp;
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;
use uuid::Uuid;

use super::actions::AdminAuctionNotice;
use super::presentation::{
    ScheduleField, ScheduleIssue, ScheduleMode, ScheduleTimes, parse_local_schedule_time,
    validate_schedule,
};
use crate::api::{MAX_JSON_SAFE_INTEGER, create_admin_auction};
use crate::surface::pending::{PendingCommands, set_if_alive};
use crate::surface::validation::{FieldError, focus_field};
use crate::surface::{AdminActionFeedback, admin_world_label};

#[component]
pub(super) fn AdminAuctionCreateForm(mut notice: Signal<Option<AdminAuctionNotice>>) -> Element {
    let mut open = use_signal(|| false);
    let pending = use_context::<PendingCommands>();

    rsx! {
        button {
            class: "admin-btn-primary admin-auction-create-opener",
            r#type: "button",
            disabled: pending.contains("create-auction"),
            onclick: move |_| {
                if !open() && !pending.contains("create-auction") {
                    // 打开新流程时清掉上一次的页面级成功提示。
                    notice.set(None);
                    open.set(true);
                }
            },
            {t!("admin-auctions-create-opener")}
        }
        if open() {
            // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；aria_busy 挂在表单上。
            Dialog {
                title: t!("admin-auctions-create-title"),
                close_label: t!("admin-auctions-create-close"),
                busy: pending.is_busy("create-auction"),
                on_close: move |_| open.set(false),
                AdminAuctionCreateFields { open, notice }
            }
        }
    }
}

/// 首次提交时冻结标识与规范时间；未知结果重试不重新取时间或生成身份。
#[derive(Clone)]
struct CreateRequest {
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    times: ScheduleTimes,
}

/// 创建表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CreateFieldErrors {
    world: Option<CreateFieldIssue>,
    sequence: Option<CreateFieldIssue>,
    schedule: Option<(ScheduleField, ScheduleIssue)>,
}

impl CreateFieldErrors {
    const fn any(&self) -> bool {
        self.world.is_some() || self.sequence.is_some() || self.schedule.is_some()
    }

    /// 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。
    const fn first_invalid_id(&self) -> Option<&'static str> {
        if self.world.is_some() {
            Some("create-world")
        } else if self.sequence.is_some() {
            Some("create-sequence")
        } else {
            match self.schedule {
                Some((ScheduleField::Preview, _)) => Some("create-preview"),
                Some((ScheduleField::Open, _)) => Some("create-open"),
                Some((ScheduleField::End, _)) => Some("create-end"),
                None => None,
            }
        }
    }
    fn schedule_issue(self, field: ScheduleField) -> Option<ScheduleIssue> {
        self.schedule.filter(|(invalid, _)| *invalid == field).map(|(_, issue)| issue)
    }

    fn clear_schedule_issue(&mut self, field: ScheduleField) {
        if self.schedule_issue(field).is_some() {
            self.schedule = None;
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateFieldIssue {
    Required,
    InvalidSequence,
    /// 服务端返回世界不存在。
    WorldNotFound,
}

fn create_field_issue_message(issue: CreateFieldIssue) -> String {
    match issue {
        CreateFieldIssue::Required => t!("admin-field-required"),
        CreateFieldIssue::InvalidSequence => {
            t!("admin-auctions-create-field-sequence-range", max: MAX_JSON_SAFE_INTEGER)
        }
        CreateFieldIssue::WorldNotFound => t!("admin-error-world-not-found"),
    }
}

fn parse_sequence(value: &str) -> Option<i64> {
    // 与 Admin 创建命令使用的公开 JSON 安全整数上限一致。
    value.trim().parse().ok().filter(|value| (1..=MAX_JSON_SAFE_INTEGER).contains(value))
}

#[component]
fn AdminAuctionCreateFields(
    mut open: Signal<bool>,
    notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let mut world_id = use_signal(String::new);
    let mut sequence = use_signal(|| "1".to_owned());
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let pending = use_context::<PendingCommands>();
    let busy = move || pending.is_busy("create-auction");
    let frozen = move || pending.contains("create-auction");
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(CreateFieldErrors::default);

    rsx! {
        Fragment {
            form {
            class: "admin-auction-create",
            novalidate: true,
            aria_busy: busy().to_string(),
            onsubmit: move |event| {
                    event.prevent_default();
                    if frozen() {
                        return;
                    }
                    let world_id_value = world_id();
                    let mut errors = CreateFieldErrors::default();
                    if world_id_value.trim().is_empty() {
                        errors.world = Some(CreateFieldIssue::Required);
                    }
                    let sequence_value = parse_sequence(&sequence());
                    if sequence_value.is_none() {
                        errors.sequence = Some(CreateFieldIssue::InvalidSequence);
                    }
                    let times = validate_schedule(
                        parse_local_schedule_time(&preview_scheduled_at()),
                        parse_local_schedule_time(&scheduled_open_at()),
                        parse_local_schedule_time(&scheduled_end_at()),
                        ScheduleMode::Create,
                        Timestamp::now(),
                    );
                    errors.schedule = times.as_ref().err().copied();
                    if errors.any() {
                        field_errors.set(errors);
                        if let Some(field_id) = errors.first_invalid_id() {
                            focus_field(field_id);
                        }
                        return;
                    }
                    let (Some(sequence_value), Ok(times)) = (sequence_value, times) else {
                        return;
                    };
                    field_errors.set(CreateFieldErrors::default());
                    feedback.set(None);
                    let request = CreateRequest {
                        auction_id: Uuid::new_v4(),
                        world_id: world_id_value,
                        sequence: sequence_value,
                        times,
                    };
                    pending.submit(
                        "create-auction".to_owned(),
                        format!("{} · {} · {sequence_value}", t!("admin-auctions-create-submit"), admin_world_label(&request.world_id)),
                        move |command_id| {
                            let request = request.clone();
                            async move {
                                create_admin_auction(
                                    command_id,
                                    request.auction_id,
                                    request.world_id,
                                    request.sequence,
                                    request.times.preview,
                                    request.times.open,
                                    request.times.end,
                                )
                                .await
                            }
                        },
                        move |_| {
                            // 页面持有成功提示；关闭弹窗不取消请求，也不丢弃回执。
                            set_if_alive(notice, Some(AdminAuctionNotice::Created));
                            set_if_alive(open, false);
                        },
                        move |code| {
                            let mut field_errors = field_errors;
                            if code == "world_not_found"
                                && let Ok(mut errors) = field_errors.try_write()
                            {
                                errors.world = Some(CreateFieldIssue::WorldNotFound);
                            }
                            set_if_alive(feedback, Some(AdminActionFeedback::Error(code)));
                        },
                    );
                },
                p { class: "admin-panel__note", {t!("admin-auctions-create-timezone")} }
                label {
                    r#for: "create-world",
                    {t!("admin-auctions-create-world")}
                }
                select {
                    id: "create-world",
                    name: "world",
                    autocomplete: "off",
                    disabled: frozen(),
                    autofocus: true,
                    required: true,
                    aria_invalid: field_errors().world.is_some().to_string(),
                    aria_describedby: field_errors().world.is_some().then_some("create-world-error"),
                    value: world_id(),
                    onchange: move |event| {
                        if frozen() { return; }
                        world_id.set(event.value());
                        field_errors.write().world = None;
                        feedback.set(None);
                    },
                    option { value: "", disabled: true, {t!("value-empty")} }
                    option { value: "main", {admin_world_label("main")} }
                    option { value: "isekai", {admin_world_label("isekai")} }
                }
                FieldError {
                    id: "create-world-error",
                    message: field_errors().world.map(create_field_issue_message),
                }
                label {
                    r#for: "create-sequence",
                    {t!("admin-auctions-create-sequence")}
                }
                input {
                    id: "create-sequence",
                    name: "sequence",
                    autocomplete: "off",
                    inputmode: "numeric",
                    r#type: "number",
                    min: "1",
                    max: MAX_JSON_SAFE_INTEGER.to_string(),
                    required: true,
                    disabled: frozen(),
                    aria_invalid: field_errors().sequence.is_some().to_string(),
                    aria_describedby: field_errors().sequence.is_some().then_some("create-sequence-error"),
                    value: sequence(),
                    oninput: move |event| {
                        if frozen() { return; }
                        sequence.set(event.value());
                        field_errors.write().sequence = None;
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-sequence-error",
                    message: field_errors().sequence.map(create_field_issue_message),
                }
                label {
                    r#for: "create-preview",
                    {t!("admin-auctions-create-preview")}
                }
                input {
                    id: "create-preview",
                    name: "preview_scheduled_at",
                    autocomplete: "off",
                    r#type: "datetime-local",
                    required: true,
                    disabled: frozen(),
                    aria_invalid: field_errors().schedule_issue(ScheduleField::Preview).is_some().to_string(),
                    aria_describedby: field_errors().schedule_issue(ScheduleField::Preview).is_some().then_some("create-preview-error"),
                    value: preview_scheduled_at(),
                    oninput: move |event| {
                        if frozen() { return; }
                        preview_scheduled_at.set(event.value());
                        field_errors.write().clear_schedule_issue(ScheduleField::Preview);
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-preview-error",
                    message: field_errors().schedule_issue(ScheduleField::Preview).map(ScheduleIssue::message),
                }
                label {
                    r#for: "create-open",
                    {t!("admin-auctions-create-open")}
                }
                input {
                    id: "create-open",
                    name: "scheduled_open_at",
                    autocomplete: "off",
                    r#type: "datetime-local",
                    required: true,
                    disabled: frozen(),
                    aria_invalid: field_errors().schedule_issue(ScheduleField::Open).is_some().to_string(),
                    aria_describedby: field_errors().schedule_issue(ScheduleField::Open).is_some().then_some("create-open-error"),
                    value: scheduled_open_at(),
                    oninput: move |event| {
                        if frozen() { return; }
                        scheduled_open_at.set(event.value());
                        field_errors.write().clear_schedule_issue(ScheduleField::Open);
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-open-error",
                    message: field_errors().schedule_issue(ScheduleField::Open).map(ScheduleIssue::message),
                }
                label {
                    r#for: "create-end",
                    {t!("admin-auctions-create-end")}
                }
                input {
                    id: "create-end",
                    name: "scheduled_end_at",
                    autocomplete: "off",
                    r#type: "datetime-local",
                    required: true,
                    disabled: frozen(),
                    aria_invalid: field_errors().schedule_issue(ScheduleField::End).is_some().to_string(),
                    aria_describedby: field_errors().schedule_issue(ScheduleField::End).is_some().then_some("create-end-error"),
                    value: scheduled_end_at(),
                    oninput: move |event| {
                        if frozen() { return; }
                        scheduled_end_at.set(event.value());
                        field_errors.write().clear_schedule_issue(ScheduleField::End);
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-end-error",
                    message: field_errors().schedule_issue(ScheduleField::End).map(ScheduleIssue::message),
                }
                button {
                    r#type: "submit",
                    disabled: frozen(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-auctions-create-submit")} }
                }
                if let Some(feedback) = feedback() {
                    p {
                        class: if feedback.is_success() {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        {if feedback.is_success() {
                            t!("admin-auctions-create-success")
                        } else {
                            feedback.message(t!("admin-auctions-create-success"))
                        }}
                    }
                }
            }
            div { class: "admin-shell__dialog-actions",
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    {t!("admin-auctions-create-close")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::{dioxus_core, dioxus_signals, rsx, use_hook, use_signal};
    use gloo_net::http::Method;

    use super::super::editor::tests::{click_button, has_text, mount_writer, writer_form};
    use super::{AdminAuctionCreateForm, parse_sequence};
    use crate::api::AdminApiFailure;
    use crate::surface::pending::use_pending_commands;
    use crate::surface::realtime::text_attributes;

    #[test]
    fn creates_only_positive_json_safe_sequence_numbers() {
        assert_eq!(parse_sequence("9007199254740991"), Some(9_007_199_254_740_991));
        for input in ["0", "-1", "9007199254740992", "1.5", "99999999999999999999"] {
            assert_eq!(parse_sequence(input), None, "{input}");
        }
    }

    #[test]
    fn create_only_marks_the_world_field_for_world_not_found() {
        const PATH: &str = "/api/v1/admin/auctions";
        for (status, body, expected) in [
            (
                404,
                r#"{"code":"world_not_found","message":"世界不存在","data":null,"request_id":null}"#,
                "true",
            ),
            (
                409,
                r#"{"code":"world_has_active_auction","message":"已有场次","data":null,"request_id":null}"#,
                "false",
            ),
        ] {
            let (mut page, _) = mount_writer(move || {
                use_hook(|| AdminApiFailure::stub_write_response(PATH, Method::POST, status, body));
                let generation = use_signal(|| 0);
                use_pending_commands(generation, |_| true);
                let notice = use_signal(|| None);
                rsx! { AdminAuctionCreateForm { notice } }
            });
            let edits = click_button(&mut page, "创建拍卖");
            let world = edits
                .iter()
                .find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
                    _ => None,
                })
                .expect("业务世界选项");
            let date_inputs = edits
                .iter()
                .filter_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                    _ => None,
                })
                .skip(1);
            page.dispatch("change", world, "main");
            for (input, value) in
                date_inputs.zip(["2099-04-01T11:00", "2099-04-01T12:00", "2099-04-01T13:00"])
            {
                page.dispatch("input", input, value);
            }
            let submitted = page.dispatch("submit", writer_form(&edits), "");
            assert!(has_text(
                &submitted,
                if expected == "true" {
                    "该业务世界已不存在"
                } else {
                    "该世界仍有未结束的拍卖"
                },
            ));
            assert_eq!(
                text_attributes(&submitted, "aria-invalid").find(|(id, _)| *id == world),
                (expected == "true").then_some((world, "true")),
            );
        }
    }
}
