//! 结算退件 Take 结果未知后的巡检贯通用例（PostgreSQL 18，运行账号）：详情页 HTML → integrations
//! 真实解析器的领取证明 → 应用层邮箱维护 → 存储入账。远端读取换成回放同一份 HTML 的本地替身，
//! 页面取自已脱敏的真实已领取详情（taken-3125358），只替换收件人、发件人、主题与正文；收件人取
//! 合成凭据的预期玩家名（Bot 收件箱里的退件寄给 Bot 自己）。替身只回放页面，不代表真实远端行为。
//! - 正例：收件人、原身份、原退件正文前缀与 `Removed attachment:` 回执都成立 → 领回事实与资金返还
//!   各一次；
//! - 反例：附件已为空但没有领取回执，或只有收件人与执行 Take 的凭据账号不符 → 不绑定任何证明，
//!   只追加 `take_observed` 观察事件；案件保持 `take_unknown`，资金不动。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::core::{ApplicationFuture, Clock};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_application::mail::{
    MailboxMaintenanceService, MoogleMailboxComposeState, MoogleMailboxDiscard,
    MoogleMailboxEvidenceStore, MoogleMailboxInboxObservation, MoogleMailboxMaintenance,
    MoogleMailboxNoticeExpectation, MoogleMailboxReturnTakeIdentity,
    MoogleMailboxReturnTakeObservation, MoogleMailboxSentEntries, MoogleMailboxSentEntry,
    MoogleMailboxSentState, MoogleMailboxUnprovenSend,
};
use auction_application::operations::ExternalWriteActivity;
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::returned_mail_body_digest;
use auction_domain::SettlementReturnAction;
use auction_integrations::mooglemail::{MoogleMailParser, MoogleMailWorld};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::Value;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::{PAYOUT, Returned};
use super::sweep::{Facts, facts, seed_mail_session, unknown_take_of};
use super::{insert_inbound, insert_sent_fulfillment};
use crate::recovery_returns::{insert_sale, settlement_minute};
use crate::support;

/// 真实已领取详情页（Read Mail，附件已空，正文末尾一行领取回执）。
const TAKEN: &str =
    include_str!("../../../../auction-integrations/tests/fixtures/mooglemail/taken-3125358.html");

/// 被退回的货款原正文（Bot 冻结的结算正文）；证据只保存它的规范摘要。
const ORIGINAL: &str = "Your auction payout is attached.\n\nAmount: 90 Credits";

/// 系统后缀写明的原收件人，与原派发冻结收件人一致。
const RETURNED_FROM: &str = "Recovery seller";

/// 合成邮件凭据的预期玩家名（见 `sweep::seed_mail_session`），即退件详情页上的收件人。
const BOT: &str = "Returns Bot";

/// 把真实页面的收件人、发件人、主题与正文换成这封退件的值，其余结构原样保留。
fn returned_page_to(recipient: &str, subject: &str, body: &str) -> String {
    let start = TAKEN.find("<textarea").expect("正文开始");
    let open_end = start + TAKEN[start..].find('>').expect("正文标签结束") + 1;
    let close = TAKEN.find("</textarea>").expect("正文结束");
    let page = format!("{}{body}{}", &TAKEN[..open_end], &TAKEN[close..])
        .replacen("value=\"Ujlhovl\"", &format!("value=\"{recipient}\""), 1)
        .replacen("value=\"Pnfcgarl\"", "value=\"MoogleMail\"", 1)
        .replacen("value=\"auction\"", &format!("value=\"{subject}\""), 1);
    assert!(page.contains(&format!("value=\"{recipient}\"")), "页面里必须有收件人字段");
    page
}

