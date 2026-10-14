//! Admin 拍卖创建表单。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{create_admin_auction, normalize_datetime};
use crate::app::surface::validation::{FieldError, focus_field};
use crate::app::surface::{AdminActionFeedback, run_admin_action};

use super::actions::AdminAuctionNotice;
use super::helpers::{AuctionScheduleIssue, auction_schedule_issue};

#[component]
pub(super) fn AdminAuctionCreateForm(
    reload_generation: Signal<u64>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let mut open = use_signal(|| false);
    let busy = use_signal(|| false);

    rsx! {
        button {
            class: "admin-btn-primary admin-auction-create-opener",
            r#type: "button",
            onclick: move |_| {
                if !open() {
                    // 打开新流程时清掉上一次的页面级成功提示。
                    notice.set(None);
                    open.set(true);
                }
            },
            {t!("admin-auctions-create-opener")}
        }
        if open() {
            // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；aria_busy 改挂到表单上。
            Dialog {
                title: t!("admin-auctions-create-title"),
                close_label: t!("admin-auctions-create-close"),
                busy: busy(),
                on_close: move |_| open.set(false),
                AdminAuctionCreateFields { reload_generation, open, busy, notice }
            }
        }
    }
}

/// 首次提交时冻结标识与规范时间；未知结果重试不重新取时间或生成身份。
#[derive(Clone)]
struct CreateRequest {
    command_id: Uuid,
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
}

/// 创建表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CreateFieldErrors {
    world: Option<CreateFieldIssue>,
    sequence: Option<CreateFieldIssue>,
    preview: Option<CreateFieldIssue>,
    open: Option<CreateFieldIssue>,
    end: Option<CreateFieldIssue>,
}

impl CreateFieldErrors {
    /// 是否存在任一字段错误。
    const fn any(&self) -> bool {
        self.world.is_some()
            || self.sequence.is_some()
            || self.preview.is_some()
            || self.open.is_some()
            || self.end.is_some()
    }

