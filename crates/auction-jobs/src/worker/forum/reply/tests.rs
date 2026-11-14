//! 回归：Final 阶段的导入节奏在重复领取、主题失败与 pending 重试三条路径上都不退回常规轮询；
//! 论坛循环跨轮保留 Final 排期，本轮领到主题后出错退出或领取调用失败都不丢弃其他主题的
//! 排期，只有空领取才裁掉作废排期，循环也不因此空转。
//!
//! 内存替身按存储语义领取：`next_scan_at <= now` 且未持有租约，按 `next_scan_at` 升序；
//! finish/fail 释放租约并写入调用方给出的下一次扫描时刻。时钟跟随 Tokio 暂停时间。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::{
    AdminForumBidPost, AdminForumCatalog, AdminForumLot, AdminForumPost, AdminForumPreviewResult,
};
use auction_application::auction::{
    ApplicationFuture, CompetitionCommandOutcome, CompetitionCommandPort, CreateProxyCommand,
    LowerProxyCommand, PlaceBidCommand,
};
use auction_application::core::Clock;
use auction_application::external_metrics::ForumTopicScanMode;
use auction_application::forum::{
    ForumActiveProxy, ForumAuctionSnapshot, ForumBidCommandRecord, ForumBidderIdentity,
    ForumCatalogLayout, ForumCommandOutcome, ForumCommandStart, ForumCommandStatus,
    ForumCredentialAvailability, ForumLotTarget, ForumPostWriter, ForumPublishedPost,
    ForumReadPost, ForumRemoteError, ForumRenderError, ForumRenderedPost, ForumRenderedThread,
    ForumRenderer, ForumReplyPhase, ForumReplyStore, ForumReplyTopic, ForumSystemLogEntry,
    ForumTopicCursor, ForumTopicMetadata, ForumTopicReader, ForumTopicScan, ForumWriteProgress,
    NotifySubscription, forum_catalog_marker, forum_system_log_marker,
};
use auction_application::public::{
    PublicEquipmentGameLink, PublicEquipmentLinkResolver, PublicEquipmentLinkTarget,
};
use auction_domain::{AuctionId, AuctionStatus, LotId, SettlementWorld, UserId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{FinalScanSchedule, ForumReplyPorts, ForumReplyRuntime, run_cycle};
use crate::worker::PersistentWorker;
use crate::worker::runtime::test_fixture::{PausedClock, base_time, unavailable, worker};

const TITLE: &str = "Fixture auction";
const CATALOG_BODY: &str = "catalog body";
const SYSTEM_LOG_BODY: &str = "system log body";
const CATALOG_POST_ID: &str = "7000001";
const SYSTEM_LOG_POST_ID: &str = "7000002";
const POLL_INTERVAL: Duration = Duration::from_secs(300);
const LEASE: Duration = Duration::from_secs(300);
/// 截止前 60 秒：Final 间隔 = 剩余 / 6 = 10 秒。
const DEADLINE_AFTER: Duration = Duration::from_secs(60);

fn at(offset: Duration) -> DateTime<Utc> {
    base_time() + chrono::Duration::from_std(offset).unwrap()
}

fn before(offset: Duration) -> DateTime<Utc> {
    base_time() - chrono::Duration::from_std(offset).unwrap()
}

fn topic(auction_id: AuctionId, reply_phase: ForumReplyPhase) -> ForumReplyTopic {
    ForumReplyTopic {
        auction_id,
        world: SettlementWorld::Main,
        reply_phase,
        thread_id: auction_id.to_string(),
        catalog_post_id: CATALOG_POST_ID.to_owned(),
        catalog_layout: ForumCatalogLayout::PagedV1,
        catalog_continuation: Vec::new(),
        system_log_post_id: SYSTEM_LOG_POST_ID.to_owned(),
        title: TITLE.to_owned(),
        catalog_body: CATALOG_BODY.to_owned(),
        system_log_body: SYSTEM_LOG_BODY.to_owned(),
        content_revision: 1,
        confirmed_terminal_version: None,
        // 系统日志在 #2；与本轮完整扫描的派生水位一致，固定楼层无需改写。
        processed_through_post_number: 2,
        content_claim_token: Uuid::new_v4(),
        content_claim_owner: "worker-test".to_owned(),
        content_claim_fencing: 1,
        content_lease_until: at(LEASE),
        content_last_error_code: None,
    }
}

struct TopicRow {
    topic: ForumReplyTopic,
    next_scan_at: DateTime<Utc>,
    leased: bool,
    last_error: Option<String>,
    /// 拍卖已离开可扫描状态（例如撤下或终态不再刷新），领取时不再返回。
    hidden: bool,
}

/// 按存储语义维护公平扫描水位的内存替身；未覆盖的端口失败关闭。
struct FakeReplyStore {
    rows: Mutex<Vec<TopicRow>>,
    /// 每次领取调用的时刻，无论是否领到主题。
    attempts: Mutex<Vec<DateTime<Utc>>>,
    claims: Mutex<Vec<(DateTime<Utc>, AuctionId)>>,
    /// 账本中仍有 pending 命令（例如序列化冲突后待重试）的拍卖。
    pending: Vec<AuctionId>,
    /// 为 true 时领取调用本身失败（例如连接池 acquire 超时），不改变任何行。
    claims_fail: Mutex<bool>,
    /// 这些拍卖的失败处置写入失败（例如连接池 acquire 超时），内容租约保持被持有。
    unrecordable: Mutex<Vec<AuctionId>>,
    earliest_deadline: DateTime<Utc>,
}

impl FakeReplyStore {
    fn new(rows: Vec<TopicRow>, pending: Vec<AuctionId>) -> Self {
        Self {
            rows: Mutex::new(rows),
            attempts: Mutex::new(Vec::new()),
            claims: Mutex::new(Vec::new()),
            pending,
            claims_fail: Mutex::new(false),
            unrecordable: Mutex::new(Vec::new()),
            earliest_deadline: at(DEADLINE_AFTER),
        }
    }

    fn set_claims_fail(&self, fail: bool) {
        *self.claims_fail.lock().unwrap() = fail;
    }

    fn make_unrecordable(&self, auction_id: AuctionId) {
        self.unrecordable.lock().unwrap().push(auction_id);
    }

    /// 持久化的下一次扫描时刻与最近错误码。
    fn persisted(&self, auction_id: AuctionId) -> (DateTime<Utc>, Option<String>) {
        let rows = self.rows.lock().unwrap();
        let row = rows.iter().find(|row| row.topic.auction_id == auction_id).unwrap();
        (row.next_scan_at, row.last_error.clone())
    }

    fn claims(&self) -> Vec<(DateTime<Utc>, AuctionId)> {
        self.claims.lock().unwrap().clone()
    }

    fn attempts(&self) -> Vec<DateTime<Utc>> {
        self.attempts.lock().unwrap().clone()
    }

    fn update(&self, auction_id: AuctionId, change: impl FnOnce(&mut TopicRow)) {
        let mut rows = self.rows.lock().unwrap();
        change(rows.iter_mut().find(|row| row.topic.auction_id == auction_id).unwrap());
    }

    fn release(
        &self,
        auction_id: AuctionId,
        next_scan_at: DateTime<Utc>,
        last_error: Option<String>,
    ) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|row| row.topic.auction_id == auction_id).unwrap();
        assert!(row.leased, "只能释放当前持有的内容租约");
        row.leased = false;
        row.next_scan_at = next_scan_at;
        row.last_error = last_error;
    }
}

