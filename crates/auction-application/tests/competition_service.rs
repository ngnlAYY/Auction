//! 竞价用例服务的编排回归：资格、截止、慢速代理围栏、命令账本冲突与并发重复写入。
//!
//! 内存替身实现事务 runner：每次事务在共享状态的副本上执行，只有成功时才提交，失败时
//! 丢弃暂存写入。替身不重建存储语义，只记录服务交给端口的写入，以断言服务在失败路径上
//! 不写任何事实、在成功路径上按“前置冻结 → 主计划”的顺序原子提交（不变量 1、2、3）。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::future::ready;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use auction_application::auction::{
    CompetitionService, CreateProxyCommand, ExecuteSlowProxyCommand, LowerProxyCommand,
    PLACE_BID_COMMAND_KIND, PlaceBidCommand, SLOW_PROXY_COMMAND_KIND, SlowProxyDue,
};
use auction_application::core::{
    ApplicationError, CommandInsert, CommandLedgerError, CompetitionCommandAuditContext,
    CompetitionCommandAuditKind, CompetitionProxyTransitionRecord, CompetitionSnapshot,
    CompetitionTransaction, CompetitionTransactionError, ProxyUsageContext, RequestHash,
    SlowProxyExecutionContext, StoredCommand, TransactionFailure, TransactionFuture,
    TransactionPolicy, TransactionRunner,
};
use auction_domain::{
    AuctionRules, AuctionStatus, BidAmount, BidMechanism, CommandId, CompetitionPlan,
    DEFAULT_START_PRICE, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyTerminalReason, ProxyTransitionKind, UserId,
};
use auction_time::Timestamp;

#[derive(Clone)]
struct World {
    ledger: HashMap<CommandId, StoredCommand>,
    snapshot: CompetitionSnapshot,
    ineligible: Vec<UserId>,
    now: Timestamp,
    history: Vec<CompetitionProxyTransitionRecord>,
    /// 已提交的主计划写入及其审计上下文，按调用顺序记录。
    applied: Vec<(CompetitionPlan, Option<CompetitionCommandAuditContext>)>,
    slow_applied: Vec<(CompetitionPlan, SlowProxyExecutionContext)>,
    /// 模拟并发事务已提交同一命令：查找时不可见，插入时报告重复。
    concurrent: Option<StoredCommand>,
}

struct FakeTransaction {
    staged: World,
}

struct FakeRunner {
    world: Arc<Mutex<World>>,
}

impl TransactionRunner for FakeRunner {
    type Transaction = FakeTransaction;

    fn run<T, E, F>(
        &self,
        _policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        let world = Arc::clone(&self.world);
        async move {
            let staged = world.lock().unwrap().clone();
            let mut transaction = FakeTransaction { staged };
            let result = operation(&mut transaction).await;
            if result.is_ok() {
                *world.lock().unwrap() = transaction.staged;
            }
            result
        }
    }
}

impl CompetitionTransaction for FakeTransaction {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send
    {
        ready(Ok(self.staged.ledger.get(&command_id).cloned()))
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<CommandInsert, CompetitionTransactionError>> + Send {
        let inserted = match self.staged.concurrent.take() {
            Some(stored) => {
                self.staged.ledger.insert(stored.command_id, stored.clone());
                CommandInsert::Duplicate(stored)
            }
            None => {
                self.staged.ledger.insert(command.command_id, command);
                CommandInsert::Inserted
            }
        };
        ready(Ok(inserted))
    }

    fn current_time(
        &mut self,
    ) -> impl Future<Output = Result<Timestamp, CompetitionTransactionError>> + Send {
        ready(Ok(self.staged.now))
    }

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send {
        let snapshot = &self.staged.snapshot;
        ready(if snapshot.state.lot_id == lot_id {
            Ok(snapshot.clone())
        } else {
            Err(CompetitionTransactionError::NotFound)
        })
    }

    fn participant_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        ready(Ok(!self.staged.ineligible.contains(&user_id)))
    }