    /// 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。
    const fn first_invalid_id(&self) -> Option<&'static str> {
        if self.world.is_some() {
            Some("create-world")
        } else if self.sequence.is_some() {
            Some("create-sequence")
        } else if self.preview.is_some() {
            Some("create-preview")
        } else if self.open.is_some() {
            Some("create-open")
        } else if self.end.is_some() {
            Some("create-end")
        } else {
            None
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateFieldIssue {
    Required,
    NotPositiveInteger,
    PreviewNotFuture,
    OpenBeforePreview,
    EndNotAfterOpen,
    /// 服务端返回世界不存在。
    WorldNotFound,
}

fn create_field_issue_message(issue: CreateFieldIssue) -> String {
    match issue {
        CreateFieldIssue::Required => t!("admin-field-required"),
        CreateFieldIssue::NotPositiveInteger => t!("admin-field-positive-integer"),
        CreateFieldIssue::PreviewNotFuture => t!("admin-auctions-create-field-preview-past"),
        CreateFieldIssue::OpenBeforePreview => {
            t!("admin-auctions-create-field-open-before-preview")
        }
        CreateFieldIssue::EndNotAfterOpen => t!("admin-auctions-create-field-end-after-open"),
        CreateFieldIssue::WorldNotFound => t!("admin-error-world-not-found"),
    }
}

#[component]
fn AdminAuctionCreateFields(
    reload_generation: Signal<u64>,
    mut open: Signal<bool>,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let mut world_id = use_signal(String::new);
    let mut sequence = use_signal(|| "1".to_owned());
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let mut request_state = use_signal(|| None::<CreateRequest>);
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(CreateFieldErrors::default);

    rsx! {
        Fragment {
            form {
            class: "admin-auction-create",
            aria_busy: busy().to_string(),
            onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let world_id_value = world_id();
                    let preview_scheduled_at_value = preview_scheduled_at();
                    let scheduled_open_at_value = scheduled_open_at();
                    let scheduled_end_at_value = scheduled_end_at();
                    // 逐字段校验并定位错误；服务端 400 类错误也尽量落回对应字段。
                    let mut errors = CreateFieldErrors::default();
                    if world_id_value.trim().is_empty() {
                        errors.world = Some(CreateFieldIssue::Required);
                    }
                    let sequence_value = sequence().trim().parse::<i64>().ok().filter(|value| *value > 0);
                    if sequence_value.is_none() {
                        errors.sequence = Some(CreateFieldIssue::NotPositiveInteger);
                    }
                    if preview_scheduled_at_value.trim().is_empty() {
                        errors.preview = Some(CreateFieldIssue::Required);
                    }
                    if scheduled_open_at_value.trim().is_empty() {
                        errors.open = Some(CreateFieldIssue::Required);
                    }
                    if scheduled_end_at_value.trim().is_empty() {
                        errors.end = Some(CreateFieldIssue::Required);
                    }
                    // 原样重试由服务端先查账本，不能用当前时钟否定已提交的请求。
                    if request_state().is_none() && errors.preview.is_none() && errors.open.is_none() && errors.end.is_none() {
                        match auction_schedule_issue(
                            &preview_scheduled_at_value,
                            &scheduled_open_at_value,
                            &scheduled_end_at_value,
                        ) {
                            None => {}
                            Some(AuctionScheduleIssue::PreviewNotFuture) => {
                                errors.preview = Some(CreateFieldIssue::PreviewNotFuture);
                            }
                            Some(AuctionScheduleIssue::OpenBeforePreview) => {
                                errors.open = Some(CreateFieldIssue::OpenBeforePreview);
                            }
                            Some(AuctionScheduleIssue::EndNotAfterOpen) => {
                                errors.end = Some(CreateFieldIssue::EndNotAfterOpen);
                            }
                            // 三项均非空却无法解析属于畸形输入；落到预览字段提示重新选择。
                            Some(AuctionScheduleIssue::Unparseable) => {
                                errors.preview = Some(CreateFieldIssue::Required);
                            }
                        }
                    }
                    if errors.any() {
                        field_errors.set(errors);
                        if let Some(field_id) = errors.first_invalid_id() {
                            focus_field(field_id);
                        }
                        return;
                    }
                    // sequence 校验未过时 errors 非空并在上面返回，这里必有值。
                    let Some(sequence_value) = sequence_value else {
                        return;
                    };
                    field_errors.set(CreateFieldErrors::default());
                    feedback.set(None);
                    let request = request_state().unwrap_or_else(|| {
                        let request = CreateRequest {
                            command_id: Uuid::new_v4(),
                            auction_id: Uuid::new_v4(),
                            world_id: world_id_value,
                            sequence: sequence_value,
                            preview_scheduled_at: normalize_datetime(&preview_scheduled_at_value),
                            scheduled_open_at: normalize_datetime(&scheduled_open_at_value),
                            scheduled_end_at: normalize_datetime(&scheduled_end_at_value),
                        };
                        request_state.set(Some(request.clone()));
                        request
                    });
                    let mut feedback = feedback;
                    let mut field_errors = field_errors;
                    let mut reload_generation = reload_generation;
                    let mut notice = notice;
                    let mut open = open;
                    run_admin_action(
                        busy,
                        async move {
                            create_admin_auction(
                                request.command_id,
                                request.auction_id,
                                request.world_id,
                                request.sequence,
                                request.preview_scheduled_at,
                                request.scheduled_open_at,
                                request.scheduled_end_at,
                            )
                            .await
                        },
                        move |_| {
                            // 对话框内反馈随关闭一并销毁；成功提示移交页面级 notice。
                            feedback.set(None);
                            notice.set(Some(AdminAuctionNotice::Created));
                            field_errors.set(CreateFieldErrors::default());
                            reload_generation.set(reload_generation().saturating_add(1));
                            open.set(false);
                        },
                        move |code| {
                            // 可归属字段的服务端错误落到字段旁，其余保持页面级反馈。
                            if code == "world_not_found" {
                                field_errors.write().world = Some(CreateFieldIssue::WorldNotFound);
                                focus_field("create-world");
                            }
                            feedback.set(Some(AdminActionFeedback::Error(code)));
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
                    disabled: busy(),
                    autofocus: true,
                    required: true,
                    aria_invalid: field_errors().world.is_some().then_some("true"),
                    aria_describedby: field_errors().world.is_some().then_some("create-world-error"),
                    value: world_id(),
                    onchange: move |event| {
                        world_id.set(event.value());
                        request_state.set(None);
                        field_errors.write().world = None;
                        feedback.set(None);
                    },
                    option { value: "", disabled: true, {t!("value-empty")} }
                    option { value: "main", {t!("admin-auctions-world-main")} }
                    option { value: "isekai", {t!("admin-auctions-world-isekai")} }
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
                    r#type: "number",
                    min: "1",
                    required: true,
                    disabled: busy(),
                    aria_invalid: field_errors().sequence.is_some().then_some("true"),
                    aria_describedby: field_errors().sequence.is_some().then_some("create-sequence-error"),
                    value: sequence(),
                    oninput: move |event| {
                        sequence.set(event.value());
                        request_state.set(None);
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
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    aria_invalid: field_errors().preview.is_some().then_some("true"),
                    aria_describedby: field_errors().preview.is_some().then_some("create-preview-error"),
                    value: preview_scheduled_at(),
                    oninput: move |event| {
                        preview_scheduled_at.set(event.value());
                        request_state.set(None);
                        field_errors.write().preview = None;
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-preview-error",
                    message: field_errors().preview.map(create_field_issue_message),
                }
                label {
                    r#for: "create-open",
                    {t!("admin-auctions-create-open")}
                }
                input {
                    id: "create-open",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    aria_invalid: field_errors().open.is_some().then_some("true"),
                    aria_describedby: field_errors().open.is_some().then_some("create-open-error"),
                    value: scheduled_open_at(),
                    oninput: move |event| {
                        scheduled_open_at.set(event.value());
                        request_state.set(None);
                        field_errors.write().open = None;
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-open-error",
                    message: field_errors().open.map(create_field_issue_message),
                }
                label {
                    r#for: "create-end",
                    {t!("admin-auctions-create-end")}
                }
                input {
                    id: "create-end",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    aria_invalid: field_errors().end.is_some().then_some("true"),
                    aria_describedby: field_errors().end.is_some().then_some("create-end-error"),
                    value: scheduled_end_at(),
                    oninput: move |event| {
                        scheduled_end_at.set(event.value());
                        request_state.set(None);
                        field_errors.write().end = None;
                        feedback.set(None);
                    },
                }
                FieldError {
                    id: "create-end-error",
                    message: field_errors().end.map(create_field_issue_message),
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
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
