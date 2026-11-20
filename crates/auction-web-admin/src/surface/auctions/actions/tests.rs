//! 排期和生命周期对话框使用真实的页级草稿、行入口与读取逻辑，列表响应由测试控制。

use std::assert_matches;
use std::cell::Cell;
use std::rc::Rc;

use dioxus::dioxus_core::{ElementId, Mutation};
use dioxus::prelude::*;
use gloo_net::http::Method;

use super::super::editor::tests::{
    CapturedWrite, DIALOG_TITLE_CLOSE, RecordRead, WriteScript, assert_field, button, click_button,
    command_button, has_live_text, has_text, init_i18n, mount_writer, settle_writer, writer_form,
};
use super::super::editor::use_draft_page;
use super::{AdminAuctionDraftDialogs, AdminAuctionRow, AuctionDrafts, use_auction_drafts};
use crate::api::{AdminAuction, AdminAuctionCapabilities};
use crate::surface::pagination::use_admin_pagination;
use crate::surface::pending::{PendingCommands, use_pending_commands};
use crate::surface::realtime::{TestPage, text_attributes};

const AUCTION_ID: &str = "20f99cb5-7b75-4853-bc49-9c71df78381c";
const CLOSE_CONFLICT: &str = "关闭";
const SCHEDULE: &str = "修改拍卖时间";
const SAVE_SCHEDULE: &str = "保存拍卖时间";
const END: &str = "schedule-scheduled_end_at";
const EDITED_END: &str = "2099-04-01T14:17";
const CANCEL: &str = "取消拍卖";
const CONFIRM_CANCEL: &str = "确认取消拍卖";
const REASON: &str = "action-reason";
const EDITED_REASON: &str = "保留待核对的取消原因";

struct AuctionPageTest {
    reads: RecordRead<AdminAuction>,
    drafts: Cell<Option<AuctionDrafts>>,
    displayed_version: Cell<Option<i64>>,
    owner: Cell<Option<PendingCommands>>,
}

#[derive(Clone, Debug, PartialEq, Routable)]
enum TestRoute {
    #[route("/")]
    Fixture {},
}

#[component]
fn Fixture() -> Element {
    let test = consume_context::<Rc<AuctionPageTest>>();
    init_i18n();
    let reload_generation = use_signal(|| 0);
    test.owner.set(Some(use_pending_commands(reload_generation, |_| true)));
    let drafts = use_auction_drafts();
    let notice = use_signal(|| None);
    let edited_auction = drafts.auction_id();
    test.drafts.set(Some(drafts));
    let pagination = use_admin_pagination("auctions", &["page_size"], Some(100), 100_000);
    let source = Rc::clone(&test);
    let (read, page) = use_draft_page(
        pagination,
        reload_generation,
        edited_auction.is_some(),
        || (),
        move |(), _| {
            let source = Rc::clone(&source);
            async move { source.reads.fetch().await }
        },
    );
    let snapshot = page.and_then(Result::ok);
    test.displayed_version.set(snapshot.as_ref().map(|auction| auction.state_version));
    let latest_version = snapshot
        .as_ref()
        .filter(|auction| edited_auction.as_deref() == Some(auction.id.as_str()))
        .map(|auction| auction.state_version);
    let capabilities = AdminAuctionCapabilities {
        modify_schedule: true,
        cancel_auction: true,
        manual_preview_open: false,
        manual_early_open: false,
        manual_early_settlement: false,
        manual_intake: false,
    };
    rsx! {
        if let Some(auction) = snapshot {
            table { tbody {
                AdminAuctionRow { auction: auction.as_ref().clone(), capabilities, notice, edits_ready: read.is_ready() }
            } }
        }
        if edited_auction.is_some() {
            AdminAuctionDraftDialogs { drafts, latest_version, read, notice, reload_generation }
        }
    }
}

fn auction() -> AdminAuction {
    serde_json::from_value(serde_json::json!({
        "id": AUCTION_ID,
        "world_id": "main", "sequence": 3, "state_version": 7, "status": "draft",
        "preview_scheduled_at": "2099-04-01T11:00:00Z",
        "scheduled_open_at": "2099-04-01T12:00:00Z", "scheduled_ends_at": "2099-04-01T13:00:00Z",
        "preview_opened_at": null, "preview_manual_open": false, "preview_revision": 0,
        "opened_at": null, "final_phase_at": null, "settled_at": null, "cancelled_at": null,
        "cancelled_reason": null, "cancelled_by": null,
        "created_at": "2099-04-01T10:00:00Z", "updated_at": "2099-04-01T10:00:00Z"
    }))
    .unwrap()
}

