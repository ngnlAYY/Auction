//! 装备页读取与 MoogleMail 共用同账号许可的回环回归：战斗页只延期不退件、熔断窗口内不发网络
//! 且不占用探测额度、与邮件请求的物理请求不重叠。

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use auction_application::admin::AdminIntakeError;
use auction_application::core::ApplicationError;
use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, MailOperation, MailRequestCategory, MailSignal,
};
use auction_application::mail::{
    MailEquipmentAttachment, MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticContext,
    MailEquipmentReadOutcome, MailEquipmentReader,
};
use auction_domain::EquipmentKey;
use auction_test_support::mooglemail::{Fault, MailDouble};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::{EquipmentSourceAdapter, build_equipment_http_client};
use crate::equipment::config::EquipmentSourcePolicy;
use crate::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailErrorCategory, MoogleMailWorld,
};

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";
const EQUIPMENT_ID: &str = "123";
const KEY: &str = "0123456789";

fn mail_client(server: &MailDouble) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(2),
    })
    .expect("loopback mail client")
}

/// 回环 endpoint 只在测试里绕过生产构造的 HTTPS 校验；URL、许可、请求与分类仍走正式路径。
fn adapter(server: &MailDouble, shared: Option<&MoogleMailClient>) -> EquipmentSourceAdapter {
    let adapter = EquipmentSourceAdapter {
        client: build_equipment_http_client().expect("equipment client"),
        endpoint: server.endpoint(),
        policy: EquipmentSourcePolicy {
            allowlist_enabled: false,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(2),
            ..EquipmentSourcePolicy::default()
        },
        credentials: None,
        mail_client: None,
        diagnostics: None,
        diagnostic_reporter: None,
        next_request_at: Mutex::new(Instant::now()),
    };
    match shared {
        Some(client) => adapter.with_mooglemail_credentials(
            MoogleMailCredentialSet::new(COOKIE).expect("fixture credentials"),
            client.clone(),
        ),
        None => adapter,
    }
}

fn equipment_url(server: &MailDouble) -> String {
    format!("{}equip/{EQUIPMENT_ID}/{KEY}", server.endpoint())
}

fn cookie() -> MoogleMailCookie {
    MoogleMailCookie::new(COOKIE).expect("fixture cookie")
}

/// 按到达顺序保存外部账号事件的指标替身。
#[derive(Default)]
struct RecordedEvents(StdMutex<Vec<ExternalMetricsEvent>>);

impl ExternalMetricsObserver for RecordedEvents {
    fn observe(&self, event: ExternalMetricsEvent) {
        self.0.lock().expect("recorded events").push(event);
    }
}

