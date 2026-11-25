//! 纯文本通知发信在回环邮件替身上的回归：发后 Sent 证明成立才算发出；明确拒绝为未发出；写入后无法证明
//! （成功页但 Sent 无新邮件、结果页不明、POST 超时）一律为未知；每种情形都只有一次写请求；内容不进
//! `Debug`。发出正例用已采集的零剩余资产 Sent 邮件（3125305），替身在写 POST 成功后才让它出现。

use std::sync::Arc;
use std::time::Duration;

use auction_application::core::{ApplicationError, ApplicationFuture};
use auction_application::mail::{MailLoginKey, MailLoginKeyReader};
use auction_test_support::mooglemail::{Fault, MailDouble};
use uuid::Uuid;

use super::{PlainNoticeMail, PlainNoticeOutcome, plain_notice_outcome};
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCredentialSet, MoogleMailDeliveryTransport,
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailSendOutcome,
    MoogleMailSendReceipt, MoogleMailWorld,
};

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";
const FAKE_CODE: &str = "FAKE-CODE-0000";

/// 纯文本通知不读登录 key；被调用即失败关闭。
struct NoLoginKeys;

impl MailLoginKeyReader for NoLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

fn transport(server: &MailDouble, request_timeout: Duration) -> MoogleMailDeliveryTransport {
    let client = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout,
    })
    .expect("loopback client");
    // 已采集的末页按真实末页结构回放，写前完整 Sent 基线可在已采集页内取得。
    server.set_captured_pages_complete(true);
    MoogleMailDeliveryTransport::new(
        client,
        MoogleMailCredentialSet::new(COOKIE).expect("fixture cookie"),
        Arc::new(NoLoginKeys),
    )
}

fn mail() -> PlainNoticeMail {
    PlainNoticeMail {
        world: MoogleMailWorld::Main,
        recipient: "FixtureRecipient".to_owned(),
        subject: "Fixture notice".to_owned(),
        body: format!("Fixture code {FAKE_CODE}."),
    }
}

/// sent-main 第 0 页中比 3125305 更新的行（按列表顺序）。
const NEWER_THAN_CAPTURED: [&str; 10] = [
    "3125366", "3125364", "3125356", "3125349", "3125345", "3125344", "3125337", "3125323",
    "3125309", "3125308",
];

/// 已采集 Sent 详情 3125305 的收件人、主题与正文（逐字，含页面追加的领取行）；Credits 已被领取，
/// 剩余资产为零，等同一封零资产邮件。
fn captured_zero_asset_mail() -> PlainNoticeMail {
    PlainNoticeMail {
        world: MoogleMailWorld::Main,
        recipient: "Uqqkeyxklyws".to_owned(),
        subject: "Auction seller payout [b11a4e25-ad30-544e-b717-52ccd3e3f4cc]".to_owned(),
        body: "Your auction payout is attached.\n\nAmount: 49,000 Credits\n\nPlease collect the \
               attached Credits.\n\nReference: b11a4e25-ad30-544e-b717-52ccd3e3f4cc\n\nRemoved \
               attachment: 49,000 Credits"
            .to_owned(),
    }
}

#[tokio::test]
async fn proven_send_is_sent_after_exactly_one_write_and_full_proof() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::WriteSucceeded));
    // 真实新邮件总在 Sent 顶端：隐藏比它新的已采集行，使它在写 POST 之后成为最新一封。
    for newer in NEWER_THAN_CAPTURED {
        server.remove_sent(newer);
    }
    server.reveal_sent_on_write("3125305");

    let outcome = transport.send_plain_notice(captured_zero_asset_mail()).await;

    assert_eq!(outcome, PlainNoticeOutcome::Sent);
    assert_eq!(server.write_requests(), 1, "只写一次");
    let requests = server.requests();
    let post = requests.iter().position(|request| request.starts_with("POST ")).expect("写 POST");
    let after = &requests[post + 1..];
    // 发后证明：读回新邮件详情（verify_new_sent），再证明 compose 回到空表单。
    let detail = after
        .iter()
        .position(|request| request.contains("filter=sent") && request.contains("mid=3125305"))
        .expect("发后读回新 Sent 详情");
    assert!(
        after[detail + 1..]
            .iter()
            .any(|request| request.starts_with("GET ") && request.contains("filter=new")),
        "证明之后复查 compose"
    );
}

#[tokio::test]
async fn success_page_without_new_sent_mail_is_unknown() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    // 远端给出成功页，但 Sent 里没有新邮件：发后证明不成立。
    server.set_fault(Some(Fault::WriteSucceeded));

    let outcome = transport.send_plain_notice(captured_zero_asset_mail()).await;

    assert_eq!(outcome, PlainNoticeOutcome::Unknown);
    assert_eq!(server.write_requests(), 1, "证明失败不重发");
}

#[tokio::test]
async fn explicit_rejection_is_not_sent_after_exactly_one_write() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::InsufficientPostage));

    let outcome = transport.send_plain_notice(mail()).await;

    assert_eq!(outcome, PlainNoticeOutcome::NotSent { retryable: false });
    assert_eq!(server.write_requests(), 1, "明确拒绝不重试");
}

#[tokio::test]
async fn unprovable_result_page_is_unknown_without_a_second_write() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::WriteResultUnknown));

    let outcome = transport.send_plain_notice(mail()).await;

    assert_eq!(outcome, PlainNoticeOutcome::Unknown);
    assert_eq!(server.write_requests(), 1, "结果未知绝不重发");
}

#[tokio::test]
async fn post_timeout_is_unknown_without_a_second_write() {
    let server = MailDouble::start().await.expect("loopback server");
    // 只有写 POST 迟到；读取照常，客户端在 POST 上先于远端完成而超时。
    let transport = transport(&server, Duration::from_millis(500));
    server.set_fault(Some(Fault::SlowWriteSucceeded(Duration::from_secs(2))));

    let outcome = transport.send_plain_notice(mail()).await;

    assert_eq!(outcome, PlainNoticeOutcome::Unknown);
    assert_eq!(server.write_requests(), 1, "超时后不重发");
}

#[test]
fn only_a_proven_send_counts_as_sent() {
    let world = MoogleMailWorld::Main;
    let proven = Ok(MoogleMailSendOutcome::Sent(MoogleMailSendReceipt {
        external_mail_id: "4242".to_owned(),
    }));
    assert_eq!(plain_notice_outcome(world, proven), PlainNoticeOutcome::Sent);

    // 写入已开始后的任何失败都只能是未知，即使分类本身看似可重试。
    let after_write = MoogleMailError::after_write(
        MoogleMailOperation::Send,
        world,
        MoogleMailErrorCategory::RemoteTemporary,
    );
    assert_eq!(plain_notice_outcome(world, Err(after_write)), PlainNoticeOutcome::Unknown);
}

#[test]
fn debug_output_omits_recipient_subject_and_code() {
    let rendered = format!("{:?}", mail());
    assert!(!rendered.contains("FixtureRecipient"));
    assert!(!rendered.contains("Fixture notice"));
    assert!(!rendered.contains(FAKE_CODE));
}