impl ForumReplyStore for FakeReplyStore {
    fn claim_published_topics(
        &self,
        _worker_id: String,
        now: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
        available_worlds: Vec<SettlementWorld>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumReplyTopic>> {
        self.attempts.lock().unwrap().push(now);
        if *self.claims_fail.lock().unwrap() {
            return unavailable();
        }
        let mut rows = self.rows.lock().unwrap();
        let mut due = rows
            .iter_mut()
            .filter(|row| {
                !row.leased
                    && !row.hidden
                    && row.next_scan_at <= now
                    && available_worlds.contains(&row.topic.world)
            })
            .collect::<Vec<_>>();
        due.sort_by_key(|row| (row.next_scan_at, row.topic.auction_id));
        let mut claimed = Vec::new();
        for row in due.into_iter().take(usize::from(limit)) {
            row.leased = true;
            let mut topic = row.topic.clone();
            topic.content_last_error_code = row.last_error.clone();
            self.claims.lock().unwrap().push((now, topic.auction_id));
            claimed.push(topic);
        }
        Box::pin(async move { Ok(claimed) })
    }

    fn reply_commands_open(&self, _auction_id: AuctionId) -> ApplicationFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }

    fn lot_targets(&self, _auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>> {
        Box::pin(async {
            Ok(vec![ForumLotTarget { code: "A01".to_owned(), lot_id: LotId::new() }])
        })
    }

    fn resolve_bidder(
        &self,
        _world: SettlementWorld,
        _external_account_id: String,
        _player_name: String,
    ) -> ApplicationFuture<'_, ForumBidderIdentity> {
        unavailable()
    }