    fn proxy_usage_context(
        &mut self,
        _lot_id: LotId,
        _user_id: UserId,
    ) -> impl Future<Output = Result<ProxyUsageContext, CompetitionTransactionError>> + Send {
        ready(Ok(ProxyUsageContext {
            usage_number: 1,
            previous_maximum: None,
            previous_status: None,
        }))
    }

    fn proxy_transition_history(
        &mut self,
        _authorization_id: ProxyAuthorizationId,
    ) -> impl Future<
        Output = Result<Vec<CompetitionProxyTransitionRecord>, CompetitionTransactionError>,
    > + Send {
        ready(Ok(self.staged.history.clone()))
    }

    fn proxy_quota_limit(
        &mut self,
        _lot_id: LotId,
    ) -> impl Future<Output = Result<u32, CompetitionTransactionError>> + Send {
        ready(Ok(self.staged.snapshot.rules.proxy_max_outbid_uses))
    }

    fn apply_plan(
        &mut self,
        plan: &CompetitionPlan,
        audit: Option<CompetitionCommandAuditContext>,
        _occurred_at: Timestamp,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        self.staged.applied.push((plan.clone(), audit));
        ready(Ok(()))
    }

    fn apply_slow_proxy_plan(
        &mut self,
        plan: &CompetitionPlan,
        context: SlowProxyExecutionContext,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        self.staged.slow_applied.push((plan.clone(), context));
        ready(Ok(()))
    }
}

/// 极简阻塞执行器；替身的 Future 都立即就绪，不依赖 tokio。
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

fn at(hour: u32, minute: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 1, 1, hour, minute, 0).unwrap()
}

fn amount(value: i64) -> BidAmount {
    BidAmount::new(value).unwrap()
}

fn proxy(lot_id: LotId, user_id: UserId, maximum: i64, mode: ProxyMode) -> ProxyAuthorization {
    ProxyAuthorization::new(
        ProxyAuthorizationId::new(),
        lot_id,
        user_id,
        CommandId::new(),
        1,
        1,
        amount(maximum),
        mode,
    )
    .unwrap()
}

/// Live 场次中 `leader` 以 `current` 领先、有效截止 12:00、当前数据库时间 10:00。
fn world(
    lot_id: LotId,
    current: i64,
    leader: UserId,
    active_proxy: Option<ProxyAuthorization>,
) -> World {
    World {
        ledger: HashMap::new(),
        snapshot: CompetitionSnapshot {
            rules: AuctionRules::default(),
            auction_status: AuctionStatus::Live,
            state: LotBidState {
                lot_id,
                start_price: amount(DEFAULT_START_PRICE),
                current_price: Some(amount(current)),
                leader_id: Some(leader),
                leader_proxy_last: false,
                material: false,
                effective_end_at: at(12, 0),
                state_version: 1,
                last_proxy_sequence: 1,
                slow_generation: 0,
                slow_authorization_id: None,
                slow_due_at: None,
            },
            active_proxy,
        },
        ineligible: Vec::new(),
        now: at(10, 0),
        history: Vec::new(),
        applied: Vec::new(),
        slow_applied: Vec::new(),
        concurrent: None,
    }
}

/// 慢速代理 `holder` 被 `rival` 以 120,000 超越，第 1 代调度在 10:10 到期。
fn waiting_world(lot_id: LotId, holder: UserId, rival: UserId) -> (World, ProxyAuthorization) {
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let mut world = world(lot_id, 120_000, rival, Some(waiting));
    world.snapshot.state.slow_generation = 1;
    world.snapshot.state.slow_authorization_id = Some(waiting.id);
    world.snapshot.state.slow_due_at = Some(at(10, 10));
    world.now = at(10, 10);
    (world, waiting)
}

fn service(world: World) -> (CompetitionService<FakeRunner>, Arc<Mutex<World>>) {
    let shared = Arc::new(Mutex::new(world));
    (CompetitionService::new(Arc::new(FakeRunner { world: Arc::clone(&shared) })), shared)
}

