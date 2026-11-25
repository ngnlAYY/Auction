//! 寄售派发发信入口在回环邮件替身上的回归：准备只读不写；发后 Sent 证明成立才算发出并给出 Sent MID；
//! 写入后无法证明（成功页但 Sent 无新邮件、POST 超时）一律为未知且只写一次；许可绑定别的投递时零写入。
//! 发出正例用已采集的零剩余资产 Sent 邮件（3125305），替身在写 POST 成功后才让它出现。复核只读：
//! 冻结身份逐字相符才找到，正文摘要不符则找不到，两者都零写入。
//!
//! 现金轮：Sent 里剩余 Credits 等于冻结值才算发出，差一个 Credit 即未知且不重发（已采集的未领取 49,000
//! Credits Sent 邮件 3125364，替身回放附 Credits 的 compose 暂存）；没有同会话资金证据入口、或缺冻结资金 UID
//! 时准备失败关闭、零写入；复核只认剩余 Credits 等于冻结值的那封（负例确认走到逐项比对）；仅 Credits 不同时
//! 请求摘要与证据摘要都不同。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    AttemptReceipt, AuthorizeDispatch, ConsignmentFuture, DispatchService, DispatchStore,
    EvidenceReceipt, MarkWriteStarted, PostPermit, RecordEvidence, RoundReceipt, StartAttempt,
    StartReceipt, WriteStart, WriteStartOutcome,
};
use auction_application::core::{ApplicationError, ApplicationFuture};
use auction_application::mail::{MailLoginKey, MailLoginKeyReader};
use auction_domain::consignment::dispatch::AttemptKey;
use auction_test_support::mooglemail::{Fault, MailDouble};
use auction_time::Timestamp;
use uuid::Uuid;

use super::{
    CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION, ConsignmentDispatchMail,
    ConsignmentDispatchOutcome, ConsignmentDispatchRecheck, ConsignmentDispatchRecheckTarget,
};
use crate::clients::mooglemail::parser::canonical_mail_body_digest;
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCredentialSet, MoogleMailDeliveryTransport,
    MoogleMailMessageFields, MoogleMailSendRequest, MoogleMailWorld,
};

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";
const DELIVERY: Uuid = Uuid::from_u128(0xd1);
const OTHER_DELIVERY: Uuid = Uuid::from_u128(0xd2);

/// 派发不读登录 key；被调用即失败关闭。
struct NoLoginKeys;

impl MailLoginKeyReader for NoLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

/// 只为铸造许可：起写总是首次提交，回执指回请求的投递与 fence。
struct FirstStart;

impl DispatchStore for FirstStart {
    fn authorize_dispatch(&self, _: AuthorizeDispatch) -> ConsignmentFuture<'_, RoundReceipt> {
        panic!("许可夹具不建轮")
    }

    fn start_attempt(&self, _: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        panic!("许可夹具不登记尝试")
    }

    fn mark_write_started(
        &self,
        command: MarkWriteStarted,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        let receipt = StartReceipt::started(
            command.command_id,
            Uuid::nil(),
            command.attempt.delivery_id,
            Uuid::nil(),
            command.attempt.fence,
        );
        Box::pin(async move { Ok(WriteStartOutcome::FirstCommitted(receipt)) })
    }

    fn record_evidence(&self, _: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        panic!("许可夹具不记证据")
    }
}

