//! 用真实 HTTP 客户端对回环替身执行受限邮件读取。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used)]

use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailErrorCategory,
    MoogleMailParser, MoogleMailWorld,
};
use auction_test_support::mooglemail::{Fault, MailDouble, UnsupportedService};
use std::{collections::HashSet, time::Duration};

fn client(server: &MailDouble) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_millis(200),
    })
    .expect("loopback client")
}
fn cookie() -> MoogleMailCookie {
    MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture").expect("fixture cookie")
}

#[tokio::test]
async fn reads_captured_mail_routes_and_details() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server);
    let parser = MoogleMailParser;
    let cookie = cookie();
    for world in [MoogleMailWorld::Main, MoogleMailWorld::Isekai] {
        let bytes = client.get(&client.inbox_page(world, 0).unwrap(), &cookie).await.unwrap();
        assert!(
            parser
                .scan_inbox_page(&bytes, world, 0, &mut HashSet::new())
                .unwrap()
                .summaries
                .is_empty()
        );
        let bytes = client.get(&client.read_mail_page(world, 0).unwrap(), &cookie).await.unwrap();
        let read = parser.scan_read_mail_page(&bytes, world, 0, &mut HashSet::new()).unwrap();
        assert!(!read.summaries.is_empty());
        if world == MoogleMailWorld::Main {
            let summary =
                read.summaries.iter().find(|row| row.external_mail_id == "3125357").unwrap();
            let bytes =
                client.get(&client.detail(world, "3125357").unwrap(), &cookie).await.unwrap();
            assert_eq!(parser.parse_detail(&bytes, summary).unwrap().external_mail_id, "3125357");
        }
        let bytes = client.get(&client.sent_page(world, 0).unwrap(), &cookie).await.unwrap();
        let sent = parser.scan_sent_page(&bytes, world, 0, &mut HashSet::new()).unwrap();
        assert!(!sent.summaries.is_empty());
        if world == MoogleMailWorld::Main {
            for mid in ["3125366", "3125364", "3125344", "3125305", "3125298"] {
                let summary =
                    sent.summaries.iter().find(|row| row.external_mail_id == mid).unwrap();
                let bytes =
                    client.get(&client.sent_detail(world, mid).unwrap(), &cookie).await.unwrap();
                assert_eq!(
                    parser
                        .parse_sent_detail(&bytes, summary, summary.sent_at)
                        .unwrap()
                        .external_mail_id,
                    mid
                );
            }
        }
        let bytes = client.get(&client.compose(world).unwrap(), &cookie).await.unwrap();
        parser.parse_compose_state(&bytes, world).unwrap();
    }
    for page in [1, 2] {
        let bytes = client
            .get(&client.read_mail_page(MoogleMailWorld::Main, page).unwrap(), &cookie)
            .await
            .unwrap();
        assert_eq!(
            parser
                .scan_read_mail_page(&bytes, MoogleMailWorld::Main, page, &mut HashSet::new())
                .unwrap()
                .summaries
                .len(),
            15
        );
    }
    for target in [
        client.detail(MoogleMailWorld::Main, "0").unwrap(),
        client.sent_detail(MoogleMailWorld::Main, "0").unwrap(),
    ] {
        let bytes = client.get(&target, &cookie).await.unwrap();
        assert_eq!(bytes, include_bytes!("fixtures/w6_3/inboxdetail-missing.html"));
    }
}