fn place_bid(command_id: CommandId, lot_id: LotId, bidder: UserId, value: i64) -> PlaceBidCommand {
    PlaceBidCommand {
        command_id,
        lot_id,
        bidder_id: bidder,
        amount: amount(value),
        request_hash: PlaceBidCommand::legacy_request_hash(lot_id, bidder, amount(value)).unwrap(),
        request_id: None,
    }
}

fn slow_command(
    lot_id: LotId,
    authorization_id: ProxyAuthorizationId,
    generation: u64,
    due_at: Timestamp,
) -> ExecuteSlowProxyCommand {
    SlowProxyDue { lot_id, authorization_id, generation, due_at }.into_command().unwrap()
}

fn competition_error(result: Result<impl std::fmt::Debug, ApplicationError>) -> String {
    match result {
        Err(ApplicationError::CompetitionTransaction(error)) => format!("{error:?}"),
        other => panic!("期望竞价事务错误: {other:?}"),
    }
}

fn assert_no_writes(world: &Arc<Mutex<World>>) {
    let world = world.lock().unwrap();
    assert!(world.ledger.is_empty(), "失败路径不得写命令账本");
    assert!(world.applied.is_empty(), "失败路径不得应用竞价计划");
    assert!(world.slow_applied.is_empty(), "失败路径不得应用慢速代理计划");
}

#[test]
fn ineligible_bidder_is_rejected_without_writes() {
    let lot_id = LotId::new();
    let (leader, bidder) = (UserId::new(), UserId::new());
    let mut initial = world(lot_id, 100_000, leader, None);
    initial.ineligible.push(bidder);
    let (service, shared) = service(initial);

    let result = block_on(service.place_bid(place_bid(CommandId::new(), lot_id, bidder, 105_000)));

    assert_eq!(competition_error(result), "BidderIneligible");
    assert_no_writes(&shared);
}

#[test]
fn ineligible_proxy_holder_is_frozen_before_the_bid_under_the_same_command() {
    let lot_id = LotId::new();
    let (holder, bidder) = (UserId::new(), UserId::new());
    let active = proxy(lot_id, holder, 150_000, ProxyMode::Fast);
    let mut initial = world(lot_id, 100_000, holder, Some(active));
    initial.ineligible.push(holder);
    let (service, shared) = service(initial);
    let command_id = CommandId::new();

    let outcome = block_on(service.place_bid(place_bid(command_id, lot_id, bidder, 105_000)))
        .expect("持有人失格不影响出价人的合法出价");

    let world = shared.lock().unwrap();
    assert_eq!(world.applied.len(), 2, "先冻结失格代理，再应用主计划");
    let (freeze, freeze_audit) = &world.applied[0];
    assert!(freeze.public_bids.is_empty());
    assert_eq!(freeze.proxy_transitions.len(), 1);
    assert_eq!(freeze.proxy_transitions[0].authorization_id, active.id);
    assert_eq!(freeze.proxy_transitions[0].kind, ProxyTransitionKind::Frozen);
    assert_eq!(freeze.proxy_transitions[0].reason, Some(ProxyTerminalReason::EligibilityLost));
    assert_eq!(freeze.proxy_transitions[0].command_id, command_id);
    assert_eq!(*freeze_audit, None, "前置冻结不写命令级审计");

    // 冻结后的代理不再反击，出价人以普通出价领先。
    let (main, main_audit) = &world.applied[1];
    assert_eq!(main.public_bids.len(), 1);
    assert_eq!(main.public_bids[0].bidder_id, bidder);
    assert_eq!(main.public_bids[0].amount, amount(105_000));
    assert_eq!(main.public_bids[0].mechanism, BidMechanism::Ordinary);
    assert_eq!(main.active_proxy_id, None);
    let audit = main_audit.expect("主计划必须携带命令级审计");
    assert_eq!(audit.command_id, command_id);
    assert_eq!(audit.actor_id, bidder);
    assert_eq!(audit.kind, CompetitionCommandAuditKind::PlaceBid);
    assert_eq!(outcome.plan, *main);
    assert_eq!(world.ledger.len(), 1);
    assert_eq!(world.ledger[&command_id].command_kind, PLACE_BID_COMMAND_KIND);
}

