//! 装备页读取与 MoogleMail 共用同账号许可的回环回归：战斗页只延期不退件、熔断窗口内不发网络
//! 且不占用探测额度、与邮件请求的物理请求不重叠；被放弃的装备读取在许可上排队时撤销登记，已发出
//! 时在途锁持有到远端回应或本次读取预算结束。

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
    adapter_with_timeout(server, shared, Duration::from_secs(2))
}

/// 同 [`adapter`]，`request_timeout` 为单次装备读取的总预算（含在途许可排队）。
fn adapter_with_timeout(
    server: &MailDouble,
    shared: Option<&MoogleMailClient>,
    request_timeout: Duration,
) -> EquipmentSourceAdapter {
    let adapter = EquipmentSourceAdapter {
        client: build_equipment_http_client().expect("equipment client"),
        endpoint: server.endpoint(),
        policy: EquipmentSourcePolicy {
            allowlist_enabled: false,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout,
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

/// 调用方只等 `wait`，到点丢弃装备读取 future；读取必须尚未完成。
async fn abandon_equipment_read(
    adapter: &EquipmentSourceAdapter,
    server: &MailDouble,
    wait: Duration,
) {
    let abandoned = tokio::time::timeout(
        wait,
        adapter.read_url_inner("main".to_owned(), equipment_url(server)),
    )
    .await;
    assert!(abandoned.is_err(), "装备读取应在 {wait:?} 内仍未完成");
}

/// 已发出的装备页请求被调用方放弃（Worker item timeout、control 路由预算、连接断开都是这种
/// 放弃）：借用的在途锁仍持有到远端回应，紧接的邮件读取等它在远端处理完才发出，不重叠。
#[tokio::test]
async fn equipment_read_abandoned_after_send_keeps_the_next_mail_read_waiting() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    let remote_delay = Duration::from_millis(600);
    server.set_fault(Some(Fault::DetachedDelay(remote_delay)));
    let started = Instant::now();

    abandon_equipment_read(&adapter, &server, Duration::from_millis(150)).await;
    let requests = server.requests();
    assert!(requests.len() == 1 && requests[0].contains("/equip/"), "{requests:?}");
    // 只影响之后到达的请求：已发出的装备页请求仍按原故障在远端处理。
    server.set_fault(None);
    client.get(&inbox, &cookie()).await.expect("紧接的邮件读取成功");

    let elapsed = started.elapsed();
    let overlap = server.max_concurrent_requests();
    assert!(
        elapsed >= remote_delay,
        "邮件读取应等远端处理完装备页请求：{elapsed:?}，远端最大并发 {overlap}"
    );
    assert_eq!(server.max_concurrent_requests(), 1, "同一账号的物理请求不得重叠");
    assert_eq!(client.queued_waiters(), (0, 0));
}

/// 装备读取的预算（含许可排队）是被放弃后继续持锁的上限：远端处理超过预算时，在途锁在预算结束
/// 时释放，不等远端处理完，后台持锁有界。此后远端看到的请求可能重叠，客户端无法排除。
#[tokio::test]
async fn abandoned_equipment_read_holds_the_lock_no_longer_than_its_budget() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let budget = Duration::from_millis(400);
    let adapter = adapter_with_timeout(&server, Some(&client), budget);
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    let remote_delay = Duration::from_secs(3);
    server.set_fault(Some(Fault::DetachedDelay(remote_delay)));
    let started = Instant::now();

    abandon_equipment_read(&adapter, &server, Duration::from_millis(100)).await;
    server.set_fault(None);
    client.get(&inbox, &cookie()).await.expect("预算结束后紧接的邮件读取成功");

    let elapsed = started.elapsed();
    assert!(elapsed >= budget, "在途锁应持有到装备读取预算结束：{elapsed:?}");
    assert!(elapsed < remote_delay - Duration::from_secs(1), "在途锁不得等远端处理完：{elapsed:?}");
    assert_eq!(server.max_concurrent_requests(), 2, "预算结束后远端仍在处理，客户端无法排除重叠");
}

/// 在借用的在途锁上排队的装备读取被放弃即撤销登记：邮件请求挂在远端期间反复放弃，锁上不留失效
/// 登记；邮件请求结束后紧接的读取直接取得锁，远端从未收到装备页请求。
#[tokio::test]
async fn equipment_reads_abandoned_on_the_lock_leave_no_stale_waiters() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = mail_client(&server);
    let adapter = adapter(&server, Some(&client));
    let cookie = cookie();
    let inbox = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    let hold = server.hold_responses();
    let first = client.get(&inbox, &cookie);
    let abandon = async {
        // 邮件请求已到达远端并挂起，在途锁由它持有。
        tokio::time::timeout(Duration::from_secs(5), async {
            while server.requests().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("邮件请求应在 5 秒内到达远端");
        for _ in 0..4 {
            abandon_equipment_read(&adapter, &server, Duration::from_millis(30)).await;
        }
        let waiters = client.queued_waiters();
        hold.release();
        waiters
    };

    let (first, waiters) = tokio::join!(first, abandon);

    first.expect("邮件读取成功");
    assert_eq!(waiters, (0, 0), "请求门与在途锁不得留下失效登记");
    client.get(&inbox, &cookie).await.expect("Worker 仍能前进");
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "装备页请求从未发出：{requests:?}");
    assert!(requests.iter().all(|request| !request.contains("/equip/")), "{requests:?}");
    assert_eq!(server.max_concurrent_requests(), 1);
}