    fn active_proxy(
        &self,
        _lot_id: LotId,
        _user_id: UserId,
    ) -> ApplicationFuture<'_, Option<ForumActiveProxy>> {
        unavailable()
    }

    fn begin_command(
        &self,
        _record: ForumBidCommandRecord,
    ) -> ApplicationFuture<'_, ForumCommandStart> {
        unavailable()
    }

    fn subscribe_notify(
        &self,
        _command_id: Uuid,
        _auction_id: AuctionId,
        _lot_id: LotId,
        _user_id: UserId,
        _post_id: String,
        _lot_code: String,
    ) -> ApplicationFuture<'_, NotifySubscription> {
        unavailable()
    }

    fn finish_command(
        &self,
        _command_id: Uuid,
        _status: String,
        _error_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn command_outcomes(
        &self,
        auction_id: AuctionId,
        _thread_id: String,
    ) -> ApplicationFuture<'_, Vec<ForumCommandOutcome>> {
        // 原帖已不在本轮读取范围内的 pending 命令：扫描必须以“仍有待重试记录”失败收口。
        let outcomes = if self.pending.contains(&auction_id) {
            vec![ForumCommandOutcome {
                post_id: "9000001".to_owned(),
                post_number: None,
                line_number: 1,
                lot_code: "A01".to_owned(),
                command_kind: "bid".to_owned(),
                status: ForumCommandStatus::Pending,
                error_code: None,
                safe_detail: None,
            }]
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(outcomes) })
    }

    fn reply_cursor(
        &self,
        _auction_id: AuctionId,
        _thread_id: String,
    ) -> ApplicationFuture<'_, Option<ForumTopicCursor>> {
        Box::pin(async { Ok(None) })
    }

    fn earliest_live_lot_deadline(
        &self,
        _auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>> {
        let deadline = self.earliest_deadline;
        Box::pin(async move { Ok(Some(deadline)) })
    }

    fn load_current_snapshot(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, ForumAuctionSnapshot> {
        Box::pin(async move {
            Ok(ForumAuctionSnapshot {
                auction_id,
                catalog_layout: ForumCatalogLayout::PagedV1,
                world: SettlementWorld::Main,
                sequence: 1,
                state_version: 1,
                state: AuctionStatus::Final,
                scheduled_open_at: base_time(),
                opened_at: None,
                scheduled_end_at: None,
                public_url: None,
                lots: Vec::new(),
            })
        })
    }

    fn record_fixed_posts_update(
        &self,
        topic: ForumReplyTopic,
        _catalog: ForumRenderedThread,
        _system_log: ForumRenderedPost,
        _processed_through_post_number: i32,
        _updated_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        self.release(topic.auction_id, next_scan_at, None);
        Box::pin(async { Ok(()) })
    }

    fn record_fixed_posts_progress(
        &self,
        _topic: ForumReplyTopic,
        _catalog: ForumRenderedThread,
        _system_log: ForumRenderedPost,
        _processed_through_post_number: i32,
        _updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        // 部分进度保留当前 claim；随后的失败处置负责释放并写入重试水位。
        Box::pin(async { Ok(()) })
    }

    fn record_topic_page_count(
        &self,
        _auction_id: AuctionId,
        _page_count: u32,
        _page_limit: u32,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn finish_topic_scan(
        &self,
        topic: ForumReplyTopic,
        _processed_through_post_number: i32,
        _scanned_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        self.release(topic.auction_id, next_scan_at, None);
        Box::pin(async { Ok(()) })
    }

    fn fail_topic_scan(
        &self,
        topic: ForumReplyTopic,
        _failed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        if self.unrecordable.lock().unwrap().contains(&topic.auction_id) {
            return unavailable();
        }
        self.release(topic.auction_id, retry_at, Some(failure_code));
        Box::pin(async { Ok(()) })
    }
}

/// 单个主题的远端读取行为：先等待（模拟多页读取与请求间隔），再成功或以可重试错误失败。
#[derive(Clone, Copy)]
struct ReadBehavior {
    delay: Duration,
    fail: bool,
}

/// 按线程 ID（即拍卖 ID）返回固定楼层完整的主题；没有用户回帖。
#[derive(Default)]
struct FakeTopicReader {
    behaviors: HashMap<String, ReadBehavior>,
}

impl ForumTopicReader for FakeTopicReader {
    fn fetch_topic(
        &self,
        _world: SettlementWorld,
        thread_id: String,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        let behavior = self
            .behaviors
            .get(&thread_id)
            .copied()
            .unwrap_or(ReadBehavior { delay: Duration::ZERO, fail: false });
        Box::pin(async move {
            tokio::time::sleep(behavior.delay).await;
            if behavior.fail {
                return Err(ForumRemoteError::new("http_status", true, false).into());
            }
            let auction_id = AuctionId::from_uuid(Uuid::parse_str(&thread_id).unwrap());
            Ok(preview(auction_id))
        })
    }

    /// 替身不做增量：忽略游标完整读取，楼层号从 #2 起连续编号。
    fn fetch_topic_since(
        &self,
        world: SettlementWorld,
        thread_id: String,
        _cursor: Option<ForumTopicCursor>,
        _fixed_through_post_number: u32,
    ) -> ApplicationFuture<'_, ForumTopicScan> {
        Box::pin(async move {
            let preview = self.fetch_topic(world, thread_id).await?;
            let post_numbers = (2..).take(preview.posts.len()).collect();
            Ok(ForumTopicScan {
                preview,
                post_numbers,
                mode: ForumTopicScanMode::Full,
                page_count: None,
                page_limit: None,
            })
        })
    }
}

fn preview(auction_id: AuctionId) -> AdminForumPreviewResult {
    AdminForumPreviewResult {
        catalog_post_id: CATALOG_POST_ID.to_owned(),
        catalog_post_text: forum_catalog_marker(auction_id),
        catalog: AdminForumCatalog {
            categories: Vec::new(),
            lots: vec![AdminForumLot {
                code: "A01".to_owned(),
                category: "One-Handed".to_owned(),
                description: "Fixture lot".to_owned(),
                seller: None,
                start_price: 1_000,
                material: false,
                current_bidder: None,
                current_price: None,
            }],
            warnings: Vec::new(),
        },
        posts: vec![AdminForumBidPost {
            post: AdminForumPost {
                id: SYSTEM_LOG_POST_ID.to_owned(),
                author_id: None,
                author: None,
                text: forum_system_log_marker(auction_id),
                edited: false,
            },
            commands: Vec::new(),
            warnings: Vec::new(),
            ignored: false,
        }],
        warnings: Vec::new(),
    }
}

/// 渲染结果与已发布正文一致，固定楼层不产生远端写入。
struct UnchangedRenderer;

impl ForumRenderer for UnchangedRenderer {
    fn render(
        &self,
        _snapshot: ForumAuctionSnapshot,
        _equipment_links: Vec<PublicEquipmentGameLink>,
    ) -> Result<ForumRenderedThread, ForumRenderError> {
        Ok(ForumRenderedThread {
            title: TITLE.to_owned(),
            description: String::new(),
            reconciliation_titles: Vec::new(),
            body: CATALOG_BODY.to_owned(),
            catalog_continuation: Vec::new(),
        })
    }

    fn render_system_log(
        &self,
        _auction_id: AuctionId,
        _catalog_page_count: usize,
        _processed_through_post_number: usize,
        _entries: Vec<ForumSystemLogEntry>,
    ) -> Result<ForumRenderedPost, ForumRenderError> {
        Ok(ForumRenderedPost { body: SYSTEM_LOG_BODY.to_owned() })
    }
}

/// 写入、竞价、链接与凭据的惰性替身；本组用例的路径不会写远端或执行竞价。
struct InertForumPorts;

impl ForumPostWriter for InertForumPorts {
    fn reply_topic(
        &self,
        _world: SettlementWorld,
        _thread_id: String,
        _body: ForumRenderedPost,
        _progress: ForumWriteProgress,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        unavailable()
    }

    fn edit_post(
        &self,
        _world: SettlementWorld,
        _thread_id: String,
        _post_id: String,
        _metadata: Option<ForumTopicMetadata>,
        _body: ForumRenderedPost,
        _progress: ForumWriteProgress,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        unavailable()
    }

    fn read_post(
        &self,
        _world: SettlementWorld,
        _thread_id: String,
        _post_id: String,
    ) -> ApplicationFuture<'_, ForumReadPost> {
        unavailable()
    }
}

