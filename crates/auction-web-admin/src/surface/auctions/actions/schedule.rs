//! Admin 正式开拍前预览、开拍与计划结束时间修改对话框。

use auction_time::Timestamp;
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::super::editor::{
    DraftFeedback, DraftRead, DraftRefreshButton, close_edit_draft, close_matching_draft,
    focus_draft_field_after_render, observe_draft_write,
};
use super::super::presentation::{
    ScheduleField, ScheduleIssue, ScheduleMode, ScheduleTimes, parse_local_schedule_time,
    validate_schedule,
};
use super::model::{AdminAuctionNotice, stale_auction_failure};
use crate::api::{AdminAuction, datetime_local_value, update_admin_auction_schedule};
use crate::surface::pending::{PendingCommandPanel, PendingCommands, set_if_alive};
use crate::surface::validation::FieldError;

#[component]
pub(super) fn AdminAuctionScheduleDialog(
    draft: Signal<Option<ScheduleDraft>>,
    latest_version: Option<i64>,
    read: DraftRead,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
) -> Element {
    let owner = use_context::<PendingCommands>();
    let mut feedback = use_signal(|| None::<String>);
    let mut issue = use_signal(|| None::<(ScheduleField, ScheduleIssue)>);
    let Some(open_draft) = draft() else {
        return rsx! {};
    };
    let entity = format!("auction:{}", open_draft.auction_id);
    let blocked = owner.contains(&entity);
    let busy = owner.is_busy(&entity);
    let failure = feedback();
    let conflict = !blocked
        && (failure.as_deref().is_some_and(stale_auction_failure)
            || read.version_changed(open_draft.state_version, latest_version));
    let can_submit = read.is_ready()
        && !blocked
        && !conflict
        && latest_version == Some(open_draft.state_version);
    let on_close = move |()| close_edit_draft(draft, reload_generation, conflict);
    let auction_id = open_draft.auction_id.clone();
    rsx! {
        Dialog {
            title: t!("admin-auctions-schedule-title", number: open_draft.sequence),
            close_label: t!("admin-auctions-action-close"),
            busy,
            on_close,
            form {
                class: "admin-auction-open",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if !can_submit {
                        return;
                    }
                    let Some(submitted) = draft() else { return; };
                    let times = match submitted.times(Timestamp::now()) {
                        Ok(times) => times,
                        Err(error) => {
                            issue.set(Some(error));
                            focus_draft_field_after_render(schedule_field_id(error.0, &submitted.auction_id));
                            return;
                        }
                    };
                    issue.set(None);
                    if submit_schedule(submitted, times, owner, notice, draft, feedback) {
                        feedback.set(None);
                    }
                },
                p { class: "admin-panel__note", {t!("admin-auctions-schedule-note")} }
                for field in [ScheduleField::Preview, ScheduleField::Open, ScheduleField::End] {
                    ScheduleInputField {
                        field,
                        auction_id: auction_id.clone(),
                        disabled: blocked,
                        read_only: conflict,
                        value: open_draft.input(field).value.clone(),
                        preview_locked: open_draft.mode == ScheduleMode::Preview,
                        draft,
                        issue,
                    }
                }
                if !blocked {
                    DraftFeedback { conflict, read: read.clone(), failure }
                }
                div { class: "admin-shell__dialog-actions",
                    if !conflict {
                        button {
                            r#type: "button",
                            disabled: busy,
                            onclick: move |_| on_close(()),
                            {t!("admin-auctions-action-close")}
                        }
                    }
                    if !blocked {
                        DraftRefreshButton { read, conflict, reload: reload_generation, on_close }
                    }
                    if !conflict && (!blocked || busy) {
                        button {
                            class: "admin-btn-primary",
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy { {t!("admin-action-working")} }
                            else { {t!("admin-auctions-schedule-submit")} }
                        }
                    }
                }
            }
            if blocked && !busy {
                PendingCommandPanel { owner }
            }
        }
    }
}

