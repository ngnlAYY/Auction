//! 处罚接管时既有订单继续履约，以及付款期限到期与付款并发的终局唯一性。
//!
//! 保护的不变量：2（付款认证与未付取消共用 UID 账户锁，恰一个终局）、4（已付款的 Sale 不得按未付取消，
//! 外部写不因处罚被改写）、6（失败关闭）、8（封闭与收据只追加）。夹具只用既有的真实写入口与合法行，
//! 不卸触发器、不放宽约束。

use std::sync::Arc;

use auction_application::consignment::{SanctionWorkOutcome, SanctionWorkRef, SanctionWorkStore};
use auction_application::settlement::{CertifyCod, CodLeg, FundingError};
use auction_domain::SettlementWorld;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::deposit::support::{
    CREDENTIAL, REPLAY, SESSION, USER_1, USER_2, accept_listing, attempt_start_proof_and_custody,
    consume_reservation, custody_event, decide_command, delivery_and_work, initial_and_final,
    item_position_claim, listing_awaiting_deposit, observation_and_asset, reservation_issued,
    succeeds,
};
use super::super::funding::cod_fixture::{certify, ledger};
use super::super::ledger136::fixture::{
    PAY_ATTEMPT, PAY_DELIVERY, PAY_MID, PAY_ROUND, PRICE, PURCHASED_AT, available_sql,
    purchase_sql, refund_dispatch_sql, seed_purchased,
};
use super::super::refusal::fixture::{
    buyer, command, count, seed_base, seed_intakes, seller, submit,
};
use super::unpaid_recovery::{DISPOSITION, disposition_sql, seed_consumption};
use crate::support::TestDatabase;

const UID: &str = "7001";
/// 原付款 Send 严格晚于 Sale 冻结的开户边界（2026-01-02）。
const SENT_AT: &str = "2026-01-03T00:00:00Z";
const ACTIVATION_COMMAND: &str = "c6a30000-0000-4000-8000-000000000001";
const EVIDENCE: &str = "c6a20000-0000-4000-8000-000000000001";

fn uuid(text: &str) -> Uuid {
    text.parse().expect("UUID")
}

async fn closures(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_sale_closures")
        .fetch_one(pool)
        .await
        .expect("封闭数")
}

/// 原件腿（main COD）的真实 Send：激活后的 delivery、Send 尝试、Sent MID 绑定、冻结轮次与尝试、
/// 远端确领证据与履约行，Send 晚于开户边界。
fn original_claim_dispatch_sql() -> String {
    let command = format!("c71{}", &PAY_DELIVERY[3..]);
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, activation_command_id,
             activation_request_digest, activated_at, created_at, updated_at)
         VALUES ('{PAY_DELIVERY}', '{command}', decode(repeat('c1', 32), 'hex'), 'settlement',
                 '{OBLIGATION_1}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '{ACTIVATION_COMMAND}', decode(repeat('c3', 32), 'hex'), '{PURCHASED_AT}',
                 '{PURCHASED_AT}', '{PURCHASED_AT}');
         INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ('{PAY_DELIVERY}', 1, '{command}', decode(repeat('c1', 32), 'hex'), 'main',
                 '{SENT_AT}',
                 jsonb_build_object('world_id', 'main', 'recipient', 'FakeSellerTwo',
                     'subject', 'fake subject', 'body_digest',
                     (SELECT jsonb_agg(209) FROM generate_series(1, 32)), 'credits', 0,
                     'cod', {PRICE}, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb));
         INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version,
             expectation_fencing_version)
         VALUES ('main', '{PAY_MID}', '{PAY_DELIVERY}', 1, 1);
         INSERT INTO public.consignment_dispatch_rounds
            (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
             bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
             prior_round, authorized_by_intake, authorized_by_consumption)
         VALUES ('{CONTRACT}', '{OBLIGATION_1}', 1, '{PAY_ROUND}', '{PAY_DELIVERY}',
                 'FakeSellerTwo', '{UID}', 'main', 'fake subject', 'consignment_cod',
                 decode(repeat('d1', 32), 'hex'), '{PAY_ROUND}', '{PURCHASED_AT}', 0, {PRICE},
                 NULL, NULL, NULL);
         INSERT INTO public.consignment_dispatch_attempts
            (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg, started_at)
         VALUES ('{PAY_DELIVERY}', 1, '{PAY_ATTEMPT}', '{CONTRACT}', '{OBLIGATION_1}', 1,
                 'original', '{SENT_AT}');
         INSERT INTO public.consignment_dispatch_evidence
            (id, attempt_id, category, remote_observation_id, validator_version, digest,
             observed_at)
         VALUES ('{EVIDENCE}', '{PAY_ATTEMPT}', 'original_claim', 'fake-cod-paid-1',
                 'test-signed-off-validator-v1', decode(repeat('e1', 32), 'hex'), '{SENT_AT}');
         UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued', delivery_id = '{PAY_DELIVERY}'
         WHERE obligation_id = '{OBLIGATION_1}';"
    )
}