impl PublicEquipmentLinkResolver for InertForumPorts {
    fn resolve_public_equipment_links<'a>(
        &'a self,
        _targets: &'a [PublicEquipmentLinkTarget],
    ) -> ApplicationFuture<'a, Vec<PublicEquipmentGameLink>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl CompetitionCommandPort for InertForumPorts {
    fn place_bid(
        &self,
        _command: PlaceBidCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        unavailable()
    }

    fn create_proxy(
        &self,
        _command: CreateProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        unavailable()
    }

    fn lower_proxy(
        &self,
        _command: LowerProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome> {
        unavailable()
    }
}

impl ForumCredentialAvailability for InertForumPorts {
    fn has_available_forum_credential(&self, _world: SettlementWorld) -> bool {
        true
    }
}

fn row(topic: ForumReplyTopic, next_scan_at: DateTime<Utc>) -> TopicRow {
    TopicRow { topic, next_scan_at, leased: false, last_error: None, hidden: false }
}

fn reply_worker(
    store: Arc<FakeReplyStore>,
    reader: FakeTopicReader,
) -> Arc<PersistentWorker<PausedClock>> {
    let ports = Arc::new(InertForumPorts);
    let runtime = ForumReplyRuntime::new(
        ForumReplyPorts {
            store,
            credential_availability: ports.clone(),
            reader: Arc::new(reader),
            writer: ports.clone(),
            renderer: Arc::new(UnchangedRenderer),
            equipment_links: ports.clone(),
            commands: ports,
        },
        10,
        Duration::from_secs(60),
        None,
    )
    .unwrap();
    let clock = Arc::new(PausedClock::new(base_time()));
    Arc::new(PersistentWorker { forum_reply: Some(runtime), ..worker(clock) })
}