async fn permit(delivery_id: Uuid, fence: u64) -> PostPermit {
    let service = DispatchService::new(Arc::new(FirstStart));
    let attempt = AttemptKey { delivery_id, fence };
    match service.mark_write_started(attempt, Uuid::new_v4(), None).await.expect("夹具起写") {
        WriteStart::FirstCommitted { permit, .. } => permit,
        WriteStart::AlreadyCommitted(_) => panic!("夹具必须给出首次许可"),
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
    server.set_captured_pages_complete(true);
    MoogleMailDeliveryTransport::new(
        client,
        MoogleMailCredentialSet::new(COOKIE).expect("fixture cookie"),
        Arc::new(NoLoginKeys),
    )
}

/// sent-main 第 0 页中比 3125305 更新的行（按列表顺序）。
const NEWER_THAN_CAPTURED: [&str; 10] = [
    "3125366", "3125364", "3125356", "3125349", "3125345", "3125344", "3125337", "3125323",
    "3125309", "3125308",
];

/// 已采集 Sent 详情 3125305 的收件人、主题与正文（逐字）；剩余资产为零，COD 与附件为空。
fn captured_mail() -> ConsignmentDispatchMail {
    ConsignmentDispatchMail {
        world: MoogleMailWorld::Main,
        delivery_id: DELIVERY,
        fence: 1,
        recipient: "Uqqkeyxklyws".to_owned(),
        subject: "Auction seller payout [b11a4e25-ad30-544e-b717-52ccd3e3f4cc]".to_owned(),
        body: "Your auction payout is attached.\n\nAmount: 49,000 Credits\n\nPlease collect the \
               attached Credits.\n\nReference: b11a4e25-ad30-544e-b717-52ccd3e3f4cc\n\nRemoved \
               attachment: 49,000 Credits"
            .to_owned(),
        credits: 0,
        cod: 0,
        equipment_ids: Vec::new(),
        claim_token: Uuid::nil(),
        identity_digest: [0; 32],
        funding_uid: None,
        observe_main_cash: false,
    }
}

/// 已采集 Sent 详情 3125364（未领取的 49,000 Credits、COD 0）的收件人、主题与正文（逐字）；现金轮形状
/// （卖款 S、退款 P 只附 Credits），冻结 Credits 由调用方给出。
fn cash_mail(credits: i64) -> ConsignmentDispatchMail {
    ConsignmentDispatchMail {
        recipient: "Rqk dncjhgp".to_owned(),
        subject: "Auction seller payout [7b999a37-fbb3-50b1-997a-be6650b10a8f]".to_owned(),
        body: "Your auction payout is attached.\n\nAmount: 49,000 Credits\n\nPlease collect the \
               attached Credits.\n\nReference: 7b999a37-fbb3-50b1-997a-be6650b10a8f"
            .to_owned(),
        credits,
        claim_token: Uuid::from_u128(0xa1),
        identity_digest: [0x1d; 32],
        funding_uid: Some("7001".to_owned()),
        observe_main_cash: true,
        ..captured_mail()
    }
}

/// 回环替身没有获准的余额页语料，同会话资金证据无法在替身上取得；为核对附 Credits 的发信与发后 Sent 差异，
/// 以零 Credits 同一内容走完写前准备（compose 预检与 Sent 基线），再把请求的 Credits 设为冻结值。生产路径
/// 的现金轮只经 `prepare_consignment_dispatch` 取证据，见 `cash_round_without_same_session_evidence_writes_nothing`。
async fn prepared_cash(
    transport: &MoogleMailDeliveryTransport,
    credits: i64,
) -> super::PreparedConsignmentDispatch {
    let mut prepared = transport
        .prepare_consignment_dispatch(ConsignmentDispatchMail { credits: 0, ..cash_mail(credits) })
        .await
        .expect("准备");
    prepared.request.credits = credits;
    prepared
}

/// 让 3125364 成为写后才出现的新 Sent 邮件（列表里比它新的只有 3125366），并回放附 Credits 的 compose 暂存。
fn reveal_cash_sent_on_write(server: &MailDouble) {
    server.set_fault(Some(Fault::WriteSucceeded));
    server.stage_credits_on_attach();
    server.remove_sent("3125366");
    server.reveal_sent_on_write("3125364");
}

#[tokio::test]
async fn cash_sent_counts_only_when_the_remaining_credits_equal_the_frozen_amount() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    reveal_cash_sent_on_write(&server);

    let prepared = prepared_cash(&transport, 49_000).await;
    let outcome = transport.execute_consignment_dispatch(prepared, permit(DELIVERY, 1).await).await;

    let ConsignmentDispatchOutcome::Proven(sent) = outcome else {
        panic!("剩余 Credits 等于冻结值应为已发出：{outcome:?}");
    };
    assert_eq!(sent.sent_mid, "3125364");
    assert_eq!(server.write_requests(), 2, "一次附 Credits 暂存、一次发送");
}

#[tokio::test]
async fn cash_sent_with_different_credits_is_unknown_without_a_second_write() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    reveal_cash_sent_on_write(&server);

    // 冻结并附上 48,999，新 Sent 邮件剩余 49,000：差额不能当作证明。
    let prepared = prepared_cash(&transport, 48_999).await;
    let outcome = transport.execute_consignment_dispatch(prepared, permit(DELIVERY, 1).await).await;

    assert_eq!(outcome, ConsignmentDispatchOutcome::Unknown);
    assert_eq!(server.write_requests(), 2, "一次附 Credits 暂存、一次发送，不重发");
}

