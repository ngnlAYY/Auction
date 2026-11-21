//! 回归：论坛 PM 提醒循环必须服从主动暂停。
//!
//! 暂停期间不得领取发送或核对工作（两者都会访问论坛）；解除后在下一次重读切片内恢复。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::forum::{
    ForumCredentialAvailability, ForumPmError, ForumPmFuture, ForumPmMessage, ForumPmReceipt,
    ForumPmSender, PreparedForumPm,
};
use auction_application::lot_watch_pm::{
    LotWatchPmClaim, LotWatchPmClaimRequest, LotWatchPmWorkStore,
};
use auction_application::public::LotWatchFuture;
use auction_application::runtime_switches::{
    RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchRead,
    RuntimeSwitches,
};
use auction_time::{TimeDelta, Timestamp};
use serde_json::json;
use uuid::Uuid;

use super::LotWatchPmRuntime;

/// 只统计领取次数的队列替身；始终没有待发工作，因此不会触发任何发送分支。
#[derive(Default)]
struct CountingStore {
    claims: AtomicUsize,
}

impl LotWatchPmWorkStore for CountingStore {
    fn claim(
        &self,
        _request: LotWatchPmClaimRequest,
    ) -> LotWatchFuture<'_, Option<LotWatchPmClaim>> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(None) })
    }

    fn start_sending(&self, _claim: LotWatchPmClaim, _sender: String) -> LotWatchFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }

    fn release_unsent(&self, _claim: LotWatchPmClaim) -> LotWatchFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn finish(
        &self,
        _claim: LotWatchPmClaim,
        _receipt: Option<ForumPmReceipt>,
    ) -> LotWatchFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// 凭据可用、但任何远端调用都以不可用失败关闭的论坛替身。
struct UnreachableForum;

impl ForumCredentialAvailability for UnreachableForum {
    fn has_available_forum_credential(&self, _world: auction_domain::SettlementWorld) -> bool {
        true
    }
}

impl ForumPmSender for UnreachableForum {
    fn prepare<'a>(
        &'a self,
        _world: auction_domain::SettlementWorld,
        _message: &'a ForumPmMessage,
    ) -> ForumPmFuture<'a, Box<dyn PreparedForumPm>> {
        Box::pin(async { Err(ForumPmError::Unavailable) })
    }

    fn reconcile<'a>(
        &'a self,
        _world: auction_domain::SettlementWorld,
        _message: &'a ForumPmMessage,
        _sender_forum_account_id: &'a str,
    ) -> ForumPmFuture<'a, Option<ForumPmReceipt>> {
        Box::pin(async { Err(ForumPmError::Unavailable) })
    }
}

fn install_pause(switches: &RuntimeSwitches, paused: bool) {
    let now = Timestamp::now();
    let events = if paused {
        vec![RuntimeSwitchEvent {
            command_id: Uuid::new_v4(),
            name: RuntimeSwitchName::ExternalAccessPaused,
            revision: 1,
            operation: RuntimeSwitchOperation::Set,
            value: Some(json!(true)),
            expires_at: Some(now + TimeDelta::seconds(3_600)),
            expired_command_id: None,
            actor_id: "operator".to_owned(),
            entrypoint: "cli".to_owned(),
            reason: "restore drill".to_owned(),
            recorded_at: now,
        }]
    } else {
        Vec::new()
    };
    switches
        .install(RuntimeSwitchRead { database_time: now, events }, std::time::Instant::now())
        .expect("test switch snapshot is valid");
}
async fn advance_for(duration: Duration) {
    for _ in 0..duration.as_secs() {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn paused_external_access_stops_pm_claims_until_cleared() {
    let store = Arc::new(CountingStore::default());
    let switches = RuntimeSwitches::default();
    install_pause(&switches, true);
    let runtime = LotWatchPmRuntime::new(
        store.clone(),
        Arc::new(UnreachableForum),
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::from_secs(5),
        Duration::from_secs(30),
    )
    .expect("valid PM runtime")
    .with_runtime_switches(switches.clone());
    let (drain, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(runtime.run("worker-test".to_owned(), receiver));

    // 暂停覆盖多个轮询周期与重读切片：一次领取都不允许发生。
    advance_for(Duration::from_secs(30)).await;
    assert_eq!(store.claims.load(Ordering::SeqCst), 0, "paused PM loop must not claim work");

    // 解除后下一次重读切片内恢复，发送轮与核对轮都重新领取。
    install_pause(&switches, false);
    advance_for(Duration::from_secs(10)).await;
    assert!(store.claims.load(Ordering::SeqCst) >= 2, "cleared pause must resume PM claims");

    drain.send_replace(true);
    task.await.expect("PM loop stops on drain");
}

#[tokio::test(start_paused = true)]
async fn pause_after_start_stops_further_pm_claims() {
    let store = Arc::new(CountingStore::default());
    let switches = RuntimeSwitches::default();
    let runtime = LotWatchPmRuntime::new(
        store.clone(),
        Arc::new(UnreachableForum),
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::from_secs(5),
        Duration::from_secs(30),
    )
    .expect("valid PM runtime")
    .with_runtime_switches(switches.clone());
    let (drain, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(runtime.run("worker-test".to_owned(), receiver));

    advance_for(Duration::from_secs(5)).await;
    let before_pause = store.claims.load(Ordering::SeqCst);
    assert!(before_pause > 0, "running PM loop claims work");

    install_pause(&switches, true);
    // 至多让当前轮次结束，之后不再领取。
    advance_for(Duration::from_secs(2)).await;
    let settled = store.claims.load(Ordering::SeqCst);
    advance_for(Duration::from_secs(60)).await;
    assert_eq!(store.claims.load(Ordering::SeqCst), settled, "pause must stop further claims");

    drain.send_replace(true);
    task.await.expect("PM loop stops on drain");
}

struct ConfirmedFenceStore {
    claim: LotWatchPmClaim,
    claimed: AtomicBool,
    finished: Mutex<Option<Option<ForumPmReceipt>>>,
    drain_sender: tokio::sync::watch::Sender<bool>,
}

impl LotWatchPmWorkStore for ConfirmedFenceStore {
    fn claim(
        &self,
        _request: LotWatchPmClaimRequest,
    ) -> LotWatchFuture<'_, Option<LotWatchPmClaim>> {
        let claim = (!self.claimed.swap(true, Ordering::SeqCst)).then(|| self.claim.clone());
        Box::pin(async move { Ok(claim) })
    }

    fn start_sending(&self, _claim: LotWatchPmClaim, _sender: String) -> LotWatchFuture<'_, bool> {
        let drain_sender = self.drain_sender.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            drain_sender.send_replace(true);
            Ok(true)
        })
    }

    fn release_unsent(&self, _claim: LotWatchPmClaim) -> LotWatchFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn finish(
        &self,
        _claim: LotWatchPmClaim,
        receipt: Option<ForumPmReceipt>,
    ) -> LotWatchFuture<'_, ()> {
        Box::pin(async move {
            *self.finished.lock().expect("finish lock") = Some(receipt);
            Ok(())
        })
    }
}