/// 未付订单：付款期限已到、原件已退回并有“关闭原买方交割”的处置，同时付款认证的全部证据齐备。
async fn seed_due_and_payable(pool: &PgPool) {
    seed_purchased(pool).await;
    succeeds(pool, &original_claim_dispatch_sql()).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    seed_consumption(pool).await;
    succeeds(
        pool,
        &disposition_sql(
            DISPOSITION,
            ("close_at_limit", "held_unpaid_recovered", "manual_or_auto"),
            true,
        ),
    )
    .await;
}

/// 付款认证命令：main 世界原件腿，毛额等于成交价；证据与冻结事实由 [`seed_due_and_payable`] 备齐。
fn payment_command(byte: u8) -> CertifyCod {
    let mut command = certify(byte, None);
    command.world = SettlementWorld::Main;
    command.receipt.leg = CodLeg::Original;
    command.gross = PRICE;
    command
}

/// 与外写相关的计数：投递行数（新增 POST 的前提是新增投递）与已提交的付款认证命令数。
async fn side_effects(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.deliveries),
                (SELECT count(*) FROM public.consignment_commands WHERE operation = 'certify_cod')",
    )
    .fetch_one(pool)
    .await
    .expect("外写计数")
}

/// 付款与未付取消同时到达同一张订单：两条写端都在 UID 账户锁内复验，恰一个终局——
/// 要么付款认证成功且封闭被拒，要么封闭成功且付款认证被拒；绝不既收款又按未付封闭。
#[tokio::test]
async fn postgres_consignment_sanction_orders_due_and_payment_race_has_exactly_one_terminal() {
    for _ in 0..4 {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("维护账号");
        let runtime = Arc::new(database.store().await.expect("运行账号"));
        let pool = owner.postgres_pool();
        seed_due_and_payable(pool).await;
        let (deliveries_before, commands_before) = side_effects(pool).await;

        let (paid, cancelled) = tokio::join!(
            runtime.test_certify_cod(payment_command(2)),
            runtime.test_cancel_unpaid("", uuid(CONTRACT), uuid(DISPOSITION))
        );
        let after = ledger(pool).await;
        let (deliveries_after, commands_after) = side_effects(pool).await;
        assert_eq!(deliveries_after, deliveries_before, "两条写端都不产生新的投递（零远端 POST）");
        match (paid, cancelled) {
            (Ok(receipt), Err(FundingError::Conflict)) => {
                assert!(!receipt.replayed);
                assert_eq!((after.receipts, after.funded), (1, 1), "付款终局：恰一份收据与 funded");
                assert_eq!(closures(pool).await, 0, "已收款的订单不得再按未付封闭");
                assert_eq!(after.payment_state, "satisfied");
                assert_eq!(commands_after, commands_before + 1);
            }
            (Err(error), Ok(true)) => {
                assert!(
                    matches!(error, FundingError::Conflict | FundingError::Unproven),
                    "{error:?}"
                );
                assert_eq!(closures(pool).await, 1, "未付终局：恰一条封闭事实");
                assert_eq!(
                    (after.receipts, after.funded, after.entries, after.sources),
                    (0, 0, 0, 0),
                    "已按未付封闭的订单不得留下任何收款事实"
                );
                assert_ne!(after.payment_state, "satisfied");
                assert_eq!(commands_after, commands_before, "被拒的付款不入账命令");
            }
            (paid, cancelled) => {
                panic!("必须恰一个终局，实得 付款={paid:?} 封闭={cancelled:?}");
            }
        }
    }
}

/// 确定顺序的反例：未付封闭先提交后，迟到的付款认证失败关闭，不留收据、分录、funded 与命令。
#[tokio::test]
async fn postgres_consignment_sanction_orders_payment_after_unpaid_closure_is_refused() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_due_and_payable(pool).await;
    let ledger_before = ledger(pool).await;
    let (deliveries_before, commands_before) = side_effects(pool).await;

    assert!(
        runtime.test_cancel_unpaid("", uuid(CONTRACT), uuid(DISPOSITION)).await.expect("封闭"),
        "未付封闭先到先得"
    );
    let late = runtime.test_certify_cod(payment_command(2)).await;
    // 确定拒绝来自 certify_cod 在资金账户锁后的封闭检查（Conflict）；去掉该检查时只剩数据库守卫
    // c7_funded_after_unpaid_closure 兜底，错误类别不同，本断言即失败。
    assert!(matches!(late, Err(FundingError::Conflict)), "迟到的付款必须确定拒绝: {late:?}");
    assert_eq!(ledger(pool).await, ledger_before, "迟到的付款零收据、零分录、零命令");
    assert_eq!(side_effects(pool).await, (deliveries_before, commands_before));
    assert_eq!(closures(pool).await, 1);
}