#[component]
fn ScheduleInputField(
    field: ScheduleField,
    auction_id: String,
    disabled: bool,
    read_only: bool,
    value: String,
    preview_locked: bool,
    mut draft: Signal<Option<ScheduleDraft>>,
    mut issue: Signal<Option<(ScheduleField, ScheduleIssue)>>,
) -> Element {
    let field_locked = field == ScheduleField::Preview && preview_locked;
    let autofocus =
        field == if preview_locked { ScheduleField::Open } else { ScheduleField::Preview };
    let input_id = schedule_field_id(field, &auction_id);
    let error_id = format!("{input_id}-error");
    let error = issue().filter(|(invalid, _)| *invalid == field).map(|(_, issue)| issue);
    let described_by = error.map(|_| error_id.clone());
    let label = match field {
        ScheduleField::Preview => t!("admin-auctions-schedule-preview"),
        ScheduleField::Open => t!("admin-auctions-schedule-open"),
        ScheduleField::End => t!("admin-auctions-schedule-end"),
    };
    rsx! {
        label { r#for: input_id.clone(), "{label}" }
        input {
            id: input_id,
            name: schedule_field_name(field),
            // 只读文本保留原本地时间字符串，允许整段选择和复制。
            r#type: if read_only { "text" } else { "datetime-local" },
            required: true,
            autofocus: autofocus && !read_only,
            disabled: disabled || (field_locked && !read_only),
            readonly: read_only,
            value,
            aria_invalid: error.is_some().to_string(),
            aria_describedby: described_by,
            oninput: move |event| {
                if disabled || field_locked || read_only { return; }
                if let Some(current) = draft.write().as_mut() {
                    current.set_input(field, event.value());
                }
                if issue().is_some_and(|(invalid, _)| invalid == field) {
                    issue.set(None);
                }
            },
        }
        FieldError { id: error_id, message: error.map(ScheduleIssue::message) }
    }
}

fn schedule_field_name(field: ScheduleField) -> &'static str {
    match field {
        ScheduleField::Preview => "preview_scheduled_at",
        ScheduleField::Open => "scheduled_open_at",
        ScheduleField::End => "scheduled_end_at",
    }
}

fn schedule_field_id(field: ScheduleField, auction_id: &str) -> String {
    format!("schedule-{}-{auction_id}", schedule_field_name(field))
}

/// 实体、版本和三个原始时刻来自同一快照。编辑与刷新都不能更新这个版本。
#[derive(Clone)]
pub(super) struct ScheduleDraft {
    auction_id: String,
    sequence: i64,
    state_version: i64,
    mode: ScheduleMode,
    preview: ScheduleInput,
    open: ScheduleInput,
    end: ScheduleInput,
}

impl ScheduleDraft {
    pub(super) fn from_auction(auction: &AdminAuction) -> Option<Self> {
        let mode = match auction.status.as_str() {
            "draft" => ScheduleMode::Draft,
            "preview" => ScheduleMode::Preview,
            _ => return None,
        };
        Some(Self {
            auction_id: auction.id.clone(),
            sequence: auction.sequence,
            state_version: auction.state_version,
            mode,
            preview: ScheduleInput::new(Some(&auction.preview_scheduled_at)),
            open: ScheduleInput::new(Some(&auction.scheduled_open_at)),
            end: ScheduleInput::new(auction.scheduled_ends_at.as_deref()),
        })
    }

    pub(super) fn auction_id(&self) -> &str {
        &self.auction_id
    }

    fn input(&self, field: ScheduleField) -> &ScheduleInput {
        match field {
            ScheduleField::Preview => &self.preview,
            ScheduleField::Open => &self.open,
            ScheduleField::End => &self.end,
        }
    }

    fn set_input(&mut self, field: ScheduleField, value: String) {
        let input = self.input_mut(field);
        input.edited |= input.value != value;
        input.value = value;
    }

    fn input_mut(&mut self, field: ScheduleField) -> &mut ScheduleInput {
        match field {
            ScheduleField::Preview => &mut self.preview,
            ScheduleField::Open => &mut self.open,
            ScheduleField::End => &mut self.end,
        }
    }

    fn times(&self, now: Timestamp) -> Result<ScheduleTimes, (ScheduleField, ScheduleIssue)> {
        validate_schedule(
            self.preview.resolve(),
            self.open.resolve(),
            self.end.resolve(),
            self.mode,
            now,
        )
    }
}

#[derive(Clone)]
struct ScheduleInput {
    original: Option<Timestamp>,
    value: String,
    edited: bool,
}

