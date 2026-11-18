//! 页面持有的拍品改价与撤回草稿、版本冲突及提交。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::super::auctions::{
    DraftFeedback, DraftRead, DraftRefreshButton, close_edit_draft, close_matching_draft,
    focus_draft_field_after_render, observe_draft_write,
};
use super::presentation::{MAX_START_PRICE, admin_lot_code_for, parse_start_price};
use super::state::{LotDialog, LotUpdateNotice};
use crate::api::{AdminLot, AdminLotUpdateAction, update_admin_lot};
use crate::surface::pending::{PendingCommandPanel, PendingCommands, set_if_alive};
use crate::surface::validation::FieldError;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LotSnapshot {
    lot_id: String,
    pub(super) code: String,
    state_version: i64,
    start_price: i64,
    seller: Option<String>,
}

impl LotSnapshot {
    pub(super) fn from_lot(lot: &AdminLot) -> Self {
        Self {
            lot_id: lot.id.clone(),
            code: admin_lot_code_for(&lot.equipment_category, lot.lot_number),
            state_version: lot.state_version,
            start_price: lot.start_price,
            seller: lot.seller.clone(),
        }
    }
}

/// 编辑值和版本来自同一打开操作；行消失或读取失败都不销毁它们。
#[derive(Clone)]
pub(super) struct LotEdit {
    kind: LotDialog,
    base: LotSnapshot,
    price: String,
}

impl LotEdit {
    fn new(kind: LotDialog, base: LotSnapshot) -> Self {
        let price = base.start_price.to_string();
        Self { kind, base, price }
    }

    pub(super) fn lot_id(&self) -> &str {
        &self.base.lot_id
    }
}

pub(super) fn open_lot_edit(
    mut draft: Signal<Option<LotEdit>>,
    kind: LotDialog,
    base: LotSnapshot,
) -> bool {
    if draft.peek().is_some() {
        return false;
    }
    draft.set(Some(LotEdit::new(kind, base)));
    true
}

#[component]
pub(super) fn AdminLotEditDialog(
    mut draft: Signal<Option<LotEdit>>,
    latest_version: Option<i64>,
    read: DraftRead,
    reload_generation: Signal<u64>,
    notice: Signal<Option<LotUpdateNotice>>,
) -> Element {
    let owner = use_context::<PendingCommands>();
    let mut failure_code = use_signal(|| None::<String>);
    let mut invalid_price = use_signal(|| false);
    let Some(edit) = draft() else {
        return rsx! {};
    };
    let entity = format!("lot:{}", edit.lot_id());
    let frozen = owner.contains(&entity);
    let busy = owner.is_busy(&entity);
    let failure = failure_code();
    let conflict = !frozen
        && (failure.as_deref() == Some("stale_lot_state")
            || read.version_changed(edit.base.state_version, latest_version));
    let can_submit =
        read.is_ready() && !frozen && !conflict && latest_version == Some(edit.base.state_version);
    let on_close = move |()| close_edit_draft(draft, reload_generation, conflict);
    let reprice = edit.kind == LotDialog::Reprice;
    let code = edit.base.code.clone();
    let price_input_id = format!("price-{}", edit.lot_id());
    let price_error_id = format!("{price_input_id}-error");
    let described_by = invalid_price().then(|| price_error_id.clone());
    let focus_id = price_input_id.clone();
    let seller = edit.base.seller.clone().unwrap_or_else(|| t!("value-empty"));

    rsx! {
        Dialog {
            title: if reprice {
                t!("admin-lots-reprice-title", code: code)
            } else {
                t!("admin-lots-withdraw-title", code: code)
            },
            close_label: t!("admin-lots-action-cancel"),
            busy,
            on_close,
            form {
                class: if reprice { "admin-auction-reprice" } else { "" },
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if !can_submit { return; }
                    let Some(current) = draft() else { return; };
                    let price = if reprice {
                        let Some(price) = parse_start_price(&current.price) else {
                            invalid_price.set(true);
                            focus_draft_field_after_render(focus_id.clone());
                            return;
                        };
                        Some(price)
                    } else {
                        None
                    };
                    if submit_lot_update(owner, &current, price, draft, failure_code, notice) {
                        failure_code.set(None);
                    }
                },
                if reprice {
                    label {
                        r#for: price_input_id.clone(),
                        {t!("admin-lots-start-price-label")}
                    }
                    input {
                        id: price_input_id,
                        name: "start_price",
                        // 数字控件切为只读文本，便于选中并复制原输入。
                        r#type: if conflict { "text" } else { "number" },
                        min: "1",
                        max: MAX_START_PRICE.to_string(),
                        step: "1",
                        required: true,
                        disabled: frozen,
                        readonly: conflict,
                        autofocus: !conflict,
                        value: edit.price,
                        aria_invalid: invalid_price().to_string(),
                        aria_describedby: described_by,
                        oninput: move |event| {
                            if frozen || conflict { return; }
                            if let Some(edit) = draft.write().as_mut() {
                                edit.price = event.value();
                            }
                            invalid_price.set(false);
                        },
                    }
                    FieldError {
                        id: price_error_id,
                        message: invalid_price().then(|| t!("admin-lots-invalid-price", max: MAX_START_PRICE)),
                    }
                } else {
                    p { {t!("admin-lots-withdraw-warning", seller: seller)} }
                }
                if !frozen {
                    DraftFeedback { conflict, read: read.clone(), failure }
                }
                div { class: "admin-shell__dialog-actions",
                    if !conflict {
                        button {
                            r#type: "button",
                            disabled: busy,
                            onclick: move |_| on_close(()),
                            {t!("admin-lots-action-cancel")}
                        }
                    }
                    if !frozen {
                        DraftRefreshButton { read, conflict, reload: reload_generation, on_close }
                    }
                    if !conflict && (!frozen || busy) {
                        button {
                            class: if reprice { "admin-btn-primary" } else { "admin-btn-danger" },
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy {
                                {t!("admin-action-working")}
                            } else if reprice {
                                {t!("admin-lots-reprice-save")}
                            } else {
                                {t!("admin-lots-withdraw-confirm")}
                            }
                        }
                    }
                }
            }
            if frozen && !busy {
                PendingCommandPanel { owner }
            }
        }
    }
}