/// 确定顺序的正例：付款先认证，之后的未付取消确定拒绝，收据与 funded 各一、零封闭事实。
#[tokio::test]
async fn postgres_consignment_sanction_orders_unpaid_closure_after_payment_is_refused() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_due_and_payable(pool).await;

    let receipt = runtime.test_certify_cod(payment_command(2)).await.expect("付款先到先得");
    assert!(!receipt.replayed);
    let paid = ledger(pool).await;
    assert_eq!((paid.receipts, paid.funded), (1, 1));
    let late = runtime.test_cancel_unpaid("", uuid(CONTRACT), uuid(DISPOSITION)).await;
    assert!(matches!(late, Err(FundingError::Conflict)), "已付款不得按未付取消: {late:?}");
    assert_eq!(closures(pool).await, 0, "零封闭事实");
    assert_eq!(ledger(pool).await, paid, "收款事实不变");
}

/// 同一数据库里的另一张订单：把夹具整条链（代管、上架、购买、原件派发）的标识整体换成另一组，
/// 用户、邮件会话与凭据沿用；`swap_users` 为真时买卖双方对调（该用户在这张订单里是卖家）。
struct Chain {
    /// 换进每个 UUID 末组首位的十六进制字符，区分订单。
    tag: char,
    /// 订单序号，区分装备编号与邮件编号。
    n: u32,
    swap_users: bool,
}

/// 各订单共用、不随订单变化的标识。
const SHARED_IDS: [&str; 5] = [USER_1, USER_2, SESSION, CREDENTIAL, REPLAY];