impl ScheduleInput {
    fn new(original: Option<&str>) -> Self {
        Self {
            original: original.and_then(|value| Timestamp::parse_from_rfc3339(value).ok()),
            value: original.and_then(datetime_local_value).unwrap_or_default(),
            edited: false,
        }
    }

    fn resolve(&self) -> Result<Timestamp, ScheduleIssue> {
        // 未改动的预填值已有确定的 UTC（包括回拨小时及秒以下精度）；新输入必须唯一可逆。
        if !self.edited
            && let Some(original) = self.original
        {
            return Ok(original);
        }
        parse_local_schedule_time(&self.value)
    }
}

/// 页级重试保留首次提交的版本、UTC 值和 command_id；只有已确认成功才清空原草稿。
fn submit_schedule(
    draft: ScheduleDraft,
    times: ScheduleTimes,
    owner: PendingCommands,
    notice: Signal<Option<AdminAuctionNotice>>,
    open: Signal<Option<ScheduleDraft>>,
    feedback: Signal<Option<String>>,
) -> bool {
    let submitted_id = draft.auction_id.clone();
    let submitted_version = draft.state_version;
    owner.submit(
        format!("auction:{}", draft.auction_id),
        t!("admin-auctions-action-schedule"),
        move |command_id| {
            observe_draft_write(
                update_admin_auction_schedule(
                    command_id,
                    draft.auction_id.clone(),
                    times.preview,
                    times.open,
                    times.end,
                    draft.state_version,
                ),
                feedback,
            )
        },
        move |_| {
            set_if_alive(notice, Some(AdminAuctionNotice::Schedule));
            set_if_alive(feedback, None);
            // 离开本框后从未决区重试时，回执不能关闭操作员新打开的另一份草稿。
            close_matching_draft(open, |draft| {
                draft.auction_id == submitted_id && draft.state_version == submitted_version
            });
        },
        |_| {},
    )
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{ScheduleDraft, ScheduleInput};
    use crate::api::AdminAuction;

    fn auction() -> AdminAuction {
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "world_id": "main", "sequence": 3, "state_version": 7, "status": "draft",
            "preview_scheduled_at": "2099-04-01T11:00:03.123456Z",
            "scheduled_open_at": "2099-04-01T12:00:00Z", "scheduled_ends_at": "2099-04-01T13:00:00Z",
            "preview_opened_at": null, "preview_manual_open": false, "preview_revision": 0,
            "opened_at": null, "final_phase_at": null, "settled_at": null, "cancelled_at": null,
            "cancelled_reason": null, "cancelled_by": null,
            "created_at": "2099-04-01T10:00:00Z", "updated_at": "2099-04-01T10:00:00Z"
        })).unwrap()
    }

    #[test]
    fn a_refresh_does_not_rebase_values_or_version() {
        let mut latest = auction();
        let mut draft = ScheduleDraft::from_auction(&latest).unwrap();
        let now = Timestamp::parse_from_rfc3339(&latest.created_at).unwrap();
        draft.end.value = "2099-04-01T14:00".to_owned();
        draft.end.edited = true;
        latest.state_version += 1;
        latest.scheduled_open_at = "2099-04-01T12:30:00Z".to_owned();
        assert_eq!(draft.state_version, 7);
        assert_eq!(
            draft.times(now).unwrap().open,
            Timestamp::parse_from_rfc3339("2099-04-01T12:00:00Z").unwrap()
        );
        let fresh = ScheduleDraft::from_auction(&latest).unwrap();
        assert_eq!(fresh.end.original, draft.end.original);
        assert!(!fresh.end.edited);
        assert_eq!(fresh.state_version, 8);
        assert_ne!(fresh.times(now).unwrap().open, draft.times(now).unwrap().open);
    }

    #[test]
    fn untouched_prefill_keeps_original_utc_precision() {
        let original = "2099-04-01T11:00:03.123456Z";
        let mut field = ScheduleInput::new(Some(original));
        assert_eq!(field.resolve(), Ok(Timestamp::parse_from_rfc3339(original).unwrap()));
        field.edited = true;
        field.value.clear();
        assert!(field.resolve().is_err());
    }
}
