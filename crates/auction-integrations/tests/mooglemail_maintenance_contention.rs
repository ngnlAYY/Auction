//! W6.6 / D2 复现：Read Mail 完整观察与 Sent 维护共用请求门时，维护在本轮预算内超时。
//!
//! 使用真实 `MailboxMaintenanceService`、真实维护适配器与 MoogleMail 客户端（同一凭据、
//! 同一个 1 容量请求门），在 W6.3 邮件替身上复现 Worker 的两种调度形状：
//! - 旧路径（`poll_isolation_enabled = false`，`auction-jobs` `mail/poll/cycle.rs` 的
//!   `tokio::join!`）：观察与维护同时开始，各自以同一个 `fetch_timeout` 为截止；
//! - 隔离路径（W1.0，`mail/maintenance/runner.rs`）：维护单独先跑，观察在其后另起截止。
//!
//! 覆盖限制：Worker 循环按开关选择形状的调度代码不在本文件内（`PersistentWorker` 依赖
//! 过多持久端口）；这里证明的是两种形状在真实服务与客户端上的预算结果。替身开启
//! `set_captured_pages_complete`，让 main 世界三页 Read Mail 与一页 Sent 成为完整枚举。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, Clock};
use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use auction_application::mail::{
    MailLoginKey, MailLoginKeyReader, MailboxFuture, MailboxMaintenanceReport,
    MailboxMaintenanceService, MoogleMailboxEvidenceStore, MoogleMailboxSweepCandidate,
};
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCredentialSet, MoogleMailDeliveryTransport,
};
use auction_test_support::mooglemail::MailDouble;
use chrono::{DateTime, Utc};
use tokio::time::{Instant, error::Elapsed, timeout_at};
use uuid::Uuid;

/// 请求门最小间隔：每个远端请求占一个令牌，维护与观察排在同一个门上。
const INTERVAL: Duration = Duration::from_millis(500);
/// 一轮维护的 `fetch_timeout`：维护单独需要两个请求（Sent 第 0 页、compose），约一个间隔；
/// 与观察交替取令牌时至少两个间隔。1.6 个间隔给两侧各留出调度抖动余量。
const BUDGET: Duration = Duration::from_millis(800);
/// 隔离后观察另起的截止：三页完整枚举需要两个间隔。
const OBSERVATION_BUDGET: Duration = Duration::from_secs(3);

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// 维护与观察都不读取登录 key；被调用即失败关闭。
struct NoLoginKeys;
impl MailLoginKeyReader for NoLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

/// 没有未证明发送：维护只执行 Sent 枚举与 compose 检查两次远端读取，不触发任何写入。
struct EmptyEvidence;
impl MoogleMailboxEvidenceStore for EmptyEvidence {
    fn unproven_sends<'a>(
        &'a self,
        _world_id: &'a str,
        _limit: i64,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxSweepCandidate>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn record_sweep_attempt<'a>(
        &'a self,
        _world_id: &'a str,
        _delivery_id: Uuid,
    ) -> MailboxFuture<'a, bool> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
    fn begin_compose_discard<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _expectation: Option<&'a DeliverySendExpectation>,
    ) -> MailboxFuture<'a, bool> {
        Box::pin(async { unreachable!("captured compose page is clean") })
    }
    fn record_sent_observation<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _observation: serde_json::Value,
    ) -> MailboxFuture<'a, ()> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
    fn record_sent_sighting<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _external_mail_id: &'a str,
        _observation: serde_json::Value,
    ) -> MailboxFuture<'a, ()> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
    fn record_compose_discard<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _observation: serde_json::Value,
    ) -> MailboxFuture<'a, ()> {
        Box::pin(async { unreachable!("captured compose page is clean") })
    }
    fn bind_recovered_send<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _external_mail_id: String,
        _metadata: serde_json::Value,
        _expectation: Option<&'a DeliverySendExpectation>,
    ) -> MailboxFuture<'a, bool> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
    fn bind_recovered_action<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _external_mail_id: String,
        _metadata: serde_json::Value,
        _expectation: &'a DeliveryTakeExpectation,
        _observation: &'a DeliverySendCredentialBinding,
    ) -> MailboxFuture<'a, bool> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
    fn requeue_unproven<'a>(
        &'a self,
        _delivery_id: Uuid,
        _fencing_version: i64,
        _retries: i64,
        _reason: &'a str,
        _expectation: &'a DeliveryTakeExpectation,
        _observation: &'a DeliverySendCredentialBinding,
    ) -> MailboxFuture<'a, bool> {
        Box::pin(async { unreachable!("no sweep candidates") })
    }
}