#[tokio::test]
async fn cash_round_without_same_session_evidence_writes_nothing() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::WriteSucceeded));

    // 本 transport 没有注入共享凭据的资金证据入口：现金轮在写前失败关闭。
    let prepared = transport.prepare_consignment_dispatch(cash_mail(49_000)).await;
    assert!(prepared.is_none(), "没有同会话资金证据不得准备现金轮");
    // 现金轮缺冻结资金 UID 时内容无效，不排队取会话。
    let reads = server.read_requests();
    let unbound = ConsignmentDispatchMail { funding_uid: None, ..cash_mail(49_000) };
    assert!(transport.prepare_consignment_dispatch(unbound).await.is_none());
    assert_eq!(server.read_requests(), reads, "内容无效不触达邮件服务");
    assert_eq!(server.write_requests(), 0, "零 POST");
}

#[tokio::test]
async fn sent_counts_only_after_the_permit_one_write_and_the_sent_proof() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::WriteSucceeded));
    for newer in NEWER_THAN_CAPTURED {
        server.remove_sent(newer);
    }
    server.reveal_sent_on_write("3125305");

    let prepared = transport.prepare_consignment_dispatch(captured_mail()).await.expect("准备");
    assert_eq!(server.write_requests(), 0, "准备只读不写");
    // 配置 Cookie 没有持久凭据身份：调用方据此不得起写。
    assert_eq!(prepared.credential(), None);

    let outcome = transport.execute_consignment_dispatch(prepared, permit(DELIVERY, 1).await).await;

    let ConsignmentDispatchOutcome::Proven(sent) = outcome else {
        panic!("Sent 证明成立应为已发出：{outcome:?}");
    };
    assert_eq!(sent.sent_mid, "3125305");
    assert_eq!(sent.validator_version, CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION);
    assert_ne!(sent.evidence_digest, [0; 32]);
    assert_eq!(server.write_requests(), 1, "只写一次");
}

#[tokio::test]
async fn success_page_without_new_sent_mail_is_unknown_without_a_second_write() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    // 远端给出成功页，但 Sent 里没有新邮件：发后证明不成立。
    server.set_fault(Some(Fault::WriteSucceeded));

    let prepared = transport.prepare_consignment_dispatch(captured_mail()).await.expect("准备");
    let outcome = transport.execute_consignment_dispatch(prepared, permit(DELIVERY, 1).await).await;

    assert_eq!(outcome, ConsignmentDispatchOutcome::Unknown);
    assert_eq!(server.write_requests(), 1, "证明失败不重发");
}

#[tokio::test]
async fn post_timeout_is_unknown_without_a_second_write() {
    let server = MailDouble::start().await.expect("loopback server");
    // 只有写 POST 迟到；客户端在 POST 上先于远端完成而超时。
    let transport = transport(&server, Duration::from_millis(500));
    server.set_fault(Some(Fault::SlowWriteSucceeded(Duration::from_secs(2))));

    let prepared = transport.prepare_consignment_dispatch(captured_mail()).await.expect("准备");
    let outcome = transport.execute_consignment_dispatch(prepared, permit(DELIVERY, 1).await).await;

    assert_eq!(outcome, ConsignmentDispatchOutcome::Unknown);
    assert_eq!(server.write_requests(), 1, "超时后不重发");
}

#[tokio::test]
async fn permit_for_another_delivery_writes_nothing() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    server.set_fault(Some(Fault::WriteSucceeded));

    let prepared = transport.prepare_consignment_dispatch(captured_mail()).await.expect("准备");
    let outcome =
        transport.execute_consignment_dispatch(prepared, permit(OTHER_DELIVERY, 1).await).await;

    assert_eq!(outcome, ConsignmentDispatchOutcome::Unknown);
    assert_eq!(server.write_requests(), 0, "许可不符零写入");
}

#[test]
fn debug_output_omits_recipient_subject_and_body() {
    let rendered = format!("{:?}", captured_mail());
    assert!(!rendered.contains("Uqqkeyxklyws"));
    assert!(!rendered.contains("payout"));
}

fn recheck_target(body_digest: [u8; 32]) -> ConsignmentDispatchRecheckTarget {
    let mail = captured_mail();
    ConsignmentDispatchRecheckTarget {
        world: mail.world,
        sending_at: Timestamp::parse_from_rfc3339("2000-01-01T00:00:00Z").expect("时刻"),
        recipient: mail.recipient,
        subject: mail.subject,
        body_digest,
        credits: mail.credits,
        cod: mail.cod,
        equipment_ids: mail.equipment_ids,
    }
}