async fn run_reply_cycle(
    worker: &PersistentWorker<PausedClock>,
) -> super::cycle::ForumReplyCycleOutcome {
    run_reply_cycle_with(worker, &mut FinalScanSchedule::default()).await
}

/// 与论坛循环一样跨轮复用同一份 Final 排期。
async fn run_reply_cycle_with(
    worker: &PersistentWorker<PausedClock>,
    final_scans: &mut FinalScanSchedule,
) -> super::cycle::ForumReplyCycleOutcome {
    let runtime = worker.forum_reply.as_ref().unwrap();
    run_cycle(worker, runtime, LEASE, POLL_INTERVAL, Some(worker.hot.snapshot().forum), final_scans)
        .await
}

/// 排期相同时按拍卖 ID 领取：第一个总是先于第二个。
fn ordered_auction_ids() -> (AuctionId, AuctionId) {
    (AuctionId::from_uuid(Uuid::from_u128(1)), AuctionId::from_uuid(Uuid::from_u128(2)))
}

/// `ordered_auction_ids` 的两个 Final 主题，第一个的远端读取总是失败。首轮两者都按 Final
/// 间隔排到 +10 秒，到期后总是先领取第一个。
fn failing_first_worker() -> (Arc<FakeReplyStore>, Arc<PersistentWorker<PausedClock>>) {
    let (failing, waiting) = ordered_auction_ids();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(failing, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(waiting, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            failing.to_string(),
            ReadBehavior { delay: Duration::ZERO, fail: true },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);
    (store, worker)
}

/// 在暂停时间里运行论坛循环 `run_for`，其间可在 `act_after` 时修改存储状态。
async fn run_forum_loop(
    worker: Arc<PersistentWorker<PausedClock>>,
    act_after: Duration,
    act: impl FnOnce(),
    run_for: Duration,
) {
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let forum_loop = tokio::spawn(crate::worker::forum::run_loop(worker, receiver));
    tokio::time::sleep(act_after).await;
    act();
    tokio::time::sleep(run_for - act_after).await;
    shutdown.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), forum_loop).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn duplicate_claim_keeps_final_cadence_when_another_topic_runs_long() {
    let first = AuctionId::new();
    let slow = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(first, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(slow, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    // 第二个主题读取 15 秒，超过第一个主题 10 秒的 Final 间隔，同轮会重领到第一个主题。
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            slow.to_string(),
            ReadBehavior { delay: Duration::from_secs(15), fail: false },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);

    let outcome = run_reply_cycle(&worker).await;

    assert!(outcome.result.is_ok());
    let now = at(Duration::from_secs(15));
    assert_eq!(worker.clock.now(), now);
    assert_eq!(
        store.claims().iter().filter(|(_, auction_id)| *auction_id == first).count(),
        2,
        "第一个主题应在同轮被重复领取一次"
    );
    // 重领时剩余 45 秒，Final 间隔 7.5 秒；修复前会被推到常规轮询之后（+300 秒）。
    let (first_next, first_error) = store.persisted(first);
    assert_eq!(first_next, now + chrono::Duration::milliseconds(7_500));
    assert!(first_next <= now + chrono::Duration::seconds(10));
    assert_eq!(first_error, None);
    assert_eq!(outcome.earliest_final_scan, Some(Duration::from_millis(7_500)));
}

#[tokio::test(start_paused = true)]
async fn failed_topic_keeps_final_hint_of_other_topics() {
    let healthy = AuctionId::new();
    let failing = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(healthy, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(failing, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            failing.to_string(),
            ReadBehavior { delay: Duration::ZERO, fail: true },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);

    let outcome = run_reply_cycle(&worker).await;

    // 修复前任一主题失败即整体返回 Err，健康主题的提前唤醒被丢弃。
    assert!(outcome.result.is_err());
    assert_eq!(outcome.earliest_final_scan, Some(Duration::from_secs(10)));
    assert_eq!(store.persisted(healthy), (at(Duration::from_secs(10)), None));
    assert_eq!(
        store.persisted(failing),
        (at(Duration::from_secs(10)), Some("http_status".to_owned()))
    );
}

#[tokio::test(start_paused = true)]
async fn forum_loop_wakes_for_final_scan_even_after_topic_failure() {
    let healthy = AuctionId::new();
    let failing = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(healthy, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(failing, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            failing.to_string(),
            ReadBehavior { delay: Duration::ZERO, fail: true },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);

    run_forum_loop(worker, Duration::ZERO, || {}, Duration::from_secs(30)).await;

    // 第二轮在 Final 间隔（10 秒）后开始；修复前要等满常规轮询 300 秒。
    let second_round = store
        .claims()
        .iter()
        .filter(|(claimed_at, _)| *claimed_at > base_time())
        .map(|(claimed_at, _)| *claimed_at)
        .min();
    assert_eq!(second_round, Some(at(Duration::from_secs(10))));
}

#[tokio::test(start_paused = true)]
async fn forum_loop_keeps_waking_for_final_topic_after_another_leaves_final() {
    let leaving = AuctionId::new();
    let staying = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(leaving, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(staying, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    // 第二个主题读取 3 秒：首轮后两者排期错开（+10 秒与 +12.5 秒）。
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            staying.to_string(),
            ReadBehavior { delay: Duration::from_secs(3), fail: false },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);

    // 第一个拍卖在第二轮之前离开 Final；第二轮只领到它，本轮不再产生 Final 提示。
    let leaving_store = store.clone();
    run_forum_loop(
        worker,
        Duration::from_secs(5),
        move || {
            leaving_store.update(leaving, |row| row.topic.reply_phase = ForumReplyPhase::Unknown);
        },
        Duration::from_secs(60),
    )
    .await;

    let claims = store.claims();
    assert!(claims.contains(&(at(Duration::from_secs(10)), leaving)));
    // 仍在 Final 的主题按自己的排期被领取；修复前要等满常规轮询 300 秒。
    assert!(
        claims.contains(&(at(Duration::from_millis(12_500)), staying)),
        "仍在 Final 的主题必须按自己的排期领取: {claims:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn forum_loop_drops_final_schedule_of_unclaimable_topic_instead_of_spinning() {
    let auction_id = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![row(topic(auction_id, ForumReplyPhase::Final), before(Duration::from_secs(1)))],
        Vec::new(),
    ));
    let worker = reply_worker(store.clone(), FakeTopicReader::default());

    // 首轮排期 +10 秒后，拍卖离开可扫描状态，到点领取不到它。
    let hidden_store = store.clone();
    run_forum_loop(
        worker,
        Duration::from_secs(5),
        move || hidden_store.update(auction_id, |row| row.hidden = true),
        Duration::from_secs(60),
    )
    .await;

    // 首轮两次领取（第二次为空），到点空领取一次后丢弃作废排期，不按 1 秒下限空转。
    assert_eq!(store.attempts(), vec![base_time(), base_time(), at(Duration::from_secs(10))]);
}

#[tokio::test(start_paused = true)]
async fn final_retry_uses_final_interval_but_write_unknown_keeps_poll_interval() {
    let pending = AuctionId::new();
    let unknown = AuctionId::new();
    let normal = AuctionId::new();
    let mut unknown_row =
        row(topic(unknown, ForumReplyPhase::Final), before(Duration::from_secs(2)));
    unknown_row.last_error = Some("write_unknown".to_owned());
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(pending, ForumReplyPhase::Final), before(Duration::from_secs(3))),
            unknown_row,
            row(topic(normal, ForumReplyPhase::Normal), before(Duration::from_secs(1))),
        ],
        vec![pending, normal],
    ));
    let reader = FakeTopicReader {
        behaviors: HashMap::from([(
            unknown.to_string(),
            ReadBehavior { delay: Duration::ZERO, fail: true },
        )]),
    };
    let worker = reply_worker(store.clone(), reader);

    let outcome = run_reply_cycle(&worker).await;

    assert!(outcome.result.is_err());
    // pending 重试沿用 Final 间隔；修复前固定为常规轮询。
    assert_eq!(
        store.persisted(pending),
        (at(Duration::from_secs(10)), Some("topic_processing_failed".to_owned()))
    );
    // 未知写入维持常规轮询，失败码不被后续读取错误覆盖。
    assert_eq!(store.persisted(unknown), (at(POLL_INTERVAL), Some("write_unknown".to_owned())));
    // 非 Final 阶段的失败保持常规轮询。
    assert_eq!(
        store.persisted(normal),
        (at(POLL_INTERVAL), Some("topic_processing_failed".to_owned()))
    );
    assert_eq!(outcome.earliest_final_scan, Some(Duration::from_secs(10)));
}