#[tokio::test]
async fn classifies_account_busy_cookie_invalid_timeout_and_drift() {
    let server = MailDouble::start().await.unwrap();
    // 账号忙会打开进程内熔断窗口；每种故障用独立客户端，避免前一分类吞掉后续请求。
    for (fault, expected) in [
        (Fault::AccountBusy, MoogleMailErrorCategory::AccountBusy),
        (Fault::CookieInvalid, MoogleMailErrorCategory::SessionExpired),
        (Fault::Timeout(Duration::from_secs(1)), MoogleMailErrorCategory::RemoteTemporary),
    ] {
        let client = client(&server);
        let target = client.inbox_page(MoogleMailWorld::Main, 0).unwrap();
        server.set_fault(Some(fault));
        assert_eq!(client.get(&target, &cookie()).await.unwrap_err().category, expected);
    }
    server.set_fault(Some(Fault::ProtocolDrift));
    let client = client(&server);
    let bytes =
        client.get(&client.inbox_page(MoogleMailWorld::Main, 0).unwrap(), &cookie()).await.unwrap();
    assert_eq!(
        MoogleMailParser
            .scan_inbox_page(&bytes, MoogleMailWorld::Main, 0, &mut HashSet::new())
            .unwrap_err()
            .category,
        MoogleMailErrorCategory::ProtocolDrift
    );
    for service in
        [UnsupportedService::Forum, UnsupportedService::Battle, UnsupportedService::Login]
    {
        assert!(server.require_fixture(service).is_err());
    }
}

#[tokio::test]
async fn reading_moves_unread_receipt_and_sent_removal_is_consistent() {
    let server = MailDouble::start().await.unwrap();
    server.set_return_receipt_unread(true);
    let client = client(&server);
    let cookie = cookie();
    let world = MoogleMailWorld::Main;
    let bytes = client.get(&client.inbox_page(world, 0).unwrap(), &cookie).await.unwrap();
    let inbox = MoogleMailParser.scan_inbox_page(&bytes, world, 0, &mut HashSet::new()).unwrap();
    assert_eq!(inbox.summaries.len(), 1);
    let bytes = client.get(&client.read_mail_page(world, 0).unwrap(), &cookie).await.unwrap();
    assert_eq!(
        MoogleMailParser
            .scan_read_mail_page(&bytes, world, 0, &mut HashSet::new())
            .unwrap()
            .summaries
            .len(),
        14
    );
    client.get(&client.detail(world, "3125357").unwrap(), &cookie).await.unwrap();
    let bytes = client.get(&client.inbox_page(world, 0).unwrap(), &cookie).await.unwrap();
    assert!(
        MoogleMailParser
            .scan_inbox_page(&bytes, world, 0, &mut HashSet::new())
            .unwrap()
            .summaries
            .is_empty()
    );
    server.remove_sent("3125366");
    let bytes = client.get(&client.sent_page(world, 0).unwrap(), &cookie).await.unwrap();
    assert_eq!(
        MoogleMailParser
            .scan_sent_page(&bytes, world, 0, &mut HashSet::new())
            .unwrap()
            .summaries
            .len(),
        14
    );
    let bytes = client.get(&client.sent_detail(world, "3125366").unwrap(), &cookie).await.unwrap();
    assert!(MoogleMailParser.parse_action_state(&bytes, world).is_err());
    let bytes = client.get(&client.sent_page(world, 0).unwrap(), &cookie).await.unwrap();
    for summary in
        MoogleMailParser.scan_sent_page(&bytes, world, 0, &mut HashSet::new()).unwrap().summaries
    {
        server.remove_sent(&summary.external_mail_id);
    }
    let bytes = client.get(&client.sent_page(world, 0).unwrap(), &cookie).await.unwrap();
    assert!(
        MoogleMailParser
            .scan_sent_page(&bytes, world, 0, &mut HashSet::new())
            .unwrap()
            .summaries
            .is_empty()
    );
}