#[tokio::test]
async fn recheck_finds_the_frozen_sent_mail_without_writing() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    let target = recheck_target(canonical_mail_body_digest(&captured_mail().body));

    let outcome = transport.recheck_consignment_dispatch(&target).await;

    let ConsignmentDispatchRecheck::Found(sent) = outcome else {
        panic!("冻结身份逐字相符应找到：{outcome:?}");
    };
    assert_eq!(sent.sent_mid, "3125305");
    assert_eq!(server.write_requests(), 0, "复核只读");
}

#[tokio::test]
async fn recheck_with_another_body_finds_nothing_and_writes_nothing() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    let mut digest = canonical_mail_body_digest(&captured_mail().body);
    digest[0] ^= 1;

    let outcome = transport.recheck_consignment_dispatch(&recheck_target(digest)).await;

    assert!(!matches!(outcome, ConsignmentDispatchRecheck::Found(_)), "{outcome:?}");
    assert_eq!(server.write_requests(), 0, "复核只读");
}

#[tokio::test]
async fn recheck_on_an_uncovered_sent_list_is_unavailable_without_writing() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));
    // 已采集的末页不当作真实末页：列表没有覆盖到很早的 `sending_at`。
    server.set_captured_pages_complete(false);
    let target = recheck_target(canonical_mail_body_digest(&captured_mail().body));

    let outcome = transport.recheck_consignment_dispatch(&target).await;

    assert_eq!(outcome, ConsignmentDispatchRecheck::Unavailable, "未覆盖不得补记");
    assert_eq!(server.write_requests(), 0, "复核只读");
}

/// 采集的未领取 Sent 3125364 的冻结身份（收件人、主题、正文逐字），冻结 Credits 由调用方给出。
fn cash_recheck_target(credits: i64) -> ConsignmentDispatchRecheckTarget {
    let mail = cash_mail(credits);
    ConsignmentDispatchRecheckTarget {
        recipient: mail.recipient.clone(),
        subject: mail.subject.clone(),
        credits,
        ..recheck_target(canonical_mail_body_digest(&mail.body))
    }
}

#[tokio::test]
async fn recheck_finds_the_sent_mail_whose_credits_equal_the_frozen_amount() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));

    let found = transport.recheck_consignment_dispatch(&cash_recheck_target(49_000)).await;

    let ConsignmentDispatchRecheck::Found(sent) = found else {
        panic!("剩余 Credits 等于冻结值应找到：{found:?}");
    };
    assert_eq!(sent.sent_mid, "3125364");
    assert_eq!(server.write_requests(), 0, "复核只读");
}

/// 负例用自己的一台替身与 transport：同一 transport 的 Sent 读取保留进程内扫描水位，第二次只做增量扫描，
/// 已见过的邮件不再成为候选，负向断言会在比对之前就空转。这里断言候选 3125364 的详情确实被读取，
/// 即走到了逐项比对，并因剩余 Credits（49,000）不等于冻结值（1）而拒绝。
#[tokio::test]
async fn recheck_reads_the_candidate_and_rejects_different_credits() {
    let server = MailDouble::start().await.expect("loopback server");
    let transport = transport(&server, Duration::from_secs(5));

    let outcome = transport.recheck_consignment_dispatch(&cash_recheck_target(1)).await;

    assert_eq!(outcome, ConsignmentDispatchRecheck::NotFound);
    let detail_reads = server
        .requests()
        .iter()
        .filter(|request| request.contains("filter=sent") && request.contains("mid=3125364"))
        .count();
    assert!(detail_reads >= 1, "候选 3125364 必须进入比对：{:?}", server.requests());
    assert_eq!(server.write_requests(), 0, "复核只读");
}

/// 仅 Credits 不同的两封请求：请求摘要与证据摘要都必须不同（证据写口据此区分重放与冲突）。
#[test]
fn credits_alone_changes_the_request_and_evidence_digests() {
    let request = |credits| MoogleMailSendRequest {
        world: MoogleMailWorld::Main,
        message: MoogleMailMessageFields {
            recipient: "FakeSeller".to_owned(),
            subject: "Fake subject".to_owned(),
            body: "Fake body".to_owned(),
        },
        credits,
        cod: 0,
        equipment_ids: Vec::new(),
        category: None,
    };
    let left = super::request_digest(&request(49_000));
    let right = super::request_digest(&request(48_999));
    assert_ne!(left, right, "请求摘要必须含 Credits");
    let evidence = |digest| super::evidence_digest(MoogleMailWorld::Main, "3125364", digest);
    assert_ne!(evidence(&left), evidence(&right), "证据摘要必须含 Credits");
}