#[test]
fn bids_at_the_effective_end_are_rejected_without_writes() {
    let lot_id = LotId::new();
    let (leader, bidder) = (UserId::new(), UserId::new());
    let mut initial = world(lot_id, 100_000, leader, None);
    initial.now = at(12, 0);
    let (service, shared) = service(initial);

    let result = block_on(service.place_bid(place_bid(CommandId::new(), lot_id, bidder, 105_000)));

    assert_eq!(competition_error(result), "InvalidState");
    assert_no_writes(&shared);
}

#[test]
fn replays_return_the_original_result_and_conflicting_requests_fail() {
    let lot_id = LotId::new();
    let (leader, bidder) = (UserId::new(), UserId::new());
    let (service, shared) = service(world(lot_id, 100_000, leader, None));
    let command_id = CommandId::new();

    let first = block_on(service.place_bid(place_bid(command_id, lot_id, bidder, 105_000)))
        .expect("首次出价必须成功");
    let replayed = block_on(service.place_bid(place_bid(command_id, lot_id, bidder, 105_000)))
        .expect("同 ID 同请求必须返回原结果");
    assert_eq!(replayed.plan.public_bids, first.plan.public_bids);
    assert_eq!(replayed.effective_end_at, first.effective_end_at);

    // 同一命令 ID 换了金额就是另一份请求：返回冲突，不能覆盖或追加事实。
    let conflict = block_on(service.place_bid(place_bid(command_id, lot_id, bidder, 110_000)));
    assert!(matches!(conflict, Err(ApplicationError::CommandLedger(CommandLedgerError::Conflict))));

    let world = shared.lock().unwrap();
    assert_eq!(world.applied.len(), 1, "重放与冲突都不得再次应用计划");
    assert_eq!(world.ledger.len(), 1);
}

#[test]
fn concurrent_duplicate_insert_replays_the_committed_result() {
    let lot_id = LotId::new();
    let (leader, bidder) = (UserId::new(), UserId::new());
    let command_id = CommandId::new();
    let command = || place_bid(command_id, lot_id, bidder, 105_000);

    // 先在另一份状态中提交同一命令，取得并发事务已写入的账本记录。
    let (committed_service, committed) = service(world(lot_id, 100_000, leader, None));
    let first = block_on(committed_service.place_bid(command())).expect("并发事务必须成功");
    let stored = committed.lock().unwrap().ledger[&command_id].clone();

    let mut racing = world(lot_id, 100_000, leader, None);
    racing.concurrent = Some(stored);
    let (service, shared) = service(racing);
    let replayed = block_on(service.place_bid(command())).expect("重复插入必须按账本重放");

    assert_eq!(replayed.plan.public_bids, first.plan.public_bids);
    assert!(shared.lock().unwrap().applied.is_empty(), "重复插入后不得再应用本事务计划");
}