#[tokio::test]
async fn write_faults_preserve_unknown_and_explicit_rejections() {
    use auction_integrations::mooglemail::{
        MoogleMailForm, MoogleMailMessageFields, MoogleMailSendResponseFailure,
    };
    let server = MailDouble::start().await.unwrap();
    let client = client(&server);
    let cookie = cookie();
    let world = MoogleMailWorld::Main;
    let target = client.compose(world).unwrap();
    let bytes = client.get(&target, &cookie).await.unwrap();
    let state = MoogleMailParser.parse_compose_state(&bytes, world).unwrap();
    let fields = MoogleMailMessageFields {
        recipient: "FixtureRecipient".into(),
        subject: "fixture".into(),
        body: "fixture".into(),
    };
    for fault in [Fault::WriteResultUnknown, Fault::InsufficientPostage, Fault::MissingRecipient] {
        server.set_fault(Some(fault));
        let form = MoogleMailForm::send(state.token(), &fields).unwrap();
        let bytes = client.post(&target, &cookie, form).await.unwrap();
        assert!(MoogleMailParser.prove_send_completed(&bytes, world).is_err());
        let failure = MoogleMailParser.classify_send_response_failure(&bytes, world).unwrap();
        match fault {
            Fault::WriteResultUnknown => assert!(failure.is_none()),
            Fault::InsufficientPostage => {
                assert!(matches!(failure, Some(MoogleMailSendResponseFailure::Rejected)))
            }
            Fault::MissingRecipient => assert!(matches!(
                failure,
                Some(MoogleMailSendResponseFailure::RecipientNotFound { .. })
            )),
            _ => unreachable!(),
        }
    }
    assert_eq!(server.write_requests(), 3);

    // 2026-11-06 获准真实发送后采集的成功页；已脱敏玩家名、世界名与扩展 DOM。
    server.set_fault(Some(Fault::WriteSucceeded));
    let form = MoogleMailForm::send(state.token(), &fields).unwrap();
    let bytes = client.post(&target, &cookie, form).await.unwrap();
    assert!(MoogleMailParser.prove_send_completed(&bytes, world).unwrap().is_none());
    assert!(MoogleMailParser.classify_send_response_failure(&bytes, world).unwrap().is_none());
}