struct ConfirmedFenceForum {
    sends: Arc<AtomicUsize>,
}

struct PreparedConfirmedFencePm {
    message: ForumPmMessage,
    sends: Arc<AtomicUsize>,
}

impl ForumCredentialAvailability for ConfirmedFenceForum {
    fn has_available_forum_credential(&self, _world: auction_domain::SettlementWorld) -> bool {
        true
    }
}

impl ForumPmSender for ConfirmedFenceForum {
    fn prepare<'a>(
        &'a self,
        _world: auction_domain::SettlementWorld,
        message: &'a ForumPmMessage,
    ) -> ForumPmFuture<'a, Box<dyn PreparedForumPm>> {
        let prepared =
            PreparedConfirmedFencePm { message: message.clone(), sends: self.sends.clone() };
        Box::pin(async move { Ok(Box::new(prepared) as Box<dyn PreparedForumPm>) })
    }

    fn reconcile<'a>(
        &'a self,
        _world: auction_domain::SettlementWorld,
        _message: &'a ForumPmMessage,
        _sender_forum_account_id: &'a str,
    ) -> ForumPmFuture<'a, Option<ForumPmReceipt>> {
        Box::pin(async { Ok(None) })
    }
}

impl PreparedForumPm for PreparedConfirmedFencePm {
    fn sender_forum_account_id(&self) -> &str {
        "12345"
    }

    fn send(self: Box<Self>) -> ForumPmFuture<'static, ForumPmReceipt> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(ForumPmReceipt {
                message: self.message,
                sender_forum_account_id: "12345".to_owned(),
                sent_message_id: "98765".to_owned(),
            })
        })
    }
}

#[tokio::test(start_paused = true)]
async fn confirmed_fence_sends_and_persists_receipt_after_deadline_and_drain() {
    let message = ForumPmMessage {
        notification_id: Uuid::new_v4(),
        recipient_forum_account_id: "54321".to_owned(),
        title: "Outbid test".to_owned(),
        body: "Notification: test.".to_owned(),
    };
    let (drain_sender, drain) = tokio::sync::watch::channel(false);
    let store = Arc::new(ConfirmedFenceStore {
        claim: LotWatchPmClaim {
            message: message.clone(),
            lot_id: Uuid::new_v4(),
            world: auction_domain::SettlementWorld::Main,
            digest: vec![1],
            token: Uuid::new_v4(),
            fence: 1,
            sender: None,
            reconcile: false,
        },
        claimed: AtomicBool::new(false),
        finished: Mutex::new(None),
        drain_sender,
    });
    let sends = Arc::new(AtomicUsize::new(0));
    let runtime = LotWatchPmRuntime::new(
        store.clone(),
        Arc::new(ConfirmedFenceForum { sends: sends.clone() }),
        Duration::from_secs(1),
        Duration::ZERO,
        Duration::from_secs(1),
        Duration::from_secs(30),
    )
    .expect("valid PM runtime");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);

    let activity = auction_application::operations::ExternalWriteActivity::default();
    let claimed_world = Mutex::new(None);
    activity
        .run(super::process_with_item_deadline(
            &runtime,
            LotWatchPmClaimRequest {
                worker_id: "worker-test".to_owned(),
                lease_seconds: 30,
                reconcile: false,
                available_worlds: vec![auction_domain::SettlementWorld::Main],
            },
            &drain,
            deadline,
            &activity,
            &claimed_world,
        ))
        .await
        .expect("activity remains available")
        .expect("confirmed fence must complete past the wrapper deadline")
        .expect("confirmed fence must finish its send");

    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let finished = store.finished.lock().expect("finish lock");
    assert_eq!(
        finished.as_ref().and_then(Option::as_ref),
        Some(&ForumPmReceipt {
            message,
            sender_forum_account_id: "12345".to_owned(),
            sent_message_id: "98765".to_owned(),
        })
    );
}