/// 把 UUID 末组首位换成 `tag`；共用标识不动。
fn shift_ids(sql: &str, tag: char) -> String {
    let bytes = sql.as_bytes();
    let is_uuid = |at: usize| {
        at + 36 <= bytes.len()
            && bytes[at..at + 36].iter().enumerate().all(|(offset, byte)| match offset {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
    };
    let mut out = String::with_capacity(sql.len());
    let mut at = 0;
    while at < bytes.len() {
        let boundary = at == 0 || !(bytes[at - 1].is_ascii_hexdigit() || bytes[at - 1] == b'-');
        if boundary && is_uuid(at) {
            let id = &sql[at..at + 36];
            if SHARED_IDS.contains(&id) {
                out.push_str(id);
            } else {
                out.push_str(&id[..24]);
                out.push(tag);
                out.push_str(&id[25..]);
            }
            at += 36;
        } else {
            out.push(char::from(bytes[at]));
            at += 1;
        }
    }
    out
}

/// 把每个 `repeat('xx', n)` 字节摘要的首半字节错开 `offset`，使唯一键上的摘要不与别的订单相撞。
fn shift_digests(sql: &str, offset: u32) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(at) = rest.find("repeat('") {
        let (head, tail) = rest.split_at(at + "repeat('".len());
        out.push_str(head);
        let mut chars = tail.chars();
        match (chars.next().and_then(|c| c.to_digit(16)), chars.next()) {
            (Some(high), Some(low)) if low.is_ascii_hexdigit() => {
                out.push(char::from_digit((high + offset) % 16, 16).expect("半字节"));
                rest = &tail[1..];
            }
            _ => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// 对调两个文本标记。
fn swap(sql: &str, first: &str, second: &str) -> String {
    sql.replace(first, "@@swap@@").replace(second, first).replace("@@swap@@", second)
}

impl Chain {
    fn shifted(&self, id: &str) -> Uuid {
        uuid(&shift_ids(id, self.tag))
    }

    fn mid(&self) -> String {
        format!("9{}01", self.n)
    }

    /// 通用换名：订单标识、装备编号、邮件编号、远端观察编号与（可选的）买卖对调。
    fn rename(&self, sql: &str) -> String {
        let mut sql = shift_ids(sql, self.tag)
            .replace("'42'", &format!("'4{}'", self.n))
            .replace("\"42\"", &format!("\"4{}\"", self.n))
            .replace("555", &format!("6{}5", self.n))
            .replace("obs-1", &format!("obs-c{}", self.n))
            .replace("'9001'", &format!("'{}'", self.mid()))
            .replace("fake-cod-paid-1", &format!("fake-cod-paid-{}", self.n));
        if self.swap_users {
            sql = swap(&sql, USER_1, USER_2);
            sql = swap(&sql, "FakeSellerOne", "FakeSellerTwo");
        }
        sql
    }

    /// 代管、上架、购买、原件派发，分批提交（整封接受的延迟校验要求提交时挂单仍是 deposit_accepted）。
    async fn seed(&self, pool: &PgPool) {
        let envelope = [
            listing_awaiting_deposit(),
            reservation_issued(),
            observation_and_asset(),
            decide_command(),
            initial_and_final(),
            item_position_claim(),
            consume_reservation(),
            accept_listing(),
            delivery_and_work(),
            custody_event(),
        ]
        .concat();
        let offset = 3 * self.n;
        for batch in [envelope, attempt_start_proof_and_custody()] {
            succeeds(pool, &self.rename(&shift_digests(&batch, offset))).await;
        }
        succeeds(pool, &self.rename(&available_sql())).await;
        succeeds(pool, &self.rename(&purchase_sql())).await;
        succeeds(pool, &self.rename(&original_claim_dispatch_sql())).await;
    }

    /// 这张订单的付款认证命令：与 [`payment_command`] 同形，标识换成本订单的。
    fn payment(&self) -> CertifyCod {
        let mut command = payment_command(20 + u8::try_from(self.n).expect("序号"));
        command.contract_id = self.shifted(CONTRACT);
        command.payment_obligation_id = self.shifted(OBLIGATION_1);
        command.original_delivery_id = self.shifted(PAY_DELIVERY);
        command.receipt.payment_mail_id = self.mid();
        command
    }

    fn listing(&self) -> Uuid {
        self.shifted("20000000-0000-0000-0000-000000000001")
    }
}

/// 订单图的整行快照：挂单、代管、合同、Sale、义务与履约、派发、投递、收款与资金分配。
async fn order_snapshot(pool: &PgPool, chains: &[&Chain]) -> String {
    let contracts: Vec<Uuid> = chains.iter().map(|chain| chain.shifted(CONTRACT)).collect();
    let listings: Vec<Uuid> = chains.iter().map(|chain| chain.listing()).collect();
    let by_contract = [
        "consignment_contracts",
        "consignment_sales",
        "consignment_dispatch_rounds",
        "consignment_dispatch_attempts",
        "consignment_cod_receipts",
        "consignment_funded_sales",
        "consignment_fee_facts",
        "consignment_sale_closures",
    ];
    let mut snapshot = String::new();
    snapshot += &sqlx::query_scalar::<_, String>(
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.id), '')
         FROM public.consignment_listings AS t WHERE t.id = ANY($1)",
    )
    .bind(&listings)
    .fetch_one(pool)
    .await
    .expect("挂单快照");
    snapshot += &sqlx::query_scalar::<_, String>(
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.id), '')
         FROM public.consignment_custody_positions AS t WHERE t.listing_id = ANY($1)",
    )
    .bind(&listings)
    .fetch_one(pool)
    .await
    .expect("代管快照");
    for table in by_contract {
        snapshot += &sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.contract_id::text), '')
             FROM public.{table} AS t WHERE t.contract_id = ANY($1)"
        )))
        .bind(&contracts)
        .fetch_one(pool)
        .await
        .expect("按合同快照");
    }
    for sql in [
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.id), '')
         FROM public.settlement_obligations AS t WHERE t.contract_id = ANY($1)",
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.obligation_id), '')
         FROM public.settlement_obligation_fulfillments AS t
         WHERE t.obligation_id IN (SELECT id FROM public.settlement_obligations
                                   WHERE contract_id = ANY($1))",
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.id), '')
         FROM public.deliveries AS t
         WHERE t.origin_id::text IN (SELECT id::text FROM public.settlement_obligations
                                     WHERE contract_id = ANY($1))",
        "SELECT coalesce(string_agg(to_jsonb(t)::text, '|' ORDER BY t.id), '')
         FROM public.settlement_funding_allocations AS t
         WHERE t.consignment_contract_id = ANY($1)",
    ] {
        snapshot += &sqlx::query_scalar::<_, String>(sql)
            .bind(&contracts)
            .fetch_one(pool)
            .await
            .expect("义务与资金快照");
    }
    snapshot
}