#[tokio::test]
async fn battle_page_defers_the_mail_item_and_opens_the_shared_breaker() {
    let server = MailDouble::start().await.expect("loopback server");
    let events = Arc::new(RecordedEvents::default());
    let client = mail_client(&server).with_external_metrics(events.clone());
    let adapter = adapter(&server, Some(&client));
    server.set_fault(Some(Fault::AccountBusy));

    let context = MailEquipmentDiagnosticContext {
        mailbox_key: "fixture-mailbox".to_owned(),
        world_id: "main".to_owned(),
        external_mail_id: "3125357".to_owned(),
        external_equipment_id: EQUIPMENT_ID.to_owned(),
        attachment_position: 1,
    };
    let attachment = MailEquipmentAttachment {
        external_equipment_id: EQUIPMENT_ID.to_owned(),
        key: EquipmentKey::new(KEY).expect("fixture key"),
    };
    // 邮件处理只在来源明确无效时退件；战斗页必须是暂时不可用，留给 Jobs 重试。
    match adapter.read_diagnostic(context, attachment).await {
        MailEquipmentReadOutcome::Failed { error, observation } => {
            assert!(matches!(
                error,
                ApplicationError::AdminIntake(AdminIntakeError::SourceUnavailable)
            ));
            assert_eq!(observation.category, MailEquipmentDiagnosticCategory::RateLimited);
        }
        MailEquipmentReadOutcome::Loaded { .. } => panic!("battle page must not load"),
    }
    assert_eq!(server.read_requests(), 1);
    // 战斗页打开的是同一账号的熔断窗口，邮件写入随之停止。
    assert!(!client.permits_write(MoogleMailWorld::Main));
    assert!(client.permits_write(MoogleMailWorld::Isekai));

    let events = events.0.lock().expect("recorded events");
    assert!(events.iter().any(|event| matches!(
        event,
        ExternalMetricsEvent::MailRequestStarted { operation: MailOperation::Equipment, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ExternalMetricsEvent::MailRequestSignal {
            operation: MailOperation::Equipment,
            signal: MailSignal::AccountBusy,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ExternalMetricsEvent::MailRequestFinished {
            operation: MailOperation::Equipment,
            category: MailRequestCategory::AccountBusy,
            ..
        }
    )));
}

#[tokio::test]
async fn breaker_window_refuses_equipment_reads_without_network_or_probe() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    let cookie = cookie();
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    server.set_fault(Some(Fault::AccountBusy));

    // 邮件读取命中战斗页，打开窗口。
    let error = client.get(&inbox, &cookie).await.expect_err("battle page");
    assert_eq!(error.category, MoogleMailErrorCategory::AccountBusy);
    assert_eq!(server.read_requests(), 1);

    // 窗口内的装备读取不发网络，按暂时不可用返回。
    let result = adapter.read_url_inner("main".to_owned(), equipment_url(&server)).await;
    assert!(matches!(result, Err(AdminIntakeError::SourceUnavailable)));
    assert_eq!(server.read_requests(), 1);

    // 装备读取没有占用窗口唯一的探测：下一次邮件读取仍作为探测发出。
    let error = client.get(&inbox, &cookie).await.expect_err("probe sees battle again");
    assert_eq!(error.category, MoogleMailErrorCategory::AccountBusy);
    assert_eq!(server.read_requests(), 2);
    let result = adapter.read_url_inner("main".to_owned(), equipment_url(&server)).await;
    assert!(matches!(result, Err(AdminIntakeError::SourceUnavailable)));
    assert_eq!(server.read_requests(), 2);
}

#[tokio::test]
async fn equipment_page_opened_window_blocks_the_next_equipment_read() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    server.set_fault(Some(Fault::AccountBusy));

    let result = adapter.read_url_inner("main".to_owned(), equipment_url(&server)).await;
    assert!(matches!(result, Err(AdminIntakeError::SourceUnavailable)));
    assert_eq!(server.read_requests(), 1);
    let result = adapter.read_url_inner("main".to_owned(), equipment_url(&server)).await;
    assert!(matches!(result, Err(AdminIntakeError::SourceUnavailable)));
    assert_eq!(server.read_requests(), 1);
}

#[tokio::test]
async fn state_lock_limiter_on_equipment_page_is_temporary_without_opening_the_breaker() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    for fault in [Fault::StateLockLimited, Fault::StateLockLimitedTooManyRequests] {
        server.set_fault(Some(fault));
        let result = adapter.read_url_inner("main".to_owned(), equipment_url(&server)).await;
        assert!(matches!(result, Err(AdminIntakeError::SourceUnavailable)), "{fault:?}");
    }
    // 限流不是账号忙：不打开熔断窗口。
    assert!(client.permits_write(MoogleMailWorld::Main));
    assert_eq!(server.read_requests(), 2);
}

#[tokio::test]
async fn equipment_reads_never_overlap_mail_requests_on_the_shared_account() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    let cookie = cookie();
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    let url = equipment_url(&server);
    // 每个请求在替身中停留 150ms；没有共享许可时装备页与邮件请求必然重叠。
    server.set_fault(Some(Fault::Timeout(Duration::from_millis(150))));

    let (first, second, third, fourth) = tokio::join!(
        adapter.read_url_inner("main".to_owned(), url.clone()),
        client.get(&inbox, &cookie),
        adapter.read_url_inner("main".to_owned(), url.clone()),
        client.get(&inbox, &cookie),
    );
    // 装备页没有语料，延迟后的 501 不是本用例的关注点；这里只证明物理请求串行。
    assert!(first.is_err() && third.is_err());
    assert!(second.is_ok() && fourth.is_ok());
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests.iter().filter(|request| request.contains("/equip/")).count(), 2);
    assert_eq!(server.max_concurrent_requests(), 1);
}

#[tokio::test]
async fn without_the_shared_client_equipment_reads_overlap_mail_requests() {
    // 对照：未注入共享客户端时装备页只按自身节奏发出，会与邮件请求重叠；上一用例的串行来自许可。
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, None);
    let cookie = cookie();
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    server.set_fault(Some(Fault::Timeout(Duration::from_millis(150))));

    let (equipment, mail) = tokio::join!(
        adapter.read_url_inner("main".to_owned(), equipment_url(&server)),
        client.get(&inbox, &cookie),
    );
    assert!(equipment.is_err());
    assert!(mail.is_ok());
    assert_eq!(server.max_concurrent_requests(), 2);
}