/// 寄给 Bot 账号的退件详情页。
fn returned_page(subject: &str, body: &str) -> String {
    returned_page_to(BOT, subject, body)
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// 只回放一份详情页的远端替身：Sent 列表完整且为空、compose 干净；结算退件观察交给真实解析器。
struct ReplayedMailbox {
    html: String,
    session: DeliverySendCredentialBinding,
}

impl MoogleMailboxMaintenance for ReplayedMailbox {
    fn compose<'a>(
        &'a self,
        _world_id: &'a str,
    ) -> ApplicationFuture<'a, MoogleMailboxComposeState> {
        Box::pin(async {
            Ok(MoogleMailboxComposeState {
                clean: true,
                recipient: String::new(),
                subject: String::new(),
                body: String::new(),
                credits: 0,
                cod: None,
                attachment_count: 0,
                equipment_ids: Vec::new(),
                body_digest: [0; 32],
                observation_credential: None,
            })
        })
    }

    fn discard_compose<'a>(
        &'a self,
        _world_id: &'a str,
        _expected: &'a MoogleMailboxComposeState,
    ) -> ApplicationFuture<'a, MoogleMailboxDiscard> {
        Box::pin(async { unreachable!("compose 干净时不丢弃") })
    }

    fn sent_entries<'a>(
        &'a self,
        _world_id: &'a str,
    ) -> ApplicationFuture<'a, MoogleMailboxSentEntries> {
        Box::pin(async {
            Ok(MoogleMailboxSentEntries {
                entries: Vec::new(),
                covered_from: None,
                complete: true,
                page_observation: None,
            })
        })
    }

    fn read_mail_observed<'a>(&'a self, _world_id: &'a str) -> ApplicationFuture<'a, usize> {
        Box::pin(async { Ok(0) })
    }

    fn sent_state<'a>(
        &'a self,
        _world_id: &'a str,
        _entry: &'a MoogleMailboxSentEntry,
        _observed_at: Timestamp,
    ) -> ApplicationFuture<'a, MoogleMailboxSentState> {
        Box::pin(async { unreachable!("Sent 列表为空") })
    }

    fn notice_expectation<'a>(
        &'a self,
        _candidate: &'a MoogleMailboxUnprovenSend,
    ) -> ApplicationFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
        Box::pin(async { Ok(None) })
    }

    fn inbox_assets<'a>(
        &'a self,
        _world_id: &'a str,
        _external_mail_id: &'a str,
    ) -> ApplicationFuture<'a, MoogleMailboxInboxObservation> {
        Box::pin(async { unreachable!("结算退件 Take 不经资产观察收口") })
    }

    fn return_take_receipt<'a>(
        &'a self,
        _world_id: &'a str,
        _external_mail_id: &'a str,
        identity: &'a MoogleMailboxReturnTakeIdentity,
    ) -> ApplicationFuture<'a, MoogleMailboxReturnTakeObservation> {
        let outcome = MoogleMailParser
            .returned_take_observation(self.html.as_bytes(), MoogleMailWorld::Main, identity)
            .expect("真实页面结构可解析");
        let observation_credential = Some(self.session.clone());
        Box::pin(async move {
            Ok(MoogleMailboxReturnTakeObservation { outcome, observation_credential })
        })
    }
}

/// 证据带系统后缀事实（原收件人与原正文摘要）的货款退件；返回案件与退件主题。
async fn returned_with_body(store: &DatabaseStore, sequence: i64, mid: &str) -> (Returned, String) {
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let delivery = insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        PAYOUT,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let digest = returned_mail_body_digest(ORIGINAL)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let case = insert_inbound(
        pool,
        mid,
        minute + Duration::minutes(40),
        Some(("MoogleMail", subject.clone())),
        json!({
            "obligation_id": sale.seller_obligation.to_string(),
            "credits": PAYOUT,
            "cod": 0,
            "attachment_ids": [],
            "returned_from": RETURNED_FROM,
            "body_digest": digest,
        }),
    )
    .await;
    store.consume_due_settlement_return(Timestamp::now()).await.expect("消费").expect("必须消费");
    (Returned { sale, delivery, case }, subject)
}

/// 以回放的详情页跑一轮真实邮箱维护，返回本轮恢复数。
async fn sweep_with(
    store: &DatabaseStore,
    html: String,
    session: DeliverySendCredentialBinding,
) -> usize {
    let service = MailboxMaintenanceService::new(
        Arc::new(ReplayedMailbox { html, session }),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
    );
    let report = service
        .sweep(
            "main",
            Instant::now() + StdDuration::from_secs(30),
            &ExternalWriteActivity::default(),
        )
        .await
        .expect("维护一轮完成");
    report.sent_recovered
}

/// 正例：详情页的原身份、原退件正文前缀与领取回执都成立，经应用层巡检与存储收口入账一次。
#[tokio::test]
async fn postgres_return_take_receipt_html_settles_through_sweep() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (returned, subject) = returned_with_body(&store, 7_650_001, "997001").await;
    let take = unknown_take_of(
        &store,
        &session,
        returned,
        "997001",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let candidate = MoogleMailboxEvidenceStore::unproven_sends(&store, "main", 200)
        .await
        .expect("读取巡检候选")
        .candidates
        .into_iter()
        .find(|slot| slot.delivery_id == take.take_delivery)
        .and_then(|slot| slot.send)
        .expect("结算退件 Take 是可恢复候选");
    assert_eq!(
        candidate.return_take,
        Some(MoogleMailboxReturnTakeIdentity {
            sender: "MoogleMail".to_owned(),
            subject: subject.clone(),
            original_body_digest: returned_mail_body_digest(ORIGINAL),
            returned_from: RETURNED_FROM.to_owned(),
            cod: 0,
            attachment_count: 1,
            expected_recipient: Some(BOT.to_owned()),
        }),
        "候选携带证据、入站快照与 Take 冻结凭据重建的原退件身份"
    );

    let body = format!(
        "{ORIGINAL}\n\nThis message was returned from {RETURNED_FROM}, kupo!\n\nRemoved attachment: 90 Credits"
    );
    assert_eq!(sweep_with(&store, returned_page(&subject, &body), session.binding()).await, 1);
    let after = facts(pool, &take).await;
    assert_eq!(
        (after.case_status.as_str(), after.action_state.as_str(), after.delivery_status.as_str()),
        ("held", "satisfied", "succeeded"),
        "按计划代管"
    );
    assert_eq!(
        (after.takes, after.funding_returns, after.return_entries),
        (1, 1, 1),
        "领回事实与资金返还各一次"
    );
    assert_eq!(
        (after.allocation_state.as_str(), after.allocation_amount),
        ("available", PAYOUT),
        "金额是 M 不是 2M"
    );
    // 同一页面再巡检一轮：投递已收口，不再是候选，不重复入账。
    let body = format!(
        "{ORIGINAL}\n\nThis message was returned from {RETURNED_FROM}, kupo!\n\nRemoved attachment: 90 Credits"
    );
    assert_eq!(sweep_with(&store, returned_page(&subject, &body), session.binding()).await, 0);
    assert_eq!(facts(pool, &take).await, after);
    pool.close().await;
}

