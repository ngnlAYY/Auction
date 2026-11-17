//! Worker 自身读取被放弃时的在途锁回归。排队中被放弃的读取撤票退出，远端从不收到；已发出的读取
//! 被调用方或外层读取预算放弃后，在途锁持有到远端回应或客户端 HTTP 预算结束，紧接的同账号请求
//! 不与远端仍在处理的请求重叠；不被放弃的读取节奏与串行不变。替身 `DetachedDelay` 不随客户端断开
//! 停止处理，在途计数保持到延迟结束，重叠会体现在 `max_concurrent_requests` 上。

use std::sync::Arc;
use std::time::{Duration, Instant};

use auction_test_support::mooglemail::{Fault, MailDouble};
use chrono::Utc;

use crate::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailErrorCategory, MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailWorld,
};

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";
/// 远端独立处理一个请求的时长；短于测试客户端的 5 秒 HTTP 预算。
const REMOTE_DELAY: Duration = Duration::from_millis(600);

/// `interval` 为请求门最小间隔，`request_timeout` 为单次请求的 HTTP 预算。
fn client(server: &MailDouble, interval: Duration, request_timeout: Duration) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: interval,
        request_timeout,
    })
    .expect("loopback client")
}

/// Worker 自己的一次常规读取（main 收件箱首页）。
async fn inbox_read(client: &MoogleMailClient) -> Result<Vec<u8>, MoogleMailErrorCategory> {
    let target = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
    let cookie = MoogleMailCookie::new(COOKIE).expect("fixture cookie");
    client.get(&target, &cookie).await.map_err(|error| error.category)
}

/// 调用方只等 `wait`，到点丢弃读取 future；读取必须尚未完成。
async fn abandon_read(client: &MoogleMailClient, wait: Duration) {
    let abandoned = tokio::time::timeout(wait, inbox_read(client)).await;
    assert!(abandoned.is_err(), "读取应在 {wait:?} 内仍未完成：{abandoned:?}");
}

/// 轮询等待测试观察到的状态成立；5 秒内不成立即失败。
async fn wait_until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("观察条件应在 5 秒内成立");
}

/// 已发出的读取被调用方放弃：在途锁仍持有到远端回应，紧接的读取等前一请求在远端处理完才发出，
/// 远端看到的同账号物理请求不重叠。
#[tokio::test]
async fn read_abandoned_after_send_keeps_the_next_read_waiting() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server, Duration::from_millis(1), Duration::from_secs(5));
    server.set_fault(Some(Fault::DetachedDelay(REMOTE_DELAY)));
    let started = Instant::now();

    abandon_read(&client, Duration::from_millis(150)).await;
    assert_eq!(server.requests().len(), 1, "放弃前请求已经发出");
    // 只影响之后到达的请求：已发出的请求仍按原故障在远端处理。
    server.set_fault(None);
    inbox_read(&client).await.expect("紧接的读取成功");

    let elapsed = started.elapsed();
    let overlap = server.max_concurrent_requests();
    assert!(
        elapsed >= REMOTE_DELAY,
        "紧接的读取应等远端处理完前一请求：{elapsed:?}，远端最大并发 {overlap}"
    );
    assert_eq!(server.requests().len(), 2);
    assert_eq!(server.max_concurrent_requests(), 1, "同一账号的物理请求不得重叠");
    assert_eq!(client.queued_waiters(), (0, 0), "请求门与在途锁不得留下失效登记");
}

/// 余额读取（`gateway/reader/snapshot.rs`）的外层预算在远端回应前耗尽：读取按预算耗尽立即返回，
/// 已发出的请求仍持有在途锁到远端回应，Worker 紧接的读取不与之重叠。
#[tokio::test]
async fn balance_budget_expiry_after_send_keeps_the_next_read_waiting() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = Arc::new(client(&server, Duration::from_millis(1), Duration::from_secs(5)));
    let gateway = MoogleMailGateway::new(
        Arc::clone(&client),
        MoogleMailCredentialSet::new(COOKIE).expect("fixture credentials"),
        MoogleMailGatewayConfig {
            fetch_budget: Duration::from_millis(150),
            ..MoogleMailGatewayConfig::default()
        },
    )
    .expect("gateway");
    server.set_fault(Some(Fault::DetachedDelay(REMOTE_DELAY)));
    let started = Instant::now();

    let error = gateway
        .read_balance(MoogleMailWorld::Main, Utc::now())
        .await
        .expect_err("读取预算先于远端回应耗尽");
    assert_eq!(error.category, MoogleMailErrorCategory::RemoteTemporary);
    assert!(started.elapsed() < REMOTE_DELAY, "预算到期即返回，不等远端：{:?}", started.elapsed());
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "预算到期前余额请求已经发出：{requests:?}");
    assert!(requests[0].contains("ss=is"), "{requests:?}");
    server.set_fault(None);
    inbox_read(&client).await.expect("紧接的读取成功");

    let elapsed = started.elapsed();
    let overlap = server.max_concurrent_requests();
    assert!(
        elapsed >= REMOTE_DELAY,
        "紧接的读取应等远端处理完余额请求：{elapsed:?}，远端最大并发 {overlap}"
    );
    assert_eq!(server.max_concurrent_requests(), 1, "同一账号的物理请求不得重叠");
}