#[test]
fn proxy_commands_record_their_audit_kind_and_replay_without_reapplying() {
    let lot_id = LotId::new();
    let (leader, bidder) = (UserId::new(), UserId::new());
    let (service, shared) = service(world(lot_id, 100_000, leader, None));
    let create = || CreateProxyCommand {
        command_id: CommandId::from_uuid(uuid::Uuid::from_u128(7)),
        lot_id,
        bidder_id: bidder,
        maximum_amount: amount(150_000),
        mode: ProxyMode::Fast,
        request_hash: CreateProxyCommand::legacy_request_hash(
            lot_id,
            bidder,
            ProxyMode::Fast,
            amount(150_000),
        )
        .unwrap(),
        request_id: None,
    };

    let created = block_on(service.create_proxy(create())).expect("新代理必须成功");
    let outcome = created.proxy.as_ref().expect("创建代理必须返回代理响应");
    // 最低可出价 100,000 + 5% = 105,000，代理以该价领先。
    assert_eq!(created.plan.public_bids.len(), 1);
    assert_eq!(created.plan.public_bids[0].amount, amount(105_000));
    assert_eq!(outcome.authorization.user_id, bidder);
    assert_eq!(outcome.quota_used, 1);
    {
        let world = shared.lock().unwrap();
        assert_eq!(world.applied.len(), 1);
        let audit = world.applied[0].1.expect("创建代理必须写命令级审计");
        assert_eq!(audit.kind, CompetitionCommandAuditKind::CreateProxy);
    }

    let replayed = block_on(service.create_proxy(create())).expect("同 ID 同请求必须返回原结果");
    assert_eq!(replayed.plan.public_bids, created.plan.public_bids);
    let replayed_proxy = replayed.proxy.as_ref().expect("重放必须返回代理响应");
    assert_eq!(replayed_proxy.authorization, outcome.authorization);
    assert_eq!(shared.lock().unwrap().applied.len(), 1, "重放不得再次应用计划");
}

#[test]
fn lowering_records_the_new_maximum_in_the_audit() {
    let lot_id = LotId::new();
    let holder = UserId::new();
    let active = proxy(lot_id, holder, 150_000, ProxyMode::Fast);
    let mut initial = world(lot_id, 120_000, holder, Some(active));
    initial.history = vec![CompetitionProxyTransitionRecord {
        transition_number: 1,
        kind: "created".to_owned(),
        old_maximum_amount: None,
        new_maximum_amount: 150_000,
        old_mode: None,
        new_mode: "fast".to_owned(),
        old_status: None,
        new_status: "active".to_owned(),
        reason: None,
        resulting_state_version: 1,
        created_at: at(9, 0),
    }];
    let (service, shared) = service(initial);
    let command = LowerProxyCommand {
        command_id: CommandId::new(),
        lot_id,
        bidder_id: holder,
        authorization_id: active.id,
        new_maximum: amount(130_000),
        request_hash: LowerProxyCommand::legacy_request_hash(
            lot_id,
            holder,
            active.id,
            amount(130_000),
        )
        .unwrap(),
        request_id: None,
    };

    let lowered = block_on(service.lower_proxy(command)).expect("领先时可降到不低于当前价");

    assert!(lowered.plan.public_bids.is_empty());
    let outcome = lowered.proxy.expect("降低代理必须返回代理响应");
    assert_eq!(outcome.authorization.maximum_amount, amount(130_000));
    let world = shared.lock().unwrap();
    let audit = world.applied[0].1.expect("降低代理必须写命令级审计");
    assert_eq!(
        audit.kind,
        CompetitionCommandAuditKind::LowerProxy {
            authorization_id: active.id,
            new_maximum: amount(130_000),
        }
    );
}

#[test]
fn due_slow_proxy_counters_and_records_its_generation() {
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let (initial, waiting) = waiting_world(lot_id, holder, rival);
    let (service, shared) = service(initial);
    let command = slow_command(lot_id, waiting.id, 1, at(10, 10));

    let plan = block_on(service.execute_slow_proxy(command)).expect("到期慢速代理必须反击");

    // 到期重算最低可出价：120,000 + 5% = 126,000。
    assert_eq!(plan.public_bids.len(), 1);
    assert_eq!(plan.public_bids[0].bidder_id, holder);
    assert_eq!(plan.public_bids[0].amount, amount(126_000));
    let world = shared.lock().unwrap();
    assert_eq!(world.slow_applied.len(), 1);
    let (applied, context) = &world.slow_applied[0];
    assert_eq!(*applied, plan);
    assert_eq!(context.generation, 1);
    assert_eq!(context.due_at, at(10, 10));
    assert_eq!(context.executed_at, at(10, 10));
    let stored = &world.ledger[&command.command_id];
    assert_eq!(stored.command_kind, SLOW_PROXY_COMMAND_KIND);
    assert_eq!(stored.result_code, "accepted");
}