#[tokio::test]
async fn account_busy_window_allows_one_probe_and_keeps_worlds_separate() {
    let server = MailDouble::start().await.unwrap();
    let client = client(&server);
    let cookie = cookie();
    let main = client.inbox_page(MoogleMailWorld::Main, 0).unwrap();
    server.set_fault(Some(Fault::AccountBusy));

    // 首次命中战斗页只升级退避，不阻止同一次请求的结果分类。
    assert_eq!(
        client.get(&main, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert_eq!(server.read_requests(), 1);

    // 当前窗口只放行一次只读探测；探测同样命中后其余读取不再发出网络。
    assert_eq!(
        client.get(&main, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert_eq!(server.read_requests(), 2);
    assert_eq!(
        client.get(&main, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert_eq!(server.read_requests(), 2);
    assert_eq!(
        client.get(&main, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert_eq!(server.read_requests(), 2);

    // 世界隔离：主世界熔断期间异世界仍可发出自己的第一次请求。
    let isekai = client.inbox_page(MoogleMailWorld::Isekai, 0).unwrap();
    assert_eq!(
        client.get(&isekai, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert_eq!(server.read_requests(), 3);
    assert!(!client.permits_write(MoogleMailWorld::Main));
    assert!(!client.permits_write(MoogleMailWorld::Isekai));
}

#[tokio::test]
async fn account_busy_rejects_writes_before_any_network_request() {
    use auction_integrations::mooglemail::{MoogleMailForm, MoogleMailMessageFields};

    let server = MailDouble::start().await.unwrap();
    let client = client(&server);
    let cookie = cookie();
    let world = MoogleMailWorld::Main;
    let compose = client.compose(world).unwrap();
    let bytes = client.get(&compose, &cookie).await.unwrap();
    let state = MoogleMailParser.parse_compose_state(&bytes, world).unwrap();
    let fields = MoogleMailMessageFields {
        recipient: "FixtureRecipient".into(),
        subject: "fixture".into(),
        body: "fixture".into(),
    };
    assert!(client.permits_write(world));

    server.set_fault(Some(Fault::AccountBusy));
    assert_eq!(
        client.get(&compose, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
    assert!(!client.permits_write(world));

    let form = MoogleMailForm::send(state.token(), &fields).unwrap();
    let error = client.post(&compose, &cookie, form).await.unwrap_err();
    assert_eq!(error.category, MoogleMailErrorCategory::AccountBusy);
    // 拒绝发生在请求发出之前，不能伪装成已开始的写入。
    assert!(!error.write_started());
    assert_eq!(server.write_requests(), 0);
}

/// 只统计是否调用留证接口；不落盘、不构造真实密文。
#[derive(Default)]
struct CountingRecorder {
    retained: std::sync::atomic::AtomicUsize,
}

impl auction_application::mail::MailDiagnosticRecorder for CountingRecorder {
    fn prepare_response(
        &self,
        _context: auction_application::mail::MailDiagnosticContext,
        _status: Option<u16>,
        _body: Option<&[u8]>,
    ) -> Result<
        auction_application::mail::PreparedMailEvidence,
        auction_application::mail::MailDiagnosticError,
    > {
        Err(auction_application::mail::MailDiagnosticError::UnsupportedPlatform)
    }

    fn prepare_message(
        &self,
        _context: auction_application::mail::MailDiagnosticContext,
        _message: &auction_application::mail::MailMessage,
    ) -> Result<
        auction_application::mail::PreparedMailEvidence,
        auction_application::mail::MailDiagnosticError,
    > {
        Err(auction_application::mail::MailDiagnosticError::UnsupportedPlatform)
    }

    fn retain(
        &self,
        _prepared: auction_application::mail::PreparedMailEvidence,
        _outcome: auction_application::mail::MailDiagnosticOutcome,
    ) -> auction_application::mail::MailDiagnosticFuture<
        '_,
        auction_application::mail::MailDiagnosticReceipt,
    > {
        Box::pin(async { Err(auction_application::mail::MailDiagnosticError::UnsupportedPlatform) })
    }

    fn retain_response(
        &self,
        _context: auction_application::mail::MailDiagnosticContext,
        _status: Option<u16>,
        _body: Option<&[u8]>,
        _outcome: auction_application::mail::MailDiagnosticOutcome,
    ) -> auction_application::mail::MailDiagnosticFuture<
        '_,
        auction_application::mail::MailDiagnosticReceipt,
    > {
        use std::sync::atomic::Ordering;
        self.retained.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(auction_application::mail::MailDiagnosticReceipt {
                evidence_id: uuid::Uuid::nil(),
                plaintext_bytes: 0,
                sha256: String::new(),
                body_available: true,
            })
        })
    }
}

#[tokio::test]
async fn duplicate_busy_pages_are_retained_once_per_busy_period() {
    use std::sync::{Arc, atomic::Ordering};

    let server = MailDouble::start().await.unwrap();
    let recorder = Arc::new(CountingRecorder::default());
    let client = client(&server).with_diagnostics(recorder.clone());
    let cookie = cookie();
    let main = client.inbox_page(MoogleMailWorld::Main, 0).unwrap();
    let isekai = client.inbox_page(MoogleMailWorld::Isekai, 0).unwrap();
    let retained = || recorder.retained.load(Ordering::SeqCst);
    server.set_fault(Some(Fault::AccountBusy));

    let first = client.get(&main, &cookie).await.unwrap_err();
    assert_eq!(first.category, MoogleMailErrorCategory::AccountBusy);
    assert_eq!(first.evidence_status, "retained");
    assert_eq!(retained(), 1);

    // 探测重演同一忙碌期的战斗页：不再新增证据，但错误分类保持不变。
    let probe = client.get(&main, &cookie).await.unwrap_err();
    assert_eq!(probe.category, MoogleMailErrorCategory::AccountBusy);
    assert_eq!(retained(), 1);

    // 熔断拒绝没有正文，也不能占用证据容量。
    let rejected = client.get(&main, &cookie).await.unwrap_err();
    assert_eq!(rejected.evidence_status, "suppressed");
    assert_eq!(retained(), 1);

    // 异世界是独立忙碌期，仍保存自己的首份战斗页。
    let other_world = client.get(&isekai, &cookie).await.unwrap_err();
    assert_eq!(other_world.evidence_status, "retained");
    assert_eq!(retained(), 2);
}