struct AuctionCommandTest {
    page: TestPage,
    test: Rc<AuctionPageTest>,
    writes: WriteScript,
}

impl AuctionCommandTest {
    fn mount() -> Self {
        let test = Rc::new(AuctionPageTest {
            reads: RecordRead::new(),
            drafts: Cell::new(None),
            displayed_version: Cell::new(None),
            owner: Cell::new(None),
        });
        let writes = WriteScript::default();
        let (mut page, _) = mount_writer({
            let test = Rc::clone(&test);
            let writes = writes.clone();
            move || {
                use_context_provider(|| Rc::clone(&test));
                use_hook(|| writes.install());
                rsx! { Router::<TestRoute> {} }
            }
        });
        settle_writer(&mut page);
        test.reads.respond(Ok(auction()));
        settle_writer(&mut page);
        Self { page, test, writes }
    }

    fn open(&mut self, entry: &str, field: &str, initial: &str) -> (ElementId, ElementId) {
        let opened = click_button(&mut self.page, entry);
        let field = format!("{field}-{AUCTION_ID}");
        let (input, _) =
            text_attributes(&opened, "id").find(|(_, id)| *id == field).expect("编辑控件");
        assert_field(&self.page, input, initial, false);
        (writer_form(&opened), input)
    }

    fn submit(&mut self, form: ElementId) -> Vec<Mutation> {
        self.page.dispatch("submit", form, "")
    }

    fn take_request(&self) -> CapturedWrite {
        let [request] = self.writes.take_writes().try_into().unwrap();
        request
    }

    fn assert_conflict(
        &self,
        updates: &[Mutation],
        input: ElementId,
        draft: &str,
        submit_label: &str,
    ) {
        assert!(has_text(updates, "本次修改没有生效"));
        assert!(!has_live_text(&self.page, submit_label));
        assert!(!self.test.owner.get().unwrap().contains(&format!("auction:{AUCTION_ID}")));
        assert_eq!(self.test.drafts.get().unwrap().auction_id().as_deref(), Some(AUCTION_ID));
        assert_field(&self.page, input, draft, true);
    }

    fn close_after_conflict_read(&mut self, close_label: &str, entry: &str, latest: AdminAuction) {
        assert_eq!(self.test.reads.started(), 2);
        let version = latest.state_version;
        self.test.reads.respond(Ok(latest));
        settle_writer(&mut self.page);
        assert_eq!(self.test.displayed_version.get(), Some(version));
        assert_eq!(self.test.reads.started(), 2);
        click_button(&mut self.page, close_label);
        assert!(self.test.drafts.get().unwrap().auction_id().is_none());
        assert_eq!(self.test.reads.started(), 2, "冲突后的读取已完成");
        assert_eq!(self.test.displayed_version.get(), Some(version));
        assert_matches!(button(&self.page, entry), Some((_, false)));
    }

    fn accept_new_command(&mut self, form: ElementId, response: AdminAuction) -> CapturedWrite {
        self.writes.reply_http(
            200,
            serde_json::json!({
                "code":"ok", "message":"操作成功", "request_id":null,
                "data": {"auction": response},
            })
            .to_string(),
        );
        self.submit(form);
        assert!(self.test.drafts.get().unwrap().auction_id().is_none());
        self.take_request()
    }
}

fn refresh_scenario(entry: &str, field: &str, initial: &str, draft: &str, submit_label: &str) {
    let mut test = AuctionCommandTest::mount();
    let (_, input) = test.open(entry, field, initial);
    test.page.dispatch("input", input, draft);
    assert_field(&test.page, input, draft, false);
    let loading = click_button(&mut test.page, "刷新记录");
    assert!(has_text(&loading, "正在刷新记录"));
    assert_eq!(test.test.displayed_version.get(), Some(7));
    test.test.reads.respond(Err("network_error".to_owned()));
    let failed = settle_writer(&mut test.page);
    assert!(has_text(&failed, "刷新记录失败"));
    assert_eq!(test.test.displayed_version.get(), Some(7));
    assert_field(&test.page, input, draft, false);

    click_button(&mut test.page, "重试");
    test.test.reads.respond(Ok(AdminAuction { state_version: 8, ..auction() }));
    let resolved = settle_writer(&mut test.page);
    assert_eq!(test.test.displayed_version.get(), Some(8));
    test.assert_conflict(&resolved, input, draft, submit_label);
}

#[test]
fn schedule_draft_survives_failed_refresh_and_successful_retry() {
    refresh_scenario(SCHEDULE, END, "2099-04-01T13:00", EDITED_END, SAVE_SCHEDULE);
}

