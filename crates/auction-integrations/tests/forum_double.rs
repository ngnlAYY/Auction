//! 真实客户端只访问回环替身；通知载荷覆盖层与真实采集结构分开。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::forum::{ForumPmMessage, ForumPmSender};
use auction_domain::SettlementWorld;
use auction_integrations::{
    ForumPreviewAdapter,
    ehentai::{
        EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiCredentialSet,
        EhentaiErrorCategory, EhentaiGateway,
    },
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailErrorCategory,
        MoogleMailWorld,
    },
};
use auction_test_support::forum::{Fault, ForumDouble};
use std::{sync::Arc, time::Duration};

fn cookie() -> EhentaiCookie {
    EhentaiCookie::from_bytes(b"ipb_member_id=fixture; ipb_pass_hash=fixture").unwrap()
}

fn client(server: &ForumDouble) -> EhentaiClient {
    let mut config = EhentaiClientConfig::production().unwrap();
    config.endpoint = server.endpoint();
    config.minimum_request_interval = Duration::from_millis(1);
    config.request_timeout = Duration::from_millis(200);
    config.test_board_mode = true;
    EhentaiClient::with_config(config).unwrap()
}

fn gateway(server: &ForumDouble) -> EhentaiGateway {
    EhentaiGateway::new(
        Arc::new(client(server)),
        EhentaiCredentialSet::from_cookies(vec![cookie()]).unwrap(),
    )
    .unwrap()
}

fn message() -> ForumPmMessage {
    let notification_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    ForumPmMessage {
        notification_id,
        recipient_forum_account_id: "100".into(),
        title: format!("Outbid {}", notification_id.simple()),
        body: format!("Outbid notice {}", notification_id),
    }
}