#[tokio::test(start_paused = true)]
async fn round_aborted_after_claim_keeps_due_final_scans_of_other_topics() {
    let (failing, waiting) = ordered_auction_ids();
    let (store, worker) = failing_first_worker();
    let mut final_scans = FinalScanSchedule::default();

    let first = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert_eq!(first.earliest_final_scan, Some(Duration::from_secs(10)));

    // 第二轮先领到第一个主题，其失败处置写入失败（例如连接池 acquire 超时），本轮出错退出；
    // 同样到期的第二个主题只是按排序没轮到。
    store.make_unrecordable(failing);
    tokio::time::sleep(Duration::from_secs(10)).await;
    let second = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert!(second.result.is_err());
    let claims = store.claims();
    assert!(claims.contains(&(at(Duration::from_secs(10)), failing)));
    assert!(!claims.contains(&(at(Duration::from_secs(10)), waiting)));
    // 修复前按最后一次领取时刻裁掉了第二个主题的排期，论坛循环退回常规轮询（300 秒）。
    assert_eq!(second.earliest_final_scan, Some(Duration::ZERO));

    // 下一轮领到第二个主题；第一个主题的租约仍被持有，空领取后裁掉它的作废排期，不再按下限
    // 空转。剩余 48 秒，Final 间隔 8 秒。
    tokio::time::sleep(Duration::from_secs(2)).await;
    let third = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert!(third.result.is_ok());
    assert!(store.claims().contains(&(at(Duration::from_secs(12)), waiting)));
    assert_eq!(third.earliest_final_scan, Some(Duration::from_secs(8)));
}