#[test]
fn lifecycle_reason_survives_failed_refresh_and_successful_retry() {
    refresh_scenario(CANCEL, REASON, "", EDITED_REASON, CONFIRM_CANCEL);
}

fn assert_schedule_body(
    request: &CapturedWrite,
    version: i64,
    preview: &str,
    open: &str,
    end: &str,
) {
    let id = request.command_id();
    let expected = format!(
        r#"{{"command_id":"{id}","preview_scheduled_at":"2099-04-01T{preview}Z","scheduled_open_at":"2099-04-01T{open}Z","scheduled_end_at":"2099-04-01T{end}Z","expected_state_version":{version}}}"#
    );
    assert_eq!(request.method, Method::PATCH);
    assert_eq!(request.url, format!("/api/v1/admin/auctions/{AUCTION_ID}/schedule"));
    assert_eq!(request.body, expected.as_bytes());
}

#[test]
fn stale_schedule_closes_and_reopens_with_the_latest_schedule() {
    for close_label in [DIALOG_TITLE_CLOSE, CLOSE_CONFLICT] {
        schedule_close_scenario(close_label);
    }
}

fn schedule_close_scenario(close_label: &str) {
    let mut test = AuctionCommandTest::mount();
    let (form, input) = test.open(SCHEDULE, END, "2099-04-01T13:00");
    test.page.dispatch("input", input, EDITED_END);
    test.writes.reply_conflict("stale_auction_state");
    let updates = test.submit(form);
    let first = test.take_request();
    assert_schedule_body(&first, 7, "11:00:00", "12:00:00", "14:17:00");
    test.assert_conflict(&updates, input, EDITED_END, SAVE_SCHEDULE);
    test.submit(form);
    assert!(test.writes.take_writes().is_empty());

    let latest = AdminAuction {
        state_version: 8,
        preview_scheduled_at: "2099-04-01T11:30:08.123456Z".to_owned(),
        scheduled_open_at: "2099-04-01T12:30:01Z".to_owned(),
        scheduled_ends_at: Some("2099-04-01T13:30:00Z".to_owned()),
        ..auction()
    };
    test.close_after_conflict_read(close_label, SCHEDULE, latest.clone());
    let (new_form, new_input) = test.open(SCHEDULE, END, "2099-04-01T13:30");
    test.page.dispatch("input", new_input, "2099-04-01T15:00");
    let second = test.accept_new_command(
        new_form,
        AdminAuction {
            state_version: 9,
            scheduled_ends_at: Some("2099-04-01T15:00:00Z".to_owned()),
            ..latest
        },
    );
    assert_schedule_body(&second, 8, "11:30:08.123456", "12:30:01", "15:00:00");
    assert_ne!(first.command_id(), second.command_id());
}

#[test]
fn wrong_status_schedule_conflict_waits_for_a_verified_retry() {
    let mut test = AuctionCommandTest::mount();
    let (form, input) = test.open(SCHEDULE, END, "2099-04-01T13:00");
    test.page.dispatch("input", input, EDITED_END);
    test.writes.reply_http(
        422,
        r#"{"code":"stale_auction_state","message":"版本已变化","data":null,"request_id":null}"#,
    );
    test.submit(form);
    let first = test.take_request();
    assert_schedule_body(&first, 7, "11:00:00", "12:00:00", "14:17:00");
    let owner = test.test.owner.get().unwrap();
    assert!(owner.contains(&format!("auction:{AUCTION_ID}")));
    assert_eq!(test.test.reads.started(), 1);
    assert!(!has_live_text(&test.page, "这条记录已被其他操作更新，本次修改没有生效。"));
    let (retry, disabled) = command_button(&test.page, first.command_id(), "原样重试").unwrap();
    assert!(!disabled);

    test.writes.reply_conflict("stale_auction_state");
    let updates = test.page.dispatch("click", retry, "");
    assert_eq!(test.take_request(), first);
    test.assert_conflict(&updates, input, EDITED_END, SAVE_SCHEDULE);
    assert_eq!(test.test.reads.started(), 2);
}

fn assert_cancel_body(request: &CapturedWrite, version: i64, reason: &str) {
    let id = request.command_id();
    let expected = format!(
        r#"{{"command_id":"{id}","expected_state_version":{version},"reason":"{reason}"}}"#
    );
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.url, format!("/api/v1/admin/auctions/{AUCTION_ID}/cancel"));
    assert_eq!(request.body, expected.as_bytes());
}