async fn double() -> MailDouble {
    let server = MailDouble::start().await.expect("loopback server");
    server.set_captured_pages_complete(true);
    server
}

/// 与 Worker 装配相同：维护适配器复制投递 transport 的凭据集合与发送器（共用请求门）。
fn service(server: &MailDouble) -> MailboxMaintenanceService {
    let client = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: INTERVAL,
        request_timeout: Duration::from_secs(5),
    })
    .expect("loopback client");
    let credentials = MoogleMailCredentialSet::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
        .expect("fixture cookie");
    let transport = MoogleMailDeliveryTransport::new(client, credentials, Arc::new(NoLoginKeys));
    MailboxMaintenanceService::new(
        Arc::new(transport.maintenance_adapter()),
        Arc::new(EmptyEvidence),
        Arc::new(SystemClock),
    )
}

/// 一轮维护的结果，口径同 `mail/maintenance/sweep.rs` 的 `run_maintenance_inner`。
#[derive(Debug, PartialEq, Eq)]
enum Round {
    Completed,
    DeadlineExceeded,
    Failed,
}

fn classify(
    result: Result<Result<MailboxMaintenanceReport, ApplicationError>, Elapsed>,
    deadline: Instant,
) -> Round {
    match result {
        Ok(Ok(report)) if report.compose_error.is_none() && report.sent_state_error.is_none() => {
            Round::Completed
        }
        Ok(Ok(_)) => Round::Failed,
        Err(_) => Round::DeadlineExceeded,
        Ok(Err(_)) if Instant::now() >= deadline => Round::DeadlineExceeded,
        Ok(Err(_)) => Round::Failed,
    }
}

fn position(requests: &[String], pattern: &str) -> Option<usize> {
    requests.iter().position(|request| request.ends_with(pattern))
}

#[tokio::test]
async fn legacy_shared_round_times_out_maintenance() {
    let server = double().await;
    let service = service(&server);
    let deadline = Instant::now() + BUDGET;
    let (observation, maintenance) = tokio::join!(
        timeout_at(deadline, service.observe_read_mail("main", deadline.into_std())),
        timeout_at(deadline, service.sweep("main", deadline.into_std())),
    );

    // D2：维护与 Read Mail 翻页交替占用请求门，第二个请求等不到令牌就越过本轮截止。
    assert_eq!(classify(maintenance, deadline), Round::DeadlineExceeded);
    assert!(!matches!(observation, Ok(Ok(_))), "the full scan cannot finish in the shared round");
    let requests = server.requests();
    assert!(position(&requests, "filter=sent").is_some(), "{requests:?}");
    assert!(position(&requests, "filter=read").is_some(), "{requests:?}");
    assert!(
        position(&requests, "filter=new").is_none(),
        "a Read Mail page took the slot compose needed: {requests:?}"
    );
}

#[tokio::test]
async fn isolated_maintenance_completes_within_the_same_budget() {
    let server = double().await;
    let service = service(&server);
    let deadline = Instant::now() + BUDGET;
    let maintenance = timeout_at(deadline, service.sweep("main", deadline.into_std())).await;

    assert_eq!(classify(maintenance, deadline), Round::Completed);
    let requests = server.requests();
    assert_eq!(
        requests,
        ["GET /?s=Bazaar&ss=mm&filter=sent", "GET /?s=Bazaar&ss=mm&filter=new"],
        "maintenance alone reads only Sent and compose"
    );

    // 观察在维护结束后另起截止，不再消耗维护预算；完整枚举仍可完成。
    let observation_deadline = Instant::now() + OBSERVATION_BUDGET;
    let observed = timeout_at(
        observation_deadline,
        service.observe_read_mail("main", observation_deadline.into_std()),
    )
    .await;
    assert!(matches!(observed, Ok(Ok(rows)) if rows > 0), "{observed:?}");
    let requests = server.requests();
    assert_eq!(
        requests[2..],
        [
            "GET /?s=Bazaar&ss=mm&filter=read",
            "GET /?s=Bazaar&ss=mm&filter=read&page=1",
            "GET /?s=Bazaar&ss=mm&filter=read&page=2",
        ],
        "observation pages follow maintenance without overlap"
    );
}