/// 远端处理超过客户端 HTTP 预算：被放弃的读取持锁到预算结束为止，不随调用方提前释放，也不等远端
/// 处理完，后台持锁有界。客户端超时不能证明远端已停止，此后远端看到的请求可能重叠，这是只保证
/// Worker 侧串行的边界。
#[tokio::test]
async fn abandoned_read_holds_the_lock_no_longer_than_its_http_budget() {
    let request_timeout = Duration::from_millis(400);
    let remote_delay = Duration::from_secs(3);
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server, Duration::from_millis(1), request_timeout);
    server.set_fault(Some(Fault::DetachedDelay(remote_delay)));
    let started = Instant::now();

    abandon_read(&client, Duration::from_millis(100)).await;
    server.set_fault(None);
    inbox_read(&client).await.expect("HTTP 预算结束后紧接的读取成功");

    let elapsed = started.elapsed();
    assert!(elapsed >= request_timeout, "在途锁应持有到 HTTP 预算结束：{elapsed:?}");
    assert!(elapsed < remote_delay - Duration::from_secs(1), "在途锁不得等远端处理完：{elapsed:?}");
    assert_eq!(server.requests().len(), 2);
    assert_eq!(server.max_concurrent_requests(), 2, "客户端超时后远端仍在处理，客户端无法排除重叠");
}

/// 在请求门上排队的读取被放弃即撤票：反复放弃后门上不留失效票据，紧接的读取按正常间隔放行，
/// 远端从未收到被放弃的读取。
#[tokio::test]
async fn reads_abandoned_at_the_gate_leave_no_stale_tickets() {
    let interval = Duration::from_millis(400);
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server, interval, Duration::from_secs(5));
    // 首个读取用掉令牌：之后的读取都要在门上等下一个时间槽。
    inbox_read(&client).await.expect("首个读取成功");

    for _ in 0..4 {
        abandon_read(&client, Duration::from_millis(30)).await;
    }

    assert_eq!(client.queued_waiters(), (0, 0), "请求门与在途锁不得留下失效登记");
    let started = Instant::now();
    inbox_read(&client).await.expect("Worker 仍能前进");
    assert!(started.elapsed() < interval * 2, "读取被失效票据推迟：{:?}", started.elapsed());
    assert_eq!(server.requests().len(), 2, "被放弃的读取从未发出");
}

/// 在在途锁上排队的读取被放弃同样撤销登记：前一请求挂在远端期间反复放弃，锁上不留失效登记；
/// 前一请求结束后紧接的读取直接取得锁，远端从未收到被放弃的读取。
#[tokio::test]
async fn reads_abandoned_on_the_in_flight_lock_leave_no_stale_waiters() {
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server, Duration::from_millis(1), Duration::from_secs(5));
    let hold = server.hold_responses();
    let first = inbox_read(&client);
    let abandon = async {
        // 首个读取已到达远端并挂起，在途锁由它持有。
        wait_until(|| server.requests().len() == 1).await;
        for _ in 0..4 {
            abandon_read(&client, Duration::from_millis(30)).await;
        }
        let waiters = client.queued_waiters();
        hold.release();
        waiters
    };

    let (first, waiters) = tokio::join!(first, abandon);

    first.expect("首个读取成功");
    assert_eq!(waiters, (0, 0), "请求门与在途锁不得留下失效登记");
    inbox_read(&client).await.expect("Worker 仍能前进");
    assert_eq!(server.requests().len(), 2, "被放弃的读取从未发出");
    assert_eq!(server.max_concurrent_requests(), 1);
}

/// 不被放弃的读取保持原有节奏：并发发起的五个读取逐个串行，起点按请求门间隔拉开，全部成功，
/// 总耗时只由门间隔决定，没有额外等待。
#[tokio::test]
async fn uncancelled_reads_keep_the_gate_pace_one_at_a_time() {
    let interval = Duration::from_millis(100);
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(&server, interval, Duration::from_secs(5));
    let started = Instant::now();

    let reads = tokio::join!(
        inbox_read(&client),
        inbox_read(&client),
        inbox_read(&client),
        inbox_read(&client),
        inbox_read(&client),
    );

    let elapsed = started.elapsed();
    for read in [reads.0, reads.1, reads.2, reads.3, reads.4] {
        read.expect("读取成功");
    }
    assert_eq!(server.requests().len(), 5);
    assert_eq!(server.max_concurrent_requests(), 1, "同一账号的物理请求必须串行");
    // 首个令牌立即可用，其后四个各等一个间隔。
    assert!(elapsed >= interval * 4, "起点必须按门间隔拉开：{elapsed:?}");
    assert!(elapsed < interval * 4 + Duration::from_millis(300), "读取不应有额外等待：{elapsed:?}");
}