/// 一次选定表的行数。
async fn funded_contracts(pool: &PgPool, chain: &Chain) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_funded_sales WHERE contract_id = $1",
    )
    .bind(chain.shifted(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("funded 数")
}

/// 买家（被处罚者）在这张订单里的角色：合同的卖家用户。
async fn contract_seller(pool: &PgPool, chain: &Chain) -> Uuid {
    sqlx::query_scalar(
        "SELECT seller_user_id FROM public.consignment_contracts WHERE contract_id = $1",
    )
    .bind(chain.shifted(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("合同卖家")
}

/// 某订单的卖款分配金额（用途 seller）。
async fn seller_allocation(pool: &PgPool, chain: &Chain) -> i64 {
    sqlx::query_scalar(
        "SELECT coalesce(sum(amount), 0)::bigint FROM public.settlement_funding_allocations
         WHERE consignment_contract_id = $1 AND purpose = 'seller'",
    )
    .bind(chain.shifted(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("卖款分配")
}

/// 处罚接管只禁新购：买家的另一张已付待件订单、另一张未付订单与他作为卖家的已付订单，
/// 在账号被拉黑前后逐行不变（状态、派发、投递与资金分配），未付订单仍能照常收款；
/// 接管本身不产生新的投递（零远端 POST），只多一条 apply_sanction 命令。
#[tokio::test]
async fn postgres_consignment_sanction_orders_takeover_keeps_existing_orders_in_flight() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let runtime = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    // A 单是处罚的来源（三份合格拒收）；B 已付待件、C 未付、D 是买家作为卖家的已付订单。
    seed_base(pool).await;
    let paid = Chain { tag: 'b', n: 3, swap_users: false };
    let unpaid = Chain { tag: 'c', n: 4, swap_users: false };
    let as_seller = Chain { tag: 'd', n: 5, swap_users: true };
    for chain in [&paid, &unpaid, &as_seller] {
        chain.seed(pool).await;
    }
    for chain in [&paid, &as_seller] {
        let receipt = runtime.test_certify_cod(chain.payment()).await.expect("已付款订单");
        assert!(!receipt.replayed);
    }
    seed_intakes(pool, 3).await;
    let mut sanction = None;
    for n in 1..=3 {
        sanction = submit(&runtime, n).await.sanction;
    }
    let sanction = sanction.expect("第三份拒收触发处罚");
    let work = SanctionWorkRef {
        work_id: sanction.account_work_id,
        sanction_id: sanction.sanction_id,
        buyer_user_id: buyer().as_uuid(),
    };

    // 前置：夹具确实造出了要保护的三种订单。
    assert_eq!(
        contract_seller(pool, &as_seller).await,
        buyer().as_uuid(),
        "D 单的卖家就是被处罚者"
    );
    assert_eq!(
        (funded_contracts(pool, &paid).await, funded_contracts(pool, &unpaid).await),
        (1, 0)
    );
    assert_eq!(seller_allocation(pool, &as_seller).await, 96, "D 单有卖款");
    assert!(
        runtime.test_purchase_gate(command(900), buyer(), seller(), None).await.expect("阻断读"),
        "接管前新购已被待执行处罚阻断"
    );
    let all = [&paid, &unpaid, &as_seller];
    let orders_before = order_snapshot(pool, &all).await;
    let kept_before = order_snapshot(pool, &[&paid, &as_seller]).await;
    let (deliveries_before, commands_before) =
        (count(pool, "deliveries").await, count(pool, "consignment_commands").await);

    let outcome = runtime.execute_sanction_work(work).await.expect("接管处罚");
    assert!(matches!(outcome, SanctionWorkOutcome::Applied { .. }), "{outcome:?}");

    // 处罚只禁新购：账号已不再 active，新购仍被阻断。
    let status: String = sqlx::query_scalar("SELECT status FROM public.users WHERE id = $1")
        .bind(buyer().as_uuid())
        .fetch_one(pool)
        .await
        .expect("买家状态");
    assert_ne!(status, "active");
    // 既有订单逐行不变，接管不产生新的投递，只多一条 apply_sanction 命令。
    assert_eq!(order_snapshot(pool, &all).await, orders_before);
    assert_eq!(count(pool, "deliveries").await, deliveries_before, "零新增远端 POST");
    assert_eq!(count(pool, "consignment_commands").await, commands_before + 1);

    // 继续履约：未付订单在处罚之后仍能收款，已付订单与卖款不受影响。
    let receipt = runtime.test_certify_cod(unpaid.payment()).await.expect("未付订单照常收款");
    assert!(!receipt.replayed);
    assert_eq!(funded_contracts(pool, &unpaid).await, 1);
    assert_eq!(order_snapshot(pool, &[&paid, &as_seller]).await, kept_before);
    assert_eq!(seller_allocation(pool, &as_seller).await, 96, "被处罚者作为卖家的卖款不变");
}