#[test]
fn stale_slow_proxy_candidates_are_skipped_without_writes() {
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let (initial, waiting) = waiting_world(lot_id, holder, rival);
    let fast = ProxyAuthorization { mode: ProxyMode::Fast, ..waiting };
    let cases: [(&str, World, ExecuteSlowProxyCommand); 6] = [
        ("世代不符", initial.clone(), slow_command(lot_id, waiting.id, 2, at(10, 10))),
        ("到期时间不符", initial.clone(), slow_command(lot_id, waiting.id, 1, at(10, 9))),
        (
            "授权不符",
            initial.clone(),
            slow_command(lot_id, ProxyAuthorizationId::new(), 1, at(10, 10)),
        ),
        (
            "模式不是慢速",
            World {
                snapshot: CompetitionSnapshot {
                    active_proxy: Some(fast),
                    ..initial.snapshot.clone()
                },
                ..initial.clone()
            },
            slow_command(lot_id, waiting.id, 1, at(10, 10)),
        ),
        (
            "尚未到期",
            World { now: at(10, 9), ..initial.clone() },
            slow_command(lot_id, waiting.id, 1, at(10, 10)),
        ),
        (
            "场次不在 Live",
            World {
                snapshot: CompetitionSnapshot {
                    auction_status: AuctionStatus::Final,
                    ..initial.snapshot.clone()
                },
                ..initial
            },
            slow_command(lot_id, waiting.id, 1, at(10, 10)),
        ),
    ];
    for (label, world, command) in cases {
        let (service, shared) = service(world);
        let result = block_on(service.execute_slow_proxy(command));
        assert_eq!(competition_error(result), "StaleSlowProxy", "{label}");
        assert_no_writes(&shared);
    }
}

#[test]
fn ineligible_slow_proxy_holder_is_frozen_instead_of_countering() {
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let (mut initial, waiting) = waiting_world(lot_id, holder, rival);
    initial.ineligible.push(holder);
    let (service, shared) = service(initial);

    let plan =
        block_on(service.execute_slow_proxy(slow_command(lot_id, waiting.id, 1, at(10, 10))))
            .expect("失格持有人的慢速代理必须被冻结");

    assert!(plan.public_bids.is_empty());
    assert_eq!(plan.proxy_transitions.len(), 1);
    assert_eq!(plan.proxy_transitions[0].kind, ProxyTransitionKind::Frozen);
    assert_eq!(plan.proxy_transitions[0].reason, Some(ProxyTerminalReason::EligibilityLost));
    assert_eq!(plan.active_proxy_id, None);
    assert_eq!(plan.final_lot.slow_authorization_id, None);
    assert_eq!(shared.lock().unwrap().slow_applied.len(), 1);
}

#[test]
fn slow_proxy_request_hash_must_match_the_stored_command() {
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let (initial, waiting) = waiting_world(lot_id, holder, rival);
    let (service, shared) = service(initial);
    let command = slow_command(lot_id, waiting.id, 1, at(10, 10));
    let plan = block_on(service.execute_slow_proxy(command)).expect("首次执行必须成功");

    // 同 ID 同请求返回原计划，不再推进。
    let replayed = block_on(service.execute_slow_proxy(command)).expect("重放必须返回原计划");
    assert_eq!(replayed, plan);
    // 同 ID 不同摘要是冲突。
    let forged = ExecuteSlowProxyCommand { request_hash: RequestHash::new([9; 32]), ..command };
    assert!(matches!(
        block_on(service.execute_slow_proxy(forged)),
        Err(ApplicationError::CommandLedger(CommandLedgerError::Conflict))
    ));
    assert_eq!(shared.lock().unwrap().slow_applied.len(), 1);
}