#[tokio::test]
async fn fetches_captured_topic_and_explicit_page_zero() {
    let server = ForumDouble::start().await.unwrap();
    let result =
        client(&server).fetch_topic(SettlementWorld::Main, "302133", &cookie()).await.unwrap();
    assert_eq!(result.catalog_post_id, "6987908");
    assert!(!result.catalog.lots.is_empty());
    let transport = reqwest::Client::new();
    let response = transport
        .get(server.endpoint().join("index.php?showtopic=302133&st=0").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let page = response.text().await.unwrap();
    assert!(ForumPreviewAdapter::new().parse_html_documents(&[&page]).is_ok());
    server.set_topic_page(20, page.clone());
    assert_eq!(
        transport
            .get(server.endpoint().join("index.php?showtopic=302133&st=20").unwrap())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        page
    );
    assert_eq!(
        transport
            .get(server.endpoint().join("index.php?showtopic=302133&st=40").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        501
    );
    assert_eq!(
        transport
            .get(server.endpoint().join("index.php?showtopic=302133&st=bad").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
}

#[tokio::test]
async fn real_client_follows_st_pagination_and_rejects_deleted_tail_post() {
    let server = ForumDouble::start().await.unwrap();
    let capture = include_str!("fixtures/w6_3/forum/topic-302133.html");
    let document = scraper::Html::parse_document(capture);
    let selector = scraper::Selector::parse(".postcolor").unwrap();
    let post = document.select(&selector).next().unwrap();
    let original = post.html();
    let normalized = document.root_element().html();
    // 单页真实结构的明确测试覆盖层：保留真实目录，另加 19 个无命令楼层形成满页。
    let extra = (1..20)
        .map(|index| {
            format!("<div class=\"postcolor\" id=\"post-{}\">System log</div>", 6987908 + index)
        })
        .collect::<String>();
    let navigation = "<a href=\"index.php?showtopic=302133&amp;st=20\">2</a>";
    server.set_topic_page(
        0,
        normalized.replace(&original, &format!("{original}{extra}{navigation}")),
    );
    let tail = normalized
        .replace(&original, "<div class=\"postcolor\" id=\"post-6987928\">System log</div>")
        .replace("6987908", "6987928");
    server.set_topic_page(20, tail.clone());
    let result =
        client(&server).fetch_topic(SettlementWorld::Main, "302133", &cookie()).await.unwrap();
    assert_eq!(result.catalog_post_id, "6987908");
    assert!(server.requests().iter().any(|path| path.contains("st=20")));
    // 删掉尾页唯一的真实结构帖子后，不把缺失页静默当作完整主题。
    server.set_topic_page(20, tail.replace("class=\"postcolor\"", "class=\"deleted\""));
    assert_eq!(
        client(&server)
            .fetch_topic(SettlementWorld::Main, "302133", &cookie())
            .await
            .unwrap_err()
            .category(),
        EhentaiErrorCategory::ProtocolDrift
    );
}

#[tokio::test]
async fn reconciles_real_pm_list_and_detail_structure_with_frozen_test_payload() {
    let server = ForumDouble::start().await.unwrap();
    let message = message();
    server.set_pm_payload(&message.title, &message.body);
    let result = gateway(&server).reconcile(SettlementWorld::Main, &message, "1000001").await;
    assert!(result.is_ok(), "{result:?}; requests={:?}", server.requests());
    let receipt = result.unwrap().unwrap();
    assert_eq!(receipt.sent_message_id, "1558026");
    assert_eq!(receipt.message, message);
    let requests = server.requests();
    assert!(requests.iter().any(|path| path.contains("CODE=03") && path.contains("MSID=1558026")));
    assert!(requests.iter().any(|path| path.contains("sort=") && path.contains("st=")));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn pm_reconciliation_preserves_sender_recipient_body_and_missing_proof_boundaries() {
    let server = ForumDouble::start().await.unwrap();
    let message = message();
    server.set_pm_payload(&message.title, &message.body);
    assert!(gateway(&server).reconcile(SettlementWorld::Main, &message, "1000002").await.is_err());
    let mut wrong_recipient = message.clone();
    wrong_recipient.recipient_forum_account_id = "999".into();
    assert!(
        gateway(&server)
            .reconcile(SettlementWorld::Main, &wrong_recipient, "1000001")
            .await
            .is_err()
    );
    server.set_pm_payload(&message.title, "different frozen body");
    assert!(gateway(&server).reconcile(SettlementWorld::Main, &message, "1000001").await.is_err());
    server.set_pm_payload("different subject", &message.body);
    assert!(
        gateway(&server)
            .reconcile(SettlementWorld::Main, &message, "1000001")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn topic_reads_classify_login_timeout_drift_and_deleted_post() {
    let server = ForumDouble::start().await.unwrap();
    for (fault, expected) in [
        (Fault::CookieInvalid, EhentaiErrorCategory::SessionExpired),
        (Fault::Timeout(Duration::from_secs(1)), EhentaiErrorCategory::RemoteTemporary),
        (Fault::ProtocolDrift, EhentaiErrorCategory::ProtocolDrift),
        (Fault::DeletedPost, EhentaiErrorCategory::ProtocolDrift),
    ] {
        server.set_fault(Some(fault));
        let error = client(&server)
            .fetch_topic(SettlementWorld::Main, "302133", &cookie())
            .await
            .unwrap_err();
        assert_eq!(error.category(), expected);
        assert!(!error.write_started());
    }
    server.set_fault(None);
    assert!(client(&server).fetch_topic(SettlementWorld::Main, "302133", &cookie()).await.is_ok());
}

#[tokio::test]
async fn real_battle_capture_is_classified_by_mail_client() {
    let server = ForumDouble::start().await.unwrap();
    server.set_fault(Some(Fault::Battle));
    // 邮件客户端的真实战斗识别边界；替身不把论坛响应误分类成论坛战斗状态。
    let mail = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_millis(200),
    })
    .unwrap();
    let target = mail.inbox_page(MoogleMailWorld::Main, 0).unwrap();
    let cookie = MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture").unwrap();
    assert_eq!(
        mail.get(&target, &cookie).await.unwrap_err().category,
        MoogleMailErrorCategory::AccountBusy
    );
}

#[tokio::test]
async fn unsupported_write_forms_fail_and_unknown_posts_are_never_success() {
    let server = ForumDouble::start().await.unwrap();
    let transport = reqwest::Client::new();
    for path in ["index.php?act=Post&CODE=03&t=302133", "index.php?act=msg"] {
        let target = server.endpoint().join(path).unwrap();
        assert_eq!(transport.get(target.clone()).send().await.unwrap().status(), 501);
        assert_eq!(transport.post(target.clone()).send().await.unwrap().status(), 501);
        server.set_fault(Some(Fault::WriteResultUnknown));
        let response = transport.post(target).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.text().await.unwrap().contains("unknown write outcome"));
        server.set_fault(None);
    }
    assert_eq!(server.write_requests(), 4);
    assert!(gateway(&server).prepare(SettlementWorld::Main, &message()).await.is_err());
    assert_eq!(server.write_requests(), 4);
    assert_eq!(
        transport
            .get(server.endpoint().join("index.php?act=Msg&CODE=03&VID=sent&MSID=999").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        501
    );
}

#[tokio::test]
async fn login_and_battle_routes_replay_real_captures_without_write_support() {
    let server = ForumDouble::start().await.unwrap();
    let transport = reqwest::Client::new();
    for (route, marker) in
        [("index.php?act=Login&CODE=00", "name=\"UserName\""), ("?s=Battle&ss=rb", "battle_main")]
    {
        let response = transport.get(server.endpoint().join(route).unwrap()).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.text().await.unwrap().contains(marker));
    }
    for route in [
        "index.php?showtopic=999",
        "index.php?showtopic=302133&st=0&st=20",
        "index.php?act=Login&CODE=01",
        "index.php?act=Msg&CODE=04&MID=100",
    ] {
        let status =
            transport.get(server.endpoint().join(route).unwrap()).send().await.unwrap().status();
        assert!(status == 501 || status == 400);
    }
}