#[tokio::test(start_paused = true)]
async fn forum_loop_claims_waiting_final_topic_soon_after_round_aborts() {
    let (failing, waiting) = ordered_auction_ids();
    let (store, worker) = failing_first_worker();

    let unrecordable_store = store.clone();
    run_forum_loop(
        worker,
        Duration::from_secs(5),
        move || unrecordable_store.make_unrecordable(failing),
        Duration::from_secs(30),
    )
    .await;

    // 第二轮（+10 秒）在第一个主题上出错退出；第二个主题在 1 秒下限后领取，修复前要等满
    // 常规轮询 300 秒。
    let claims = store.claims();
    assert!(
        claims.contains(&(at(Duration::from_secs(11)), waiting)),
        "第二个主题必须在 1 秒下限后领取: {claims:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn failed_claim_keeps_due_final_scans_and_retries_after_minimum_interval() {
    let first = AuctionId::new();
    let second = AuctionId::new();
    let store = Arc::new(FakeReplyStore::new(
        vec![
            row(topic(first, ForumReplyPhase::Final), before(Duration::from_secs(2))),
            row(topic(second, ForumReplyPhase::Final), before(Duration::from_secs(1))),
        ],
        Vec::new(),
    ));
    let worker = reply_worker(store.clone(), FakeTopicReader::default());
    let mut final_scans = FinalScanSchedule::default();

    let healthy = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert_eq!(healthy.earliest_final_scan, Some(Duration::from_secs(10)));

    // 到期时领取调用本身失败（例如连接池 acquire 超时），无法判断主题是否仍可领取。
    store.set_claims_fail(true);
    tokio::time::sleep(Duration::from_secs(10)).await;
    let failed = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert!(failed.result.is_err());
    // 保留已到期排期并按 Final 最短间隔重试：既不丢弃后退回常规轮询，也不按 1 秒下限空转。
    assert_eq!(failed.earliest_final_scan, Some(Duration::from_secs(5)));

    // 领取恢复后，下一轮照常领取两个主题。
    store.set_claims_fail(false);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let recovered = run_reply_cycle_with(&worker, &mut final_scans).await;
    assert!(recovered.result.is_ok());
    let claims = store.claims();
    assert!(claims.contains(&(at(Duration::from_secs(15)), first)));
    assert!(claims.contains(&(at(Duration::from_secs(15)), second)));
}