/// 读取案件最新事件。
async fn latest_event(pool: &PgPool, case: Uuid) -> (String, Value) {
    sqlx::query_as(
        "SELECT event_kind, details FROM public.settlement_return_events
         WHERE return_mail_id = $1 ORDER BY case_version DESC LIMIT 1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取最新事件")
}

/// 除案件版本外，观察事件不改写状态、动作、投递或资金。
fn unchanged_except_case_version(before: &Facts, after: &Facts) -> bool {
    after.case_status == before.case_status
        && after.action_state == before.action_state
        && after.delivery_status == before.delivery_status
        && after.takes == before.takes
        && after.funding_returns == before.funding_returns
        && after.allocation_state == before.allocation_state
        && after.allocation_amount == before.allocation_amount
        && after.return_entries == before.return_entries
}

/// 反例：附件已为空但正文没有领取回执——同一贯通路径不绑定任何证明，只追加观察事件；案件状态
/// 与资金都不变。
#[tokio::test]
async fn postgres_return_take_empty_page_without_receipt_stays_unknown() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (returned, subject) = returned_with_body(&store, 7_650_011, "997011").await;
    let take = unknown_take_of(
        &store,
        &session,
        returned,
        "997011",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let before = facts(pool, &take).await;
    let body = format!("{ORIGINAL}\n\nThis message was returned from {RETURNED_FROM}, kupo!");
    assert_eq!(sweep_with(&store, returned_page(&subject, &body), session.binding()).await, 0);
    let after = facts(pool, &take).await;
    assert_eq!(after.case_version, before.case_version + 1, "只追加一次观察事件");
    assert!(
        unchanged_except_case_version(&before, &after),
        "没有领取回执不绑定领回、不动投递或资金"
    );
    let (kind, details) = latest_event(pool, take.returned.case).await;
    assert_eq!(kind, "take_observed");
    assert_eq!(details["observation"], json!("receipt_unproven"));
    let takes: i64 = sqlx::query_scalar("SELECT count(*) FROM public.settlement_return_takes")
        .fetch_one(pool)
        .await
        .expect("计数");
    assert_eq!(takes, 0);
    pool.close().await;
}

/// 反例：只有收件人改变——发件人、主题、原退件正文前缀与领取回执全都成立，但收件人不是执行 Take
/// 的凭据所属账号。同一贯通路径不绑定任何证明，案件与资金都不变。
#[tokio::test]
async fn postgres_return_take_receipt_for_another_recipient_stays_unknown() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (returned, subject) = returned_with_body(&store, 7_650_021, "997021").await;
    let take = unknown_take_of(
        &store,
        &session,
        returned,
        "997021",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    let before = facts(pool, &take).await;
    let body = format!(
        "{ORIGINAL}\n\nThis message was returned from {RETURNED_FROM}, kupo!\n\nRemoved attachment: 90 Credits"
    );
    let page = returned_page_to("Other Bot", &subject, &body);
    assert_eq!(sweep_with(&store, page, session.binding()).await, 0);
    let after = facts(pool, &take).await;
    assert_eq!(after.case_version, before.case_version + 1, "只追加一次观察事件");
    assert!(
        unchanged_except_case_version(&before, &after),
        "收件人不是 Take 账号时不绑定领回、不动投递或资金"
    );
    let (kind, details) = latest_event(pool, take.returned.case).await;
    assert_eq!(kind, "take_observed");
    assert_eq!(details["observation"], json!("receipt_unproven"));
    let takes: i64 = sqlx::query_scalar("SELECT count(*) FROM public.settlement_return_takes")
        .fetch_one(pool)
        .await
        .expect("计数");
    assert_eq!(takes, 0);
    pool.close().await;
}