#[test]
fn wrong_status_lifecycle_conflict_waits_for_a_verified_retry() {
    let mut test = AuctionCommandTest::mount();
    let (form, input) = test.open(CANCEL, REASON, "");
    test.page.dispatch("input", input, EDITED_REASON);
    test.writes.reply_http(
        422,
        r#"{"code":"stale_auction_state","message":"版本已变化","data":null,"request_id":null}"#,
    );
    test.submit(form);
    let first = test.take_request();
    assert_cancel_body(&first, 7, EDITED_REASON);
    let owner = test.test.owner.get().unwrap();
    assert!(owner.contains(&format!("auction:{AUCTION_ID}")));
    assert_eq!(test.test.reads.started(), 1);
    assert!(!has_live_text(&test.page, "这条记录已被其他操作更新，本次修改没有生效。"));
    let (retry, disabled) = command_button(&test.page, first.command_id(), "原样重试").unwrap();
    assert!(!disabled);

    test.writes.reply_conflict("stale_auction_state");
    let updates = test.page.dispatch("click", retry, "");
    assert_eq!(test.take_request(), first);
    test.assert_conflict(&updates, input, EDITED_REASON, CONFIRM_CANCEL);
    assert_eq!(test.test.reads.started(), 2);
}

#[test]
fn lifecycle_ignores_another_auctions_receipt_and_retries_the_original_command() {
    const OTHER_AUCTION: &str = "20f99cb5-7b75-4853-bc49-9c71df78381d";
    let mut test = AuctionCommandTest::mount();
    let (form, input) = test.open(CANCEL, REASON, "");
    test.page.dispatch("input", input, EDITED_REASON);
    let cancelled = AdminAuction {
        status: "cancelled".to_owned(),
        state_version: 8,
        scheduled_ends_at: None,
        cancelled_at: Some("2026-11-01T00:00:00Z".to_owned()),
        cancelled_by: Some("operator".to_owned()),
        cancelled_reason: Some(EDITED_REASON.to_owned()),
        ..auction()
    };
    for id in [OTHER_AUCTION, AUCTION_ID] {
        let response = AdminAuction { id: id.to_owned(), ..cancelled.clone() };
        test.writes.reply_http(200, serde_json::json!({
            "code": "ok", "message": "操作成功", "request_id": null, "data": {"auction": response},
        }).to_string());
    }
    test.submit(form);
    let first = test.take_request();
    assert_cancel_body(&first, 7, EDITED_REASON);
    let owner = test.test.owner.get().unwrap();
    let entity = format!("auction:{AUCTION_ID}");
    assert!(owner.contains(&entity));
    assert_eq!(test.test.reads.started(), 1);
    assert_eq!(test.test.drafts.get().unwrap().auction_id().as_deref(), Some(AUCTION_ID));
    let (retry, disabled) = command_button(&test.page, first.command_id(), "原样重试").unwrap();
    assert!(!disabled);
    test.page.dispatch("click", retry, "");
    assert_eq!(test.take_request(), first);
    assert!(!owner.contains(&entity));
    assert!(test.test.drafts.get().unwrap().auction_id().is_none());
    assert_eq!(test.test.reads.started(), 2);
}

#[test]
fn stale_lifecycle_confirmation_closes_and_reopens_with_a_blank_reason() {
    for close_label in [DIALOG_TITLE_CLOSE, CLOSE_CONFLICT] {
        lifecycle_close_scenario(close_label);
    }
}

fn lifecycle_close_scenario(close_label: &str) {
    let mut test = AuctionCommandTest::mount();
    let (form, input) = test.open(CANCEL, REASON, "");
    test.page.dispatch("input", input, EDITED_REASON);
    test.writes.reply_conflict("stale_auction_state");
    let updates = test.submit(form);
    let first = test.take_request();
    assert_cancel_body(&first, 7, EDITED_REASON);
    test.assert_conflict(&updates, input, EDITED_REASON, CONFIRM_CANCEL);

    let latest = AdminAuction { state_version: 8, ..auction() };
    test.close_after_conflict_read(close_label, CANCEL, latest.clone());
    let (new_form, new_input) = test.open(CANCEL, REASON, "");
    let reason = "重新确认后的原因";
    test.page.dispatch("input", new_input, reason);
    let second = test.accept_new_command(
        new_form,
        AdminAuction {
            status: "cancelled".to_owned(),
            state_version: 9,
            scheduled_ends_at: None,
            cancelled_at: Some("2026-11-01T00:00:00Z".to_owned()),
            cancelled_by: Some("operator".to_owned()),
            cancelled_reason: Some(reason.to_owned()),
            ..latest
        },
    );
    assert_cancel_body(&second, 8, reason);
    assert_ne!(first.command_id(), second.command_id());
}