/// 版本来自打开时的快照；未知结果重试同一完整负载，旧回执只关闭对应的草稿。
fn submit_lot_update(
    owner: PendingCommands,
    edit: &LotEdit,
    price: Option<i64>,
    draft: Signal<Option<LotEdit>>,
    failure: Signal<Option<String>>,
    notice: Signal<Option<LotUpdateNotice>>,
) -> bool {
    let (action, success, label) = match edit.kind {
        LotDialog::Reprice => (
            AdminLotUpdateAction::SetStartPrice,
            LotUpdateNotice::Repriced,
            t!("admin-lots-reprice"),
        ),
        LotDialog::Withdraw => {
            (AdminLotUpdateAction::Withdraw, LotUpdateNotice::Withdrawn, t!("admin-lots-withdraw"))
        }
    };
    let lot_id = edit.lot_id().to_owned();
    let state_version = edit.base.state_version;
    let base = edit.base.clone();
    let kind = edit.kind;
    let submitted = owner.submit(
        format!("lot:{lot_id}"),
        label,
        move |command_id| {
            observe_draft_write(
                update_admin_lot(command_id, lot_id.clone(), state_version, action, price),
                failure,
            )
        },
        move |_| {
            close_matching_draft(draft, |edit| edit.base == base && edit.kind == kind);
            set_if_alive(notice, Some(success));
        },
        |_| {},
    );
    if submitted {
        set_if_alive(notice, None);
    }
    submitted
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use dioxus::dioxus_core::{ElementId, Mutation};
    use dioxus::prelude::*;
    use gloo_net::http::Method;

    use super::super::row::AdminLotRow;
    use super::{AdminLotEditDialog, LotEdit};
    use crate::api::AdminLot;
    use crate::surface::auctions::draft_page_tests::{
        CapturedWrite, DIALOG_TITLE_CLOSE, RecordRead, WriteScript, assert_field, button,
        click_button, command_button, has_live_text, has_text, init_i18n, mount_writer,
        settle_writer, writer_form,
    };
    use crate::surface::auctions::use_draft_page;
    use crate::surface::pagination::use_admin_pagination;
    use crate::surface::pending::{PendingCommands, use_pending_commands};
    use crate::surface::realtime::{TestPage, text_attributes};

    const LOT_ID: &str = "20f99cb5-7b75-4853-bc49-9c71df78381b";
    const EDITED_PRICE: &str = "777";
    const REPRICE: &str = "改起拍价";
    const CLOSE_AND_REFRESH: &str = "关闭并刷新列表";

    struct LotsPageTest {
        reads: RecordRead<AdminLot>,
        controls: Cell<Option<LotEditControls>>,
        displayed_version: Cell<Option<i64>>,
    }

    #[derive(Clone, Copy)]
    struct LotEditControls {
        draft: Signal<Option<LotEdit>>,
        owner: PendingCommands,
    }

    fn harness(test: Rc<LotsPageTest>) -> Element {
        init_i18n();
        let reload_generation = use_signal(|| 0_u64);
        let owner = use_pending_commands(reload_generation, |_| true);
        let draft = use_signal(|| None::<LotEdit>);
        let notice = use_signal(|| None);
        let selected_lot = use_signal(|| None);
        let edited = draft.read();
        let pagination = use_admin_pagination("lots", &["page_size"], Some(100), 100_000);
        let source = Rc::clone(&test);
        let (read, page) = use_draft_page(
            pagination,
            reload_generation,
            edited.is_some(),
            || (),
            move |(), _| {
                let source = Rc::clone(&source);
                async move { source.reads.fetch().await }
            },
        );
        test.controls.set(Some(LotEditControls { draft, owner }));
        let snapshot = page.and_then(Result::ok);
        test.displayed_version.set(snapshot.as_ref().map(|lot| lot.state_version));
        let latest_version = snapshot
            .as_ref()
            .filter(|lot| edited.as_ref().is_some_and(|draft| draft.lot_id() == lot.id))
            .map(|lot| lot.state_version);
        rsx! {
            if let Some(lot) = snapshot {
                table { tbody {
                    AdminLotRow {
                        lot: lot.as_ref().clone(), selected_lot, notice,
                        allow_lot_update: true, edits_ready: read.is_ready(), draft,
                    }
                } }
            }
            if edited.is_some() {
                AdminLotEditDialog { draft, latest_version, read, reload_generation, notice }
            }
        }
    }

    fn lot(state_version: i64, price: i64, status: &str) -> AdminLot {
        serde_json::from_value(serde_json::json!({
            "id": LOT_ID, "auction_id": "20f99cb5-7b75-4853-bc49-9c71df78381c",
            "equipment_id": "20f99cb5-7b75-4853-bc49-9c71df78381d", "submission_id": null,
            "lot_number": 1, "status": status, "start_price": price, "current_price": price,
            "leader_user_id": null, "effective_end_at": null, "slow_due_at": null,
            "slow_generation": 0, "slow_authorization_id": null, "state_version": state_version,
            "created_at": "2026-11-01T00:00:00Z", "updated_at": "2026-11-01T01:00:00Z",
            "external_equipment_id": "123", "equipment_name": "Phase Robe",
            "equipment_category": "Cloth", "equipment_attributes": {},
            "equipment_collected_at": "2026-11-01T00:00:00Z", "seller": null,
            "submission_source_kind": null, "submission_source_reference": null,
            "mailbox_key": null, "external_mail_id": null, "submission_received_at": null
        }))
        .unwrap()
    }

    struct LotCommandTest {
        page: TestPage,
        test: Rc<LotsPageTest>,
        controls: LotEditControls,
        writes: WriteScript,
    }

    impl LotCommandTest {
        fn mount() -> Self {
            let test = Rc::new(LotsPageTest {
                reads: RecordRead::new(),
                controls: Cell::new(None),
                displayed_version: Cell::new(None),
            });
            let writes = WriteScript::default();
            let (mut page, _) = mount_writer({
                let test = Rc::clone(&test);
                let writes = writes.clone();
                move || {
                    use_hook(|| writes.install());
                    harness(Rc::clone(&test))
                }
            });
            settle_writer(&mut page);
            let controls = test.controls.get().unwrap();
            test.reads.respond(Ok(lot(7, 100, "pending")));
            settle_writer(&mut page);
            Self { page, test, controls, writes }
        }

        fn open_price(&mut self, initial: &str) -> (ElementId, ElementId) {
            let opened = click_button(&mut self.page, REPRICE);
            let input = text_attributes(&opened, "id")
                .find_map(|(id, name)| (name == format!("price-{LOT_ID}")).then_some(id))
                .unwrap();
            assert_field(&self.page, input, initial, false);
            (writer_form(&opened), input)
        }

        fn editing_price(&mut self) -> (ElementId, ElementId) {
            let (form, input) = self.open_price("100");
            self.page.dispatch("input", input, EDITED_PRICE);
            assert_field(&self.page, input, EDITED_PRICE, false);
            (form, input)
        }

        fn submit(&mut self, form: ElementId) -> Vec<Mutation> {
            self.page.dispatch("submit", form, "")
        }

        fn occupied(&self) -> bool {
            self.controls.owner.contains(&format!("lot:{LOT_ID}"))
        }

        fn take_request(&self) -> CapturedWrite {
            let [request] = self.writes.take_writes().try_into().unwrap();
            request
        }

        fn assert_conflict(&self, updates: &[Mutation], input: ElementId) {
            assert!(!self.occupied());
            assert!(has_text(updates, "本次修改没有生效"));
            assert!(!has_live_text(&self.page, "保存"));
            assert!(matches!(button(&self.page, CLOSE_AND_REFRESH), Some((_, false))));
            assert_field(&self.page, input, EDITED_PRICE, true);
        }

        fn close_and_refresh(&mut self, close_label: &str, current: AdminLot) {
            click_button(&mut self.page, close_label);
            assert!(self.controls.draft.peek().is_none());
            assert!(button(&self.page, CLOSE_AND_REFRESH).is_none());
            assert_eq!(self.test.displayed_version.get(), Some(7));
            assert!(
                matches!(button(&self.page, REPRICE), Some((_, true))),
                "{close_label}关闭后须重读列表"
            );
            let version = current.state_version;
            self.test.reads.respond(Ok(current));
            settle_writer(&mut self.page);
            assert_eq!(self.test.displayed_version.get(), Some(version));
        }
    }

    fn assert_lot_request(request: &CapturedWrite, version: i64, action: &str, price: Option<i64>) {
        let id = request.command_id();
        let price = price.map(|price| format!(r#","start_price":{price}"#)).unwrap_or_default();
        let expected = format!(
            r#"{{"command_id":"{id}","expected_state_version":{version},"action":"{action}"{price}}}"#
        );
        assert_eq!(request.method, Method::PATCH);
        assert_eq!(request.url, format!("/api/v1/admin/lots/{LOT_ID}"));
        assert_eq!(request.body, expected.as_bytes());
    }

    #[test]
    fn reprice_draft_survives_failed_refresh_and_successful_retry() {
        let mut test = LotCommandTest::mount();
        let (_, input) = test.editing_price();
        let loading = click_button(&mut test.page, "刷新记录");
        assert!(has_text(&loading, "正在刷新记录"));
        assert_eq!(test.test.displayed_version.get(), Some(7));
        test.test.reads.respond(Err("network_error".to_owned()));
        let failed = settle_writer(&mut test.page);
        assert!(has_text(&failed, "刷新记录失败"));
        assert_eq!(test.test.displayed_version.get(), Some(7));
        assert_field(&test.page, input, EDITED_PRICE, false);
        click_button(&mut test.page, "重试");
        test.test.reads.respond(Ok(lot(8, 200, "pending")));
        let resolved = settle_writer(&mut test.page);
        assert_eq!(test.test.displayed_version.get(), Some(8));
        test.assert_conflict(&resolved, input);
        assert_eq!(test.controls.draft.peek().as_ref().unwrap().base.state_version, 7);
    }

    #[test]
    fn stale_price_closes_and_the_fresh_row_opens_a_new_draft() {
        for close_label in [CLOSE_AND_REFRESH, DIALOG_TITLE_CLOSE] {
            price_close_scenario(close_label);
        }
    }

    #[test]
    fn closing_a_price_draft_without_a_conflict_does_not_refresh_the_list() {
        let mut test = LotCommandTest::mount();
        test.editing_price();
        click_button(&mut test.page, DIALOG_TITLE_CLOSE);
        assert!(test.controls.draft.peek().is_none());
        assert!(matches!(button(&test.page, REPRICE), Some((_, false))));
        test.open_price("100");
    }

    fn price_close_scenario(close_label: &str) {
        let mut test = LotCommandTest::mount();
        let (form, input) = test.editing_price();
        test.writes.reply_conflict("stale_lot_state");
        let rejected = test.submit(form);
        let first = test.take_request();
        assert_lot_request(&first, 7, "set_start_price", Some(777));
        test.assert_conflict(&rejected, input);
        test.submit(form);
        assert!(test.writes.take_writes().is_empty());

        test.close_and_refresh(close_label, lot(8, 200, "pending"));
        let (new_form, new_input) = test.open_price("200");
        test.page.dispatch("input", new_input, "333");
        test.writes.reply_http(
            200,
            serde_json::json!({
                "code": "ok", "message": "改价成功", "data": {"lot": lot(9, 333, "pending")},
                "request_id": "review-fixture",
            })
            .to_string(),
        );
        test.submit(new_form);
        let second = test.take_request();
        assert_lot_request(&second, 8, "set_start_price", Some(333));
        assert_ne!(first.command_id(), second.command_id());
        assert!(test.controls.draft.peek().is_none());
    }

    #[test]
    fn a_withdrawn_lot_has_no_edit_entry_on_the_refreshed_row() {
        let mut test = LotCommandTest::mount();
        let opened = click_button(&mut test.page, "撤拍");
        test.writes.reply_conflict("stale_lot_state");
        let rejected = test.submit(writer_form(&opened));
        assert!(has_text(&rejected, "本次修改没有生效"));
        assert!(!test.occupied());
        assert_lot_request(&test.take_request(), 7, "withdraw", None);
        test.close_and_refresh(CLOSE_AND_REFRESH, lot(8, 100, "withdrawn"));
        assert!(button(&test.page, REPRICE).is_none());
        assert!(button(&test.page, "撤拍").is_none());
    }

    #[test]
    fn incomplete_conflict_receipts_leave_the_original_price_intent_locked() {
        for body in [
            r#"{"code":"stale_lot_state","message":"版本已变化","data":null}"#,
            r#"["stale_lot_state","版本已变化",null,null]"#,
        ] {
            let mut test = LotCommandTest::mount();
            let (form, _) = test.editing_price();
            test.writes.reply_http(409, body);
            test.submit(form);
            let first = test.take_request();
            assert!(test.occupied());
            assert!(matches!(
                command_button(&test.page, first.command_id(), "原样重试"),
                Some((_, false))
            ));
            assert!(button(&test.page, CLOSE_AND_REFRESH).is_none());
            assert!(!has_live_text(&test.page, "这条记录已被其他操作更新，本次修改没有生效。"));
            assert_eq!(test.controls.draft.peek().as_ref().unwrap().price, EDITED_PRICE);
            test.submit(form);
            assert!(test.writes.take_writes().is_empty());
        }
    }

    #[test]
    fn timed_out_price_replays_identical_bytes_before_a_verified_conflict_unlocks_it() {
        let mut test = LotCommandTest::mount();
        let (form, input) = test.editing_price();
        test.writes.reply_timeout();
        test.submit(form);
        let first = test.take_request();
        assert_lot_request(&first, 7, "set_start_price", Some(777));
        assert!(test.occupied());
        let (retry, disabled) = command_button(&test.page, first.command_id(), "原样重试")
            .expect("原命令条目的共享重试按钮");
        assert!(!disabled);
        assert!(command_button(&test.page, uuid::Uuid::nil(), "原样重试").is_none());
        assert!(button(&test.page, CLOSE_AND_REFRESH).is_none());
        assert!(button(&test.page, "刷新记录").is_none());
        assert!(!has_live_text(&test.page, "保存"));
        test.submit(form);
        assert!(test.writes.take_writes().is_empty());

        test.writes.reply_conflict("stale_lot_state");
        let rejected = test.page.dispatch("click", retry, "");
        assert_eq!(test.take_request(), first);
        test.assert_conflict(&rejected, input);
        assert!(command_button(&test.page, first.command_id(), "原样重试").is_none());
    }
}
